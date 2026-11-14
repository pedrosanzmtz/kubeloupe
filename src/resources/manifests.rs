//! Reading a manifest file for `apply`: split it into documents, expand
//! `kind: List`, order the objects like `kubectl apply` and fill in the
//! namespace. Pure, so it is unit tested; the cluster side is `apply.rs`.

use serde_json::Value;

/// One object of a manifest file.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Manifest {
    /// 1-based position of its document in the file (the items of a `List`
    /// share their document's number).
    pub document: u32,
    /// 1-based line its document starts on.
    pub line: u32,
    /// The object, or why its document could not be read.
    pub value: Result<Value, String>,
}

/// Split multi-document YAML at `---` lines, skip empty documents, and
/// expand `kind: List` into its items.
pub(super) fn split_manifests(yaml: &str) -> Vec<Manifest> {
    let mut out = Vec::new();
    let mut document = 0u32;
    for (line, text) in documents(yaml) {
        let content = text
            .lines()
            .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
        if !content {
            continue;
        }
        document += 1;
        let push = |out: &mut Vec<Manifest>, value| {
            out.push(Manifest {
                document,
                line,
                value,
            })
        };
        match serde_yaml::from_str::<Value>(&text) {
            Ok(Value::Null) => document -= 1,
            Ok(v) if is_list(&v) => {
                let Some(Value::Array(items)) = v.get("items").cloned() else {
                    continue;
                };
                for item in items {
                    let value = if item.is_object() {
                        Ok(item)
                    } else {
                        Err("a List item must be a Kubernetes object".to_string())
                    };
                    push(&mut out, value);
                }
            }
            Ok(v) if v.is_object() => push(&mut out, Ok(v)),
            Ok(_) => push(
                &mut out,
                Err("the document must be a Kubernetes object".to_string()),
            ),
            Err(e) => {
                let msg = match e.location() {
                    Some(loc) => format!(
                        "invalid YAML at line {} column {}: {}",
                        line as usize + loc.line() - 1,
                        loc.column(),
                        strip_location(&e.to_string())
                    ),
                    None => format!("invalid YAML: {e}"),
                };
                push(&mut out, Err(msg));
            }
        }
    }
    out
}

/// serde_yaml ends its messages with "at line L column C" (relative to the
/// document); the absolute position is reported instead.
fn strip_location(msg: &str) -> &str {
    match msg.rfind(" at line ") {
        Some(i) => &msg[..i],
        None => msg,
    }
}

/// `kind: List` (or `ConfigMapList` and the like, as `kubectl get -o yaml`
/// prints them).
fn is_list(v: &Value) -> bool {
    v.get("kind")
        .and_then(Value::as_str)
        .is_some_and(|k| k.ends_with("List"))
        && v.get("items").is_some_and(Value::is_array)
}

/// The documents of a YAML stream with their 1-based first line. A line
/// starting with `---` separates them; anything after `--- ` on that line
/// (other than a comment) belongs to the next document.
fn documents(yaml: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut start = 1u32;
    let mut current = String::new();
    for (i, line) in yaml.lines().enumerate() {
        let n = i as u32 + 1;
        if let Some(rest) = separator(line) {
            out.push((start, std::mem::take(&mut current)));
            start = n;
            if !rest.is_empty() && !rest.starts_with('#') {
                current.push_str(rest);
                current.push('\n');
            } else {
                start = n + 1;
            }
            continue;
        }
        current.push_str(line);
        current.push('\n');
    }
    out.push((start, current));
    out
}

fn separator(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("---")?;
    if rest.is_empty() || rest.starts_with([' ', '\t']) {
        Some(rest.trim())
    } else {
        None
    }
}

/// The group and version of an `apiVersion` (`apps/v1`, or `v1` for core).
pub(super) fn split_api_version(api_version: &str) -> (&str, &str) {
    api_version.split_once('/').unwrap_or(("", api_version))
}

/// Apply order, like `kubectl apply`: Namespaces, then CRDs, then the rest.
pub(super) fn phase(api_version: &str, kind: &str) -> u8 {
    match (split_api_version(api_version).0, kind) {
        ("", "Namespace") => 0,
        ("apiextensions.k8s.io", "CustomResourceDefinition") => 1,
        _ => 2,
    }
}

/// Set the namespace a document is applied in and return it: its own, or
/// `default` if the kind is namespaced and it has none (`default` itself
/// falls back to "default"). Cluster-scoped kinds get none, as with kubectl.
pub(super) fn set_namespace(value: &mut Value, namespaced: bool, default: &str) -> String {
    let Some(meta) = value
        .as_object_mut()
        .map(|o| {
            o.entry("metadata")
                .or_insert_with(|| Value::Object(Default::default()))
        })
        .and_then(Value::as_object_mut)
    else {
        return String::new();
    };
    if !namespaced {
        meta.remove("namespace");
        return String::new();
    }
    let own = meta
        .get("namespace")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let ns = own.unwrap_or_else(|| {
        if default.is_empty() {
            "default".to_string()
        } else {
            default.to_string()
        }
    });
    meta.insert("namespace".into(), Value::String(ns.clone()));
    ns
}

