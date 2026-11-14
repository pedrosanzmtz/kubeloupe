//! Per-window state shared by the commands: the selected context, its
//! client, and the running streams, sessions and watches.

use kube::api::TerminalSize;
use kube::Client;
use kubeloupe::client_for_context_with;
use kubeloupe::resources::ResourceKind;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::State;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_session(prefix: &str) -> String {
    format!(
        "{}-{}",
        prefix,
        NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) struct ExecSession {
    pub(crate) stdin_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub(crate) size_tx: Option<futures::channel::mpsc::Sender<TerminalSize>>,
    pub(crate) task: tauri::async_runtime::JoinHandle<()>,
}

pub(crate) struct ForwardSession {
    #[allow(dead_code)]
    pub(crate) local_port: u16,
    #[allow(dead_code)]
    pub(crate) pod_port: u16,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

pub(crate) struct AppState {
    /// Selected context (`None` = the kubeconfig's current-context).
    pub(crate) context: Mutex<Option<String>>,
    /// Client for the selected context, built lazily and dropped when the
    /// context changes. `Client` is cheap to clone.
    pub(crate) client: tokio::sync::Mutex<Option<Client>>,
    /// The running log stream, if any.
    /// The running log stream (pod or workload), with its stream id.
    pub(crate) log_task: Mutex<Option<(u64, tauri::async_runtime::JoinHandle<()>)>>,
    /// Open terminal sessions, by session id.
    pub(crate) exec_sessions: Mutex<HashMap<String, ExecSession>>,
    /// Active port-forwards, by session id.
    pub(crate) forwards: Mutex<HashMap<String, ForwardSession>>,
    /// Running resource watchers, by watch id.
    pub(crate) watches: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    /// What the last discovery found, so watches can look up their columns.
    pub(crate) kinds: Mutex<Vec<ResourceKind>>,
    /// Running node drains, by drain id.
    pub(crate) drains: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

pub(crate) fn selected_context(state: &State<'_, AppState>) -> Option<String> {
    state.context.lock().unwrap().clone()
}

/// The cached client for the selected context, building it on first use.
pub(crate) async fn client(state: &State<'_, AppState>) -> Result<Client, String> {
    let mut slot = state.client.lock().await;
    if let Some(c) = slot.as_ref() {
        return Ok(c.clone());
    }
    let ctx = selected_context(state);
    // A GUI can't answer an exec plugin's prompts.
    let c = client_for_context_with(ctx.as_deref(), false)
        .await
        .map_err(|e| e.to_string())?;
    *slot = Some(c.clone());
    Ok(c)
}

pub(crate) fn stop_log_stream(state: &State<'_, AppState>) {
    if let Some((_, handle)) = state.log_task.lock().unwrap().take() {
        handle.abort();
    }
}

/// Stop the log stream only if it is still `stream_id` (another viewer may
/// have replaced it since).
pub(crate) fn stop_log_stream_if(state: &State<'_, AppState>, stream_id: u64) {
    let mut slot = state.log_task.lock().unwrap();
    if matches!(slot.as_ref(), Some((id, _)) if *id == stream_id) {
        if let Some((_, handle)) = slot.take() {
            handle.abort();
        }
    }
}

pub(crate) fn stop_all_exec(state: &State<'_, AppState>) {
    let mut sessions = state.exec_sessions.lock().unwrap();
    for (_, session) in sessions.drain() {
        session.task.abort();
    }
}

pub(crate) fn stop_all_watches(state: &State<'_, AppState>) {
    let mut watches = state.watches.lock().unwrap();
    for (_, task) in watches.drain() {
        task.abort();
    }
}

pub(crate) fn stop_all_forwards(state: &State<'_, AppState>) {
    let mut forwards = state.forwards.lock().unwrap();
    for (_, forward) in forwards.drain() {
        forward.task.abort();
    }
}
