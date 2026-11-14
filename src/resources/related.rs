//! Related objects and a per-kind summary for the detail panel (and the
//! CLI's `describe`): a Deployment's ReplicaSets and pods, a Service's
//! endpoints and pods, the pods on a Node, the pods using a ConfigMap...
//!
//! What each kind shows is one entry in [`KINDS`]: a summary, facts, and a
//! list of [`Rel`]ations. Every request is scoped by a namespace or a
//! selector; nothing scans the whole cluster.

use super::related_local::{self as local, RefKind};
use super::related_summary as summary;
use super::{get_resource, ops::dyn_api, ResourceRef};
use crate::actions::describe_error;
use anyhow::Result;
use futures::future::join_all;
use kube::api::{DynamicObject, ListParams};
use kube::Client;
use serde::Serialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Types shared with the UI.
// ---------------------------------------------------------------------------

/// The per-kind part of the detail panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Related {
    /// The one-line summary, in parts ("3/3 ready", "RollingUpdate 25%/25%",
    /// "revision 4"); shown joined with " · ".
    pub summary: Vec<String>,
    /// Labelled facts ("Rollout": "complete", "Kubelet": "v1.31.0").
    pub facts: Vec<Fact>,
    pub groups: Vec<RelatedGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Fact {
    pub label: String,
    pub value: String,
}

/// A small table of related objects (or of plain rows: ports, taints).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RelatedGroup {
    pub title: String,
    /// apiVersion and kind of the rows' objects; both empty when the rows
    /// are not objects (ports, taints, metrics).
    pub api_version: String,
    pub kind: String,
    pub columns: Vec<String>,
    /// Index of the column that names the row's object (shown as a link).
    pub name_column: u32,
    pub items: Vec<RelatedItem>,
    /// Why the group could not be loaded; empty if it was.
    pub error: String,
}

/// One row: the object it stands for (empty for plain rows) and its cells,
/// one per column.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RelatedItem {
    pub namespace: String,
    pub name: String,
    pub cells: Vec<String>,
}

impl Fact {
    pub(super) fn new(label: &str, value: impl Into<String>) -> Self {
        Fact {
            label: label.into(),
            value: value.into(),
        }
    }
}

impl RelatedGroup {
    /// A group of objects of `kind` (`api_version`).
    pub(super) fn objects(
        title: &str,
        api_version: &str,
        kind: &str,
        columns: &[&str],
        items: Vec<RelatedItem>,
    ) -> Self {
        RelatedGroup {
            title: title.into(),
            api_version: api_version.into(),
            kind: kind.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            items,
            ..Default::default()
        }
    }

    /// A group of plain rows.
    pub(super) fn plain(title: &str, columns: &[&str], rows: Vec<Vec<String>>) -> Self {
        let items = rows
            .into_iter()
            .map(|cells| RelatedItem {
                cells,
                ..Default::default()
            })
            .collect();
        Self::objects(title, "", "", columns, items)
    }

    /// The named object is in column `i` instead of the first.
    pub(super) fn name_in(mut self, i: u32) -> Self {
        self.name_column = i;
        self
    }

