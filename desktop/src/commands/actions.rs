//! Resource actions: run an action, bulk delete, drain a node.

use crate::state::{client, next_session, AppState};
use kubeloupe::actions::{self, ActionId, BulkItemResult, DrainEvent, DrainSummary};
use kubeloupe::resources::ResourceRef;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

/// Run one action (everything except drain) on one object. Returns a short
/// summary. The change reaches the table through the live watch. Errors are
/// plain-words sentences ready to show inline.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_run_action(
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
pub(crate) struct ObjectId {
    /// Empty for cluster-scoped kinds.
    namespace: String,
    name: String,
}

/// Delete many objects of one kind. Every object is attempted; the result
/// has one entry per object with its error, if any.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_bulk_delete(
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
pub(crate) struct DrainProgress {
    drain_id: String,
    event: DrainEvent,
}

/// A node drain ended. `error` is empty when it ran to the end (see
/// `summary`) and says why not otherwise (for instance 403 on cordon).
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DrainFinished {
    drain_id: String,
    summary: DrainSummary,
    error: String,
}

/// Cordon a node and evict its pods in the background. Progress arrives as
/// `DrainProgress` events and the end as `DrainFinished`; returns the drain id.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_drain_node(
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
pub(crate) async fn cmd_cancel_drain(
    state: State<'_, AppState>,
    drain_id: String,
) -> Result<(), String> {
    if let Some(h) = state.drains.lock().unwrap().remove(&drain_id) {
        h.abort();
    }
    Ok(())
}
