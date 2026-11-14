//! `kubeloupe get` and `kubeloupe yaml`: list or fetch objects of any kind
//! and print them as a table, YAML, JSON or `kind/name` lines.

use crate::resources::{self, ResourceKind, ResourceRef};
use anyhow::{bail, Context, Result};
use kube::api::{DynamicObject, ListParams};
use kube::Client;
use serde_json::{json, Value};

/// `-o` of `kubeloupe get`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// The same columns as the app
    #[default]
    Table,
    /// The table plus IP addresses (pods, nodes) and labels
    Wide,
    /// One YAML document (a `List` for several objects)
    Yaml,
    /// Like yaml, as JSON
    Json,
    /// `kind[.group]/name`, one per line
    Name,
}

/// Everything `kubeloupe get` takes besides the kind and name.
#[derive(Debug, Clone, Default)]
pub struct GetOptions {
    /// List only this namespace; `None` lists all of them.
    pub namespace: Option<String>,
    pub output: OutputFormat,
    /// Label selector (`-l app=web,tier!=db`).
    pub selector: Option<String>,
    /// Field selector (`--field-selector status.phase=Running`).
    pub field_selector: Option<String>,
    /// Show Secret values in yaml and json output.
    pub reveal: bool,
}

/// Split `kind/name` or `kind [name]` into the kind and the optional name.
pub fn split_target<'a>(
    first: &'a str,
    second: Option<&'a str>,
) -> Result<(&'a str, Option<&'a str>)> {
    match (first.split_once('/'), second) {
        (Some(_), Some(_)) => bail!("give either <kind>/<name> or <kind> <name>, not both"),
        (Some((k, n)), None) if !k.is_empty() && !n.is_empty() => Ok((k, Some(n))),
        (Some(_), None) => bail!("expected <kind>/<name>, got '{first}'"),
        (None, name) => Ok((first, name)),
    }
}

/// Find a kind (plural, kind, short name or `plural.group`) among what the
/// cluster serves, CRDs included.
pub async fn resolve_kind_named(client: &Client, kind: &str) -> Result<ResourceKind> {
    let kinds = resources::discover(client).await?;
    resources::resolve_kind(&kinds, kind)
        .cloned()
        .with_context(|| format!("unknown resource type: {kind}"))
}

/// The namespace of one named object: none for cluster-scoped kinds, else
/// `-n` or the context's namespace (like kubectl).
pub fn object_namespace(client: &Client, r: &ResourceRef, namespace: Option<&str>) -> String {
    if !r.namespaced {
        return String::new();
    }
    namespace
        .map(str::to_string)
        .unwrap_or_else(|| client.default_namespace().to_string())
}

/// `kubeloupe get <kind> [name]`.
pub async fn print_get(
    client: &Client,
    kind: &str,
    name: Option<&str>,
    opts: &GetOptions,
) -> Result<()> {
    let kind = resolve_kind_named(client, kind).await?;
    let objects = match name {
        Some(name) => {
            if opts.selector.is_some() || opts.field_selector.is_some() {
                bail!("-l and --field-selector filter a list; leave out the name");
            }
            let ns = object_namespace(client, &kind.resource, opts.namespace.as_deref());
            vec![resources::get_resource(client, &kind.resource, &ns, name).await?]
        }
        None => {
            let mut lp = ListParams::default();
            if let Some(s) = &opts.selector {
                lp = lp.labels(s);
            }
            if let Some(f) = &opts.field_selector {
                lp = lp.fields(f);
            }
            let api = resources::dyn_api(client, &kind.resource, opts.namespace.as_deref());
            api.list(&lp).await?.items
        }
    };
    let listed = name.is_none();
    if listed
        && objects.is_empty()
        && matches!(opts.output, OutputFormat::Table | OutputFormat::Wide)
    {
        match &opts.namespace {
            Some(ns) if kind.resource.namespaced => eprintln!("No resources found in {ns}."),
            _ => eprintln!("No resources found."),
        }
        return Ok(());
    }
    print!(
        "{}",
        render_objects(&kind, objects, opts.output, opts.reveal, listed)?
    );
    Ok(())
}