    /// The group as an aligned text table, for the CLI.
    pub fn to_text(&self) -> String {
        let mut rows = vec![self.columns.clone()];
        rows.extend(self.items.iter().map(|i| i.cells.clone()));
        let widths: Vec<usize> = (0..self.columns.len())
            .map(|c| {
                rows.iter()
                    .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let mut out = format!("{}:\n", self.title);
        if !self.error.is_empty() {
            out.push_str(&format!("  {}\n", self.error));
            return out;
        }
        if self.items.is_empty() {
            out.push_str("  <none>\n");
            return out;
        }
        for r in rows {
            let line: Vec<String> = r
                .iter()
                .enumerate()
                .map(|(c, s)| format!("{s:<w$}", w = widths.get(c).copied().unwrap_or(0)))
                .collect();
            out.push_str(&format!("  {}\n", line.join("  ").trim_end()));
        }
        out
    }
}

impl RelatedItem {
    pub(super) fn object(namespace: &str, name: &str, cells: Vec<String>) -> Self {
        RelatedItem {
            namespace: namespace.into(),
            name: name.into(),
            cells,
        }
    }

    /// The row of `obj` (a listed object), with `cells`.
    pub(super) fn of(obj: &Value, cells: Vec<String>) -> Self {
        Self::object(
            &local::str_at(obj, "/metadata/namespace"),
            &name_of(obj),
            cells,
        )
    }
}

pub(super) fn name_of(obj: &Value) -> String {
    local::str_at(obj, "/metadata/name")
}

// ---------------------------------------------------------------------------
// The registry.
// ---------------------------------------------------------------------------

/// A kind of object a relation lists, and how its rows look.
pub(super) struct Listed {
    title: &'static str,
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    plural: &'static str,
    columns: &'static [&'static str],
    cells: fn(&Value) -> Vec<String>,
    /// Order of the rows (the list comes back sorted by name).
    sort: fn(&mut [Value]),
}

impl Listed {
    fn resource(&self) -> ResourceRef {
        ResourceRef {
            group: self.group.into(),
            version: self.version.into(),
            kind: self.kind.into(),
            plural: self.plural.into(),
            namespaced: true,
        }
    }

    fn group(&self, title: &str, objs: Vec<Value>) -> RelatedGroup {
        let mut objs = objs;
        (self.sort)(&mut objs);
        let items = objs
            .iter()
            .map(|o| RelatedItem::of(o, (self.cells)(o)))
            .collect();
        RelatedGroup::objects(
            title,
            &self.resource().api_resource().api_version,
            self.kind,
            self.columns,
            items,
        )
    }

