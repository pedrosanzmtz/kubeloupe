//! Create or update objects from a manifest file, like `kubectl apply -f`:
//! many documents, applied in order, validated with a dry run first, with a
//! result per object.

use super::manifests::{crd_defines, phase, set_namespace, split_api_version, split_manifests};
use super::ops::{apply_value, check_document, object_api};
use super::*;
use crate::actions::describe_error;
use anyhow::Result;
use kube::api::DynamicObject;
use kube::Client;
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

/// What happened to one object of a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ApplyStatus {
    /// It did not exist and was created (a dry run: would be).
    Created,
    /// It existed and changed.
    Configured,
    /// It existed and the apply changed nothing.
    Unchanged,
    /// The cluster rejected it, or the document could not be read.
    Failed,
    /// Not applied, because other documents failed validation.
    Skipped,
    /// A dry run could not check it yet: it needs a CRD or a namespace that
    /// this same manifest creates.
    Pending,
}

/// The result for one object of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ApplyResult {
    /// 1-based position of its document in the file.
    pub document: u32,
    /// 1-based line its document starts on (for a YAML error: the error's line).
    pub line: u32,
    /// Empty if the document could not be read.
    pub api_version: String,
    pub kind: String,
    pub name: String,
    /// Empty for cluster-scoped kinds.
    pub namespace: String,
    pub status: ApplyStatus,
    /// Why it failed, was skipped or is pending; empty otherwise.
    pub message: String,
    /// Fields the apply took (or would take) over from other field managers.
    pub takeovers: Vec<FieldTakeover>,
}

/// How [`apply_manifests`] applies.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManifestOptions {
    /// Validate only (`dryRun=All`): nothing is stored.
    pub dry_run: bool,
    /// Apply the documents that pass validation even if others fail. By
    /// default nothing is applied unless every document passes.
    pub skip_invalid: bool,
}

struct Entry {
    result: ApplyResult,
    /// The object to apply; `None` if the document could not be read.
    value: Option<Value>,
    phase: u8,
}

