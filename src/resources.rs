//! Generic resource access: API discovery, a declarative column registry and
//! list / get / YAML / apply / delete / watch on `DynamicObject`, keyed by a
//! [`ResourceRef`] (group, version, kind, plural, scope).
//!
//! Everything the UI and the CLI show for a kind other than a pod's rich
//! detail goes through here. Adding columns for a built-in kind is one entry
//! in [`KINDS`]; CRDs get Name / Namespace / Age plus the printer columns the
//! CRD declares (what `kubectl get` shows).

use crate::{age_of_secs, created_secs};
use anyhow::{bail, Context, Result};
use futures::StreamExt;
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Placeholder shown instead of Secret values until the user reveals them.
pub const REDACTED: &str = "<hidden: reveal to view>";

/// Field manager used for server-side apply.
const FIELD_MANAGER: &str = "kubeloupe";

// ---------------------------------------------------------------------------
// Types shared with the UI.
// ---------------------------------------------------------------------------

/// Identifies a resource type well enough to build an `Api<DynamicObject>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResourceRef {
    /// API group; empty for the core group.
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
}

impl ResourceRef {
    pub fn api_resource(&self) -> ApiResource {
        ApiResource {
            group: self.group.clone(),
            version: self.version.clone(),
            api_version: if self.group.is_empty() {
                self.version.clone()
            } else {
                format!("{}/{}", self.group, self.version)
            },
            kind: self.kind.clone(),
            plural: self.plural.clone(),
        }
    }

    /// Stable id: `v1/pods`, `apps/v1/deployments`.
    pub fn id(&self) -> String {
        if self.group.is_empty() {
            format!("{}/{}", self.version, self.plural)
        } else {
            format!("{}/{}/{}", self.group, self.version, self.plural)
        }
    }

    fn is(&self, group: &str, kind: &str) -> bool {
        self.group == group && self.kind == kind
    }
}

/// Where a kind appears in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Workloads,
    Network,
    Config,
    Storage,
    Cluster,
    /// CRDs, grouped by API group.
    Custom,
    /// Anything else the server serves (leases, apiservices, ...).
    Other,
}

/// How a column's cell is produced and rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ColumnKind {
    Text,
    /// Sorted numerically.
    Number,
    /// Derived from the creation time by the UI, so it stays fresh.
    Age,
    /// Coloured by value (Running, Pending, Failed, ...).
    Status,
    /// Live usage merged in by the UI from the metrics API.
    PodCpu,
    PodMemory,
    NodeCpu,
    NodeMemory,
}

/// A table column as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ColumnDef {
    pub key: String,
    pub label: String,
    pub kind: ColumnKind,
    #[serde(skip)]
    #[cfg_attr(feature = "specta", specta(skip))]
    source: Source,
}

/// One entry of the sidebar: a discovered resource type with its columns.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResourceKind {
    pub resource: ResourceRef,
    /// [`ResourceRef::id`].
    pub id: String,
    /// Display name ("Deployments", "Network Policies").
    pub label: String,
    pub category: Category,
    /// Defined by a CustomResourceDefinition.
    pub crd: bool,
    pub verbs: Vec<String>,
    pub short_names: Vec<String>,
    pub columns: Vec<ColumnDef>,
}

/// A row of a resource table: identity plus one cell per column. Columns the
/// UI fills itself (age, metrics) have an empty cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResourceRow {
    /// `namespace/name`, or `name` for cluster-scoped objects.
    pub key: String,
    pub name: String,
    pub namespace: String,
    /// Creation time (unix seconds).
    pub created: i64,
    pub cells: Vec<String>,
}

