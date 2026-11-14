//! Pod and workload log streams, and saving logs.

use crate::state::{client, AppState};
use kubeloupe::{
    batch_lines, stream_pod_logs, stream_workload_logs, LogOptions, StopReason, TaggedLine,
    WorkloadEvent, WorkloadKind,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::time::Duration;
use tauri::{AppHandle, State};
use tauri_specta::Event;

/// A batch of log lines. Log events carry the id of the stream that produced
/// them: several streams run at once (one per dock tab), and a tab drops
/// stragglers from a stream it has already replaced.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PodLog {
    stream_id: u64,
    data: Vec<String>,
}

/// The log stream failed.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PodLogError {
    stream_id: u64,
    data: String,
}

/// The log stream ended.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PodLogDone {
    stream_id: u64,
}

/// Flush a log batch after this many lines or this long, whichever is first.
const LOG_BATCH_LINES: usize = 500;
const LOG_BATCH_INTERVAL: Duration = Duration::from_millis(50);

#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_stream_pod_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    namespace: String,
    name: String,
    container: Option<String>,
    previous: Option<bool>,
    tail_lines: Option<i64>,
    since_seconds: Option<i64>,
    timestamps: Option<bool>,
    stream_id: u64,
) -> Result<(), String> {
    let client = client(&state).await?;
    let opts = LogOptions {
        container,
        previous: previous.unwrap_or(false),
        tail_lines,
        since_seconds,
        timestamps: timestamps.unwrap_or(false),
    };
    state.log_streams.start(stream_id, move || {
        tauri::async_runtime::spawn(async move {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let emitter = app.clone();
            let batcher = tauri::async_runtime::spawn(batch_lines(
                rx,
                LOG_BATCH_LINES,
                LOG_BATCH_INTERVAL,
                move |batch| {
                    let _ = PodLog {
                        stream_id,
                        data: batch,
                    }
                    .emit(&emitter);
                },
            ));
            let result = stream_pod_logs(&client, &namespace, &name, &opts, move |line| {
                let _ = tx.send(line);
            })
            .await;
            // The sender was dropped with the closure, so the batcher flushes and exits.
            let _ = batcher.await;
            if let Err(e) = result {
                let msg = format!("Log stream ended with an error: {e}");
                let _ = PodLogError {
                    stream_id,
                    data: msg,
                }
                .emit(&app);
            }
            let _ = PodLogDone { stream_id }.emit(&app);
        })
    })
}

/// A batch of merged log lines from the pods of a workload. Same `streamId`
/// scheme as [`PodLog`]; every line says which pod and container it is from.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkloadLog {
    stream_id: u64,
    data: Vec<TaggedLine>,
}

#[derive(Debug, Clone, Copy, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkloadNoticeKind {
    /// A per-container stream started (a pod appeared).
    Started,
    /// The container exited and its log ended.
    Ended,
    /// The pod was deleted.
    Deleted,
    /// The stream failed; see `message`.
    Failed,
    /// More containers match than the stream limit: `skipped` are waiting.
    Truncated,
}

/// A change in which pods a workload stream is following.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkloadNotice {
    stream_id: u64,
    kind: WorkloadNoticeKind,
    /// Empty for `truncated`.
    pod: String,
    container: String,
    message: String,
    /// For `truncated`: the stream limit and how many containers are waiting.
    limit: u32,
    skipped: u32,
}

/// Options of a workload log stream.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkloadLogOptions {
    /// One container name for every pod; `None` follows all containers.
    container: Option<String>,
    /// Applies to the pods present when the stream starts; pods that appear
    /// later are read from their beginning.
    tail_lines: Option<i64>,
    since_seconds: Option<i64>,
    timestamps: Option<bool>,
}

fn workload_notice(stream_id: u64, ev: WorkloadEvent) -> Option<WorkloadNotice> {
    let (kind, pod, container, message, limit, skipped) = match ev {
        WorkloadEvent::Lines(_) => return None,
        WorkloadEvent::Started { pod, container } => (
            WorkloadNoticeKind::Started,
            pod,
            container,
            String::new(),
            0,
            0,
        ),
        WorkloadEvent::Stopped {
            pod,
            container,
            reason,
        } => {
            let (kind, message) = match reason {
                StopReason::Deleted => (WorkloadNoticeKind::Deleted, "pod deleted".to_string()),
                StopReason::Ended => (WorkloadNoticeKind::Ended, "log ended".to_string()),
                StopReason::Failed(e) => (WorkloadNoticeKind::Failed, e),
            };
            (kind, pod, container, message, 0, 0)
        }
        WorkloadEvent::Truncated { limit, skipped } => (
            WorkloadNoticeKind::Truncated,
            String::new(),
            String::new(),
            format!("following {limit} containers, {skipped} more waiting"),
            limit as u32,
            skipped as u32,
        ),
    };
    Some(WorkloadNotice {
        stream_id,
        kind,
        pod,
        container,
        message,
        limit,
        skipped,
    })
}

/// Follow the logs of all pods of a Deployment, StatefulSet, DaemonSet, Job
/// or ReplicaSet (`kind` is case-insensitive). Runs next to the other log
/// streams (up to `MAX_LOG_STREAMS`) until `cmd_stop_logs` or a context switch.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_stream_workload_logs(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    options: WorkloadLogOptions,
    stream_id: u64,
) -> Result<(), String> {
    let kind = kind.parse::<WorkloadKind>().map_err(|e| e.to_string())?;
    let client = client(&state).await?;
    let opts = LogOptions {
        container: options.container,
        previous: false,
        tail_lines: options.tail_lines,
        since_seconds: options.since_seconds,
        timestamps: options.timestamps.unwrap_or(false),
    };
    state.log_streams.start(stream_id, move || {
        tauri::async_runtime::spawn(async move {
            let emitter = app.clone();
            let result =
                stream_workload_logs(
                    &client,
                    kind,
                    &namespace,
                    &name,
                    &opts,
                    move |ev| match ev {
                        WorkloadEvent::Lines(data) => {
                            let _ = WorkloadLog { stream_id, data }.emit(&emitter);
                        }
                        ev => {
                            if let Some(n) = workload_notice(stream_id, ev) {
                                let _ = n.emit(&emitter);
                            }
                        }
                    },
                )
                .await;
            if let Err(e) = result {
                let _ = PodLogError {
                    stream_id,
                    data: format!("Log stream ended with an error: {e}"),
                }
                .emit(&app);
            }
            let _ = PodLogDone { stream_id }.emit(&app);
        })
    })
}

/// Save log text to the user's Downloads folder and return the path. A
/// webview Blob download is not reliable across the platforms Tauri targets
/// and there is no file dialog without an extra plugin, so we write directly.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_save_logs(filename: String, content: String) -> Result<String, String> {
    let safe: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() || safe.starts_with('.') {
        format!("log{safe}")
    } else {
        safe
    };
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("cannot locate home directory")?;
    let dir = std::path::PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe);
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Stop one log stream (pod or workload, with every per-pod task under it).
/// Other streams keep running.
#[tauri::command]
#[specta::specta]
pub(crate) async fn cmd_stop_logs(
    state: State<'_, AppState>,
    stream_id: u64,
) -> Result<(), String> {
    state.log_streams.stop(stream_id);
    Ok(())
}
