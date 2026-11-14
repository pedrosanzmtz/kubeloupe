//! Generic operations on `DynamicObject`: list, get, YAML get/apply, delete.

use super::*;
use anyhow::{bail, Context, Result};
use kube::api::{Api, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::Client;
use serde::Serialize;
use serde_json::Value;

/// Field manager used for server-side apply.
const FIELD_MANAGER: &str = "kubeloupe";

// ---------------------------------------------------------------------------
// Generic operations.
// ---------------------------------------------------------------------------

/// An `Api` for the kind: scoped to `namespace` if the kind is namespaced and
/// one is given, otherwise cluster-wide.
pub fn dyn_api(client: &Client, r: &ResourceRef, namespace: Option<&str>) -> Api<DynamicObject> {
    let ar = r.api_resource();
    match namespace.filter(|n| r.namespaced && !n.is_empty()) {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &ar),
        None => Api::all_with(client.clone(), &ar),
    }
}

/// Api for operating on one named object: a namespaced kind needs its namespace.
pub(super) fn object_api(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
) -> Result<Api<DynamicObject>> {
    if r.namespaced && namespace.is_empty() {
        bail!("{} is namespaced: a namespace is required", r.kind);
    }
    Ok(dyn_api(client, r, Some(namespace)))
}

/// List a kind as table rows.
pub async fn list_resources(
    client: &Client,
    kind: &ResourceKind,
    namespace: Option<&str>,
) -> Result<Vec<ResourceRow>> {
    let api = dyn_api(client, &kind.resource, namespace);
    let list = api.list(&ListParams::default()).await?;
    Ok(list
        .items
        .iter()
        .map(|o| make_row(&kind.columns, o))
        .collect())
}

pub async fn get_resource(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
) -> Result<DynamicObject> {
    Ok(object_api(client, r, namespace)?.get(name).await?)
}

pub(super) fn is_secret(r: &ResourceRef) -> bool {
    r.is("", "Secret")
}

pub(super) const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Replace every Secret value (and the annotation that embeds a copy of
/// them) with [`REDACTED`].
fn redact_secret(v: &mut Value) {
    for field in ["data", "stringData"] {
        if let Some(map) = v.get_mut(field).and_then(Value::as_object_mut) {
            for val in map.values_mut() {
                *val = Value::String(REDACTED.into());
            }
        }
    }
    if let Some(x) = v
        .pointer_mut("/metadata/annotations")
        .and_then(Value::as_object_mut)
        .and_then(|a| a.get_mut(LAST_APPLIED))
    {
        *x = Value::String(REDACTED.into());
    }
}

fn contains_redaction(v: &Value) -> bool {
    ["/data", "/stringData", "/metadata/annotations"]
        .iter()
        .filter_map(|p| v.pointer(p).and_then(Value::as_object))
        .any(|m| m.values().any(|x| x.as_str() == Some(REDACTED)))
}

/// An object as YAML for editing: `apiVersion`, `kind`, `metadata` first,
/// without `managedFields`. Secret values are replaced by [`REDACTED`]
/// unless `reveal` is set.
pub async fn get_resource_yaml(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    reveal: bool,
) -> Result<String> {
    let obj = get_resource(client, r, namespace, name).await?;
    to_yaml(object_value(r, obj, reveal)?)
}

/// An object as JSON for output: without `managedFields`, with `apiVersion`
/// and `kind` filled in (lists leave them out), and Secret values replaced
/// by [`REDACTED`] unless `reveal` is set.
pub fn object_value(r: &ResourceRef, mut obj: DynamicObject, reveal: bool) -> Result<Value> {
    obj.metadata.managed_fields = None;
    let mut value = serde_json::to_value(&obj)?;
    if let Some(map) = value.as_object_mut() {
        let ar = r.api_resource();
        map.entry("apiVersion")
            .or_insert_with(|| Value::String(ar.api_version.clone()));
        map.entry("kind")
            .or_insert_with(|| Value::String(ar.kind.clone()));
    }
    if is_secret(r) && !reveal {
        redact_secret(&mut value);
    }
    Ok(value)
}

/// YAML with `apiVersion`, `kind` and `metadata` first.
pub fn to_yaml(value: Value) -> Result<String> {
    Ok(serde_yaml::to_string(&ordered(value))?)
}

/// Put `apiVersion`, `kind` and `metadata` first; `serde_json` maps are
/// sorted alphabetically otherwise.
fn ordered(value: Value) -> serde_yaml::Value {
    let Value::Object(mut map) = value else {
        return serde_yaml::to_value(&value).unwrap_or(serde_yaml::Value::Null);
    };
    let mut out = serde_yaml::Mapping::new();
    for key in ["apiVersion", "kind", "metadata"] {
        if let Some(v) = map.remove(key) {
            out.insert(key.into(), serde_yaml::to_value(&v).unwrap_or_default());
        }
    }
    for (k, v) in map {
        out.insert(k.into(), serde_yaml::to_value(&v).unwrap_or_default());
    }
    serde_yaml::Value::Mapping(out)
}

/// Remove what the server owns and that would make a second apply of the
/// same document fail or fight other writers.
///
/// `resourceVersion` is the important one: in a server-side-apply body it is
/// an optimistic-concurrency precondition, so a YAML that was loaded before
/// the previous save carries a stale value and the next save answers 409
/// Conflict. The rest (`uid`, `generation`, `creationTimestamp`, `selfLink`,
/// `managedFields`) is read-only metadata, and `status` is owned by
/// controllers: applying it would make this field manager a co-owner of
/// fields it never meant to manage, and later conflict with the controller.
fn strip_server_fields(value: &mut Value) {
    if let Some(root) = value.as_object_mut() {
        root.remove("status");
        if let Some(meta) = root.get_mut("metadata").and_then(Value::as_object_mut) {
            for key in [
                "resourceVersion",
                "managedFields",
                "uid",
                "generation",
                "creationTimestamp",
                "selfLink",
                "deletionTimestamp",
                "deletionGracePeriodSeconds",
            ] {
                meta.remove(key);
            }
        }
    }
}

/// Fields an apply took over from another field manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct FieldTakeover {
    /// The manager that owned the fields (e.g. "kubectl-create", "helm").
    pub manager: String,
    /// Field paths, as the API server reports them (e.g. ".spec.replicas").
    pub fields: Vec<String>,
}

