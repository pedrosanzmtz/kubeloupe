#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::SinkExt;
use kube::api::TerminalSize;
use kube::Client;
use kubeloupe::actions::{self, ActionId, BulkItemResult, DrainEvent, DrainSummary};
use kubeloupe::resources::{
    self, AccessResult, ResourceDetail, ResourceKind, ResourceRef, ResourceRow,
};
use kubeloupe::{
    batch_lines, client_for_context, exec_pod, get_namespaces, get_node_metrics, get_overview,
    get_pod_details, get_pod_metrics, list_contexts, serve_port_forward, stream_pod_logs,
    stream_workload_logs, watch, ContextInfo, LogOptions, NamespaceInfo, NodeUsage, Overview,
    PodDetail, PodUsage, StopReason, TaggedLine, WorkloadEvent, WorkloadKind,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
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
    /// The running log stream (pod or workload), with its stream id.
    log_task: Mutex<Option<(u64, tauri::async_runtime::JoinHandle<()>)>>,
    /// Open terminal sessions, by session id.
    exec_sessions: Mutex<HashMap<String, ExecSession>>,
    /// Active port-forwards, by session id.
    forwards: Mutex<HashMap<String, ForwardSession>>,
    /// Running resource watchers, by watch id.
    watches: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    /// What the last discovery found, so watches can look up their columns.
    kinds: Mutex<Vec<ResourceKind>>,
    /// Running node drains, by drain id.
    drains: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
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
    if let Some((_, handle)) = state.log_task.lock().unwrap().take() {
        handle.abort();
    }
}

/// Stop the log stream only if it is still `stream_id` (another viewer may
/// have replaced it since).
fn stop_log_stream_if(state: &State<'_, AppState>, stream_id: u64) {
    let mut slot = state.log_task.lock().unwrap();
    if matches!(slot.as_ref(), Some((id, _)) if *id == stream_id) {
        if let Some((_, handle)) = slot.take() {
            handle.abort();
        }
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
async fn cmd_list_namespaces(state: State<'_, AppState>) -> Result<Vec<NamespaceInfo>, String> {
    let client = client(&state).await?;
    get_namespaces(&client).await.map_err(|e| e.to_string())
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
    *state.log_task.lock().unwrap() = Some((stream_id, handle));
    Ok(())
}

/// A batch of merged log lines from the pods of a workload. Same `streamId`
/// scheme as [`PodLog`]; every line says which pod and container it is from.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct WorkloadLog {
    stream_id: u64,
    data: Vec<TaggedLine>,
}

#[derive(Debug, Clone, Copy, Serialize, Type)]
#[serde(rename_all = "camelCase")]
enum WorkloadNoticeKind {
    /// A per-container stream started (a pod appeared).
    Started,
    /// The container exited and its log ended.
    Ended,
    /// The pod was deleted.
    Deleted,
    /// The stream failed; see `message`.
    Failed,
    /// More containers match than the stream limit: `skipped` are waiting.
    Truncated,
}

/// A change in which pods a workload stream is following.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct WorkloadNotice {
    stream_id: u64,
    kind: WorkloadNoticeKind,
    /// Empty for `truncated`.
    pod: String,
    container: String,
    message: String,
    /// For `truncated`: the stream limit and how many containers are waiting.
    limit: u32,
    skipped: u32,
}

/// Options of a workload log stream.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
struct WorkloadLogOptions {
    /// One container name for every pod; `None` follows all containers.
    container: Option<String>,
    /// Applies to the pods present when the stream starts; pods that appear
    /// later are read from their beginning.
    tail_lines: Option<i64>,
    since_seconds: Option<i64>,
    timestamps: Option<bool>,
}

fn workload_notice(stream_id: u64, ev: WorkloadEvent) -> Option<WorkloadNotice> {
    let (kind, pod, container, message, limit, skipped) = match ev {
        WorkloadEvent::Lines(_) => return None,
        WorkloadEvent::Started { pod, container } => (
            WorkloadNoticeKind::Started,
            pod,
            container,
            String::new(),
            0,
            0,
        ),
        WorkloadEvent::Stopped {
            pod,
            container,
            reason,
        } => {
            let (kind, message) = match reason {
                StopReason::Deleted => (WorkloadNoticeKind::Deleted, "pod deleted".to_string()),
                StopReason::Ended => (WorkloadNoticeKind::Ended, "log ended".to_string()),
                StopReason::Failed(e) => (WorkloadNoticeKind::Failed, e),
            };
            (kind, pod, container, message, 0, 0)
        }
        WorkloadEvent::Truncated { limit, skipped } => (
            WorkloadNoticeKind::Truncated,
            String::new(),
            String::new(),
            format!("following {limit} containers, {skipped} more waiting"),
            limit as u32,
            skipped as u32,
        ),
    };
    Some(WorkloadNotice {
        stream_id,
        kind,
        pod,
        container,
        message,
        limit,
        skipped,
    })
}