/// `kubeloupe yaml <kind>/<name>`: one object as YAML, Secret values hidden
/// unless `reveal`.
pub async fn print_yaml(
    client: &Client,
    kind: &str,
    name: &str,
    namespace: Option<&str>,
    reveal: bool,
) -> Result<()> {
    let kind = resolve_kind_named(client, kind).await?;
    let ns = object_namespace(client, &kind.resource, namespace);
    let yaml = resources::get_resource_yaml(client, &kind.resource, &ns, name, reveal).await?;
    print!("{yaml}");
    Ok(())
}

/// Render objects of one kind in `format`. `as_list` wraps yaml and json in
/// a `List` (what `get <kind>` prints) rather than printing one object.
pub fn render_objects(
    kind: &ResourceKind,
    mut objects: Vec<DynamicObject>,
    format: OutputFormat,
    reveal: bool,
    as_list: bool,
) -> Result<String> {
    objects.sort_by(|a, b| {
        (&a.metadata.namespace, &a.metadata.name).cmp(&(&b.metadata.namespace, &b.metadata.name))
    });
    Ok(match format {
        OutputFormat::Table => {
            let rows: Vec<_> = objects
                .iter()
                .map(|o| resources::make_row(&kind.columns, o))
                .collect();
            resources::render_table(kind, &rows)
        }
        OutputFormat::Wide => render_wide(kind, &objects),
        OutputFormat::Name => objects
            .iter()
            .map(|o| format!("{}\n", object_name(&kind.resource, o)))
            .collect(),
        OutputFormat::Yaml | OutputFormat::Json => {
            let items = objects
                .into_iter()
                .map(|o| resources::object_value(&kind.resource, o, reveal))
                .collect::<Result<Vec<_>>>()?;
            let value = if as_list {
                json!({
                    "apiVersion": "v1",
                    "kind": "List",
                    "metadata": { "resourceVersion": "" },
                    "items": items,
                })
            } else {
                items.into_iter().next().unwrap_or(Value::Null)
            };
            if format == OutputFormat::Yaml {
                resources::to_yaml(value)?
            } else {
                serde_json::to_string_pretty(&value)? + "\n"
            }
        }
    })
}

/// `kind[.group]/name`, as `kubectl get -o name` prints it.
pub fn object_name(r: &ResourceRef, obj: &DynamicObject) -> String {
    let kind = r.kind.to_lowercase();
    let name = obj.metadata.name.as_deref().unwrap_or_default();
    if r.group.is_empty() {
        format!("{kind}/{name}")
    } else {
        format!("{kind}.{}/{name}", r.group)
    }
}