/// Apply edited YAML with server-side apply (field manager "kubeloupe").
///
/// Server-managed fields are stripped first (see [`strip_server_fields`]),
/// so saving the same editor content again, or content that was loaded
/// before an earlier save, never fails on a stale `resourceVersion`.
///
/// The apply is forced, like `kubectl edit`: the document is an explicit
/// edit by the user and wins over fields another manager (kubectl, Helm, a
/// controller) owns. Without force, the first edit of almost any object
/// created by kubectl would fail. So the takeover is not silent, a
/// non-forced dry run runs first, and the fields it reports as conflicting
/// are returned: those are the fields this save took over.
pub async fn apply_resource_yaml(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    yaml: &str,
) -> Result<Vec<FieldTakeover>> {
    match apply_resource_yaml_with(client, r, namespace, name, yaml, &ApplyOptions::default())
        .await?
    {
        ApplyOutcome::Applied { takeovers } => Ok(takeovers),
        // Only reported when a resourceVersion was given.
        ApplyOutcome::Conflict { .. } => bail!("{name} changed on the server"),
    }
}

/// How [`apply_resource_yaml_with`] applies.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Validate only: the API server runs the whole apply (admission,
    /// schema, ownership) with `dryRun=All` and stores nothing.
    pub dry_run: bool,
    /// The `metadata.resourceVersion` the YAML was loaded at. If set and the
    /// object has changed since, nothing is applied and the outcome is
    /// [`ApplyOutcome::Conflict`].
    pub resource_version: Option<String>,
}

/// What an apply did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ApplyOutcome {
    /// Applied (or, for a dry run, would apply). `takeovers` are the fields
    /// it took (or would take) over from other field managers.
    Applied { takeovers: Vec<FieldTakeover> },
    /// Someone else wrote to the object since it was loaded; nothing was
    /// applied. `resource_version` is the object's current one.
    Conflict { resource_version: String },
}

/// [`apply_resource_yaml`] with a dry run and a check that the object has
/// not changed since it was loaded (see [`ApplyOptions`]).
///
/// The check is a read before the apply plus the `resourceVersion` as a
/// precondition in the forced apply itself, so a write that lands between
/// the two is not overwritten silently either.
pub async fn apply_resource_yaml_with(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    yaml: &str,
    opts: &ApplyOptions,
) -> Result<ApplyOutcome> {
    let value = prepare_apply(r, name, yaml)?;
    let api = object_api(client, r, namespace)?;
    Ok(apply_value(&api, name, value, opts).await?.0)
}

