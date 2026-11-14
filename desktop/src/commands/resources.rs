//! Resource details, YAML get / apply, delete.

use crate::state::{client, AppState};
use kubeloupe::actions::{self};
use kubeloupe::resources::{self, ResourceDetail, ResourceRef};
use tauri::State;

/// Detail of any object; pods include their `pod` section.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_get_resource_detail(
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
pub(crate) async fn cmd_get_resource_yaml(
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

/// Server-side apply of edited YAML. `resource_version` is the version the
/// YAML was loaded at (empty: don't check); if the object changed since,
/// nothing is applied and the outcome says so. `dry_run` validates only.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_apply_resource_yaml(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
    yaml: String,
    resource_version: String,
    dry_run: bool,
) -> Result<resources::ApplyOutcome, String> {
    let client = client(&state).await?;
    let opts = resources::ApplyOptions {
        dry_run,
        resource_version: Some(resource_version).filter(|v| !v.is_empty()),
    };
    resources::apply_resource_yaml_with(&client, &resource, &namespace, &name, &yaml, &opts)
        .await
        // `:#` keeps the cause, e.g. the YAML parser's line and column.
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_delete_resource(
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
