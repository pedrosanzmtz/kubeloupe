//! Interactive terminal sessions in pods.

use crate::state::{client, next_session, AppState, ExecSession, SessionCleanup};
use futures::SinkExt;
use kube::api::{AttachedProcess, TerminalSize};
use kubeloupe::exec_pod;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The shell of a terminal session exited (payload: the session id).
#[derive(Debug, Clone, Serialize, Type, Event)]
pub(crate) struct ExecClosed(pub(crate) String);

/// Output of a terminal session.
#[derive(Debug, Clone, Serialize, Type, Event)]
pub(crate) struct ExecOutput {
    session: String,
    stream: String,
    data: String,
}

impl ExecOutput {
    /// Output of a local shell, which has a single stream.
    pub(crate) fn stdout(session: String, data: String) -> Self {
        ExecOutput {
            session,
            stream: "stdout".to_string(),
            data,
        }
    }
}

/// Open an interactive shell in a pod. Returns the session id; output
/// arrives as `exec-output` events, `exec-closed` when the shell exits.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_exec_start(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let client = client(&state).await?;
    let attached = exec_pod(&client, &namespace, &name, container.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    let session = next_session("exec");
    start_session(&app, &state, session.clone(), attached, cols, rows, None).await;
    Ok(session)
}

/// Write a line of text into a terminal session (progress, notes).
pub(crate) fn emit_note(app: &AppHandle, session: &str, text: &str) {
    let _ = ExecOutput {
        session: session.to_string(),
        stream: "stderr".to_string(),
        data: format!("\x1b[90m{text}\x1b[0m\r\n"),
    }
    .emit(app);
}

/// Pump an attached or exec'd process to `exec-output` events and register
/// it as `session`, so `cmd_exec_input`, `cmd_exec_resize` and
/// `cmd_exec_close` drive it. `cleanup` runs when the session ends or is
/// closed.
pub(crate) async fn start_session(
    app: &AppHandle,
    state: &State<'_, AppState>,
    session: String,
    mut attached: AttachedProcess,
    cols: u16,
    rows: u16,
    cleanup: Option<SessionCleanup>,
) {
    let app = app.clone();
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
    let task_cleanup = cleanup.clone();
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
        if let Some(c) = task_cleanup {
            c.run().await;
        }
        let _ = ExecClosed(session_id).emit(&app);
    });
    state.exec_sessions.lock().unwrap().insert(
        session.clone(),
        ExecSession {
            stdin_tx,
            size_tx,
            task,
            cleanup,
        },
    );
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_exec_input(
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
pub(crate) async fn cmd_exec_resize(
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
pub(crate) async fn cmd_exec_close(
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    let entry = state.exec_sessions.lock().unwrap().remove(&session);
    if let Some(entry) = entry {
        entry.task.abort();
        if let Some(c) = entry.cleanup {
            tauri::async_runtime::spawn(c.run());
        }
    }
    Ok(())
}
