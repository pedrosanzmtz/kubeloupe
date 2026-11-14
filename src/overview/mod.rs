//! The cluster overview: counts, problem pods, capacity and warning events,
//! each narrowed or left out when RBAC forbids it.
//!
//! It is built from watches ([`watch_overview`]), not from repeated lists:
//! the initial list happens once, then only changes arrive, and the summary
//! is recomputed from the watched state at most once a second.
//! [`get_overview`] is the one-shot form (the first summary).

mod live;
mod summary;

pub use live::{watch_overview, OverviewUpdate};
pub use summary::{pod_status, PENDING_TOO_LONG_SECS};

use anyhow::{bail, Result};
use futures::StreamExt;
use kube::Client;
use serde::Serialize;

/// A warning event, linked to the object it is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EventInfo {
    /// `namespace/name` of the event.
    pub key: String,
    /// The event type: `Warning` or `Normal`.
    pub kind: String,
    pub reason: String,
    pub message: String,
    /// `Kind/name` of the involved object, for display.
    pub object: String,
    pub object_api_version: String,
    pub object_kind: String,
    /// Empty for cluster-scoped objects.
    pub object_namespace: String,
    pub object_name: String,
    /// When it last happened (unix seconds), so a UI can show its age.
    pub time: i64,
    pub count: i32,
}

/// A pod worth looking at: a waiting error (CrashLoopBackOff, ...),
/// running but not ready, or pending for more than five minutes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ProblemPod {
    pub namespace: String,
    pub name: String,
    /// As in the Pods table.
    pub status: String,
    /// Why it is listed: the status, "Not ready" or "Pending for 12m".
    pub reason: String,
    pub restarts: i32,
    /// Unix seconds.
    pub created: i64,
}

/// Pod counts, across the cluster or in one namespace. A pod counts once:
/// a crash-looping pod is in `waiting_errors`, not `running`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodCounts {
    /// The namespace these were narrowed to because listing pods across
    /// the cluster is forbidden; `None` = the whole cluster.
    pub namespace: Option<String>,
    pub total: usize,
    pub running: usize,
    pub pending: usize,
    pub succeeded: usize,
    pub failed: usize,
    /// A container waits for a reason other than starting up:
    /// CrashLoopBackOff, ImagePullBackOff, CreateContainerConfigError, ...
    pub waiting_errors: usize,
    pub by_namespace: Vec<(String, usize)>,
    /// The worst problem pods first, at most 50.
    pub problems: Vec<ProblemPod>,
    /// How many problem pods there are in all.
    pub problems_total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NodeCounts {
    pub total: usize,
    pub ready: usize,
}

/// Requests and usage against what the nodes can allocate, like Lens.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Capacity {
    pub cpu_requests_millicores: u64,
    pub cpu_allocatable_millicores: u64,
    /// 0 unless `metrics`.
    pub cpu_usage_millicores: u64,
    pub memory_requests_bytes: u64,
    pub memory_allocatable_bytes: u64,
    /// 0 unless `metrics`.
    pub memory_usage_bytes: u64,
    /// Whether usage came from the metrics API (metrics-server).
    pub metrics: bool,
    /// Pods bound to a node that have not finished.
    pub pods: u64,
    pub pods_allocatable: u64,
}

/// Warning events, across the cluster or in one namespace.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RecentEvents {
    /// As in [`PodCounts::namespace`].
    pub namespace: Option<String>,
    /// Newest first.
    pub items: Vec<EventInfo>,
}

/// The overview. Each part is computed on its own: a part the user may not
/// see is `None`, with a line in `notes` saying why, so a user with access
/// to only some namespaces still gets an overview.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Overview {
    pub namespaces: Option<usize>,
    pub nodes: Option<NodeCounts>,
    pub pods: Option<PodCounts>,
    /// Only when both the nodes and the pods of the whole cluster are
    /// visible: requests in one namespace say nothing about the nodes.
    pub capacity: Option<Capacity>,
    /// Warning events only.
    pub events: Option<RecentEvents>,
    /// What is missing or narrowed, and why.
    pub notes: Vec<String>,
}

/// Fetch the overview once: the first summary of [`watch_overview`].
///
/// Parts the user may not see cluster-wide are narrowed to `namespace`
/// (`None` = the context's namespace) or left out; only an error other than
/// a 403 fails the whole call.
pub async fn get_overview(client: &Client, namespace: Option<&str>) -> Result<Overview> {
    let mut updates = watch_overview(client, namespace);
    while let Some(update) = updates.next().await {
        match update {
            OverviewUpdate::Summary(overview) => return Ok(*overview),
            OverviewUpdate::State(crate::watch::WatchState::Reconnecting { error }) => {
                bail!(error)
            }
            OverviewUpdate::State(_) => {}
        }
    }
    bail!("the overview watch ended before its first summary")
}