/// Apply every object of a (multi-document) YAML manifest with server-side
/// apply, as the field manager "kubeloupe".
///
/// The whole batch is validated with a dry run first; unless
/// `opts.dry_run`, it is then applied if every document passed (or, with
/// `opts.skip_invalid`, the ones that passed). Namespaces go first, then
/// CRDs, then the rest; kinds defined by a CRD of the same batch are looked
/// up again once the CRD is served. Namespaced objects without a namespace
/// go to `default_namespace`. Results come back in file order.
pub async fn apply_manifests(
    client: &Client,
    kinds: &[ResourceKind],
    yaml: &str,
    default_namespace: &str,
    opts: ManifestOptions,
) -> Vec<ApplyResult> {
    let mut entries: Vec<Entry> = split_manifests(yaml)
        .into_iter()
        .map(|m| read_entry(m.document, m.line, m.value))
        .collect();
    // Stable: file order within a phase.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| entries[i].phase);
    let batch_crds: HashSet<(String, String)> = entries
        .iter()
        .filter_map(|e| e.value.as_ref().and_then(crd_defines))
        .collect();

    // 1. Validate everything with a dry run.
    let mut new_namespaces = HashSet::new();
    let mut kinds = kinds.to_vec();
    let mut refreshed = false;
    for &i in &order {
        let e = &mut entries[i];
        let Some(value) = e.value.clone() else {
            continue;
        };
        let (group, _) = split_api_version(&e.result.api_version);
        let in_batch = batch_crds.contains(&(group.to_string(), e.result.kind.clone()));
        let mut r = resolve(&kinds, &e.result.api_version, &e.result.kind);
        if r.is_none() && !in_batch && !refreshed {
            // The caller's discovery may predate a CRD created since.
            refreshed = true;
            if let Ok(found) = discover(client).await {
                kinds = found;
            }
            r = resolve(&kinds, &e.result.api_version, &e.result.kind);
        }
        let Some(r) = r else {
            if in_batch {
                pending(
                    e,
                    "its CustomResourceDefinition is created by this manifest first",
                );
            } else {
                fail(e, unknown_kind(&e.result.api_version, &e.result.kind));
            }
            continue;
        };
        let mut value = value;
        let ns = set_namespace(&mut value, r.namespaced, default_namespace);
        e.result.namespace = ns.clone();
        if r.namespaced && new_namespaces.contains(&ns) {
            pending(
                e,
                &format!("namespace {ns} is created by this manifest first"),
            );
            continue;
        }
        match apply_one(client, &r, &ns, &e.result.name, value, true).await {
            Ok((status, takeovers)) => {
                e.result.status = status;
                e.result.takeovers = takeovers;
                if r.is("", "Namespace") && status == ApplyStatus::Created {
                    new_namespaces.insert(e.result.name.clone());
                }
            }
            Err(err) => fail(e, describe(&err)),
        }
    }

    let failed = entries
        .iter()
        .filter(|e| e.result.status == ApplyStatus::Failed)
        .count();
    if opts.dry_run {
        return finish(entries);
    }
    if failed > 0 && !opts.skip_invalid {
        let why = format!(
            "not applied: {failed} document{} failed validation",
            if failed == 1 { "" } else { "s" }
        );
        for e in &mut entries {
            if e.result.status != ApplyStatus::Failed {
                e.result.status = ApplyStatus::Skipped;
                e.result.message = why.clone();
                e.result.takeovers.clear();
            }
        }
        return finish(entries);
    }

    // 2. Apply what passed, in order.
    let mut rediscovered = false;
    for &i in &order {
        if entries[i].result.status == ApplyStatus::Failed {
            continue;
        }
        let Some(value) = entries[i].value.clone() else {
            continue;
        };
        let (api_version, kind) = (
            entries[i].result.api_version.clone(),
            entries[i].result.kind.clone(),
        );
        let mut r = resolve(&kinds, &api_version, &kind);
        let mut fresh = false;
        if r.is_none() && !rediscovered && entries[i].phase == 2 {
            // Its CRD was just created: wait until the cluster serves it.
            rediscovered = true;
            kinds = wait_for_kinds(client, kinds, &entries, &order).await;
            r = resolve(&kinds, &api_version, &kind);
            fresh = true;
        }
        let e = &mut entries[i];
        let Some(r) = r else {
            fail(e, unknown_kind(&api_version, &kind));
            continue;
        };
        let mut value = value;
        let ns = set_namespace(&mut value, r.namespaced, default_namespace);
        e.result.namespace = ns.clone();
        e.result.message.clear();
        let mut tries = if fresh { 5 } else { 1 };
        loop {
            tries -= 1;
            match apply_one(client, &r, &ns, &e.result.name, value.clone(), false).await {
                Ok((status, takeovers)) => {
                    e.result.status = status;
                    e.result.takeovers = takeovers;
                }
                // A new CRD can be listed in discovery a moment before its
                // endpoint answers.
                Err(err) if tries > 0 && status_code(&err) == Some(404) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Err(err) => fail(e, describe(&err)),
            }
            break;
        }
    }
    finish(entries)
}

