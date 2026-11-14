//! The column registry ([`KINDS`]) and how a row's cells are filled.

use super::jsonpath::{
    array_at, cell_text, count_at, count_keys, eval_str, first_of, parse_path, path_i64, path_str,
    ratio, Seg,
};
use super::*;
use crate::created_secs;
use kube::api::DynamicObject;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Column registry.
// ---------------------------------------------------------------------------

/// Where a cell's text comes from.
#[derive(Debug, Clone, Default)]
pub(super) enum Source {
    /// Filled by the UI (age, metrics).
    #[default]
    Ui,
    /// A (JSONPath-like) path into the object.
    Path(Vec<Seg>),
    /// Computed from the whole object.
    Func(fn(&Value) -> String),
}

#[derive(Clone, Copy)]
enum StaticSrc {
    Ui,
    Path(&'static str),
    Func(fn(&Value) -> String),
}

/// A static column definition in [`KINDS`].
struct Col {
    key: &'static str,
    label: &'static str,
    kind: ColumnKind,
    src: StaticSrc,
}

const fn text(key: &'static str, label: &'static str, path: &'static str) -> Col {
    Col {
        key,
        label,
        kind: ColumnKind::Text,
        src: StaticSrc::Path(path),
    }
}
const fn number(key: &'static str, label: &'static str, path: &'static str) -> Col {
    Col {
        key,
        label,
        kind: ColumnKind::Number,
        src: StaticSrc::Path(path),
    }
}
const fn status(key: &'static str, label: &'static str, f: fn(&Value) -> String) -> Col {
    Col {
        key,
        label,
        kind: ColumnKind::Status,
        src: StaticSrc::Func(f),
    }
}
const fn func(key: &'static str, label: &'static str, f: fn(&Value) -> String) -> Col {
    Col {
        key,
        label,
        kind: ColumnKind::Text,
        src: StaticSrc::Func(f),
    }
}
const fn ui(key: &'static str, label: &'static str, kind: ColumnKind) -> Col {
    Col {
        key,
        label,
        kind,
        src: StaticSrc::Ui,
    }
}

const NAME: Col = text("name", "Name", ".metadata.name");
const NAMESPACE: Col = text("namespace", "Namespace", ".metadata.namespace");
const AGE: Col = ui("age", "Age", ColumnKind::Age);

/// A built-in kind: where it sits in the sidebar and what its table shows.
pub(super) struct KindSpec {
    pub(super) group: &'static str,
    pub(super) kind: &'static str,
    pub(super) label: &'static str,
    pub(super) category: Category,
    columns: &'static [Col],
}

use Category::{Cluster, Config, Network, Storage, Workloads};

/// The column registry. Order here is the sidebar order. To add columns for
/// another built-in kind, add one entry (a path into the object, or a small
/// function for derived values). Namespace columns are dropped automatically
/// for cluster-scoped kinds.
pub(super) const KINDS: &[KindSpec] = &[
    KindSpec {
        group: "",
        kind: "Pod",
        label: "Pods",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            func("ready", "Ready", pod_ready),
            status("status", "Status", pod_status),
            func("restarts", "Restarts", pod_restarts),
            text("node", "Node", ".spec.nodeName"),
            AGE,
            ui("cpu", "CPU", ColumnKind::PodCpu),
            ui("memory", "Memory", ColumnKind::PodMemory),
        ],
    },
    KindSpec {
        group: "apps",
        kind: "Deployment",
        label: "Deployments",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            func("ready", "Ready", |v| {
                ratio(v, ".status.readyReplicas", ".spec.replicas")
            }),
            number("uptodate", "Up-to-date", ".status.updatedReplicas"),
            number("available", "Available", ".status.availableReplicas"),
            AGE,
        ],
    },
    KindSpec {
        group: "apps",
        kind: "StatefulSet",
        label: "StatefulSets",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            func("ready", "Ready", |v| {
                ratio(v, ".status.readyReplicas", ".spec.replicas")
            }),
            AGE,
        ],
    },
    KindSpec {
        group: "apps",
        kind: "DaemonSet",
        label: "DaemonSets",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            number("desired", "Desired", ".status.desiredNumberScheduled"),
            number("current", "Current", ".status.currentNumberScheduled"),
            number("ready", "Ready", ".status.numberReady"),
            number("uptodate", "Up-to-date", ".status.updatedNumberScheduled"),
            number("available", "Available", ".status.numberAvailable"),
            AGE,
        ],
    },
    KindSpec {
        group: "apps",
        kind: "ReplicaSet",
        label: "ReplicaSets",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            number("desired", "Desired", ".spec.replicas"),
            number("current", "Current", ".status.replicas"),
            number("ready", "Ready", ".status.readyReplicas"),
            AGE,
        ],
    },
    KindSpec {
        group: "batch",
        kind: "Job",
        label: "Jobs",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            func("completions", "Completions", |v| {
                ratio(v, ".status.succeeded", ".spec.completions")
            }),
            number("active", "Active", ".status.active"),
            AGE,
        ],
    },
    KindSpec {
        group: "batch",
        kind: "CronJob",
        label: "CronJobs",
        category: Workloads,
        columns: &[
            NAME,
            NAMESPACE,
            text("schedule", "Schedule", ".spec.schedule"),
            text("suspend", "Suspend", ".spec.suspend"),
            func("active", "Active", |v| count_at(v, ".status.active")),
            text("last", "Last schedule", ".status.lastScheduleTime"),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "Service",
        label: "Services",
        category: Network,
        columns: &[
            NAME,
            NAMESPACE,
            text("type", "Type", ".spec.type"),
            text("clusterip", "Cluster IP", ".spec.clusterIP"),
            func("externalip", "External IP", service_external_ip),
            func("ports", "Ports", service_ports),
            AGE,
        ],
    },
    KindSpec {
        group: "networking.k8s.io",
        kind: "Ingress",
        label: "Ingresses",
        category: Network,
        columns: &[
            NAME,
            NAMESPACE,
            text("class", "Class", ".spec.ingressClassName"),
            func("hosts", "Hosts", ingress_hosts),
            func("address", "Address", |v| {
                first_of(&[
                    path_str(v, ".status.loadBalancer.ingress[0].ip"),
                    path_str(v, ".status.loadBalancer.ingress[0].hostname"),
                ])
            }),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "Endpoints",
        label: "Endpoints",
        category: Network,
        columns: &[
            NAME,
            NAMESPACE,
            func("endpoints", "Endpoints", endpoints_addrs),
            AGE,
        ],
    },
    KindSpec {
        group: "discovery.k8s.io",
        kind: "EndpointSlice",
        label: "Endpoint Slices",
        category: Network,
        columns: &[
            NAME,
            NAMESPACE,
            text("addresstype", "Address type", ".addressType"),
            func("ports", "Ports", slice_ports),
            func("endpoints", "Endpoints", slice_endpoints),
            AGE,
        ],
    },
    KindSpec {
        group: "networking.k8s.io",
        kind: "NetworkPolicy",
        label: "Network Policies",
        category: Network,
        columns: &[
            NAME,
            NAMESPACE,
            func("podselector", "Pod selector", |v| {
                selector_str(v.pointer("/spec/podSelector"))
            }),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "ConfigMap",
        label: "Config Maps",
        category: Config,
        columns: &[
            NAME,
            NAMESPACE,
            func("data", "Data", |v| {
                (count_keys(v, "/data") + count_keys(v, "/binaryData")).to_string()
            }),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "Secret",
        label: "Secrets",
        category: Config,
        columns: &[
            NAME,
            NAMESPACE,
            text("type", "Type", ".type"),
            func("data", "Data", |v| count_keys(v, "/data").to_string()),
            AGE,
        ],
    },
    KindSpec {
        group: "autoscaling",
        kind: "HorizontalPodAutoscaler",
        label: "HPAs",
        category: Config,
        columns: &[
            NAME,
            NAMESPACE,
            func("reference", "Reference", |v| {
                format!(
                    "{}/{}",
                    path_str(v, ".spec.scaleTargetRef.kind"),
                    path_str(v, ".spec.scaleTargetRef.name")
                )
            }),
            number("min", "Min", ".spec.minReplicas"),
            number("max", "Max", ".spec.maxReplicas"),
            number("replicas", "Replicas", ".status.currentReplicas"),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "ResourceQuota",
        label: "Resource Quotas",
        category: Config,
        columns: &[NAME, NAMESPACE, AGE],
    },
    KindSpec {
        group: "",
        kind: "LimitRange",
        label: "Limit Ranges",
        category: Config,
        columns: &[NAME, NAMESPACE, AGE],
    },
    KindSpec {
        group: "",
        kind: "PersistentVolumeClaim",
        label: "Persistent Volume Claims",
        category: Storage,
        columns: &[
            NAME,
            NAMESPACE,
            status("status", "Status", |v| path_str(v, ".status.phase")),
            text("volume", "Volume", ".spec.volumeName"),
            text("capacity", "Capacity", ".status.capacity.storage"),
            text("modes", "Access modes", ".spec.accessModes"),
            text("class", "Storage class", ".spec.storageClassName"),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "PersistentVolume",
        label: "Persistent Volumes",
        category: Storage,
        columns: &[
            NAME,
            text("capacity", "Capacity", ".spec.capacity.storage"),
            text("modes", "Access modes", ".spec.accessModes"),
            text(
                "reclaim",
                "Reclaim policy",
                ".spec.persistentVolumeReclaimPolicy",
            ),
            status("status", "Status", |v| path_str(v, ".status.phase")),
            func("claim", "Claim", |v| {
                let ns = path_str(v, ".spec.claimRef.namespace");
                let name = path_str(v, ".spec.claimRef.name");
                if name.is_empty() {
                    String::new()
                } else {
                    format!("{ns}/{name}")
                }
            }),
            text("class", "Storage class", ".spec.storageClassName"),
            AGE,
        ],
    },
    KindSpec {
        group: "storage.k8s.io",
        kind: "StorageClass",
        label: "Storage Classes",
        category: Storage,
        columns: &[
            NAME,
            text("provisioner", "Provisioner", ".provisioner"),
            text("reclaim", "Reclaim policy", ".reclaimPolicy"),
            text("binding", "Binding mode", ".volumeBindingMode"),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "Node",
        label: "Nodes",
        category: Cluster,
        columns: &[
            NAME,
            text(
                "ready",
                "Ready",
                ".status.conditions[?(@.type==\"Ready\")].status",
            ),
            text("version", "Version", ".status.nodeInfo.kubeletVersion"),
            AGE,
            ui("cpu", "CPU", ColumnKind::NodeCpu),
            ui("memory", "Memory", ColumnKind::NodeMemory),
        ],
    },
    KindSpec {
        group: "",
        kind: "Namespace",
        label: "Namespaces",
        category: Cluster,
        columns: &[
            NAME,
            status("status", "Status", |v| path_str(v, ".status.phase")),
            AGE,
        ],
    },
    KindSpec {
        group: "",
        kind: "Event",
        label: "Events",
        category: Cluster,
        columns: &[
            status("type", "Type", |v| path_str(v, ".type")),
            text("reason", "Reason", ".reason"),
            func("object", "Object", |v| {
                format!(
                    "{}/{}",
                    path_str(v, ".involvedObject.kind"),
                    path_str(v, ".involvedObject.name")
                )
            }),
            text("message", "Message", ".message"),
            number("count", "Count", ".count"),
            NAMESPACE,
            AGE,
        ],
    },
];

pub(super) fn spec_for(group: &str, kind: &str) -> Option<&'static KindSpec> {
    KINDS.iter().find(|k| k.group == group && k.kind == kind)
}

fn column_def(c: &Col) -> ColumnDef {
    ColumnDef {
        key: c.key.to_string(),
        label: c.label.to_string(),
        kind: c.kind,
        source: match c.src {
            StaticSrc::Ui => Source::Ui,
            StaticSrc::Path(p) => Source::Path(parse_path(p)),
            StaticSrc::Func(f) => Source::Func(f),
        },
    }
}

/// `additionalPrinterColumns` entry of a CRD.
#[derive(Debug, Clone)]
pub(super) struct PrinterColumn {
    pub(super) name: String,
    pub(super) type_: String,
    pub(super) json_path: String,
    pub(super) priority: i32,
}

/// Columns for a kind with no registry entry: Name, Namespace, the CRD's
/// printer columns (priority 0, like `kubectl get`), then Age.
fn fallback_columns(namespaced: bool, printer: &[PrinterColumn]) -> Vec<ColumnDef> {
    let mut cols = vec![column_def(&NAME)];
    if namespaced {
        cols.push(column_def(&NAMESPACE));
    }
    let mut has_age = false;
    for (i, p) in printer.iter().filter(|p| p.priority == 0).enumerate() {
        if p.json_path == ".metadata.creationTimestamp" {
            // The conventional Age column; the UI keeps it fresh.
            let mut age = column_def(&AGE);
            age.label = p.name.clone();
            cols.push(age);
            has_age = true;
            continue;
        }
        cols.push(ColumnDef {
            // Printer column names are free text; the index keeps keys unique.
            key: format!("pc{i}"),
            label: p.name.clone(),
            kind: match p.type_.as_str() {
                "integer" | "number" => ColumnKind::Number,
                _ => ColumnKind::Text,
            },
            source: Source::Path(parse_path(&p.json_path)),
        });
    }
    if !has_age {
        cols.push(column_def(&AGE));
    }
    cols
}

/// Columns for `resource`, from the registry or the printer-column fallback.
pub(super) fn columns_for(resource: &ResourceRef, printer: &[PrinterColumn]) -> Vec<ColumnDef> {
    match spec_for(&resource.group, &resource.kind) {
        Some(spec) => spec
            .columns
            .iter()
            .filter(|c| resource.namespaced || c.key != "namespace")
            .map(column_def)
            .collect(),
        None => fallback_columns(resource.namespaced, printer),
    }
}

// --- Derived columns --------------------------------------------------------

fn pod_ready(v: &Value) -> String {
    let total = array_at(v, "/spec/containers").count();
    let ready = array_at(v, "/status/containerStatuses")
        .filter(|c| c.get("ready") == Some(&Value::Bool(true)))
        .count();
    format!("{ready}/{total}")
}

fn pod_restarts(v: &Value) -> String {
    array_at(v, "/status/containerStatuses")
        .filter_map(|c| c.get("restartCount").and_then(Value::as_i64))
        .sum::<i64>()
        .to_string()
}

/// The phase, refined the way `kubectl get pods` does: a terminating pod or
/// a container that is waiting (CrashLoopBackOff, ImagePullBackOff, ...)
/// says more than "Running" / "Pending".
fn pod_status(v: &Value) -> String {
    if v.pointer("/metadata/deletionTimestamp").is_some() {
        return "Terminating".into();
    }
    let waiting = array_at(v, "/status/initContainerStatuses")
        .chain(array_at(v, "/status/containerStatuses"))
        .filter_map(|c| c.pointer("/state/waiting/reason").and_then(Value::as_str))
        .find(|r| *r != "PodInitializing");
    if let Some(reason) = waiting {
        return reason.to_string();
    }
    first_of(&[path_str(v, ".status.reason"), path_str(v, ".status.phase")])
}

fn service_external_ip(v: &Value) -> String {
    let lb: Vec<String> = array_at(v, "/status/loadBalancer/ingress")
        .map(|i| first_of(&[path_str(i, ".ip"), path_str(i, ".hostname")]))
        .collect();
    if !lb.is_empty() {
        return lb.join(",");
    }
    path_str(v, ".spec.externalIPs")
}

fn service_ports(v: &Value) -> String {
    array_at(v, "/spec/ports")
        .map(|p| {
            let proto = p.get("protocol").and_then(Value::as_str).unwrap_or("TCP");
            let port = path_i64(p, ".port");
            match p.get("nodePort").and_then(Value::as_i64) {
                Some(np) => format!("{port}:{np}/{proto}"),
                None => format!("{port}/{proto}"),
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn ingress_hosts(v: &Value) -> String {
    array_at(v, "/spec/rules")
        .filter_map(|r| r.get("host").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(",")
}

/// At most three items, then "+N more".
fn abbreviate(items: Vec<String>) -> String {
    const MAX: usize = 3;
    let extra = items.len().saturating_sub(MAX);
    let mut out = items.into_iter().take(MAX).collect::<Vec<_>>().join(",");
    if extra > 0 {
        out.push_str(&format!(" +{extra} more"));
    }
    out
}

fn endpoints_addrs(v: &Value) -> String {
    let mut items = Vec::new();
    for subset in array_at(v, "/subsets") {
        let ports: Vec<i64> = array_at(subset, "/ports")
            .filter_map(|p| p.get("port").and_then(Value::as_i64))
            .collect();
        for addr in array_at(subset, "/addresses") {
            let ip = path_str(addr, ".ip");
            if ports.is_empty() {
                items.push(ip);
            } else {
                items.extend(ports.iter().map(|port| format!("{ip}:{port}")));
            }
        }
    }
    abbreviate(items)
}

fn slice_ports(v: &Value) -> String {
    array_at(v, "/ports")
        .filter_map(|p| p.get("port").and_then(Value::as_i64))
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn slice_endpoints(v: &Value) -> String {
    abbreviate(
        array_at(v, "/endpoints")
            .filter_map(|e| e.pointer("/addresses/0").and_then(Value::as_str))
            .map(String::from)
            .collect(),
    )
}

fn selector_str(sel: Option<&Value>) -> String {
    let labels = sel
        .and_then(|s| s.get("matchLabels"))
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty());
    match labels {
        None => "<all>".into(),
        Some(m) => m
            .iter()
            .map(|(k, v)| format!("{k}={}", cell_text(v)))
            .collect::<Vec<_>>()
            .join(","),
    }
}

// ---------------------------------------------------------------------------
// Rows.
// ---------------------------------------------------------------------------

/// Format an RFC 3339 timestamp as an age ("5m"), or return it unchanged.
pub(super) fn age_text(ts: &str) -> String {
    ts.parse::<chrono::DateTime<chrono::Utc>>()
        .map(crate::format_age)
        .unwrap_or_else(|_| ts.to_string())
}

/// `2024-05-01T10:00:00Z`: timestamps in text columns (a CronJob's last
/// schedule, a date-typed printer column) are shown as ages.
fn looks_like_timestamp(s: &str) -> bool {
    s.len() >= 20
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[10] == b'T'
        && s.parse::<chrono::DateTime<chrono::Utc>>().is_ok()
}

/// Build the table row of `obj` for `columns`.
pub fn make_row(columns: &[ColumnDef], obj: &DynamicObject) -> ResourceRow {
    let value = serde_json::to_value(obj).unwrap_or(Value::Null);
    let name = obj.metadata.name.clone().unwrap_or_default();
    let namespace = obj.metadata.namespace.clone().unwrap_or_default();
    let cells = columns
        .iter()
        .map(|c| match &c.source {
            Source::Ui => String::new(),
            Source::Func(f) => f(&value),
            Source::Path(p) => {
                let text = eval_str(&value, p);
                if c.kind == ColumnKind::Text && looks_like_timestamp(&text) {
                    age_text(&text)
                } else {
                    text
                }
            }
        })
        .collect();
    ResourceRow {
        key: if namespace.is_empty() {
            name.clone()
        } else {
            format!("{namespace}/{name}")
        },
        name,
        namespace,
        created: created_secs(&obj.metadata.creation_timestamp),
        cells,
    }
}
#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn pod_columns() {
        let pod = json!({
            "spec": { "containers": [{"name": "a"}, {"name": "b"}] },
            "status": { "phase": "Running", "containerStatuses": [
                {"ready": true, "restartCount": 1},
                {"ready": false, "restartCount": 2,
                 "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
            ]}
        });
        assert_eq!(pod_ready(&pod), "1/2");
        assert_eq!(pod_restarts(&pod), "3");
        assert_eq!(pod_status(&pod), "CrashLoopBackOff");
        let done = json!({"status": {"phase": "Succeeded"}});
        assert_eq!(pod_status(&done), "Succeeded");
        let gone = json!({"metadata": {"deletionTimestamp": "x"}, "status": {"phase": "Running"}});
        assert_eq!(pod_status(&gone), "Terminating");
    }

    #[test]
    fn registry_columns_and_namespace_scope() {
        let keys: Vec<_> = columns_for(&rref("", "Pod", "pods", true), &[])
            .into_iter()
            .map(|c| c.key)
            .collect();
        assert_eq!(
            keys,
            [
                "name",
                "namespace",
                "ready",
                "status",
                "restarts",
                "node",
                "age",
                "cpu",
                "memory"
            ]
        );
        let nodes = columns_for(&rref("", "Node", "nodes", false), &[]);
        assert!(!nodes.iter().any(|c| c.key == "namespace"));
    }

    #[test]
    fn crd_fallback_uses_printer_columns() {
        let r = rref("example.com", "Widget", "widgets", true);
        let pc = |name: &str, type_: &str, path: &str, priority| PrinterColumn {
            name: name.into(),
            type_: type_.into(),
            json_path: path.into(),
            priority,
        };
        let printer = vec![
            pc("Size", "integer", ".spec.size", 0),
            pc("Wide", "string", ".spec.wide", 1),
            pc("Since", "date", ".status.since", 0),
            pc("Age", "date", ".metadata.creationTimestamp", 0),
        ];
        let cols = columns_for(&r, &printer);
        let labels: Vec<_> = cols.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Name", "Namespace", "Size", "Since", "Age"]);
        assert_eq!(cols[2].kind, ColumnKind::Number);
        assert_eq!(cols[4].kind, ColumnKind::Age);

        let obj: DynamicObject = serde_json::from_value(json!({
            "apiVersion": "example.com/v1", "kind": "Widget",
            "metadata": { "name": "w", "namespace": "n" },
            "spec": { "size": 3 },
            "status": { "since": "2020-01-01T00:00:00Z" }
        }))
        .unwrap();
        let row = make_row(&cols, &obj);
        assert_eq!(row.key, "n/w");
        assert_eq!(row.cells[..3], ["w", "n", "3"]);
        // A date-typed column shows an age, not the raw timestamp.
        assert!(row.cells[3].ends_with('d'), "{:?}", row.cells);
        assert_eq!(row.cells[4], "");
        // Without printer columns: Name, Namespace, Age.
        assert_eq!(columns_for(&r, &[]).len(), 3);
    }

    #[test]
    fn registry_has_every_sidebar_kind_once() {
        let mut seen = HashSet::new();
        for k in KINDS {
            assert!(seen.insert((k.group, k.kind)), "duplicate {}", k.kind);
        }
        assert_eq!(KINDS.len(), 23);
    }
}
