//! Running commands and interactive shells in pods.

use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, AttachedProcess};
use kube::Client;
use tokio::io::AsyncReadExt;

// ---------------------------------------------------------------------------
// Pod exec.
// ---------------------------------------------------------------------------

/// Open an interactive TTY shell (`/bin/sh`) in a pod. The caller drives
/// stdin/stdout/stderr and the terminal size on the returned process.
pub async fn exec_pod(
    client: &Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
) -> Result<AttachedProcess> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let mut params = AttachParams::interactive_tty();
    params.container = container.map(str::to_string);
    Ok(pods.exec(name, ["/bin/sh"], &params).await?)
}

/// Result of a non-interactive command run in a pod.
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run a command in a pod (non-TTY) and collect its output.
pub async fn exec_command(
    client: &Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    command: &[String],
) -> Result<ExecResult> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = AttachParams {
        container: container.map(str::to_string),
        stdin: false,
        stdout: true,
        stderr: true,
        tty: false,
        ..AttachParams::default()
    };
    let mut attached = pods.exec(name, command, &params).await?;
    let stdout_reader = attached.stdout();
    let stderr_reader = attached.stderr();
    // join() returns Ok even when the command fails — the failure is only
    // visible on the status channel, so read it too.
    let status_fut = attached.take_status();
    let read_stdout = async {
        match stdout_reader {
            Some(mut r) => {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf).await;
                buf
            }
            None => String::new(),
        }
    };
    let read_stderr = async {
        match stderr_reader {
            Some(mut r) => {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf).await;
                buf
            }
            None => String::new(),
        }
    };
    let read_status = async {
        match status_fut {
            Some(f) => f.await,
            None => None,
        }
    };
    let (stdout, mut stderr, joined, status) =
        tokio::join!(read_stdout, read_stderr, attached.join(), read_status);
    let mut success = joined.is_ok();
    if let Some(s) = status {
        if s.status.as_deref() == Some("Failure") {
            success = false;
            if let Some(msg) = s.message {
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&msg);
            }
        }
    }
    Ok(ExecResult {
        stdout,
        stderr,
        success,
    })
}
