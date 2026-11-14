//! The live overview, pushed to the webview as events while it is open.

use crate::state::{client, AppState};
use futures::StreamExt;
use kubeloupe::watch::WatchState;
use kubeloupe::{watch_overview, Overview, OverviewUpdate};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event;

/// A new overview for the overview feed `id` (sent when it changed, at
/// most about once a second).
#[derive(Debug, Clone, Serialize, Type, Event)]
#[tauri_specta(event_name = "overview-update")]
pub(crate) struct OverviewChanged {
    id: u32,
    overview: Overview,
}

/// Connection state of the overview feed `id`.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[tauri_specta(event_name = "overview-state")]
pub(crate) struct OverviewStateChanged {
    id: u32,
    #[serde(flatten)]
    state: WatchState,
}

/// The feed's key in `AppState::watches`, so a context switch stops it
/// with the other watches.
fn key(id: u32) -> String {
    format!("overview-{id}")
}

/// Start the overview feed `id` (chosen by the UI, so no event can arrive
/// before it knows the id). Parts the user may not see cluster-wide fall
/// back to `namespace` (`null` = the context's namespace). Replaces a feed
/// with the same id.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_overview_start(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
    namespace: Option<String>,
) -> Result<(), String> {
    let client = client(&state).await?;
    let mut updates = watch_overview(&client, namespace.as_deref());
    let task = tauri::async_runtime::spawn(async move {
        while let Some(update) = updates.next().await {
            let _ = match update {
                OverviewUpdate::Summary(overview) => OverviewChanged {
                    id,
                    overview: *overview,
                }
                .emit(&app),
                OverviewUpdate::State(state) => OverviewStateChanged { id, state }.emit(&app),
            };
        }
    });
    if let Some(old) = state.watches.lock().unwrap().insert(key(id), task) {
        old.abort();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_overview_stop(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    if let Some(task) = state.watches.lock().unwrap().remove(&key(id)) {
        task.abort();
    }
    Ok(())
}