fn read_entry(document: u32, line: u32, value: Result<Value, String>) -> Entry {
    let mut result = ApplyResult {
        document,
        line,
        api_version: String::new(),
        kind: String::new(),
        name: String::new(),
        namespace: String::new(),
        status: ApplyStatus::Created,
        message: String::new(),
        takeovers: Vec::new(),
    };
    let value = match value {
        Ok(v) => v,
        Err(msg) => {
            // Point at the parser's line.
            if let Some(n) = msg
                .strip_prefix("invalid YAML at line ")
                .and_then(|r| r.split(' ').next())
                .and_then(|n| n.parse().ok())
            {
                result.line = n;
            }
            result.status = ApplyStatus::Failed;
            result.message = msg;
            return Entry {
                result,
                value: None,
                phase: 2,
            };
        }
    };
    let text = |p: &str| {
        value
            .pointer(p)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    result.api_version = text("/apiVersion");
    result.kind = text("/kind");
    result.name = text("/metadata/name");
    result.namespace = text("/metadata/namespace");
    let problem = if result.api_version.is_empty() || result.kind.is_empty() {
        Some("apiVersion and kind are required")
    } else if result.name.is_empty() && !text("/metadata/generateName").is_empty() {
        Some("metadata.generateName is not supported by apply: set metadata.name")
    } else if result.name.is_empty() {
        Some("metadata.name is required")
    } else {
        None
    };
    let phase = phase(&result.api_version, &result.kind);
    if let Some(p) = problem {
        result.status = ApplyStatus::Failed;
        result.message = p.to_string();
        return Entry {
            result,
            value: None,
            phase,
        };
    }
    Entry {
        result,
        value: Some(value),
        phase,
    }
}

fn fail(e: &mut Entry, message: String) {
    e.result.status = ApplyStatus::Failed;
    e.result.message = message;
    e.result.takeovers.clear();
}

fn pending(e: &mut Entry, why: &str) {
    e.result.status = ApplyStatus::Pending;
    e.result.message = format!("checked when applied: {why}");
}

fn finish(entries: Vec<Entry>) -> Vec<ApplyResult> {
    entries.into_iter().map(|e| e.result).collect()
}

fn unknown_kind(api_version: &str, kind: &str) -> String {
    format!("the cluster does not serve {kind} in {api_version}")
}

/// The kind of `apiVersion`/`kind` among the discovered ones, at the
/// document's own version (discovery has the preferred one).
fn resolve(kinds: &[ResourceKind], api_version: &str, kind: &str) -> Option<ResourceRef> {
    let (group, version) = split_api_version(api_version);
    kinds
        .iter()
        .find(|k| k.resource.group == group && k.resource.kind == kind)
        .map(|k| ResourceRef {
            version: version.to_string(),
            ..k.resource.clone()
        })
}

/// Discover again until every kind still unknown among the entries is
/// served (CRDs take a moment), for at most ~15s.
async fn wait_for_kinds(
    client: &Client,
    mut kinds: Vec<ResourceKind>,
    entries: &[Entry],
    order: &[usize],
) -> Vec<ResourceKind> {
    for attempt in 0..30 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Ok(found) = discover(client).await {
            kinds = found;
        }
        let missing = order.iter().map(|&i| &entries[i]).any(|e| {
            e.value.is_some()
                && e.result.status != ApplyStatus::Failed
                && resolve(&kinds, &e.result.api_version, &e.result.kind).is_none()
        });
        if !missing {
            break;
        }
    }
    kinds
}

/// Apply one object and tell whether it was created, changed or not.
async fn apply_one(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    value: Value,
    dry_run: bool,
) -> Result<(ApplyStatus, Vec<FieldTakeover>)> {
    let value = check_document(r, name, value)?;
    let api = object_api(client, r, namespace)?;
    let before = match api.get(name).await {
        Ok(o) => Some(o),
        Err(kube::Error::Api(e)) if e.code == 404 => None,
        Err(e) => return Err(e.into()),
    };
    let opts = ApplyOptions {
        dry_run,
        resource_version: None,
    };
    let (outcome, after) = apply_value(&api, name, value, &opts).await?;
    let takeovers = match outcome {
        ApplyOutcome::Applied { takeovers } => takeovers,
        // Only without a resourceVersion check, which is not used here.
        ApplyOutcome::Conflict { .. } => Vec::new(),
    };
    let status = match (before, after) {
        (None, _) => ApplyStatus::Created,
        (Some(b), Some(a)) if same_object(&b, &a) => ApplyStatus::Unchanged,
        _ => ApplyStatus::Configured,
    };
    Ok((status, takeovers))
}

/// Whether two versions of an object are the same apart from bookkeeping
/// (resourceVersion, managedFields).
fn same_object(a: &DynamicObject, b: &DynamicObject) -> bool {
    fn norm(o: &DynamicObject) -> Value {
        let mut o = o.clone();
        o.metadata.resource_version = None;
        o.metadata.managed_fields = None;
        serde_json::to_value(o).unwrap_or_default()
    }
    norm(a) == norm(b)
}

fn status_code(err: &anyhow::Error) -> Option<u16> {
    err.chain()
        .find_map(|e| match e.downcast_ref::<kube::Error>() {
            Some(kube::Error::Api(r)) => Some(r.code),
            _ => None,
        })
}

/// [`describe_error`], except that a 404 here means a missing namespace or
/// API, not an object that is "already gone".
fn describe(err: &anyhow::Error) -> String {
    let api = err
        .chain()
        .find_map(|e| match e.downcast_ref::<kube::Error>() {
            Some(kube::Error::Api(r)) => Some(r.message.clone()),
            _ => None,
        });
    match (status_code(err), api) {
        (Some(404), Some(msg)) => format!("Not found: {msg}"),
        _ => describe_error(err),
    }
}

// ---------------------------------------------------------------------------
// CLI.
// ---------------------------------------------------------------------------