/// Result of an access check for one kind.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct AccessResult {
    pub id: String,
    pub allowed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct OwnerLink {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub controller: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ConditionInfo {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: String,
    pub message: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RelatedEvent {
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i64,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SecretKey {
    pub key: String,
    /// Decoded size in bytes.
    pub bytes: u32,
}

/// Everything the generic detail panel shows besides the YAML.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ResourceDetail {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub api_version: String,
    pub kind: String,
    pub created: i64,
    pub age: String,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub finalizers: Vec<String>,
    pub owners: Vec<OwnerLink>,
    pub conditions: Vec<ConditionInfo>,
    /// Keys of a Secret's data (never the values).
    pub secret_keys: Vec<SecretKey>,
    pub events: Vec<RelatedEvent>,
}

// ---------------------------------------------------------------------------
// Column registry.
// ---------------------------------------------------------------------------

/// Where a cell's text comes from.
#[derive(Debug, Clone, Default)]
enum Source {
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
struct KindSpec {
    group: &'static str,
    kind: &'static str,
    label: &'static str,
    category: Category,
    columns: &'static [Col],
}

use Category::{Cluster, Config, Network, Storage, Workloads};

/// The column registry. Order here is the sidebar order. To add columns for
/// another built-in kind, add one entry (a path into the object, or a small
/// function for derived values). Namespace columns are dropped automatically
/// for cluster-scoped kinds.
const KINDS: &[KindSpec] = &[
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

fn spec_for(group: &str, kind: &str) -> Option<&'static KindSpec> {
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
struct PrinterColumn {
    name: String,
    type_: String,
    json_path: String,
    priority: i32,
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
fn columns_for(resource: &ResourceRef, printer: &[PrinterColumn]) -> Vec<ColumnDef> {
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

// ---------------------------------------------------------------------------
// A small JSONPath: `.a.b`, `[0]`, `[*]`, `['k']`, `[?(@.f=="v")]`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Seg {
    Field(String),
    Index(usize),
    All,
    /// `[?(@.path == lit)]` / `!=`
    Filter {
        path: Vec<String>,
        negate: bool,
        lit: Value,
    },
    /// Something we cannot evaluate: matches nothing.
    Unsupported,
}

fn parse_path(p: &str) -> Vec<Seg> {
    let p = p.trim();
    let p = p.strip_prefix('$').unwrap_or(p);
    let chars: Vec<char> = p.chars().collect();
    let mut segs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                // Find the matching `]`, skipping quoted text.
                let start = i + 1;
                let mut j = start;
                let mut quote: Option<char> = None;
                while j < chars.len() {
                    match (quote, chars[j]) {
                        (None, ']') => break,
                        (None, c @ ('"' | '\'')) => quote = Some(c),
                        (Some(q), c) if c == q => quote = None,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = chars[start..j.min(chars.len())].iter().collect();
                segs.push(parse_bracket(inner.trim()));
                i = j + 1;
            }
            c => {
                // `.name` or a bare leading name; `\.` escapes a dot.
                if c == '.' {
                    i += 1;
                }
                let mut name = String::new();
                while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    name.push(chars[i]);
                    i += 1;
                }
                if !name.is_empty() {
                    segs.push(Seg::Field(name));
                }
            }
        }
    }
    segs
}

fn unquote(s: &str) -> Option<&str> {
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    (s.len() >= 2 && s.ends_with(q)).then(|| &s[1..s.len() - 1])
}

fn parse_bracket(inner: &str) -> Seg {
    if inner == "*" {
        return Seg::All;
    }
    if let Ok(n) = inner.parse::<usize>() {
        return Seg::Index(n);
    }
    if let Some(key) = unquote(inner) {
        return Seg::Field(key.to_string());
    }
    let Some(expr) = inner
        .strip_prefix("?(")
        .and_then(|e| e.strip_suffix(')'))
        .map(str::trim)
    else {
        return Seg::Unsupported;
    };
    let (negate, at) = match (expr.find("=="), expr.find("!=")) {
        (Some(i), _) => (false, i),
        (None, Some(i)) => (true, i),
        _ => return Seg::Unsupported,
    };
    let Some(left) = expr[..at].trim().strip_prefix('@') else {
        return Seg::Unsupported;
    };
    let right = expr[at + 2..].trim();
    let lit = match unquote(right) {
        Some(s) => Value::String(s.to_string()),
        None => match serde_json::from_str(right) {
            Ok(v) => v,
            Err(_) => return Seg::Unsupported,
        },
    };
    Seg::Filter {
        path: left
            .split('.')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        negate,
        lit,
    }
}

fn eval<'a>(root: &'a Value, segs: &[Seg]) -> Vec<&'a Value> {
    let mut cur = vec![root];
    for seg in segs {
        let mut next = Vec::new();
        for v in cur {
            match seg {
                Seg::Field(f) => next.extend(v.get(f)),
                Seg::Index(i) => next.extend(v.get(*i)),
                Seg::All => match v {
                    Value::Array(a) => next.extend(a),
                    Value::Object(o) => next.extend(o.values()),
                    _ => {}
                },
                Seg::Filter { path, negate, lit } => {
                    if let Value::Array(a) = v {
                        for item in a {
                            let mut at = Some(item);
                            for p in path {
                                at = at.and_then(|x| x.get(p));
                            }
                            if (at == Some(lit)) != *negate {
                                next.push(item);
                            }
                        }
                    }
                }
                Seg::Unsupported => {}
            }
        }
        cur = next;
    }
    cur
}

/// Text of a JSON value as a table cell.
fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(cell_text).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

fn eval_str(v: &Value, segs: &[Seg]) -> String {
    eval(v, segs)
        .into_iter()
        .map(cell_text)
        .collect::<Vec<_>>()
        .join(",")
}

/// Evaluate `path` (e.g. `.status.phase`) and join the matches.
fn path_str(v: &Value, path: &str) -> String {
    eval_str(v, &parse_path(path))
}

fn path_i64(v: &Value, path: &str) -> i64 {
    eval(v, &parse_path(path))
        .first()
        .and_then(|x| x.as_i64())
        .unwrap_or(0)
}

/// `a/b` where both are numbers at the given paths (missing = 0).
fn ratio(v: &Value, num: &str, den: &str) -> String {
    format!("{}/{}", path_i64(v, num), path_i64(v, den))
}

fn count_at(v: &Value, path: &str) -> String {
    eval(v, &parse_path(path))
        .first()
        .and_then(|x| x.as_array().map(Vec::len))
        .unwrap_or(0)
        .to_string()
}

fn count_keys(v: &Value, pointer: &str) -> usize {
    v.pointer(pointer)
        .and_then(Value::as_object)
        .map_or(0, |o| o.len())
}

fn first_of(options: &[String]) -> String {
    options
        .iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_default()
}

fn array_at<'a>(v: &'a Value, pointer: &str) -> impl Iterator<Item = &'a Value> {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
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
fn age_text(ts: &str) -> String {
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
fn object_api(client: &Client, r: &ResourceRef, namespace: &str) -> Result<Api<DynamicObject>> {
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

fn is_secret(r: &ResourceRef) -> bool {
    r.is("", "Secret")
}

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

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
    let mut obj = get_resource(client, r, namespace, name).await?;
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

/// Apply edited YAML with server-side apply (field manager "kubeloupe").
///
/// Server-managed fields are stripped first (see [`strip_server_fields`]),
/// so saving the same editor content again, or content that was loaded
/// before an earlier save, never fails on a stale `resourceVersion`. The
/// apply is forced: the document is an explicit edit by the user, and it
/// should win over fields another manager (kubectl, a controller) last set
/// instead of failing with a field-ownership conflict.
pub async fn apply_resource_yaml(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    yaml: &str,
) -> Result<()> {
    let mut value: Value = serde_yaml::from_str(yaml).context("invalid YAML")?;
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
    let api = object_api(client, r, namespace)?;
    api.patch(
        name,
        &PatchParams::apply(FIELD_MANAGER).force(),
        &Patch::Apply(&value),
    )
    .await?;
    Ok(())
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

/// Metadata, owners, conditions and related events for the detail panel.
pub async fn get_resource_detail(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
) -> Result<ResourceDetail> {
    let obj = get_resource(client, r, namespace, name).await?;
    let value = serde_json::to_value(&obj)?;
    let meta = &obj.metadata;
    let uid = meta.uid.clone().unwrap_or_default();
    let created = created_secs(&meta.creation_timestamp);

    let mut annotations = meta.annotations.clone().unwrap_or_default();
    if is_secret(r) {
        if let Some(a) = annotations.get_mut(LAST_APPLIED) {
            *a = REDACTED.into();
        }
    }
    let owners = meta
        .owner_references
        .iter()
        .flatten()
        .map(|o| OwnerLink {
            api_version: o.api_version.clone(),
            kind: o.kind.clone(),
            name: o.name.clone(),
            controller: o.controller.unwrap_or(false),
        })
        .collect();
    let conditions = array_at(&value, "/status/conditions")
        .filter(|c| c.get("type").is_some())
        .map(|c| {
            let when = first_of(&[
                path_str(c, ".lastTransitionTime"),
                path_str(c, ".lastUpdateTime"),
            ]);
            ConditionInfo {
                type_: path_str(c, ".type"),
                status: path_str(c, ".status"),
                reason: path_str(c, ".reason"),
                message: path_str(c, ".message"),
                age: if when.is_empty() {
                    when
                } else {
                    age_text(&when)
                },
            }
        })
        .collect();
    let secret_keys = if is_secret(r) {
        value
            .pointer("/data")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| SecretKey {
                        key: k.clone(),
                        // Decoded size of the base64 value.
                        bytes: (v.as_str().unwrap_or("").trim_end_matches('=').len() * 3 / 4)
                            as u32,
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let events = related_events(client, namespace, &uid).await;
    Ok(ResourceDetail {
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
        uid,
        api_version: r.api_resource().api_version,
        kind: r.kind.clone(),
        created,
        age: age_of_secs(created),
        labels: meta.labels.clone().unwrap_or_default(),
        annotations,
        finalizers: meta.finalizers.clone().unwrap_or_default(),
        owners,
        conditions,
        secret_keys,
        events,
    })
}

/// Events whose involved object has this uid, newest first. Best effort:
/// without permission to read events the panel just shows none.
async fn related_events(client: &Client, namespace: &str, uid: &str) -> Vec<RelatedEvent> {
    if uid.is_empty() {
        return Vec::new();
    }
    let api: Api<Event> = if namespace.is_empty() {
        // Events of cluster-scoped objects are filed in some namespace.
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), namespace)
    };
    let lp = ListParams::default().fields(&format!("involvedObject.uid={uid}"));
    let Ok(list) = api.list(&lp).await else {
        return Vec::new();
    };
    let mut events: Vec<(i64, RelatedEvent)> = list
        .items
        .into_iter()
        .map(|e| {
            let ts = e
                .last_timestamp
                .as_ref()
                .map(|t| t.0.timestamp())
                .or_else(|| e.event_time.as_ref().map(|t| t.0.timestamp()))
                .or_else(|| {
                    e.metadata
                        .creation_timestamp
                        .as_ref()
                        .map(|t| t.0.timestamp())
                })
                .unwrap_or(0);
            (
                ts,
                RelatedEvent {
                    type_: e.type_.unwrap_or_default(),
                    reason: e.reason.unwrap_or_default(),
                    message: e.message.unwrap_or_default(),
                    count: i64::from(e.count.unwrap_or(1)),
                    age: age_of_secs(ts),
                },
            )
        })
        .collect();
    events.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
    events.into_iter().take(50).map(|(_, e)| e).collect()
}

/// Render rows like `kubectl get`: aligned columns, age computed here, and
/// without the columns that only the desktop UI can fill (live metrics).
pub fn render_table(kind: &ResourceKind, rows: &[ResourceRow]) -> String {
    let shown: Vec<usize> = kind
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            !matches!(
                c.kind,
                ColumnKind::PodCpu
                    | ColumnKind::PodMemory
                    | ColumnKind::NodeCpu
                    | ColumnKind::NodeMemory
            )
        })
        .map(|(i, _)| i)
        .collect();
    let mut table: Vec<Vec<String>> = vec![shown
        .iter()
        .map(|&i| kind.columns[i].label.to_uppercase())
        .collect()];
    let mut sorted: Vec<&ResourceRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    for row in sorted {
        table.push(
            shown
                .iter()
                .map(|&i| match kind.columns[i].kind {
                    ColumnKind::Age => age_of_secs(row.created),
                    _ => row.cells.get(i).cloned().unwrap_or_default(),
                })
                .collect(),
        );
    }
    let widths: Vec<usize> = (0..shown.len())
        .map(|c| {
            table
                .iter()
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for r in &table {
        let line: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonpath_fields_indexes_and_filters() {
        let v = json!({
            "status": { "phase": "Running", "conditions": [
                {"type": "Ready", "status": "True"},
                {"type": "Other", "status": "False"}
            ], "ports": [80, 443] },
            "metadata": { "annotations": { "a.b/c": "x" } }
        });
        assert_eq!(path_str(&v, ".status.phase"), "Running");
        assert_eq!(path_str(&v, "$.status.phase"), "Running");
        assert_eq!(path_str(&v, "status.phase"), "Running");
        assert_eq!(path_str(&v, ".status.ports[1]"), "443");
        assert_eq!(path_str(&v, ".status.ports"), "80,443");
        assert_eq!(path_str(&v, ".status.ports[*]"), "80,443");
        assert_eq!(
            path_str(&v, ".status.conditions[?(@.type==\"Ready\")].status"),
            "True"
        );
        assert_eq!(
            path_str(&v, ".status.conditions[?(@.type!='Ready')].status"),
            "False"
        );
        assert_eq!(path_str(&v, ".metadata.annotations['a.b/c']"), "x");
        assert_eq!(path_str(&v, ".metadata.annotations.a\\.b/c"), "x");
        assert_eq!(path_str(&v, ".missing.deeper"), "");
        assert_eq!(path_str(&v, ".status.conditions[?(@.a==1 && @.b==2)]"), "");
    }

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

    fn rref(group: &str, kind: &str, plural: &str, namespaced: bool) -> ResourceRef {
        ResourceRef {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: plural.into(),
            namespaced,
        }
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
    fn yaml_puts_identity_first() {
        let v =
            json!({"spec": {"a": 1}, "metadata": {"name": "n"}, "kind": "K", "apiVersion": "v1"});
        let y = serde_yaml::to_string(&ordered(v)).unwrap();
        assert!(y.starts_with("apiVersion: v1\nkind: K\nmetadata:"), "{y}");
    }

    fn kind(group: &str, kind: &str, plural: &str, short: &[&str]) -> ResourceKind {
        let resource = rref(group, kind, plural, true);
        ResourceKind {
            id: resource.id(),
            label: String::new(),
            category: Category::Other,
            crd: false,
            verbs: vec![],
            short_names: short.iter().map(|s| s.to_string()).collect(),
            columns: vec![],
            resource,
        }
    }

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

    #[test]
    fn render_table_aligns_and_skips_metrics_columns() {
        let mut k = kind("", "Pod", "pods", &[]);
        k.columns = columns_for(&k.resource, &[]);
        let row = |name: &str, status: &str| ResourceRow {
            key: format!("ns/{name}"),
            name: name.into(),
            namespace: "ns".into(),
            created: 0,
            cells: vec![
                name.into(),
                "ns".into(),
                "1/1".into(),
                status.into(),
                "0".into(),
                "node".into(),
                "".into(),
                "".into(),
                "".into(),
            ],
        };
        let out = render_table(&k, &[row("b", "Running"), row("a", "Pending")]);
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(
            lines[0],
            "NAME  NAMESPACE  READY  STATUS   RESTARTS  NODE  AGE"
        );
        assert!(lines[1].starts_with("a     ns"), "{out}");
        assert!(lines[2].starts_with("b     ns"), "{out}");
        assert!(!out.contains("CPU"));
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