    fn failed(&self, title: &str, error: String) -> RelatedGroup {
        RelatedGroup {
            error,
            ..self.group(title, Vec::new())
        }
    }
}

fn by_name(_: &mut [Value]) {}

const PODS: Listed = Listed {
    title: "Pods",
    group: "",
    version: "v1",
    kind: "Pod",
    plural: "pods",
    columns: &["Name", "Ready", "Status", "Restarts", "Age"],
    cells: local::pod_cells,
    sort: by_name,
};

/// Pods on a node, from every namespace.
const NODE_PODS: Listed = Listed {
    columns: &["Name", "Namespace", "Ready", "Status", "Restarts"],
    cells: local::node_pod_cells,
    ..PODS
};

/// Pods that use a ConfigMap, Secret or PVC (filled in by [`Rel::PodsUsing`]).
const USING_PODS: Listed = Listed {
    title: "Used by pods",
    columns: &["Name", "Ready", "Status", "Used as"],
    cells: local::pod_cells,
    ..PODS
};

const REPLICASETS: Listed = Listed {
    title: "ReplicaSets",
    group: "apps",
    version: "v1",
    kind: "ReplicaSet",
    plural: "replicasets",
    columns: &["Name", "Revision", "Ready", "Age"],
    cells: local::replicaset_cells,
    sort: local::by_revision_desc,
};

const JOBS: Listed = Listed {
    title: "Jobs",
    group: "batch",
    version: "v1",
    kind: "Job",
    plural: "jobs",
    columns: &["Name", "Status", "Completions", "Duration", "Age"],
    cells: local::job_cells,
    sort: local::newest_first,
};

const ENDPOINT_SLICES: Listed = Listed {
    title: "EndpointSlices",
    group: "discovery.k8s.io",
    version: "v1",
    kind: "EndpointSlice",
    plural: "endpointslices",
    columns: &["Name", "Ready", "Addresses", "Ports"],
    cells: local::slice_cells,
    sort: by_name,
};

const INGRESSES: Listed = Listed {
    title: "Ingresses",
    group: "networking.k8s.io",
    version: "v1",
    kind: "Ingress",
    plural: "ingresses",
    columns: &["Name", "Hosts", "Paths"],
    cells: local::ingress_cells,
    sort: by_name,
};

const SERVICES: Listed = Listed {
    title: "Services",
    group: "",
    version: "v1",
    kind: "Service",
    plural: "services",
    columns: &["Name", "Type", "Cluster IP", "Ports"],
    cells: local::service_cells,
    sort: by_name,
};

/// How to find one group of related objects.
pub(super) enum Rel {
    /// Groups computed from the object alone (ports, rules, taints, links).
    Local(fn(&Value) -> Vec<RelatedGroup>),
    /// Objects in the namespace whose controller is this object, narrowed
    /// by the label selector at the pointer when there is one.
    Owned(&'static Listed, &'static str),
    /// Pods matching the LabelSelector at the pointer.
    Pods(&'static str),
    /// Pods matching the plain label map at the pointer (a Service's selector).
    PodsByLabels(&'static str),
    /// A Service's EndpointSlices (`kubernetes.io/service-name`).
    EndpointSlices,
    /// Ingresses in the namespace that route to this Service.
    IngressesRouting,
    /// Pods on this Node (`spec.nodeName`), and its requests vs. allocatable.
    NodePods,
    /// Pods in the namespace that mount or reference this object.
    PodsUsing(RefKind),
    /// Services in the namespace whose selector matches this pod.
    ServicesSelecting,
}

/// What the panel shows for one kind.
pub(super) struct KindRelated {
    group: &'static str,
    kind: &'static str,
    summary: fn(&Value) -> Vec<String>,
    facts: fn(&Value) -> Vec<Fact>,
    rels: &'static [Rel],
}

fn no_facts(_: &Value) -> Vec<Fact> {
    Vec::new()
}

fn no_summary(_: &Value) -> Vec<String> {
    Vec::new()
}

/// Adding a kind is one entry here.
const KINDS: &[KindRelated] = &[
    KindRelated {
        group: "apps",
        kind: "Deployment",
        summary: summary::deployment,
        facts: summary::deployment_facts,
        rels: &[
            Rel::Owned(&REPLICASETS, "/spec/selector"),
            Rel::Pods("/spec/selector"),
        ],
    },
    KindRelated {
        group: "apps",
        kind: "StatefulSet",
        summary: summary::statefulset,
        facts: summary::statefulset_facts,
        rels: &[
            Rel::Pods("/spec/selector"),
            Rel::Local(local::statefulset_service),
        ],
    },
    KindRelated {
        group: "apps",
        kind: "DaemonSet",
        summary: summary::daemonset,
        facts: no_facts,
        rels: &[Rel::Pods("/spec/selector")],
    },
    KindRelated {
        group: "apps",
        kind: "ReplicaSet",
        summary: summary::replicaset,
        facts: no_facts,
        rels: &[Rel::Pods("/spec/selector")],
    },
    KindRelated {
        group: "batch",
        kind: "Job",
        summary: summary::job,
        facts: summary::job_facts,
        rels: &[Rel::Pods("/spec/selector")],
    },
    KindRelated {
        group: "batch",
        kind: "CronJob",
        summary: summary::cronjob,
        facts: summary::cronjob_facts,
        rels: &[Rel::Owned(&JOBS, "")],
    },
    KindRelated {
        group: "",
        kind: "Service",
        summary: summary::service,
        facts: summary::service_facts,
        rels: &[
            Rel::Local(local::service_ports),
            Rel::PodsByLabels("/spec/selector"),
            Rel::EndpointSlices,
            Rel::IngressesRouting,
        ],
    },
    KindRelated {
        group: "networking.k8s.io",
        kind: "Ingress",
        summary: summary::ingress,
        facts: summary::ingress_facts,
        rels: &[Rel::Local(local::ingress_rules)],
    },
    KindRelated {
        group: "",
        kind: "Node",
        summary: summary::node,
        facts: summary::node_facts,
        rels: &[Rel::NodePods, Rel::Local(local::node_taints)],
    },
    KindRelated {
        group: "",
        kind: "PersistentVolumeClaim",
        summary: summary::pvc,
        facts: no_facts,
        rels: &[
            Rel::Local(local::pvc_volume),
            Rel::PodsUsing(RefKind::Claim),
        ],
    },
    KindRelated {
        group: "",
        kind: "ConfigMap",
        summary: summary::configmap,
        facts: no_facts,
        rels: &[Rel::PodsUsing(RefKind::ConfigMap)],
    },
    KindRelated {
        group: "",
        kind: "Secret",
        summary: summary::secret,
        facts: no_facts,
        rels: &[Rel::PodsUsing(RefKind::Secret)],
    },
    KindRelated {
        group: "autoscaling",
        kind: "HorizontalPodAutoscaler",
        summary: summary::hpa,
        facts: summary::hpa_facts,
        rels: &[
            Rel::Local(local::hpa_target),
            Rel::Local(local::hpa_metrics),
        ],
    },
    KindRelated {
        group: "",
        kind: "Pod",
        summary: no_summary,
        facts: no_facts,
        rels: &[Rel::ServicesSelecting, Rel::Local(local::pod_mounts)],
    },
];

fn spec_of(r: &ResourceRef) -> Option<&'static KindRelated> {
    KINDS
        .iter()
        .find(|k| k.group == r.group && k.kind == r.kind)
}

// ---------------------------------------------------------------------------
// Fetching.
// ---------------------------------------------------------------------------

/// Get the object, then its [`related_objects`].
pub async fn get_related(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
) -> Result<Related> {
    let obj = get_resource(client, r, namespace, name).await?;
    Ok(related_objects(client, r, &obj).await)
}

/// The summary, facts and related objects of `obj` (of kind `r`). Kinds
/// without an entry get an empty [`Related`]. A group that cannot be
/// loaded (no permission, say) carries its error instead of failing all.
pub async fn related_objects(client: &Client, r: &ResourceRef, obj: &DynamicObject) -> Related {
    let Some(spec) = spec_of(r) else {
        return Related::default();
    };
    let v = serde_json::to_value(obj).unwrap_or(Value::Null);
    let ctx = Ctx {
        client,
        v: &v,
        namespace: obj.metadata.namespace.clone().unwrap_or_default(),
        name: obj.metadata.name.clone().unwrap_or_default(),
        uid: obj.metadata.uid.clone().unwrap_or_default(),
    };
    let groups = join_all(spec.rels.iter().map(|rel| ctx.fetch(rel)))
        .await
        .into_iter()
        .flatten()
        .collect();
    Related {
        summary: (spec.summary)(&v),
        facts: (spec.facts)(&v),
        groups,
    }
}

struct Ctx<'a> {
    client: &'a Client,
    v: &'a Value,
    namespace: String,
    name: String,
    uid: String,
}

impl Ctx<'_> {
    async fn list(
        &self,
        what: &Listed,
        namespace: Option<&str>,
        lp: ListParams,
    ) -> Result<Vec<Value>, String> {
        let api = dyn_api(self.client, &what.resource(), namespace);
        let list = api.list(&lp).await.map_err(|e| describe_error(&e.into()))?;
        Ok(list
            .items
            .iter()
            .filter_map(|o| serde_json::to_value(o).ok())
            .collect())
    }

