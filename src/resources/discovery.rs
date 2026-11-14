//! API discovery, access checks and resolving a kind by name.

use super::columns::{columns_for, spec_for, PrinterColumn, KINDS};
use super::*;
use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, ListParams};
use kube::Client;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Discovery.
// ---------------------------------------------------------------------------

/// Used only when the CRD list itself cannot be read: groups that are not
/// part of the built-in API look like CRDs.
fn looks_custom(group: &str) -> bool {
    group.contains('.') && !group.ends_with(".k8s.io")
}

type PrinterColumns = HashMap<(String, String, String), Vec<PrinterColumn>>;

/// Printer columns of every CRD version, keyed by (group, plural, version).
/// `None` if the CRDs cannot be listed.
async fn crd_printer_columns(client: &Client) -> Option<PrinterColumns> {
    let api: Api<CustomResourceDefinition> = Api::all(client.clone());
    let list = api.list(&ListParams::default()).await.ok()?;
    let mut out = HashMap::new();
    for crd in list.items {
        for v in &crd.spec.versions {
            let cols = v
                .additional_printer_columns
                .iter()
                .flatten()
                .map(|c| PrinterColumn {
                    name: c.name.clone(),
                    type_: c.type_.clone(),
                    json_path: c.json_path.clone(),
                    priority: c.priority.unwrap_or(0),
                })
                .collect();
            out.insert(
                (
                    crd.spec.group.clone(),
                    crd.spec.names.plural.clone(),
                    v.name.clone(),
                ),
                cols,
            );
        }
    }
    Some(out)
}

