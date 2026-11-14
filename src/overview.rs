//! The cluster overview: counts, pods by namespace, recent events.

use crate::{format_age, get_namespaces, get_nodes, get_pods};
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

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Overview {
    pub namespaces: usize,
    pub nodes: usize,
    pub nodes_ready: usize,
    pub pods_total: usize,
    pub pods_running: usize,
    pub pods_pending: usize,
    pub pods_failed: usize,
    pub pods_by_namespace: Vec<(String, usize)>,
    pub events: Vec<EventInfo>,
}

/// Fetch cluster overview: counts, pods by namespace, recent events.
pub async fn get_overview(client: &Client) -> Result<Overview> {
    let (pods, namespaces, nodes, events) = tokio::join!(
        get_pods(client, None),
        get_namespaces(client),
        get_nodes(client),
        get_events(client),
    );
    let pods = pods?;
    let namespaces = namespaces?;
    let nodes = nodes?;
    let events = events?;

    let mut by_namespace: BTreeMap<String, usize> = BTreeMap::new();
    for pod in &pods {
        *by_namespace.entry(pod.namespace.clone()).or_insert(0) += 1;
    }
    let mut pods_by_namespace: Vec<(String, usize)> = by_namespace.into_iter().collect();
    pods_by_namespace.sort_by_key(|b| std::cmp::Reverse(b.1));

    Ok(Overview {
        namespaces: namespaces.len(),
        nodes_ready: nodes.iter().filter(|n| n.ready == "True").count(),
        nodes: nodes.len(),
        pods_running: pods.iter().filter(|p| p.phase == "Running").count(),
        pods_pending: pods.iter().filter(|p| p.phase == "Pending").count(),
        pods_failed: pods
            .iter()
            .filter(|p| p.phase == "Failed" || p.phase == "CrashLoopBackOff")
            .count(),
        pods_total: pods.len(),
        pods_by_namespace,
        events,
    })
}

async fn get_events(client: &Client) -> Result<Vec<EventInfo>> {
    let events: Api<Event> = Api::all(client.clone());
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
