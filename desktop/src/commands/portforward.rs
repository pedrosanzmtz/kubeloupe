//! Forwarding a pod port to localhost.

use crate::state::{client, next_session, AppState, ForwardSession};
use kubeloupe::serve_port_forward;
use serde::Serialize;
use specta::Type;
use tauri::State;

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct PortForwardInfo {
    session: String,
    local_port: u16,
}

/// Forward a pod port to localhost. Returns the session id and the local
/// port the forward listens on.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_portforward_start(
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
pub(crate) async fn cmd_portforward_stop(
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    let mut forwards = state.forwards.lock().unwrap();
    if let Some(entry) = forwards.remove(&session) {
        entry.task.abort();
    }
    Ok(())
}
