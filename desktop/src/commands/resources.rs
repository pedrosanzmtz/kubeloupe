//! Pod and resource details, YAML get / apply, delete.

use crate::state::{client, AppState};
use kubeloupe::actions::{self};
use kubeloupe::resources::{self, ResourceDetail, ResourceRef};
use kubeloupe::{get_pod_details, PodDetail};
use tauri::State;

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_get_pod_details(
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

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_apply_resource_yaml(
    state: State<'_, AppState>,
    resource: ResourceRef,
    namespace: String,
    name: String,
    yaml: String,
) -> Result<Vec<resources::FieldTakeover>, String> {
    let client = client(&state).await?;
    resources::apply_resource_yaml(&client, &resource, &namespace, &name, &yaml)
        .await
        .map_err(|e| e.to_string())
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