/// Follow the logs of all pods of a Deployment, StatefulSet, DaemonSet, Job
/// or ReplicaSet (`kind` is case-insensitive). Replaces any running log
/// stream; stopped by `cmd_stop_pod_logs`, `cmd_stop_workload_logs`, another
/// log stream or a context switch.
#[tauri::command]
#[specta::specta]
async fn cmd_stream_workload_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    options: WorkloadLogOptions,
    stream_id: u64,
) -> Result<(), String> {
    stop_log_stream(&state);
    let kind = kind.parse::<WorkloadKind>().map_err(|e| e.to_string())?;
    let client = client(&state).await?;
    let opts = LogOptions {
        container: options.container,
        previous: false,
        tail_lines: options.tail_lines,
        since_seconds: options.since_seconds,
        timestamps: options.timestamps.unwrap_or(false),
    };
    let handle = tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        let result =
            stream_workload_logs(
                &client,
                kind,
                &namespace,
                &name,
                &opts,
                move |ev| match ev {
                    WorkloadEvent::Lines(data) => {
                        let _ = WorkloadLog { stream_id, data }.emit(&emitter);
                    }
                    ev => {
                        if let Some(n) = workload_notice(stream_id, ev) {
                            let _ = n.emit(&emitter);
                        }
                    }
                },
            )
            .await;
        if let Err(e) = result {
            let _ = PodLogError {
                stream_id,
                data: format!("Log stream ended with an error: {e}"),
            }
            .emit(&app);
        }
        let _ = PodLogDone { stream_id }.emit(&app);
    });
    *state.log_task.lock().unwrap() = Some((stream_id, handle));
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

