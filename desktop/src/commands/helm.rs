//! Helm releases: native listing and decoding, rollback and uninstall
//! through the `helm` CLI.

use crate::state::{client, selected_context, AppState};
use kubeloupe::actions::describe_error;
use kubeloupe::helm::{
    self, HelmChartInfo, HelmRelease, HelmReleaseList, HelmRevisionKey, HelmTarget,
};
use tauri::State;

/// The releases in `namespaces` (empty = all), from labels only. Namespaces
/// whose Secrets may not be listed come back as `warnings`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_list(
    state: State<'_, AppState>,
    namespaces: Vec<String>,
) -> Result<HelmReleaseList, String> {
    let client = client(&state).await?;
    helm::list_helm_releases(&client, &namespaces)
        .await
        .map_err(|e| describe_error(&e))
}

/// The chart of each given revision (a few payloads downloaded at a time),
/// for the list's chart columns. Errors are per release.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_charts(
    state: State<'_, AppState>,
    keys: Vec<HelmRevisionKey>,
) -> Result<Vec<HelmChartInfo>, String> {
    let client = client(&state).await?;
    Ok(helm::release_charts(&client, &keys).await)
}

/// One revision decoded (`revision` 0 = the newest). Secret values in the
/// manifest are hidden unless `reveal`.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_get(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    revision: u32,
    reveal: bool,
) -> Result<HelmRelease, String> {
    let client = client(&state).await?;
    let revision = (revision > 0).then_some(revision);
    helm::get_helm_release(&client, &namespace, &name, revision, reveal)
        .await
        .map_err(|e| describe_error(&e))
}

/// The installed helm's version; an error says how to install it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_version() -> Result<String, String> {
    helm::helm_version().await.map_err(|e| format!("{e:#}"))
}

/// Helm talks to the app's context. Without a selection, name the
/// kubeconfig's current context explicitly, so helm cannot drift to another
/// one if the file changes meanwhile.
fn target(state: &State<'_, AppState>) -> HelmTarget {
    let context = selected_context(state).or_else(|| {
        kube::config::Kubeconfig::read()
            .ok()
            .and_then(|k| k.current_context)
    });
    HelmTarget {
        kubeconfig: None,
        context,
    }
}

/// `helm rollback <name> <revision>`; returns a short summary.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_rollback(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    revision: u32,
) -> Result<String, String> {
    helm::helm_rollback(&target(&state), &namespace, &name, revision)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// `helm uninstall <name>`; returns a short summary.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_helm_uninstall(
    state: State<'_, AppState>,
    namespace: String,
    name: String,
) -> Result<String, String> {
    helm::helm_uninstall(&target(&state), &namespace, &name)
        .await
        .map_err(|e| format!("{e:#}"))
}
