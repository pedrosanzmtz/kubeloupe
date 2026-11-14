#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crate::settings::SettingsStore;
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tauri_specta::{collect_commands, collect_events, Builder};

mod commands;
mod local_shell;
mod log_streams;
mod settings;
#[cfg(unix)]
mod shell_env;
mod state;

/// The typed command and event registry. Also used to generate
/// `ui-src/src/lib/bindings.ts`.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::cluster::cmd_list_namespaces,
            commands::cluster::cmd_discover,
            commands::cluster::cmd_check_access,
            commands::logs::cmd_stream_pod_logs,
            commands::logs::cmd_stop_logs,
            commands::logs::cmd_stream_workload_logs,
            commands::logs::cmd_save_logs,
            commands::overview::cmd_overview_start,
            commands::overview::cmd_overview_stop,
            commands::cluster::cmd_default_namespace,
            commands::cluster::cmd_get_metrics,
            commands::usage::cmd_usage_history,
            commands::watch::cmd_watch_start,
            commands::watch::cmd_watch_stop,
            commands::watch::cmd_detail_watch,
            commands::cluster::cmd_list_contexts,
            commands::cluster::cmd_set_context,
            commands::exec::cmd_exec_start,
            commands::exec::cmd_exec_input,
            commands::exec::cmd_exec_resize,
            commands::exec::cmd_exec_close,
            commands::local_shell::cmd_local_shell_start,
            commands::local_shell::cmd_local_shell_input,
            commands::local_shell::cmd_local_shell_resize,
            commands::local_shell::cmd_local_shell_close,
            commands::portforward::cmd_portforward_start,
            commands::portforward::cmd_portforward_stop,
            commands::portforward::cmd_portforward_list,
            commands::portforward::cmd_service_ports,
            commands::resources::cmd_get_resource_detail,
            commands::resources::cmd_get_related,
            commands::resources::cmd_get_resource_yaml,
            commands::resources::cmd_apply_resource_yaml,
            commands::resources::cmd_delete_resource,
            commands::resources::cmd_apply_manifests,
            commands::actions::cmd_run_action,
            commands::actions::cmd_bulk_delete,
            commands::actions::cmd_drain_node,
            commands::actions::cmd_cancel_drain,
            commands::helm::cmd_helm_list,
            commands::helm::cmd_helm_charts,
            commands::helm::cmd_helm_get,
            commands::helm::cmd_helm_version,
            commands::helm::cmd_helm_rollback,
            commands::helm::cmd_helm_uninstall,
            commands::settings::cmd_get_settings,
            commands::settings::cmd_update_settings
        ])
        .events(collect_events![
            commands::watch::WatchBatch,
            commands::watch::WatchStateChanged,
            commands::watch::DetailWatchEvent,
            commands::overview::OverviewChanged,
            commands::overview::OverviewStateChanged,
            commands::logs::PodLog,
            commands::logs::PodLogError,
            commands::logs::PodLogDone,
            commands::logs::WorkloadLog,
            commands::logs::WorkloadNotice,
            commands::exec::ExecOutput,
            commands::exec::ExecClosed,
            commands::actions::DrainProgress,
            commands::actions::DrainFinished
        ])
        // Counters and epoch seconds stay far below 2^53.
        .dangerously_cast_bigints_to_number()
}

/// Where the generated bindings live.
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui-src/src/lib/bindings.ts");

fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)
        .expect("failed to export TypeScript bindings");
}

fn main() {
    // First, before any thread starts: it changes the process environment.
    #[cfg(unix)]
    shell_env::import_login_shell_env();

    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .manage(AppState {
            context: Mutex::new(None),
            client: tokio::sync::Mutex::new(None),
            log_streams: Default::default(),
            exec_sessions: Mutex::new(HashMap::new()),
            local_shells: Mutex::new(HashMap::new()),
            forwards: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
            kinds: Mutex::new(Vec::new()),
            drains: Mutex::new(HashMap::new()),
            usage: Default::default(),
        })
        // Opens forwarded URLs in the browser; the capability limits it to
        // http://127.0.0.1 and http://localhost.
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let path = app
                .path()
                .app_config_dir()
                .ok()
                .map(|d| d.join("settings.json"));
            app.manage(SettingsStore::open(path));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building kubeloupe")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Write settings changed in the last moments before quitting.
                if let Some(store) = app.try_state::<Arc<SettingsStore>>() {
                    store.flush();
                }
                // Kill the local shells and delete their temp kubeconfigs.
                state::stop_all_local_shells(&app.state::<AppState>());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerates `bindings.ts`; CI runs this and fails if the committed
    /// file changes.
    #[test]
    fn export_typescript_bindings() {
        export_bindings(&specta_builder());
    }
}