/// Stop a workload log stream (and every per-pod task under it), unless a
/// newer stream has already replaced it.
#[tauri::command]
#[specta::specta]
async fn cmd_stop_workload_logs(state: State<'_, AppState>, stream_id: u64) -> Result<(), String> {
    stop_log_stream_if(&state, stream_id);
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

/// One coalesced set of table rows for the watch `id`: the first batch is a
/// full snapshot, later ones are incremental.
#[derive(Debug, Clone, Serialize, Type, Event)]
struct WatchBatch {
    id: String,
    snapshot: bool,
    applied: Vec<ResourceRow>,
    /// Keys of removed rows.
    deleted: Vec<String>,
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
async fn pump_watch<S>(app: AppHandle, id: String, updates: S)
where
    S: futures::Stream<Item = watch::Update<ResourceRow>> + Unpin,
{
    watch::run_batched(updates, watch::DEFAULT_FLUSH, |out| match out {
        watch::Output::Batch(batch) => {
            let _ = WatchBatch {
                id: id.clone(),
                snapshot: batch.snapshot,
                applied: batch.applied,
                deleted: batch.deleted,
            }
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

/// Discover every kind the cluster serves (including CRDs) with its columns.
#[tauri::command]
#[specta::specta]
async fn cmd_discover(state: State<'_, AppState>) -> Result<Vec<ResourceKind>, String> {
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
async fn cmd_check_access(
    state: State<'_, AppState>,
    kinds: Vec<ResourceRef>,
) -> Result<Vec<AccessResult>, String> {
    let client = client(&state).await?;
    Ok(resources::check_access(&client, &kinds).await)
}

/// The discovered kind for `resource`, discovering first if needed.
async fn kind_of(
    state: &State<'_, AppState>,
    client: &Client,
    resource: &ResourceRef,
) -> Result<ResourceKind, String> {
    let find = |kinds: &[ResourceKind]| kinds.iter().find(|k| &k.resource == resource).cloned();
    if let Some(k) = find(&state.kinds.lock().unwrap()) {
        return Ok(k);
    }
    let kinds = resources::discover(client)
        .await
        .map_err(|e| e.to_string())?;
    let found = find(&kinds);
    *state.kinds.lock().unwrap() = kinds;
    found.ok_or_else(|| format!("{} is not served by this cluster", resource.kind))
}

/// Start watching a kind (in `namespace`, if it is namespaced). Returns the
/// watch id carried by the `watch-batch` / `watch-state` events.
#[tauri::command]
#[specta::specta]
async fn cmd_watch_start(
    app: AppHandle,
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: Option<String>,
) -> Result<String, String> {
    let client = client(&state).await?;
    let kind = kind_of(&state, &client, &resource).await?;
    let id = next_session("watch");
    let (_store, updates) = watch::watch_resource(&client, &kind, namespace.as_deref());
    let task = tauri::async_runtime::spawn(pump_watch(app, id.clone(), updates));
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
    state.kinds.lock().unwrap().clear();
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
async fn cmd_get_resource_detail(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
) -> Result<ResourceDetail, String> {
    let client = client(&state).await?;
    resources::get_resource_detail(&client, &resource, &namespace, &name)
        .await
        .map_err(|e| e.to_string())
}

/// An object as YAML. Secret values stay hidden unless `reveal` is set.
#[tauri::command]
#[specta::specta]
async fn cmd_get_resource_yaml(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
    reveal: bool,
) -> Result<String, String> {
    let client = client(&state).await?;
    resources::get_resource_yaml(&client, &resource, &namespace, &name, reveal)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn cmd_apply_resource_yaml(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
    yaml: String,
) -> Result<(), String> {
    let client = client(&state).await?;
    resources::apply_resource_yaml(&client, &resource, &namespace, &name, &yaml)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn cmd_delete_resource(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
) -> Result<(), String> {
    let client = client(&state).await?;
    resources::delete_resource(&client, &resource, &namespace, &name)
        .await
        .map_err(|e| actions::describe_error(&e))
}

/// Run one action (everything except drain) on one object. Returns a short
/// summary. The change reaches the table through the live watch. Errors are
/// plain-words sentences ready to show inline.
#[tauri::command]
#[specta::specta]
async fn cmd_run_action(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
    action: ActionId,
    replicas: Option<i32>,
) -> Result<String, String> {
    let client = client(&state).await?;
    actions::run_action(&client, &resource, &namespace, &name, action, replicas)
        .await
        .map_err(|e| actions::describe_error(&e))
}

/// One object of a bulk delete.
#[derive(Debug, Clone, Deserialize, Type)]
struct ObjectId {
    /// Empty for cluster-scoped kinds.
    namespace: String,
    name: String,
}

/// Delete many objects of one kind. Every object is attempted; the result
/// has one entry per object with its error, if any.
#[tauri::command]
#[specta::specta]
async fn cmd_bulk_delete(
    state: State<'_, AppState>,
    resource: ResourceRef,
    items: Vec<ObjectId>,
) -> Result<Vec<BulkItemResult>, String> {
    let client = client(&state).await?;
    let items: Vec<(String, String)> = items.into_iter().map(|i| (i.namespace, i.name)).collect();
    Ok(actions::delete_many(&client, &resource, &items).await)
}

/// Progress of a node drain started by `cmd_drain_node`.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct DrainProgress {
    drain_id: String,
    event: DrainEvent,
}

/// A node drain ended. `error` is empty when it ran to the end (see
/// `summary`) and says why not otherwise (for instance 403 on cordon).
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
struct DrainFinished {
    drain_id: String,
    summary: DrainSummary,
    error: String,
}

/// Cordon a node and evict its pods in the background. Progress arrives as
/// `DrainProgress` events and the end as `DrainFinished`; returns the drain id.
#[tauri::command]
#[specta::specta]
async fn cmd_drain_node(
    app: AppHandle,
    state: State<'_, AppState>,
    node: String,
    timeout_secs: u32,
) -> Result<String, String> {
    let client = client(&state).await?;
    let drain_id = next_session("drain");
    let id = drain_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        let progress_id = id.clone();
        let result = actions::drain_node(
            &client,
            &node,
            &actions::DrainOptions::new(Duration::from_secs(timeout_secs.into())),
            move |event| {
                let _ = DrainProgress {
                    drain_id: progress_id.clone(),
                    event,
                }
                .emit(&emitter);
            },
        )
        .await;
        let (summary, error) = match result {
            Ok(s) => (s, String::new()),
            Err(e) => (DrainSummary::default(), actions::describe_error(&e)),
        };
        app.state::<AppState>().drains.lock().unwrap().remove(&id);
        let _ = DrainFinished {
            drain_id: id,
            summary,
            error,
        }
        .emit(&app);
    });
    state
        .drains
        .lock()
        .unwrap()
        .insert(drain_id.clone(), handle);
    Ok(drain_id)
}

/// Stop following a drain. Pods already evicted stay evicted and the node
/// stays cordoned.
#[tauri::command]
#[specta::specta]
async fn cmd_cancel_drain(state: State<'_, AppState>, drain_id: String) -> Result<(), String> {
    if let Some(h) = state.drains.lock().unwrap().remove(&drain_id) {
        h.abort();
    }
    Ok(())
}

/// The typed command and event registry. Also used to generate
/// `ui-src/src/lib/bindings.ts`.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            cmd_list_namespaces,
            cmd_discover,
            cmd_check_access,
            cmd_get_pod_details,
            cmd_stream_pod_logs,
            cmd_stop_pod_logs,
            cmd_stream_workload_logs,
            cmd_stop_workload_logs,
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
            cmd_get_resource_detail,
            cmd_get_resource_yaml,
            cmd_apply_resource_yaml,
            cmd_delete_resource,
            cmd_run_action,
            cmd_bulk_delete,
            cmd_drain_node,
            cmd_cancel_drain
        ])
        .events(collect_events![
            WatchBatch,
            WatchStateChanged,
            PodLog,
            PodLogError,
            PodLogDone,
            WorkloadLog,
            WorkloadNotice,
            ExecOutput,
            ExecClosed,
            DrainProgress,
            DrainFinished
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
            kinds: Mutex::new(Vec::new()),
            drains: Mutex::new(HashMap::new()),
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