    /// Objects of `what` in the object's namespace, filtered by `keep`.
    async fn listed(
        &self,
        what: &Listed,
        lp: ListParams,
        keep: impl Fn(&Value) -> bool,
    ) -> RelatedGroup {
        match self.list(what, Some(&self.namespace), lp).await {
            Ok(objs) => what.group(what.title, objs.into_iter().filter(keep).collect()),
            Err(e) => what.failed(what.title, e),
        }
    }

    async fn fetch(&self, rel: &Rel) -> Vec<RelatedGroup> {
        let ns = self.namespace.as_str();
        match rel {
            Rel::Local(f) => f(self.v),
            Rel::Owned(what, selector) => {
                let mut lp = ListParams::default();
                if let Some(s) = self.v.pointer(selector).and_then(local::label_selector) {
                    lp = lp.labels(&s);
                }
                let uid = self.uid.as_str();
                vec![self.listed(what, lp, |o| local::owned_by(o, uid)).await]
            }
            Rel::Pods(selector) => {
                let Some(s) = self.v.pointer(selector).and_then(local::label_selector) else {
                    return vec![PODS.group("Pods", Vec::new())];
                };
                vec![
                    self.listed(&PODS, ListParams::default().labels(&s), |_| true)
                        .await,
                ]
            }
            Rel::PodsByLabels(selector) => {
                let Some(s) = self.v.pointer(selector).and_then(local::map_selector) else {
                    // A Service without a selector has its endpoints set by hand.
                    return Vec::new();
                };
                vec![
                    self.listed(&PODS, ListParams::default().labels(&s), |_| true)
                        .await,
                ]
            }
            Rel::EndpointSlices => {
                let lp = ListParams::default()
                    .labels(&format!("kubernetes.io/service-name={}", self.name));
                vec![self.listed(&ENDPOINT_SLICES, lp, |_| true).await]
            }
            Rel::IngressesRouting => {
                let name = self.name.as_str();
                vec![
                    self.listed(&INGRESSES, ListParams::default(), |i| {
                        local::ingress_services(i).iter().any(|s| s == name)
                    })
                    .await,
                ]
            }
            Rel::NodePods => {
                let lp = ListParams::default().fields(&format!("spec.nodeName={}", self.name));
                match self.list(&NODE_PODS, None, lp).await {
                    Ok(pods) => vec![
                        NODE_PODS.group("Pods", pods.clone()),
                        local::node_capacity(self.v, &pods),
                    ],
                    Err(e) => vec![NODE_PODS.failed("Pods", e)],
                }
            }
            Rel::PodsUsing(kind) => {
                let name = self.name.as_str();
                match self
                    .list(&USING_PODS, Some(ns), ListParams::default())
                    .await
                {
                    Ok(pods) => vec![using_group(
                        &USING_PODS,
                        local::pods_using(pods, *kind, name),
                    )],
                    Err(e) => vec![USING_PODS.failed(USING_PODS.title, e)],
                }
            }
            Rel::ServicesSelecting => {
                let labels = self.v.pointer("/metadata/labels").cloned();
                vec![
                    self.listed(&SERVICES, ListParams::default(), |s| {
                        local::selects(s.pointer("/spec/selector"), labels.as_ref())
                    })
                    .await,
                ]
            }
        }
    }
}

