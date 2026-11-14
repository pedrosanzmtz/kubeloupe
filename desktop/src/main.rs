#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crate::state::AppState;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri_specta::{collect_commands, collect_events, Builder};

mod commands;
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
            commands::logs::cmd_stop_pod_logs,
            commands::logs::cmd_stream_workload_logs,
            commands::logs::cmd_stop_workload_logs,
            commands::logs::cmd_save_logs,
            commands::cluster::cmd_get_overview,
            commands::cluster::cmd_default_namespace,
            commands::cluster::cmd_get_metrics,
            commands::watch::cmd_watch_start,
            commands::watch::cmd_watch_stop,
            commands::watch::cmd_detail_watch,
            commands::cluster::cmd_list_contexts,
            commands::cluster::cmd_set_context,
            commands::exec::cmd_exec_start,
            commands::exec::cmd_exec_input,
            commands::exec::cmd_exec_resize,
            commands::exec::cmd_exec_close,
            commands::portforward::cmd_portforward_start,
            commands::portforward::cmd_portforward_stop,
            commands::resources::cmd_get_resource_detail,
            commands::resources::cmd_get_resource_yaml,
            commands::resources::cmd_apply_resource_yaml,
            commands::resources::cmd_delete_resource,
            commands::resources::cmd_apply_manifests,
            commands::actions::cmd_run_action,
            commands::actions::cmd_bulk_delete,
            commands::actions::cmd_drain_node,
            commands::actions::cmd_cancel_drain
        ])
        .events(collect_events![
            commands::watch::WatchBatch,
            commands::watch::WatchStateChanged,
            commands::watch::DetailWatchEvent,
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
            log_task: Mutex::new(None),
            exec_sessions: Mutex::new(HashMap::new()),
            forwards: Mutex::new(HashMap::new()),
            watches: Mutex::new(HashMap::new()),
            kinds: Mutex::new(Vec::new()),
            drains: Mutex::new(HashMap::new()),
        })
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running kubeloupe");
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
