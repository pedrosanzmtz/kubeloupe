//! Reading and streaming pod logs, and batching log lines.

use anyhow::Result;
use futures::{AsyncBufReadExt, TryStreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, LogParams};
use kube::Client;

/// Options shared by the CLI and the desktop app for reading pod logs.
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// Container name; `None` lets the API server pick the default.
    pub container: Option<String>,
    /// Read the logs of the previous (crashed/restarted) container instance.
    pub previous: bool,
    /// Number of lines from the end; `None` = everything.
    pub tail_lines: Option<i64>,
    /// Only logs newer than this many seconds.
    pub since_seconds: Option<i64>,
    /// Prefix each line with its RFC3339 timestamp.
    pub timestamps: bool,
}

impl LogOptions {
    fn to_params(&self, follow: bool) -> LogParams {
        LogParams {
            follow,
            previous: self.previous,
            tail_lines: self.tail_lines,
            since_seconds: self.since_seconds,
            timestamps: self.timestamps,
            container: self.container.clone(),
            ..LogParams::default()
        }
    }
}

/// Fetch a pod's logs in one go (non-streaming, for the CLI).
pub async fn get_pod_logs(
    client: &Client,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
) -> Result<String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    Ok(pods.logs(name, &opts.to_params(false)).await?)
}

/// Stream a pod's logs, calling `on_line` for each line. Runs until the
/// stream ends (or the caller drops the future). With `previous` set the
/// container is no longer running, so the log is read once (no follow) and
/// the stream simply ends.
pub async fn stream_pod_logs<F>(
    client: &Client,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
    on_line: F,
) -> Result<()>
where
    F: FnMut(String) + Send,
{
    stream_pod_logs_follow(client, namespace, name, opts, !opts.previous, on_line).await
}

/// [`stream_pod_logs`] with an explicit `follow`.
pub(crate) async fn stream_pod_logs_follow<F>(
    client: &Client,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
    follow: bool,
    mut on_line: F,
) -> Result<()>
where
    F: FnMut(String) + Send,
{
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = opts.to_params(follow);
    let mut lines = pods.log_stream(name, &params).await?.lines();
    while let Some(line) = lines.try_next().await? {
        on_line(line);
    }
    Ok(())
}

/// Collect lines from `rx` into batches and hand each to `emit`. A batch is
/// flushed when it reaches `max_lines` or when `interval` has passed since its
/// first line, so a noisy pod costs one event per batch instead of per line.
/// Returns (after flushing the remainder) once all senders are dropped.
pub async fn batch_lines<T, F>(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<T>,
    max_lines: usize,
    interval: std::time::Duration,
    mut emit: F,
) where
    F: FnMut(Vec<T>),
{
    let mut batch: Vec<T> = Vec::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let sleep = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            line = rx.recv() => match line {
                Some(line) => {
                    if batch.is_empty() {
                        deadline = Some(tokio::time::Instant::now() + interval);
                    }
                    batch.push(line);
                    if batch.len() >= max_lines {
                        deadline = None;
                        emit(std::mem::take(&mut batch));
                    }
                }
                None => break,
            },
            _ = sleep => {
                deadline = None;
                if !batch.is_empty() {
                    emit(std::mem::take(&mut batch));
                }
            }
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn batch_lines_flushes_on_size_and_on_close() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for i in 0..5 {
            tx.send(i.to_string()).unwrap();
        }
        drop(tx);
        let mut out: Vec<Vec<String>> = Vec::new();
        batch_lines(rx, 2, std::time::Duration::from_millis(50), |b| out.push(b)).await;
        let sizes: Vec<usize> = out.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
        assert_eq!(out[2], vec!["4".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn batch_lines_flushes_on_interval() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(batch_lines(
            rx,
            500,
            std::time::Duration::from_millis(50),
            move |b| {
                let _ = out_tx.send(b);
            },
        ));
        tx.send("a".into()).unwrap();
        tx.send("b".into()).unwrap();
        // Sender still open: only the timer can flush.
        let first = out_rx.recv().await.unwrap();
        assert_eq!(first, vec!["a".to_string(), "b".to_string()]);
        tx.send("c".into()).unwrap();
        drop(tx);
        task.await.unwrap();
        assert_eq!(out_rx.recv().await.unwrap(), vec!["c".to_string()]);
    }

    #[test]
    fn log_options_map_to_params() {
        let opts = LogOptions {
            previous: true,
            container: Some("app".into()),
            since_seconds: Some(300),
            ..LogOptions::default()
        };
        let p = opts.to_params(!opts.previous);
        assert!(p.previous && !p.follow);
        assert_eq!(p.container.as_deref(), Some("app"));
        assert_eq!(p.since_seconds, Some(300));
    }
}