/// `kubeloupe apply -f <file|->`: apply a manifest and print a line per
/// object, like kubectl. Fails if any object failed.
pub async fn cli_apply(
    client: &Client,
    file: &str,
    namespace: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let yaml = if file == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        s
    } else {
        std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("cannot read {file}: {e}"))?
    };
    let kinds = discover(client).await?;
    let ns = namespace.unwrap_or_else(|| client.default_namespace());
    let results = apply_manifests(
        client,
        &kinds,
        &yaml,
        ns,
        ManifestOptions {
            dry_run,
            skip_invalid: false,
        },
    )
    .await;
    if results.is_empty() {
        anyhow::bail!("no objects in {file}");
    }
    for r in &results {
        println!("{}", result_line(r, dry_run));
    }
    let failed = results
        .iter()
        .filter(|r| r.status == ApplyStatus::Failed)
        .count();
    if failed > 0 {
        anyhow::bail!("{failed} of {} objects failed", results.len());
    }
    Ok(())
}

/// `deployment.apps/web created`, kubectl style.
fn result_line(r: &ApplyResult, dry_run: bool) -> String {
    let (group, _) = split_api_version(&r.api_version);
    let what = match (r.kind.is_empty(), group.is_empty()) {
        (true, _) => format!("document {} (line {})", r.document, r.line),
        (false, true) => format!("{}/{}", r.kind.to_lowercase(), r.name),
        (false, false) => format!("{}.{group}/{}", r.kind.to_lowercase(), r.name),
    };
    let status = match r.status {
        ApplyStatus::Created => "created",
        ApplyStatus::Configured => "configured",
        ApplyStatus::Unchanged => "unchanged",
        ApplyStatus::Failed => "failed",
        ApplyStatus::Skipped => "skipped",
        ApplyStatus::Pending => "pending",
    };
    let mut line = format!("{what} {status}");
    if dry_run && !matches!(r.status, ApplyStatus::Failed | ApplyStatus::Pending) {
        line.push_str(" (dry run)");
    }
    if !r.message.is_empty() {
        line.push_str(": ");
        line.push_str(&r.message);
    }
    for t in &r.takeovers {
        line.push_str(&format!(
            "\n  took over from {}: {}",
            t.manager,
            t.fields.join(", ")
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn resolves_at_the_documents_version() {
        let kinds = vec![kind("apps", "Deployment", "deployments", &[])];
        let r = resolve(&kinds, "apps/v1beta2", "Deployment").unwrap();
        assert_eq!(r.version, "v1beta2");
        assert_eq!(r.plural, "deployments");
        assert!(resolve(&kinds, "v1", "Deployment").is_none());
    }

    #[test]
    fn reads_identity_and_rejects_incomplete_documents() {
        let ok = read_entry(
            1,
            3,
            Ok(
                serde_json::json!({"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "n"}}),
            ),
        );
        assert_eq!(ok.phase, 0);
        assert_eq!(ok.result.name, "n");
        assert!(ok.value.is_some());

        let gen = read_entry(
            2,
            1,
            Ok(
                serde_json::json!({"apiVersion": "v1", "kind": "Pod", "metadata": {"generateName": "p-"}}),
            ),
        );
        assert_eq!(gen.result.status, ApplyStatus::Failed);
        assert!(gen.result.message.contains("generateName"));

        let bad = read_entry(3, 4, Err("invalid YAML at line 9 column 2: x".into()));
        assert_eq!(bad.result.line, 9);
        assert_eq!(bad.result.status, ApplyStatus::Failed);
    }

    #[test]
    fn prints_kubectl_style_lines() {
        let mut r = ApplyResult {
            document: 1,
            line: 1,
            api_version: "apps/v1".into(),
            kind: "Deployment".into(),
            name: "web".into(),
            namespace: "demo".into(),
            status: ApplyStatus::Created,
            message: String::new(),
            takeovers: vec![],
        };
        assert_eq!(result_line(&r, false), "deployment.apps/web created");
        assert_eq!(
            result_line(&r, true),
            "deployment.apps/web created (dry run)"
        );
        r.api_version = "v1".into();
        r.kind = "ConfigMap".into();
        r.status = ApplyStatus::Failed;
        r.message = "boom".into();
        assert_eq!(result_line(&r, true), "configmap/web failed: boom");
    }

    #[test]
    fn status_is_camel_case() {
        assert_eq!(
            serde_json::to_value(ApplyStatus::Unchanged).unwrap(),
            "unchanged"
        );
    }
}
