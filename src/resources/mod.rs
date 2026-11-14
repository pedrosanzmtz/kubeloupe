//! Generic resource access: API discovery, a declarative column registry and
//! list / get / YAML / apply / delete / watch on `DynamicObject`, keyed by a
//! [`ResourceRef`] (group, version, kind, plural, scope).
//!
//! Everything the UI and the CLI show for a kind other than a pod's rich
//! detail goes through here. Adding columns for a built-in kind is one entry
//! in `KINDS` (`columns.rs`); CRDs get Name / Namespace / Age plus the printer
//! columns the CRD declares (what `kubectl get` shows).
//!
//! - `columns`: the column registry and how a row's cells are filled
//! - `jsonpath`: the small JSONPath the columns are written in
//! - `discovery`: API discovery, access checks, resolving a kind by name
//! - `ops`: list / get / YAML get and apply / delete
//! - `manifests`, `apply`: create or update objects from a manifest file
//! - `detail`: the detail panel for any kind
//! - `pod_detail`: the pod section of the detail panel (containers, probes, ...)
//! - `detail_watch`: live updates of one object's detail
//! - `table`: `kubectl get`-style table output for the CLI

mod apply;
mod columns;
mod detail;
mod detail_watch;
mod discovery;
mod jsonpath;
mod manifests;
mod ops;
mod pod_detail;
mod table;

pub use apply::{apply_manifests, cli_apply, ApplyResult, ApplyStatus, ManifestOptions};
pub use columns::make_row;
pub use detail::{detail_of, get_resource_detail, summarize_events};
pub use detail_watch::{run_detail_watch, DetailState, DETAIL_FLUSH};
pub use discovery::{can_list, check_access, discover, resolve_kind};
pub use ops::{
    apply_resource_yaml, apply_resource_yaml_with, delete_resource, dyn_api, get_resource,
    get_resource_yaml, list_resources, object_value, to_yaml, ApplyOptions, ApplyOutcome,
    FieldTakeover,
};
pub use pod_detail::{
    env_refs, pod_extras, probe_summary, state_info, ContainerDetail, ContainerKind, EnvInfo,
    MountInfo, PodExtras, PortInfo, ProbeInfo, StateInfo,
};
pub use table::{align_columns, render_table, table_cells, table_header};

use columns::Source;
use kube::api::ApiResource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Placeholder shown instead of Secret values until the user reveals them.
pub const REDACTED: &str = "<hidden: reveal to view>";

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
    /// Short names from discovery plus kubectl's aliases (`deploy`, `svc`).
    pub short_names: Vec<String>,
    pub columns: Vec<ColumnDef>,
    /// What can be done to objects of this kind ([`crate::actions::actions_for`]).
    pub actions: Vec<crate::actions::ActionDef>,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct OwnerLink {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub controller: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ConditionInfo {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: String,
    pub message: String,
    pub age: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RelatedEvent {
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i64,
    pub age: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SecretKey {
    pub key: String,
    /// Decoded size in bytes.
    pub bytes: u32,
}

/// Everything the detail panel shows besides the YAML.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// The pod section: filled for pods, empty (default) for other kinds.
    /// Not an `Option`, which would split the TypeScript type in two.
    pub pod: PodExtras,
}

#[cfg(test)]
mod test_support {
    use super::*;

    pub(super) fn rref(group: &str, kind: &str, plural: &str, namespaced: bool) -> ResourceRef {
        ResourceRef {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: plural.into(),
            namespaced,
        }
    }

    pub(super) fn kind(group: &str, kind: &str, plural: &str, short: &[&str]) -> ResourceKind {
        let resource = rref(group, kind, plural, true);
        ResourceKind {
            id: resource.id(),
            label: String::new(),
            category: Category::Other,
            crd: false,
            verbs: vec![],
            short_names: short.iter().map(|s| s.to_string()).collect(),
            columns: vec![],
            actions: vec![],
            resource,
        }
    }
}
