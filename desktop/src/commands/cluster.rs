//! Contexts, namespaces, metrics, discovery and access checks.

use crate::state::{
    client, selected_context, stop_all_exec, stop_all_forwards, stop_all_watches, stop_log_stream,
    AppState,
};
use kubeloupe::resources::{self, AccessResult, ResourceKind, ResourceRef};
use kubeloupe::{
    default_namespace, get_node_metrics, get_pod_metrics, list_contexts,
    list_namespaces_if_allowed, ContextInfo, NamespaceInfo, NodeUsage, PodUsage,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

#[tauri::command]
#[specta::specta]
/// The cluster's namespaces, or `null` if the user may not list them (the
/// UI then lets them type one).
pub(crate) async fn cmd_list_namespaces(
    state: State<'_, AppState>,
) -> Result<Option<Vec<NamespaceInfo>>, String> {
    let client = client(&state).await?;
    list_namespaces_if_allowed(&client)
        .await
        .map_err(|e| e.to_string())
}

/// The namespace to start on: `null` (all namespaces) if the user may list
/// pods cluster-wide, otherwise the context's namespace or the first one
/// they can access.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_default_namespace(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let client = client(&state).await?;
    Ok(default_namespace(&client).await)
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct MetricsSnapshot {
    available: bool,
    pods: Vec<PodUsage>,
    nodes: Vec<NodeUsage>,
}

/// Which metrics a view shows.
#[derive(Debug, Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MetricsKind {
    Pods,
    Nodes,
}

/// Usage for what is on screen: pod metrics in `namespace` (`null` = all
/// namespaces), or node metrics (`namespace` is ignored). The other list
/// is empty.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_get_metrics(
    state: State<'_, AppState>,
    namespace: Option<String>,
    what: MetricsKind,
) -> Result<MetricsSnapshot, String> {
    let client = client(&state).await?;
    let (pods, nodes) = match what {
        MetricsKind::Pods => (get_pod_metrics(&client, namespace.as_deref()).await, None),
        MetricsKind::Nodes => (Ok(None), Some(get_node_metrics(&client).await)),
    };
    let pods = pods.map_err(|e| e.to_string())?;
    let nodes = nodes.transpose().map_err(|e| e.to_string())?.flatten();
    let available = pods.is_some() || nodes.is_some();
    Ok(MetricsSnapshot {
        available,
        pods: pods.unwrap_or_default(),
        nodes: nodes.unwrap_or_default(),
    })
}

/// Discover every kind the cluster serves (including CRDs) with its columns.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_discover(state: State<'_, AppState>) -> Result<Vec<ResourceKind>, String> {
    let client = client(&state).await?;
    let kinds = resources::discover(&client)
        .await
        .map_err(|e| e.to_string())?;
    *state.kinds.lock().unwrap() = kinds.clone();
    Ok(kinds)
}

/// Which of the kinds the current user may list in `namespace` (`null` =
/// across all namespaces; ignored for cluster-scoped kinds).
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_check_access(
    state: State<'_, AppState>,
    kinds: Vec<ResourceRef>,
    namespace: Option<String>,
) -> Result<Vec<AccessResult>, String> {
    let client = client(&state).await?;
    Ok(resources::check_access(&client, &kinds, namespace.as_deref()).await)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_list_contexts(
    state: State<'_, AppState>,
) -> Result<Vec<ContextInfo>, String> {
    let mut contexts = list_contexts().await.map_err(|e| e.to_string())?;
    if let Some(selected) = selected_context(&state) {
        for ctx in &mut contexts {
            ctx.current = ctx.name == selected;
        }
    }
    Ok(contexts)
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_set_context(
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<(), String> {
    stop_log_stream(&state);
    stop_all_exec(&state);
    stop_all_forwards(&state);
    stop_all_watches(&state);
    state.kinds.lock().unwrap().clear();
    *state.context.lock().unwrap() = name;
    *state.client.lock().await = None;
    Ok(())
}
