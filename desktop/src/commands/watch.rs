//! Live watches of a kind, forwarded to the webview as batched events.

use crate::state::{client, next_session, AppState};
use kube::Client;
use kubeloupe::resources::{self, ResourceKind, ResourceRef, ResourceRow};
use kubeloupe::watch;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

/// One coalesced set of table rows for the watch `id`: the first batch is a
/// full snapshot, later ones are incremental.
#[derive(Debug, Clone, Serialize, Type, Event)]
pub(crate) struct WatchBatch {
    id: String,
    snapshot: bool,
    applied: Vec<ResourceRow>,
    /// Keys of removed rows.
    deleted: Vec<String>,
}

/// Payload of the `watch-state` event.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[tauri_specta(event_name = "watch-state")]
pub(crate) struct WatchStateChanged {
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
pub(crate) async fn cmd_watch_start(
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
pub(crate) async fn cmd_watch_stop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if let Some(task) = state.watches.lock().unwrap().remove(&id) {
        task.abort();
    }
    Ok(())
}
