#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::SinkExt;
use kube::api::TerminalSize;
use kube::Client;
use kubeloupe::{
    apply_pod_yaml, batch_lines, client_for_context, exec_pod, get_namespaces, get_node_metrics,
    get_nodes, get_overview, get_pod_details, get_pod_metrics, get_pod_yaml, get_pods,
    list_contexts, serve_port_forward, stream_pod_logs, watch, ContextInfo, LogOptions,
    NamespaceInfo, NodeInfo, NodeUsage, Overview, PodDetail, PodInfo, PodUsage,
};
use serde::Serialize;
use specta::Type;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, State};
use tauri_specta::{collect_commands, collect_events, Builder, Event};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn next_session(prefix: &str) -> String {
    format!(
        "{}-{}",
        prefix,
        NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
    )
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
    /// Running resource watchers, by watch id.
    watches: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
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

fn stop_all_watches(state: &State<'_, AppState>) {
    let mut watches = state.watches.lock().unwrap();
    for (_, task) in watches.drain() {
        task.abort();
    }
}

fn stop_all_forwards(state: &State<'_, AppState>) {
    let mut forwards = state.forwards.lock().unwrap();
    for (_, forward) in forwards.drain() {
        forward.task.abort();
    }
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
async fn cmd_list_namespaces(state: State<'_, AppState>) -> Result<Vec<NamespaceInfo>, String> {
    let client = client(&state).await?;
    get_namespaces(&client).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn cmd_list_nodes(state: State<'_, AppState>) -> Result<Vec<NodeInfo>, String> {
    let client = client(&state).await?;
    get_nodes(&client).await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
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

/// A batch of log lines. Log events carry the id of the stream that produced
/// them, so the UI can drop stragglers from a stream it has already replaced.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct PodLog {
    stream_id: u64,
    data: Vec<String>,
}

/// The log stream failed.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct PodLogError {
    stream_id: u64,
    data: String,
}

/// The log stream ended.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct PodLogDone {
    stream_id: u64,
}

/// Flush a log batch after this many lines or this long, whichever is first.
const LOG_BATCH_LINES: usize = 500;
const LOG_BATCH_INTERVAL: Duration = Duration::from_millis(50);

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
async fn cmd_stream_pod_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    previous: Option<bool>,
    tail_lines: Option<i64>,
    since_seconds: Option<i64>,
    timestamps: Option<bool>,
    stream_id: u64,
) -> Result<(), String> {
    stop_log_stream(&state);
    let client = client(&state).await?;
    let opts = LogOptions {
        container,
        previous: previous.unwrap_or(false),
        tail_lines,
        since_seconds,
        timestamps: timestamps.unwrap_or(false),
    };
    let handle = tauri::async_runtime::spawn(async move {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let emitter = app.clone();
        let batcher = tauri::async_runtime::spawn(batch_lines(
            rx,
            LOG_BATCH_LINES,
            LOG_BATCH_INTERVAL,
            move |batch| {
                let _ = PodLog {
                    stream_id,
                    data: batch,
                }
                .emit(&emitter);
            },
        ));
        let result = stream_pod_logs(&client, &namespace, &name, &opts, move |line| {
            let _ = tx.send(line);
        })
        .await;
        // The sender was dropped with the closure, so the batcher flushes and exits.
        let _ = batcher.await;
        if let Err(e) = result {
            let msg = format!("Log stream ended with an error: {e}");
            let _ = PodLogError {
                stream_id,
                data: msg,
            }
            .emit(&app);
        }
        let _ = PodLogDone { stream_id }.emit(&app);
    });
    *state.log_task.lock().unwrap() = Some(handle);
    Ok(())
}