/// The apply of [`apply_resource_yaml_with`] on a checked document (see
/// [`check_document`]). Also returns the object as the server answered it
/// (stored, or what a dry run would store); `None` on a conflict.
pub(super) async fn apply_value(
    api: &Api<DynamicObject>,
    name: &str,
    value: Value,
    opts: &ApplyOptions,
) -> Result<(ApplyOutcome, Option<DynamicObject>)> {
    let expected = opts.resource_version.as_deref().filter(|v| !v.is_empty());
    if let Some(expected) = expected {
        let current = current_resource_version(api, name).await?;
        if current != expected {
            return Ok((
                ApplyOutcome::Conflict {
                    resource_version: current,
                },
                None,
            ));
        }
    }

    let patch = Patch::Apply(&value);
    let (takeovers, mut answer) = match api
        .patch(name, &PatchParams::apply(FIELD_MANAGER).dry_run(), &patch)
        .await
    {
        Ok(obj) => (Vec::new(), Some(obj)),
        Err(kube::Error::Api(e)) if e.code == 409 => (parse_apply_conflicts(&e.message), None),
        Err(e) => return Err(e.into()),
    };
    if opts.dry_run {
        // The non-forced dry run stops at the ownership conflicts; validate
        // the rest of the object the way the real save will send it.
        if !takeovers.is_empty() {
            answer = Some(
                api.patch(
                    name,
                    &PatchParams::apply(FIELD_MANAGER).force().dry_run(),
                    &patch,
                )
                .await?,
            );
        }
        return Ok((ApplyOutcome::Applied { takeovers }, answer));
    }

    let mut body = value;
    if let (Some(rv), Some(meta)) = (
        expected,
        body.get_mut("metadata").and_then(Value::as_object_mut),
    ) {
        meta.insert("resourceVersion".into(), Value::String(rv.to_string()));
    }
    match api
        .patch(
            name,
            &PatchParams::apply(FIELD_MANAGER).force(),
            &Patch::Apply(&body),
        )
        .await
    {
        Ok(obj) => Ok((ApplyOutcome::Applied { takeovers }, Some(obj))),
        // A forced apply has no ownership conflicts: a 409 is the
        // resourceVersion precondition.
        Err(kube::Error::Api(e)) if e.code == 409 && expected.is_some() => Ok((
            ApplyOutcome::Conflict {
                resource_version: current_resource_version(api, name).await?,
            },
            None,
        )),
        Err(e) => Err(e.into()),
    }
}

async fn current_resource_version(api: &Api<DynamicObject>, name: &str) -> Result<String> {
    Ok(api
        .get_metadata(name)
        .await?
        .metadata
        .resource_version
        .unwrap_or_default())
}

/// Parse and check edited YAML, and strip what the server owns.
fn prepare_apply(r: &ResourceRef, name: &str, yaml: &str) -> Result<Value> {
    let value: Value = serde_yaml::from_str(yaml).context("invalid YAML")?;
    check_document(r, name, value)
}

/// Check that a parsed document is the object `r`/`name` (and not a Secret
/// with hidden values), and strip what the server owns.
pub(super) fn check_document(r: &ResourceRef, name: &str, mut value: Value) -> Result<Value> {
    if !value.is_object() {
        bail!("the YAML must be a single Kubernetes object");
    }
    if is_secret(r) && contains_redaction(&value) {
        bail!("Secret values are hidden: reveal them before saving");
    }
    strip_server_fields(&mut value);
    let ar = r.api_resource();
    if value.get("apiVersion").and_then(Value::as_str) != Some(ar.api_version.as_str())
        || value.get("kind").and_then(Value::as_str) != Some(ar.kind.as_str())
    {
        bail!(
            "the YAML must be a {} with apiVersion {}",
            ar.kind,
            ar.api_version
        );
    }
    if value.pointer("/metadata/name").and_then(Value::as_str) != Some(name) {
        bail!("metadata.name must stay \"{name}\" (rename is not supported)");
    }
    Ok(value)
}

/// Parse the field-ownership conflicts out of a server-side apply 409
/// message. The API server reports one conflict as
/// `conflict with "mgr" using v1: .data.a`, and several as blocks of
/// `conflicts with "mgr" using apps/v1:` followed by `- .path` lines.
fn parse_apply_conflicts(message: &str) -> Vec<FieldTakeover> {
    let mut out: Vec<FieldTakeover> = Vec::new();
    let mut rest = message;
    while let Some(start) = rest.find(" with \"") {
        let after = &rest[start + " with \"".len()..];
        let Some(end) = after.find('"') else { break };
        let manager = after[..end].to_string();
        let body = &after[end + 1..];
        // The block runs until the next manager's block.
        let next = body
            .find("conflict with \"")
            .or_else(|| body.find("conflicts with \""));
        let block = &body[..next.unwrap_or(body.len())];
        let Some(colon) = block.find(':') else { break };
        let paths = &block[colon + 1..];
        let mut fields: Vec<String> = paths
            .lines()
            .map(|l| l.trim().trim_start_matches("- ").trim())
            .filter(|l| l.starts_with('.'))
            .map(str::to_string)
            .collect();
        fields.dedup();
        if !fields.is_empty() {
            match out.iter_mut().find(|t| t.manager == manager) {
                Some(t) => t.fields.extend(fields),
                None => out.push(FieldTakeover { manager, fields }),
            }
        }
        rest = &body[next.unwrap_or(body.len())..];
    }
    if out.is_empty() {
        // Unknown format: still say that something was taken over.
        out.push(FieldTakeover {
            manager: "another field manager".into(),
            fields: vec![message.trim().to_string()],
        });
    }
    out
}