/// Group of the pods using a ConfigMap, Secret or PVC, with how they use it.
fn using_group(what: &Listed, rows: Vec<(Value, String)>) -> RelatedGroup {
    let objs: Vec<Value> = rows.iter().map(|(p, _)| p.clone()).collect();
    let mut g = what.group(what.title, objs);
    // `group` keeps the list order, so the rows line up.
    for (item, (pod, how)) in g.items.iter_mut().zip(rows) {
        item.cells = local::pod_cells(&pod)
            .into_iter()
            .take(3)
            .chain([how])
            .collect();
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_each_kind_once() {
        for (i, k) in KINDS.iter().enumerate() {
            assert!(
                KINDS[..i]
                    .iter()
                    .all(|o| (o.group, o.kind) != (k.group, k.kind)),
                "{} twice",
                k.kind
            );
        }
        let r = crate::pod_ref();
        assert!(spec_of(&r).is_some());
    }

    #[test]
    fn listed_groups_name_their_kind() {
        let g = REPLICASETS.group("ReplicaSets", Vec::new());
        assert_eq!(
            (g.api_version.as_str(), g.kind.as_str()),
            ("apps/v1", "ReplicaSet")
        );
        assert_eq!(PODS.group("Pods", Vec::new()).api_version, "v1");
        assert_eq!(USING_PODS.columns.len(), 4);
    }

    #[test]
    fn text_rendering_aligns_columns() {
        let g = RelatedGroup::plain(
            "Ports",
            &["Name", "Port"],
            vec![
                vec!["http".into(), "80".into()],
                vec!["metrics".into(), "9090".into()],
            ],
        );
        assert_eq!(
            g.to_text(),
            "Ports:\n  Name     Port\n  http     80\n  metrics  9090\n"
        );
        let empty = RelatedGroup::plain("Taints", &["Key"], vec![]);
        assert_eq!(empty.to_text(), "Taints:\n  <none>\n");
        let failed = RelatedGroup {
            error: "pods is forbidden".into(),
            ..empty
        };
        assert_eq!(failed.to_text(), "Taints:\n  pods is forbidden\n");
    }
}
