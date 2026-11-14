//! Reading and changing the remembered settings (`settings.rs`).

use crate::settings::{Settings, SettingsPatch, SettingsStore};
use std::sync::Arc;
use tauri::State;

/// The settings, as loaded on start and changed since.
#[tauri::command]
#[specta::specta]
pub(crate) fn cmd_get_settings(store: State<'_, Arc<SettingsStore>>) -> Result<Settings, String> {
    Ok(store.get())
}

/// Apply `patch` (fields left out are kept) and return the new settings.
/// The file is written shortly after, once changes stop.
#[tauri::command]
#[specta::specta]
pub(crate) fn cmd_update_settings(
    store: State<'_, Arc<SettingsStore>>,
    patch: SettingsPatch,
) -> Result<Settings, String> {
    Ok(store.update(patch))
}
