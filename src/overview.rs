//! The cluster overview: counts, pods by namespace, recent events, each
//! narrowed or left out when RBAC forbids it.

use crate::{format_age, get_namespaces, get_nodes, get_pods, is_forbidden, PodInfo};
use anyhow::Result;
use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::Event;
use kube::api::{Api, ListParams};
use kube::Client;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EventInfo {
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub object: String,
    pub age: String,
    pub count: i32,
}

/// Pod counts, across the cluster or in one namespace.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodCounts {
    /// The namespace these were narrowed to because listing pods across
    /// the cluster is forbidden; `None` = the whole cluster.
    pub namespace: Option<String>,
    pub total: usize,
    pub running: usize,
    pub pending: usize,
    pub failed: usize,
    pub by_namespace: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NodeCounts {
    pub total: usize,
    pub ready: usize,
}

/// Recent events, across the cluster or in one namespace.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RecentEvents {
    /// As in [`PodCounts::namespace`].
    pub namespace: Option<String>,
    pub items: Vec<EventInfo>,
}

/// The overview. Each part is computed on its own: a part the user may not
/// see is `None`, with a line in `notes` saying why, so a user with access
/// to only some namespaces still gets an overview.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Overview {
    pub namespaces: Option<usize>,
    pub nodes: Option<NodeCounts>,
    pub pods: Option<PodCounts>,
    pub events: Option<RecentEvents>,
    /// What is missing or narrowed, and why.
    pub notes: Vec<String>,
}

/// List across the cluster (`list(None)`); if that is forbidden, in
/// `fallback` instead. `None` if both are forbidden, with a note either way.
/// Other errors (an unreachable cluster) fail.
async fn list_scoped<T, F, Fut>(
    what: &str,
    fallback: &str,
    list: F,
) -> (Result<Option<(Option<String>, T)>>, Option<String>)
where
    F: Fn(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    match list(None).await {
        Ok(items) => return (Ok(Some((None, items))), None),
        Err(e) if !is_forbidden(&e) => return (Err(e), None),
        Err(_) => {}
    }
    match list(Some(fallback.to_string())).await {
        Ok(items) => (
            Ok(Some((Some(fallback.to_string()), items))),
            Some(format!(
                "{what}: showing namespace {fallback} only (not allowed across all namespaces)"
            )),
        ),
        Err(e) if is_forbidden(&e) => (
            Ok(None),
            Some(format!(
                "{what}: not allowed across all namespaces or in namespace {fallback}"
            )),
        ),
        Err(e) => (Err(e), None),
    }
}

/// Fetch the overview: counts, pods by namespace, recent events.
///
/// Parts the user may not see cluster-wide are narrowed to `namespace`
/// (`None` = the context's namespace) or left out; only an error other than
/// a 403 fails the whole call.
pub async fn get_overview(client: &Client, namespace: Option<&str>) -> Result<Overview> {
    let fallback = namespace.unwrap_or(client.default_namespace()).to_string();
    let ((pods, pods_note), (events, events_note), namespaces, nodes) = tokio::join!(
        list_scoped("Pods", &fallback, |ns| async move {
            get_pods(client, ns.as_deref()).await
        }),
        list_scoped("Events", &fallback, |ns| async move {
            get_events(client, ns.as_deref()).await
        }),
        get_namespaces(client),
        get_nodes(client),
    );
    let mut notes: Vec<String> = pods_note.into_iter().chain(events_note).collect();

    let namespaces = match namespaces {
        Ok(list) => Some(list.len()),
        Err(e) if is_forbidden(&e) => {
            notes.push("Namespaces: not allowed to list namespaces".into());
            None
        }
        Err(e) => return Err(e),
    };
    let nodes = match nodes {
        Ok(list) => Some(NodeCounts {
            ready: list.iter().filter(|n| n.ready == "True").count(),
            total: list.len(),
        }),
        Err(e) if is_forbidden(&e) => {
            notes.push("Nodes: not allowed to list nodes".into());
            None
        }
        Err(e) => return Err(e),
    };
    let pods = pods?.map(|(namespace, pods)| pod_counts(namespace, &pods));
    let events = events?.map(|(namespace, items)| RecentEvents { namespace, items });

    Ok(Overview {
        namespaces,
        nodes,
        pods,
        events,
        notes,
    })
}

fn pod_counts(namespace: Option<String>, pods: &[PodInfo]) -> PodCounts {
    let mut by_namespace: BTreeMap<String, usize> = BTreeMap::new();
    for pod in pods {
        *by_namespace.entry(pod.namespace.clone()).or_insert(0) += 1;
    }
    let mut by_namespace: Vec<(String, usize)> = by_namespace.into_iter().collect();
    by_namespace.sort_by_key(|b| std::cmp::Reverse(b.1));
    PodCounts {
        namespace,
        total: pods.len(),
        running: pods.iter().filter(|p| p.phase == "Running").count(),
        pending: pods.iter().filter(|p| p.phase == "Pending").count(),
        failed: pods
            .iter()
            .filter(|p| p.phase == "Failed" || p.phase == "CrashLoopBackOff")
            .count(),
        by_namespace,
    }
}

async fn get_events(client: &Client, namespace: Option<&str>) -> Result<Vec<EventInfo>> {
    let events: Api<Event> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let list = events.list(&ListParams::default()).await?;
    let mut infos: Vec<(DateTime<Utc>, EventInfo)> = list
        .items
        .into_iter()
        .map(|e| {
            let ts = e
                .last_timestamp
                .as_ref()
                .map(|t| t.0)
                .or_else(|| e.event_time.as_ref().map(|t| t.0))
                .or_else(|| e.first_timestamp.as_ref().map(|t| t.0))
                .unwrap_or_else(Utc::now);
            let object = format!(
                "{}/{}",
                e.involved_object.kind.clone().unwrap_or_default(),
                e.involved_object.name.clone().unwrap_or_default()
            );
            (
                ts,
                EventInfo {
                    kind: e.type_.clone().unwrap_or_default(),
                    reason: e.reason.clone().unwrap_or_default(),
                    message: e.message.clone().unwrap_or_default(),
                    object,
                    age: format_age(ts),
                    count: e.count.unwrap_or(1),
                },
            )
        })
        .collect();
    infos.sort_by_key(|b| std::cmp::Reverse(b.0));
    Ok(infos.into_iter().take(15).map(|(_, info)| info).collect())
}