/// The `(group, kind)` a CRD document defines, if it is one.
pub(super) fn crd_defines(value: &Value) -> Option<(String, String)> {
    let api_version = value.get("apiVersion")?.as_str()?;
    let kind = value.get("kind")?.as_str()?;
    if phase(api_version, kind) != 1 {
        return None;
    }
    Some((
        value.pointer("/spec/group")?.as_str()?.to_string(),
        value.pointer("/spec/names/kind")?.as_str()?.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn values(ms: &[Manifest]) -> Vec<(u32, u32, String)> {
        ms.iter()
            .map(|m| {
                let v = m.value.as_ref().unwrap();
                (
                    m.document,
                    m.line,
                    v["metadata"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn splits_documents_and_skips_empty_ones() {
        let yaml = "---\n# only a comment\n---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---   \n\n--- # c\napiVersion: v1\nkind: Secret\nmetadata:\n  name: b\n---\n";
        let ms = split_manifests(yaml);
        assert_eq!(
            values(&ms),
            vec![(1, 4, "a".to_string()), (2, 11, "b".to_string())]
        );
    }

    #[test]
    fn a_document_without_separator_is_one_object() {
        let ms = split_manifests("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: x\n");
        assert_eq!(values(&ms), vec![(1, 1, "x".to_string())]);
        assert!(split_manifests("").is_empty());
        assert!(split_manifests("\n\n# nothing\n").is_empty());
    }

    #[test]
    fn content_after_the_separator_belongs_to_the_next_document() {
        let ms = split_manifests("--- {apiVersion: v1, kind: ConfigMap, metadata: {name: c}}\n");
        assert_eq!(values(&ms), vec![(1, 1, "c".to_string())]);
        // `----` or `---x` are not separators.
        let ms = split_manifests("a: |\n  ---x\n");
        assert_eq!(ms.len(), 1);
    }

    #[test]
    fn expands_lists() {
        let yaml = "apiVersion: v1\nkind: List\nitems:\n- apiVersion: v1\n  kind: ConfigMap\n  metadata: {name: a}\n- apiVersion: v1\n  kind: ConfigMap\n  metadata: {name: b}\n---\napiVersion: v1\nkind: ConfigMapList\nitems:\n- {apiVersion: v1, kind: ConfigMap, metadata: {name: c}}\n";
        let ms = split_manifests(yaml);
        assert_eq!(
            values(&ms),
            vec![
                (1, 1, "a".to_string()),
                (1, 1, "b".to_string()),
                (2, 11, "c".to_string())
            ]
        );
    }

    #[test]
    fn a_broken_document_reports_its_absolute_line() {
        let yaml =
            "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: a}\n---\napiVersion: v1\nkind: [\n";
        let ms = split_manifests(yaml);
        assert_eq!(ms.len(), 2);
        assert!(ms[0].value.is_ok());
        let err = ms[1].value.as_ref().unwrap_err();
        assert_eq!(ms[1].line, 5);
        assert!(err.starts_with("invalid YAML at line 7"), "{err}");
        let scalar = split_manifests("just text\n");
        assert!(scalar[0].value.as_ref().unwrap_err().contains("must be"));
    }

    #[test]
    fn orders_namespaces_then_crds_then_the_rest() {
        assert_eq!(phase("v1", "Namespace"), 0);
        assert_eq!(
            phase("apiextensions.k8s.io/v1", "CustomResourceDefinition"),
            1
        );
        assert_eq!(phase("apps/v1", "Deployment"), 2);
        // Only the core Namespace kind.
        assert_eq!(phase("example.com/v1", "Namespace"), 2);
    }

    #[test]
    fn fills_in_the_namespace_of_namespaced_kinds_only() {
        let mut v = json!({"kind": "ConfigMap", "metadata": {"name": "a"}});
        assert_eq!(set_namespace(&mut v, true, "team"), "team");
        assert_eq!(v["metadata"]["namespace"], "team");

        let mut own = json!({"metadata": {"name": "a", "namespace": "mine"}});
        assert_eq!(set_namespace(&mut own, true, "team"), "mine");

        let mut none = json!({"kind": "ConfigMap"});
        assert_eq!(set_namespace(&mut none, true, ""), "default");
        assert_eq!(none["metadata"]["namespace"], "default");

        let mut cluster = json!({"metadata": {"name": "n", "namespace": "x"}});
        assert_eq!(set_namespace(&mut cluster, false, "team"), "");
        assert!(cluster["metadata"].get("namespace").is_none());
    }

    #[test]
    fn finds_what_a_crd_defines() {
        let crd = json!({
            "apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
            "spec": {"group": "example.com", "names": {"kind": "Widget"}}
        });
        assert_eq!(
            crd_defines(&crd),
            Some(("example.com".to_string(), "Widget".to_string()))
        );
        assert_eq!(
            crd_defines(&json!({"apiVersion": "v1", "kind": "Pod"})),
            None
        );
    }
}