/// Ask the server what it serves: every kind of every group (at the group's
/// preferred version), including CRDs, with scope, verbs and columns.
///
/// This is a hand-rolled version of `kube::discovery::Discovery::run`, which
/// queries every version of every group and fails as a whole when a single
/// aggregated API (an unhealthy metrics-server, say) is down. Here a failing
/// group is skipped, as `kubectl` does.
pub async fn discover(client: &Client) -> Result<Vec<ResourceKind>> {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::APIResourceList;

    let groups = client.list_api_groups().await?.groups;
    let mut wanted: Vec<(bool, String)> = groups
        .iter()
        .filter_map(|g| {
            let v = g
                .preferred_version
                .as_ref()
                .or_else(|| g.versions.first())?;
            Some((false, v.group_version.clone()))
        })
        .collect();
    wanted.push((true, "v1".to_string()));

    let lists: Vec<Option<APIResourceList>> = futures::stream::iter(wanted)
        .map(|(core, gv)| {
            let client = client.clone();
            async move {
                let res = if core {
                    client.list_core_api_resources(&gv).await
                } else {
                    client.list_api_group_resources(&gv).await
                };
                match res {
                    Ok(l) => Some(l),
                    Err(e) => {
                        tracing::warn!("discovery of {gv} failed: {e}");
                        None
                    }
                }
            }
        })
        .buffer_unordered(16)
        .collect()
        .await;

    let printer = crd_printer_columns(client).await;
    let crd_set: Option<HashSet<(String, String)>> = printer.as_ref().map(|m| {
        m.keys()
            .map(|(g, p, _)| (g.clone(), p.clone()))
            .collect::<HashSet<_>>()
    });

    let mut kinds = Vec::new();
    for list in lists.into_iter().flatten() {
        let (group, version) = match list.group_version.split_once('/') {
            Some((g, v)) => (g.to_string(), v.to_string()),
            None => (String::new(), list.group_version.clone()),
        };
        // events.k8s.io duplicates the core Events.
        if group == "events.k8s.io" {
            continue;
        }
        for r in list.resources {
            if r.name.contains('/') || !r.verbs.iter().any(|v| v == "list") {
                continue;
            }
            let resource = ResourceRef {
                group: group.clone(),
                version: version.clone(),
                kind: r.kind.clone(),
                plural: r.name.clone(),
                namespaced: r.namespaced,
            };
            let spec = spec_for(&group, &r.kind);
            let crd = match &crd_set {
                Some(set) => set.contains(&(group.clone(), r.name.clone())),
                None => looks_custom(&group),
            };
            let printer_cols = printer
                .as_ref()
                .and_then(|m| m.get(&(group.clone(), r.name.clone(), version.clone())))
                .map(Vec::as_slice)
                .unwrap_or_default();
            kinds.push(ResourceKind {
                id: resource.id(),
                label: spec.map_or_else(|| title_case(&r.name), |s| s.label.to_string()),
                category: match spec {
                    Some(s) => s.category,
                    None if crd => Category::Custom,
                    None => Category::Other,
                },
                crd,
                verbs: r.verbs.clone(),
                short_names: r.short_names.clone().unwrap_or_default(),
                columns: columns_for(&resource, printer_cols),
                actions: crate::actions::actions_for(&resource, &r.verbs),
                resource,
            });
        }
    }
    // Registry kinds in registry order, then the rest by group and kind.
    let rank = |k: &ResourceKind| {
        KINDS
            .iter()
            .position(|s| s.group == k.resource.group && s.kind == k.resource.kind)
    };
    kinds.sort_by(|a, b| match (rank(a), rank(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => {
            (&a.resource.group, &a.resource.kind).cmp(&(&b.resource.group, &b.resource.kind))
        }
    });
    Ok(kinds)
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// Which of `kinds` the current user may list (cluster-wide for namespaced
/// kinds), via SelfSubjectAccessReview. A check that cannot be made counts as
/// allowed, so a restrictive cluster never hides everything.
pub async fn check_access(client: &Client, kinds: &[ResourceRef]) -> Vec<AccessResult> {
    let api: Api<SelfSubjectAccessReview> = Api::all(client.clone());
    futures::stream::iter(kinds.iter().cloned())
        .map(|r| {
            let api = api.clone();
            async move {
                let review = SelfSubjectAccessReview {
                    spec: SelfSubjectAccessReviewSpec {
                        resource_attributes: Some(ResourceAttributes {
                            verb: Some("list".into()),
                            group: Some(r.group.clone()),
                            resource: Some(r.plural.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let allowed = match api.create(&Default::default(), &review).await {
                    Ok(res) => res.status.map(|s| s.allowed).unwrap_or(true),
                    Err(_) => true,
                };
                AccessResult {
                    id: r.id(),
                    allowed,
                }
            }
        })
        .buffer_unordered(16)
        .collect()
        .await
}

/// Aliases kubectl users type that discovery does not provide.
fn alias_of(name: &str) -> &str {
    match name {
        "po" => "pods",
        "deploy" => "deployments",
        "sts" => "statefulsets",
        "ds" => "daemonsets",
        "rs" => "replicasets",
        "cj" => "cronjobs",
        "svc" => "services",
        "ing" => "ingresses",
        "netpol" => "networkpolicies",
        "cm" => "configmaps",
        "hpa" => "horizontalpodautoscalers",
        "quota" => "resourcequotas",
        "pvc" => "persistentvolumeclaims",
        "pv" => "persistentvolumes",
        "sc" => "storageclasses",
        "no" => "nodes",
        "ns" => "namespaces",
        "ev" => "events",
        "ep" => "endpoints",
        other => other,
    }
}

/// Find a kind by what a person types: plural, kind, short name, or
/// `plural.group`. Ambiguities resolve to the first match in sidebar order.
pub fn resolve_kind<'a>(kinds: &'a [ResourceKind], name: &str) -> Option<&'a ResourceKind> {
    let q = alias_of(&name.to_lowercase()).to_string();
    kinds.iter().find(|k| {
        let r = &k.resource;
        q == r.plural
            || q == r.kind.to_lowercase()
            || k.short_names.contains(&q)
            || (!r.group.is_empty() && q == format!("{}.{}", r.plural, r.group))
    })
}
#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn resolve_kind_by_plural_kind_alias_and_group() {
        let kinds = vec![
            kind("apps", "Deployment", "deployments", &["deploy"]),
            kind("example.com", "Widget", "widgets", &["wg"]),
        ];
        for q in ["deployments", "Deployment", "deploy", "DEPLOY"] {
            assert_eq!(resolve_kind(&kinds, q).unwrap().resource.kind, "Deployment");
        }
        assert_eq!(resolve_kind(&kinds, "wg").unwrap().resource.kind, "Widget");
        assert_eq!(
            resolve_kind(&kinds, "widgets.example.com")
                .unwrap()
                .resource
                .kind,
            "Widget"
        );
        assert!(resolve_kind(&kinds, "nope").is_none());
    }
}