/// Save log text to the user's Downloads folder and return the path. A
/// webview Blob download is not reliable across the platforms Tauri targets
/// and there is no file dialog without an extra plugin, so we write directly.
#[tauri::command]
#[specta::specta]
async fn cmd_save_logs(filename: String, content: String) -> Result<String, String> {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() || safe.starts_with('.') {
        format!("log{safe}")
    } else {
        safe
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("cannot locate home directory")?;
    let dir = std::path::PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe);
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
#[specta::specta]
async fn cmd_stop_pod_logs(state: State<'_, AppState>) -> Result<(), String> {
    stop_log_stream(&state);
    Ok(())
}

#[tauri::command]
#[specta::specta]
async fn cmd_get_overview(state: State<'_, AppState>) -> Result<Overview, String> {
    let client = client(&state).await?;
    get_overview(&client).await.map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Type)]
struct MetricsSnapshot {
    available: bool,
    pods: Vec<PodUsage>,
    nodes: Vec<NodeUsage>,
}

#[tauri::command]
#[specta::specta]
async fn cmd_get_metrics(state: State<'_, AppState>) -> Result<MetricsSnapshot, String> {
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

/// One coalesced set of changes for the watch `id`.
#[derive(Debug, Clone, Serialize, Type)]
struct WatchBatchOf<T> {
    id: String,
    snapshot: bool,
    applied: Vec<T>,
    deleted: Vec<String>,
}

/// Payload of the `watch-batch` event. Which variant arrives depends on the
/// `kind` the watch was started with; the variants are not tagged on the wire.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(untagged)]
enum WatchBatch {
    Pods(WatchBatchOf<PodInfo>),
    Nodes(WatchBatchOf<NodeInfo>),
    Namespaces(WatchBatchOf<NamespaceInfo>),
}

/// Payload of the `watch-state` event.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[tauri_specta(event_name = "watch-state")]
struct WatchStateChanged {
    id: String,
    #[serde(flatten)]
    state: watch::WatchState,
}

/// Forward a watch to the webview: state changes as `watch-state`, rows as
/// coalesced `watch-batch` events.
async fn pump_watch<T, S>(
    app: AppHandle,
    id: String,
    updates: S,
    wrap: fn(WatchBatchOf<T>) -> WatchBatch,
) where
    T: watch::Keyed + Clone + Serialize + Send + 'static,
    S: futures::Stream<Item = watch::Update<T>> + Unpin,
{
    watch::run_batched(updates, watch::DEFAULT_FLUSH, |out| match out {
        watch::Output::Batch(batch) => {
            let _ = wrap(WatchBatchOf {
                id: id.clone(),
                snapshot: batch.snapshot,
                applied: batch.applied,
                deleted: batch.deleted,
            })
            .emit(&app);
        }
        watch::Output::State(state) => {
            let _ = WatchStateChanged {
                id: id.clone(),
                state,
            }
            .emit(&app);
        }
    })
    .await;
}

/// Start watching `kind` ("pods", "nodes" or "namespaces"). Returns the
/// watch id carried by the `watch-batch` / `watch-state` events. The first
/// batch is a full snapshot; later ones are incremental.
#[tauri::command]
#[specta::specta]
async fn cmd_watch_start(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
    namespace: Option<String>,
) -> Result<String, String> {
    let client = client(&state).await?;
    let id = next_session("watch");
    let task_id = id.clone();
    let task = match kind.as_str() {
        "pods" => {
            let (_store, updates) = watch::watch_pods(&client, namespace.as_deref());
            tauri::async_runtime::spawn(pump_watch(app, task_id, updates, WatchBatch::Pods))
        }
        "nodes" => {
            let (_store, updates) = watch::watch_nodes(&client);
            tauri::async_runtime::spawn(pump_watch(app, task_id, updates, WatchBatch::Nodes))
        }
        "namespaces" => {
            let (_store, updates) = watch::watch_namespaces(&client);
            tauri::async_runtime::spawn(pump_watch(app, task_id, updates, WatchBatch::Namespaces))
        }
        other => return Err(format!("cannot watch {other}")),
    };
    state.watches.lock().unwrap().insert(id.clone(), task);
    Ok(id)
}

#[tauri::command]
#[specta::specta]
async fn cmd_watch_stop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if let Some(task) = state.watches.lock().unwrap().remove(&id) {
        task.abort();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
async fn cmd_set_context(state: State<'_, AppState>, name: Option<String>) -> Result<(), String> {
    stop_log_stream(&state);
    stop_all_exec(&state);
    stop_all_forwards(&state);
    stop_all_watches(&state);
    *state.context.lock().unwrap() = name;
    *state.client.lock().await = None;
    Ok(())
}

/// The shell of a terminal session exited (payload: the session id).
#[derive(Debug, Clone, Serialize, Type, Event)]
struct ExecClosed(String);

/// Output of a terminal session.
#[derive(Debug, Clone, Serialize, Type, Event)]
struct ExecOutput {
    session: String,
    stream: String,
    data: String,
}

/// Open an interactive shell in a pod. Returns the session id; output
/// arrives as `exec-output` events, `exec-closed` when the shell exits.
#[tauri::command]
#[specta::specta]
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
        let _ = tx
            .send(TerminalSize {
                width: cols,
                height: rows,
            })
            .await;
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
                        let _ = ExecOutput {
                            session: session_status,
                            stream: "stderr".to_string(),
                            data: format!("\r\n{msg}\r\n"),
                        }
                        .emit(&app_status);
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
                            let _ = ExecOutput {
                                session: session_id.clone(),
                                stream: "stdout".to_string(),
                                data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                            }
                            .emit(&app_out);
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
                            let _ = ExecOutput {
                                session: session_id.clone(),
                                stream: "stderr".to_string(),
                                data: String::from_utf8_lossy(&buf[..n]).into_owned(),
                            }
                            .emit(&app_out);
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
        let _ = ExecClosed(session_id).emit(&app);
    });
    state.exec_sessions.lock().unwrap().insert(
        session.clone(),
        ExecSession {
            stdin_tx,
            size_tx,
            task,
        },
    );
    Ok(session)
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
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
        tx.send(TerminalSize {
            width: cols,
            height: rows,
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
async fn cmd_exec_close(state: State<'_, AppState>, session: String) -> Result<(), String> {
    let mut sessions = state.exec_sessions.lock().unwrap();
    if let Some(entry) = sessions.remove(&session) {
        entry.task.abort();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Type)]
struct PortForwardInfo {
    session: String,
    local_port: u16,
}

/// Forward a pod port to localhost. Returns the session id and the local
/// port the forward listens on.
#[tauri::command]
#[specta::specta]
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
    Ok(PortForwardInfo {
        session,
        local_port,
    })
}

#[tauri::command]
#[specta::specta]
async fn cmd_portforward_stop(state: State<'_, AppState>, session: String) -> Result<(), String> {
    let mut forwards = state.forwards.lock().unwrap();
    if let Some(entry) = forwards.remove(&session) {
        entry.task.abort();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
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
#[specta::specta]
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

/// The typed command and event registry. Also used to generate
/// `ui-src/src/lib/bindings.ts`.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            cmd_list_pods,
            cmd_list_namespaces,
            cmd_list_nodes,
            cmd_get_pod_details,
            cmd_stream_pod_logs,
            cmd_stop_pod_logs,
            cmd_save_logs,
            cmd_get_overview,
            cmd_get_metrics,
            cmd_watch_start,
            cmd_watch_stop,
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
        .events(collect_events![
            WatchBatch,
            WatchStateChanged,
            PodLog,
            PodLogError,
            PodLogDone,
            ExecOutput,
            ExecClosed
        ])
        // Counters and epoch seconds stay far below 2^53.
        .dangerously_cast_bigints_to_number()
}

/// Where the generated bindings live.
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui-src/src/lib/bindings.ts");

fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)
        .expect("failed to export TypeScript bindings");
}

fn main() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .manage(AppState {
            context: Mutex::new(None),
            client: tokio::sync::Mutex::new(None),
            log_task: Mutex::new(None),
            exec_sessions: Mutex::new(HashMap::new()),
            forwards: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
        })
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running kubeloupe");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerates `bindings.ts`; CI runs this and fails if the committed
    /// file changes.
    #[test]
    fn export_typescript_bindings() {
        export_bindings(&specta_builder());
    }
}