/// An extra `-o wide` column: its header and how to compute a cell.
type WideColumn = (&'static str, fn(&Value) -> String);

/// The table plus IP addresses (pods, nodes) and a LABELS column.
fn render_wide(kind: &ResourceKind, objects: &[DynamicObject]) -> String {
    let ip: Option<WideColumn> = match (&*kind.resource.group, &*kind.resource.kind) {
        ("", "Pod") => Some(("IP", pod_ip)),
        ("", "Node") => Some(("INTERNAL-IP", node_internal_ip)),
        _ => None,
    };
    let mut header = resources::table_header(kind);
    header.extend(ip.map(|(label, _)| label.to_string()));
    header.push("LABELS".into());
    let mut table = vec![header];
    for obj in objects {
        let row = resources::make_row(&kind.columns, obj);
        let mut cells = resources::table_cells(kind, &row);
        if let Some((_, f)) = ip {
            let value = serde_json::to_value(obj).unwrap_or(Value::Null);
            cells.push(or_none(f(&value)));
        }
        cells.push(labels_text(obj));
        table.push(cells);
    }
    resources::align_columns(&table)
}

fn pod_ip(v: &Value) -> String {
    v.pointer("/status/podIP")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn node_internal_ip(v: &Value) -> String {
    v.pointer("/status/addresses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|a| a["type"] == "InternalIP")
        .and_then(|a| a["address"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn labels_text(obj: &DynamicObject) -> String {
    let labels = obj.metadata.labels.clone().unwrap_or_default();
    or_none(
        labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

fn or_none(s: String) -> String {
    if s.is_empty() {
        "<none>".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::Category;

    fn kind(group: &str, kind: &str, plural: &str) -> ResourceKind {
        let resource = ResourceRef {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: plural.into(),
            namespaced: true,
        };
        ResourceKind {
            id: resource.id(),
            label: kind.into(),
            category: Category::Other,
            crd: false,
            verbs: vec![],
            short_names: vec![],
            columns: vec![],
            actions: vec![],
            resource,
        }
    }

    fn obj(v: Value) -> DynamicObject {
        serde_json::from_value(v).unwrap()
    }

    fn pod(name: &str, ip: &str) -> DynamicObject {
        obj(json!({
            "metadata": { "name": name, "namespace": "demo", "labels": { "app": "web", "tier": "fe" } },
            "status": { "podIP": ip }
        }))
    }

    fn secret() -> DynamicObject {
        obj(json!({
            "metadata": { "name": "creds", "namespace": "demo" },
            "data": { "password": "aHVudGVyMg==" }
        }))
    }

    #[test]
    fn split_target_forms() {
        assert_eq!(split_target("pod/web", None).unwrap(), ("pod", Some("web")));
        assert_eq!(
            split_target("pod", Some("web")).unwrap(),
            ("pod", Some("web"))
        );
        assert_eq!(split_target("pods", None).unwrap(), ("pods", None));
        assert!(split_target("pod/web", Some("x")).is_err());
        assert!(split_target("pod/", None).is_err());
        assert!(split_target("/web", None).is_err());
    }

    #[test]
    fn name_output_includes_group_for_non_core_kinds() {
        let pods = kind("", "Pod", "pods");
        let out = render_objects(
            &pods,
            vec![pod("b", ""), pod("a", "")],
            OutputFormat::Name,
            false,
            true,
        )
        .unwrap();
        assert_eq!(out, "pod/a\npod/b\n");
        let deploy = kind("apps", "Deployment", "deployments");
        let out = render_objects(
            &deploy,
            vec![pod("web", "")],
            OutputFormat::Name,
            false,
            true,
        )
        .unwrap();
        assert_eq!(out, "deployment.apps/web\n");
    }

    #[test]
    fn json_list_fills_kind_and_api_version() {
        let deploy = kind("apps", "Deployment", "deployments");
        let out = render_objects(
            &deploy,
            vec![pod("web", "")],
            OutputFormat::Json,
            false,
            true,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["kind"], "List");
        assert_eq!(v["items"][0]["kind"], "Deployment");
        assert_eq!(v["items"][0]["apiVersion"], "apps/v1");
        assert_eq!(v["items"][0]["metadata"]["name"], "web");
    }

    #[test]
    fn yaml_single_object_is_not_a_list() {
        let pods = kind("", "Pod", "pods");
        let out = render_objects(
            &pods,
            vec![pod("web", "")],
            OutputFormat::Yaml,
            false,
            false,
        )
        .unwrap();
        assert!(
            out.starts_with("apiVersion: v1\nkind: Pod\nmetadata:"),
            "{out}"
        );
    }

    #[test]
    fn secrets_are_redacted_unless_revealed() {
        let secrets = kind("", "Secret", "secrets");
        let hidden =
            render_objects(&secrets, vec![secret()], OutputFormat::Json, false, true).unwrap();
        assert!(hidden.contains(resources::REDACTED), "{hidden}");
        assert!(!hidden.contains("aHVudGVyMg=="));
        let shown =
            render_objects(&secrets, vec![secret()], OutputFormat::Yaml, true, false).unwrap();
        assert!(shown.contains("aHVudGVyMg=="), "{shown}");
    }

    #[test]
    fn wide_adds_ip_and_labels() {
        let pods = kind("", "Pod", "pods");
        let out = render_objects(
            &pods,
            vec![pod("web", "10.0.0.7"), pod("api", "")],
            OutputFormat::Wide,
            false,
            true,
        )
        .unwrap();
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines[0], "IP        LABELS");
        assert_eq!(lines[1], "<none>    app=web,tier=fe");
        assert_eq!(lines[2], "10.0.0.7  app=web,tier=fe");
    }
}