pub async fn delete_resource(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
) -> Result<()> {
    object_api(client, r, namespace)?
        .delete(name, &DeleteParams::default())
        .await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_conflicts_single() {
        let msg = r#"Apply failed with 1 conflict: conflict with "kubectl-create" using apps/v1: .spec.replicas"#;
        assert_eq!(
            super::parse_apply_conflicts(msg),
            vec![super::FieldTakeover {
                manager: "kubectl-create".into(),
                fields: vec![".spec.replicas".into()],
            }]
        );
    }

    #[test]
    fn apply_conflicts_multiple_managers() {
        let msg = "Apply failed with 3 conflicts: conflicts with \"helm\" using apps/v1:\n\
                   - .spec.replicas\n\
                   - .spec.template.spec.containers[name=\"web\"].image\n\
                   conflict with \"kubectl-edit\" using apps/v1: .metadata.labels.app";
        let got = super::parse_apply_conflicts(msg);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].manager, "helm");
        assert_eq!(
            got[0].fields,
            vec![
                ".spec.replicas".to_string(),
                ".spec.template.spec.containers[name=\"web\"].image".to_string()
            ]
        );
        assert_eq!(got[1].manager, "kubectl-edit");
        assert_eq!(got[1].fields, vec![".metadata.labels.app".to_string()]);
    }

    #[test]
    fn apply_conflicts_unknown_format() {
        let got = super::parse_apply_conflicts("something else went wrong");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].fields, vec!["something else went wrong".to_string()]);
    }

    #[test]
    fn secrets_are_redacted_and_cannot_be_saved_redacted() {
        let mut v = json!({
            "kind": "Secret",
            "metadata": { "annotations": { LAST_APPLIED: "{secret}" , "keep": "me"} },
            "data": { "password": "c2VjcmV0" },
            "stringData": { "x": "y" }
        });
        redact_secret(&mut v);
        assert_eq!(v["data"]["password"], REDACTED);
        assert_eq!(v["stringData"]["x"], REDACTED);
        assert_eq!(v["metadata"]["annotations"][LAST_APPLIED], REDACTED);
        assert_eq!(v["metadata"]["annotations"]["keep"], "me");
        assert!(contains_redaction(&v));
        assert!(!contains_redaction(&json!({"data": {"a": "Yg=="}})));
    }

    #[test]
    fn strip_removes_server_fields_but_keeps_spec() {
        let mut v = json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": { "name": "c", "resourceVersion": "5", "uid": "u",
                "managedFields": [], "creationTimestamp": "t", "generation": 1,
                "labels": {"a": "b"}, "ownerReferences": [{"uid": "o"}] },
            "data": {"k": "v"}, "status": {"x": 1}
        });
        strip_server_fields(&mut v);
        assert_eq!(
            v,
            json!({
                "apiVersion": "v1", "kind": "ConfigMap",
                "metadata": { "name": "c", "labels": {"a": "b"}, "ownerReferences": [{"uid": "o"}] },
                "data": {"k": "v"}
            })
        );
    }

    #[test]
    fn prepare_checks_identity_and_strips_server_fields() {
        let cm = ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "ConfigMap".into(),
            plural: "configmaps".into(),
            namespaced: true,
        };
        let ok = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\n  resourceVersion: '7'\ndata:\n  k: v\n";
        let v = prepare_apply(&cm, "c", ok).unwrap();
        assert!(v.pointer("/metadata/resourceVersion").is_none());
        let renamed = prepare_apply(&cm, "other", ok).unwrap_err().to_string();
        assert!(renamed.contains("metadata.name must stay"), "{renamed}");
        // The parser's position is kept so the editor can point at it.
        let broken = prepare_apply(&cm, "c", "kind: [\n").unwrap_err();
        assert!(format!("{broken:#}").contains("line"), "{broken:#}");
    }

    #[test]
    fn apply_outcome_is_tagged() {
        let out = ApplyOutcome::Conflict {
            resource_version: "9".into(),
        };
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            json!({"outcome": "conflict", "resource_version": "9"})
        );
    }

    #[test]
    fn yaml_puts_identity_first() {
        let v =
            json!({"spec": {"a": 1}, "metadata": {"name": "n"}, "kind": "K", "apiVersion": "v1"});
        let y = serde_yaml::to_string(&ordered(v)).unwrap();
        assert!(y.starts_with("apiVersion: v1\nkind: K\nmetadata:"), "{y}");
    }
}
