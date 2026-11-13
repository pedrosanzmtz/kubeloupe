#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::SinkExt;
use kube::api::TerminalSize;
use kube::Client;
use kubeloupe::{
    client_for_context,
    apply_pod_yaml, exec_pod, get_namespaces, get_node_metrics, get_nodes, get_overview,
    get_pod_details, get_pod_metrics, get_pod_yaml, get_pods, list_contexts, serve_port_forward,
    stream_pod_logs, ContextInfo, NamespaceInfo, NodeInfo, NodeUsage, Overview, PodDetail, PodInfo,
    PodUsage,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn next_session(prefix: &str) -> String {
    format!("{}-{}", prefix, NEXT_SESSION.fetch_add(1, Ordering::Relaxed))
}

struct ExecSession {
    stdin_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    size_tx: Option<futures::channel::mpsc::Sender<TerminalSize>>,
    task: tauri::async_runtime::JoinHandle<()>,
}

struct ForwardSession {
    #[allow(dead_code)]
    local_port: u16,
    #[allow(dead_code)]
    pod_port: u16,
    task: tokio::task::JoinHandle<()>,
}

struct AppState {
    /// Selected context (`None` = the kubeconfig's current-context).
    context: Mutex<Option<String>>,
    /// Client for the selected context, built lazily and dropped when the
    /// context changes. `Client` is cheap to clone.
    client: tokio::sync::Mutex<Option<Client>>,
    /// The running log stream, if any.
    log_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Open terminal sessions, by session id.
    exec_sessions: Mutex<HashMap<String, ExecSession>>,
    /// Active port-forwards, by session id.
    forwards: Mutex<HashMap<String, ForwardSession>>,
}

fn selected_context(state: &State<'_, AppState>) -> Option<String> {
    state.context.lock().unwrap().clone()
}

/// The cached client for the selected context, building it on first use.
async fn client(state: &State<'_, AppState>) -> Result<Client, String> {
    let mut slot = state.client.lock().await;
    if let Some(c) = slot.as_ref() {
        return Ok(c.clone());
    }
    let ctx = selected_context(state);
    let c = client_for_context(ctx.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    *slot = Some(c.clone());
    Ok(c)
}

fn stop_log_stream(state: &State<'_, AppState>) {
    if let Some(handle) = state.log_task.lock().unwrap().take() {
        handle.abort();
    }
}

fn stop_all_exec(state: &State<'_, AppState>) {
    let mut sessions = state.exec_sessions.lock().unwrap();
    for (_, session) in sessions.drain() {
        session.task.abort();
    }
}

fn stop_all_forwards(state: &State<'_, AppState>) {
    let mut forwards = state.forwards.lock().unwrap();
    for (_, forward) in forwards.drain() {
        forward.task.abort();
    }
}

#[tauri::command]
async fn cmd_list_pods(
    state: State<'_, AppState>,
    namespace: Option<String>,
) -> Result<Vec<PodInfo>, String> {
    let client = client(&state).await?;
    get_pods(&client, namespace.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_namespaces(state: State<'_, AppState>) -> Result<Vec<NamespaceInfo>, String> {
    let client = client(&state).await?;
    get_namespaces(&client)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_list_nodes(state: State<'_, AppState>) -> Result<Vec<NodeInfo>, String> {
    let client = client(&state).await?;
    get_nodes(&client).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_get_pod_details(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> Result<PodDetail, String> {
    let client = client(&state).await?;
    get_pod_details(&client, &namespace, &name)
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
    let client = client(&state).await?;
    let handle = tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        let result = stream_pod_logs(&client, &namespace, &name, None, move |line| {
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
    let client = client(&state).await?;
    get_overview(&client).await.map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
struct MetricsSnapshot {
    available: bool,
    pods: Vec<PodUsage>,
    nodes: Vec<NodeUsage>,
}

#[tauri::command]
async fn cmd_get_metrics(state: State<'_, AppState>) -> Result<MetricsSnapshot, String> {
    let client = client(&state).await?;
    let (pods, nodes) = tokio::join!(
        get_pod_metrics(&client),
        get_node_metrics(&client),
    );
    let pods = pods.map_err(|e| e.to_string())?;
    let nodes = nodes.map_err(|e| e.to_string())?;
    let available = pods.is_some() || nodes.is_some();
    Ok(MetricsSnapshot {
        available,
        pods: pods.unwrap_or_default(),
        nodes: nodes.unwrap_or_default(),
    })
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
    stop_all_exec(&state);
    stop_all_forwards(&state);
    *state.context.lock().unwrap() = name;
    *state.client.lock().await = None;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ExecOutput {
    session: String,
    stream: String,
    data: String,
}

/// Open an interactive shell in a pod. Returns the session id; output
/// arrives as `exec-output` events, `exec-closed` when the shell exits.
#[tauri::command]
async fn cmd_exec_start(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let client = client(&state).await?;
    let mut attached = exec_pod(&client, &namespace, &name, container.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    let session = next_session("exec");
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let mut stdin_writer = attached.stdin();
    let mut stdout_reader = attached.stdout();
    let mut stderr_reader = attached.stderr();
    let mut size_tx = attached.terminal_size();
    if let Some(tx) = size_tx.as_mut() {
        let _ = tx.send(TerminalSize { width: cols, height: rows }).await;
    }
    let session_id = session.clone();
    let app_out = app.clone();
    // Surface a failed exec (e.g. no kubelet) in the terminal: join() alone
    // returns Ok even on failure; the Status channel carries the error.
    if let Some(status_fut) = attached.take_status() {
        let app_status = app.clone();
        let session_status = session.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(status) = status_fut.await {
                if status.status.as_deref() == Some("Failure") {
                    if let Some(msg) = status.message {
                        let _ = app_status.emit(
                            "exec-output",
                            ExecOutput {
                                session: session_status,
                                stream: "stderr".to_string(),
                                data: format!("\r\n{msg}\r\n"),
                            },
                        );
                    }
                }
            }
        });
    }
    let task = tauri::async_runtime::spawn(async move {
        let stdin_pump = async {
            if let Some(mut w) = stdin_writer.take() {
                while let Some(bytes) = stdin_rx.recv().await {
                    if w.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                let _ = w.shutdown().await;
            }
        };
        let stdout_pump = async {
            if let Some(mut r) = stdout_reader.take() {
                let mut buf = [0u8; 4096];
                loop {
                    match r.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let _ = app_out.emit(
                                "exec-output",
                                ExecOutput {
                                    session: session_id.clone(),
                                    stream: "stdout".to_string(),
                                    data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                                },
                            );
                        }
                    }
                }
            }
        };
        let stderr_pump = async {
            if let Some(mut r) = stderr_reader.take() {
                let mut buf = [0u8; 4096];
                loop {
                    match r.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let _ = app_out.emit(
                                "exec-output",
                                ExecOutput {
                                    session: session_id.clone(),
                                    stream: "stderr".to_string(),
                                    data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                                },
                            );
                        }
                    }
                }
            }
        };
        // The stdin pump waits on its channel, so it would outlive the shell;
        // finish when either all pumps end or the process itself exits.
        tokio::select! {
            () = async { tokio::join!(stdin_pump, stdout_pump, stderr_pump); } => {},
            _ = attached.join() => {},
        }
        let _ = app.emit("exec-closed", session_id);
    });
    state.exec_sessions.lock().unwrap().insert(
        session.clone(),
        ExecSession { stdin_tx, size_tx, task },
    );
    Ok(session)
}

#[tauri::command]
async fn cmd_exec_input(
    state: State<'_, AppState>,
    session: String,
    data: String,
) -> Result<(), String> {
    // Clone the sender under the lock, then await outside it: holding a
    // std MutexGuard across an await would make this future non-Send.
    let tx = {
        let sessions = state.exec_sessions.lock().unwrap();
        sessions.get(&session).map(|entry| entry.stdin_tx.clone())
    };
    let Some(tx) = tx else {
        return Err("exec session not found".to_string());
    };
    tx.send(data.into_bytes())
        .await
        .map_err(|_| "exec session closed".to_string())
}

#[tauri::command]
async fn cmd_exec_resize(
    state: State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let tx = {
        let sessions = state.exec_sessions.lock().unwrap();
        sessions
            .get(&session)
            .and_then(|entry| entry.size_tx.clone())
    };
    if let Some(mut tx) = tx {
        tx.send(TerminalSize { width: cols, height: rows })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn cmd_exec_close(state: State<'_, AppState>, session: String) -> Result<(), String> {
    let mut sessions = state.exec_sessions.lock().unwrap();
    if let Some(entry) = sessions.remove(&session) {
        entry.task.abort();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct PortForwardInfo {
    session: String,
    local_port: u16,
}

/// Forward a pod port to localhost. Returns the session id and the local
/// port the forward listens on.
#[tauri::command]
async fn cmd_portforward_start(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    port: u16,
) -> Result<PortForwardInfo, String> {
    let client = client(&state).await?;
    let (local_port, task) = serve_port_forward(&client, &namespace, &name, port)
        .await
        .map_err(|e| e.to_string())?;
    let session = next_session("pf");
    state.forwards.lock().unwrap().insert(
        session.clone(),
        ForwardSession {
            local_port,
            pod_port: port,
            task,
        },
    );
    Ok(PortForwardInfo { session, local_port })
}

#[tauri::command]
async fn cmd_portforward_stop(state: State<'_, AppState>, session: String) -> Result<(), String> {
    let mut forwards = state.forwards.lock().unwrap();
    if let Some(entry) = forwards.remove(&session) {
        entry.task.abort();
    }
    Ok(())
}

#[tauri::command]
async fn cmd_get_pod_yaml(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> Result<String, String> {
    let client = client(&state).await?;
    get_pod_yaml(&client, &namespace, &name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cmd_apply_pod_yaml(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    yaml: String,
) -> Result<(), String> {
    let client = client(&state).await?;
    apply_pod_yaml(&client, &namespace, &name, &yaml)
        .await
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            context: Mutex::new(None),
            client: tokio::sync::Mutex::new(None),
            log_task: Mutex::new(None),
            exec_sessions: Mutex::new(HashMap::new()),
            forwards: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            cmd_list_pods,
            cmd_list_namespaces,
            cmd_list_nodes,
            cmd_get_pod_details,
            cmd_stream_pod_logs,
            cmd_stop_pod_logs,
            cmd_get_overview,
            cmd_get_metrics,
            cmd_list_contexts,
            cmd_set_context,
            cmd_exec_start,
            cmd_exec_input,
            cmd_exec_resize,
            cmd_exec_close,
            cmd_portforward_start,
            cmd_portforward_stop,
            cmd_get_pod_yaml,
            cmd_apply_pod_yaml
        ])
        .run(tauri::generate_context!())
        .expect("error while running kubeloupe");
}
