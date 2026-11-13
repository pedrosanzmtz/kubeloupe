#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use kubeloupe::{
    get_namespaces, get_nodes, get_overview, get_pod_details, get_pods, list_contexts,
    stream_pod_logs, ContextInfo, NamespaceInfo, NodeInfo, Overview, PodDetail, PodInfo,
};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

struct AppState {
    /// Selected context (`None` = the kubeconfig's current-context).
    context: Mutex<Option<String>>,
    /// The running log stream, if any.
    log_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

fn selected_context(state: &State<'_, AppState>) -> Option<String> {
    state.context.lock().unwrap().clone()
}

fn stop_log_stream(state: &State<'_, AppState>) {
    if let Some(handle) = state.log_task.lock().unwrap().take() {
        handle.abort();
    }
}

#[tauri::command]
async fn cmd_list_pods(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> Result<Vec<PodInfo>, String> {
    let ctx = selected_context(&state);
    get_pods(ctx.as_deref(), namespace.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_namespaces(state: State<'_, AppState>) -> Result<Vec<NamespaceInfo>, String> {
    let ctx = selected_context(&state);
    get_namespaces(ctx.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_nodes(state: State<'_, AppState>) -> Result<Vec<NodeInfo>, String> {
    let ctx = selected_context(&state);
    get_nodes(ctx.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_get_pod_details(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> Result<PodDetail, String> {
    let ctx = selected_context(&state);
    get_pod_details(ctx.as_deref(), &namespace, &name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_stream_pod_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> Result<(), String> {
    stop_log_stream(&state);
    let ctx = selected_context(&state);
    let handle = tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        let result = stream_pod_logs(ctx.as_deref(), &namespace, &name, None, move |line| {
            let _ = emitter.emit("pod-log", line);
        })
        .await;
        if result.is_err() {
            let _ = app.emit("pod-log-error", "Log stream ended with an error");
        }
        let _ = app.emit("pod-log-done", ());
    });
    *state.log_task.lock().unwrap() = Some(handle);
    Ok(())
}

#[tauri::command]
async fn cmd_stop_pod_logs(state: State<'_, AppState>) -> Result<(), String> {
    stop_log_stream(&state);
    Ok(())
}

#[tauri::command]
async fn cmd_get_overview(state: State<'_, AppState>) -> Result<Overview, String> {
    let ctx = selected_context(&state);
    get_overview(ctx.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_contexts(state: State<'_, AppState>) -> Result<Vec<ContextInfo>, String> {
    let mut contexts = list_contexts().await.map_err(|e| e.to_string())?;
    if let Some(selected) = selected_context(&state) {
        for ctx in &mut contexts {
            ctx.current = ctx.name == selected;
        }
    }
    Ok(contexts)
}

#[tauri::command]
async fn cmd_set_context(
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<(), String> {
    stop_log_stream(&state);
    *state.context.lock().unwrap() = name;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            context: Mutex::new(None),
            log_task: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            cmd_list_pods,
            cmd_list_namespaces,
            cmd_list_nodes,
            cmd_get_pod_details,
            cmd_stream_pod_logs,
            cmd_stop_pod_logs,
            cmd_get_overview,
            cmd_list_contexts,
            cmd_set_context
        ])
        .run(tauri::generate_context!())
        .expect("error while running kubeloupe");
}
