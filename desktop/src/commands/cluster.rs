//! Contexts, namespaces, the overview, metrics, discovery and access checks.

use crate::state::{
    client, selected_context, stop_all_exec, stop_all_forwards, stop_all_watches, stop_log_stream,
    AppState,
};
use kubeloupe::resources::{self, AccessResult, ResourceKind, ResourceRef};
use kubeloupe::{
    get_namespaces, get_node_metrics, get_overview, get_pod_metrics, list_contexts, ContextInfo,
    NamespaceInfo, NodeUsage, Overview, PodUsage,
};
use serde::Serialize;
use specta::Type;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_list_namespaces(
    state: State<'_, AppState>,
) -> Result<Vec<NamespaceInfo>, String> {
    let client = client(&state).await?;
    get_namespaces(&client).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_get_overview(state: State<'_, AppState>) -> Result<Overview, String> {
    let client = client(&state).await?;
    get_overview(&client).await.map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct MetricsSnapshot {
    available: bool,
    pods: Vec<PodUsage>,
    nodes: Vec<NodeUsage>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_get_metrics(state: State<'_, AppState>) -> Result<MetricsSnapshot, String> {
    let client = client(&state).await?;
    let (pods, nodes) = tokio::join!(get_pod_metrics(&client), get_node_metrics(&client),);
    let pods = pods.map_err(|e| e.to_string())?;
    let nodes = nodes.map_err(|e| e.to_string())?;
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

/// Which of the kinds the current user may list.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_check_access(
    state: State<'_, AppState>,
    kinds: Vec<ResourceRef>,
) -> Result<Vec<AccessResult>, String> {
    let client = client(&state).await?;
    Ok(resources::check_access(&client, &kinds).await)
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
