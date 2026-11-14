//! Local shells in the dock, pointed at the app's context and namespace.
//! Output arrives as `exec-output` events and `exec-closed` when the shell
//! exits, like pod terminals.

use crate::commands::exec::{ExecClosed, ExecOutput};
use crate::local_shell::{LocalShell, ShellSpec};
use crate::state::{next_session, selected_context, AppState};
use kubeloupe::{shell_kubeconfig, shell_kubeconfig_env_here, TempKubeconfig};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

/// A started local shell: its session id, and the context and namespace
/// kubectl uses in it.
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct LocalShellStarted {
    session: String,
    context: String,
    namespace: String,
}

/// Start the user's shell on this machine with `KUBECONFIG` pointing at a
/// temp kubeconfig that selects the app's context and `namespace` (`null`:
/// the context's own). The user's kubeconfig files are not changed.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_local_shell_start(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<LocalShellStarted, String> {
    let context = selected_context(&state);
    let kubeconfig =
        kube::config::Kubeconfig::read().map_err(|e| format!("cannot read the kubeconfig: {e}"))?;
    let generated = shell_kubeconfig(&kubeconfig, context.as_deref(), namespace.as_deref())
        .map_err(|e| e.to_string())?;
    let temp = TempKubeconfig::create(&generated.yaml).map_err(|e| format!("{e:#}"))?;
    let mut spec = ShellSpec::user_shell();
    spec.env
        .push(("KUBECONFIG".into(), shell_kubeconfig_env_here(temp.path())));

    let session = next_session("local");
    let (out_app, out_session) = (app.clone(), session.clone());
    let (exit_app, exit_session) = (app, session.clone());
    let shell = LocalShell::spawn(
        &spec,
        cols,
        rows,
        Some(temp),
        move |data| {
            let _ = ExecOutput::stdout(out_session.clone(), data).emit(&out_app);
        },
        move || {
            let _ = ExecClosed(exit_session).emit(&exit_app);
        },
    )?;
    state
        .local_shells
        .lock()
        .unwrap()
        .insert(session.clone(), shell);
    Ok(LocalShellStarted {
        session,
        context: generated.context,
        namespace: generated.namespace,
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_local_shell_input(
    state: State<'_, AppState>,
    session: String,
    data: String,
) -> Result<(), String> {
    let shells = state.local_shells.lock().unwrap();
    let shell = shells
        .get(&session)
        .ok_or_else(|| "local shell not found".to_string())?;
    shell.write(data.into_bytes())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_local_shell_resize(
    state: State<'_, AppState>,
    session: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let shells = state.local_shells.lock().unwrap();
    match shells.get(&session) {
        Some(shell) => shell.resize(cols, rows),
        None => Ok(()),
    }
}

/// Kill the shell and delete its temp kubeconfig.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_local_shell_close(
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    let shell = state.local_shells.lock().unwrap().remove(&session);
    drop(shell);
    Ok(())
}
