//! Usage history (#47): the ring buffers in `AppState::usage`, fed by
//! `cmd_get_metrics` and by the detail panels through `cmd_usage_history`.

use crate::commands::cluster::MetricsKind;
use crate::state::{client, selected_context, AppState};
use kubeloupe::{
    get_one_node_metrics, get_one_pod_metrics, get_usage_reference, pod_key, NodeUsage, PodUsage,
    SeriesView, Spark, UsageReference, MIN_SPACING_MS,
};
use serde::Serialize;
use specta::Type;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record what a metrics poll returned and give back the sparklines of those
/// pods or nodes, with when their history starts (0 = none). `ctx` is the
/// context they were fetched from: if it changed meanwhile, nothing is
/// recorded, so one cluster's pods never land in another's history.
pub(crate) fn record(
    state: &AppState,
    ctx: &Option<String>,
    pods: &[PodUsage],
    nodes: &[NodeUsage],
) -> (HashMap<String, Spark>, u64) {
    let now = now_ms();
    let mut history = state.usage.lock().unwrap();
    if *state.context.lock().unwrap() != *ctx {
        return (HashMap::new(), 0);
    }
    if !pods.is_empty() {
        history.record_pods(now, pods);
        let keys: Vec<String> = pods
            .iter()
            .map(|p| pod_key(&p.namespace, &p.name))
            .collect();
        let since = history.since(true, keys.iter().map(String::as_str));
        (history.pod_sparks(pods), since)
    } else if !nodes.is_empty() {
        history.record_nodes(now, nodes);
        let since = history.since(false, nodes.iter().map(|n| n.name.as_str()));
        (history.node_sparks(nodes), since)
    } else {
        (HashMap::new(), 0)
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct UsageHistoryView {
    history: SeriesView,
    reference: UsageReference,
}

/// The usage history of one pod (`namespace` set) or node, for the charts in
/// its detail panel, with its requests and limits or allocatable. Polled
/// while the panel is open: when no table has sampled the object lately, it
/// samples it itself, so history keeps growing with only the panel open.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_usage_history(
    state: State<'_, AppState>,
    what: MetricsKind,
    namespace: Option<String>,
    name: String,
) -> Result<UsageHistoryView, String> {
    let ctx = selected_context(&state);
    let client = client(&state).await?;
    let pod = matches!(what, MetricsKind::Pods);
    let ns = if pod { namespace.as_deref() } else { None };
    if pod && ns.is_none() {
        return Err("A pod's usage history needs its namespace".into());
    }
    let key = match ns {
        Some(ns) => pod_key(ns, &name),
        None => name.clone(),
    };
    let last = state.usage.lock().unwrap().last_sample(pod, &key);
    if last.is_none_or(|t| now_ms() >= t + MIN_SPACING_MS) {
        // Best effort, like the table metrics: no sample yet is fine.
        match ns {
            Some(ns) => {
                if let Ok(Some(u)) = get_one_pod_metrics(&client, ns, &name).await {
                    record(&state, &ctx, &[u], &[]);
                }
            }
            None => {
                if let Ok(Some(u)) = get_one_node_metrics(&client, &name).await {
                    record(&state, &ctx, &[], &[u]);
                }
            }
        }
    }
    // Without access to the object the chart just has no reference lines.
    let reference = get_usage_reference(&client, ns, &name)
        .await
        .unwrap_or_default();
    let history = state.usage.lock().unwrap().view(pod, &key);
    Ok(UsageHistoryView { history, reference })
}
