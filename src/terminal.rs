//! Interactive sessions on the CLI: `kubeloupe debug` and `kubeloupe
//! node-shell`. Connects the local terminal (raw mode, window size) or plain
//! pipes to an attached or exec'd process.

use crate::debug::{add_debug_container, attach_container, describe_debug_error, EPHEMERAL_NOTE};
use crate::node_shell::{
    create_node_shell, delete_node_shell, describe_node_shell_error, exec_node_shell,
    new_node_shell_name,
};
use anyhow::{anyhow, Result};
use futures::SinkExt;
use kube::api::{AttachedProcess, TerminalSize};
use kube::Client;
use std::io::{IsTerminal, Read, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Whether the session gets a TTY: only when stdin and stdout are terminals.
pub fn use_tty() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Puts the local terminal in raw mode while alive (Unix).
struct RawMode {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl RawMode {
    fn enable() -> Self {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr/tcsetattr only read and write the termios we
            // own, for the stdin file descriptor.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    return RawMode { saved: None };
                }
                let saved = t;
                libc::cfmakeraw(&mut t);
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
                RawMode { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        RawMode {}
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(t) = self.saved {
            // SAFETY: restores the settings read in `enable`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
    }
}

fn local_size() -> Option<TerminalSize> {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ writes a winsize into the struct we pass.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
                return Some(TerminalSize {
                    width: ws.ws_col,
                    height: ws.ws_row,
                });
            }
        }
    }
    None
}

/// Read stdin on a plain thread: tokio's stdin cannot be cancelled and would
/// keep the process alive after the session ends.
fn stdin_channel() -> tokio::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Connect the local terminal to `process` until it exits.
pub async fn run_attached(mut process: AttachedProcess, tty: bool) -> Result<()> {
    let _raw = tty.then(RawMode::enable);
    if let Some(mut tx) = process.terminal_size() {
        if let Some(size) = local_size() {
            let _ = tx.send(size).await;
        }
        #[cfg(unix)]
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let Ok(mut winch) = signal(SignalKind::window_change()) else {
                return;
            };
            while winch.recv().await.is_some() {
                if let Some(size) = local_size() {
                    if tx.send(size).await.is_err() {
                        break;
                    }
                }
            }
        });
    }
    let mut stdin_rx = stdin_channel();
    let stdin_writer = process.stdin();
    let stdout_reader = process.stdout();
    let stderr_reader = process.stderr();
    let stdin_pump = async move {
        if let Some(mut w) = stdin_writer {
            while let Some(bytes) = stdin_rx.recv().await {
                if w.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            // Stdin ended (piped input): let the remote shell see EOF.
            let _ = w.shutdown().await;
        }
        std::future::pending::<()>().await
    };
    let stdout_pump = async move {
        if let Some(mut r) = stdout_reader {
            let mut buf = [0u8; 4096];
            while let Ok(n) = r.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(&buf[..n]);
                let _ = out.flush();
            }
        }
    };
    let stderr_pump = async move {
        if let Some(mut r) = stderr_reader {
            let mut buf = [0u8; 4096];
            while let Ok(n) = r.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let _ = std::io::stderr().write_all(&buf[..n]);
            }
        }
    };
    let status = process.take_status();
    tokio::select! {
        () = async { tokio::join!(stdout_pump, stderr_pump); } => {},
        () = stdin_pump => {},
    }
    drop(_raw);
    if let Some(status) = status {
        if let Some(s) = status.await {
            // A non-zero exit of the shell is the user's business.
            let nonzero = s.reason.as_deref() == Some("NonZeroExitCode");
            if s.status.as_deref() == Some("Failure") && !nonzero {
                return Err(anyhow!(s
                    .message
                    .unwrap_or_else(|| "session failed".into())));
            }
        }
    }
    Ok(())
}

/// `kubeloupe debug <pod> [-c container] [--image]`.
pub async fn cli_debug(
    client: &Client,
    namespace: &str,
    pod: &str,
    container: Option<&str>,
    image: &str,
) -> Result<()> {
    let tty = use_tty();
    eprintln!("{EPHEMERAL_NOTE}");
    let name = add_debug_container(
        client,
        namespace,
        pod,
        container.unwrap_or(""),
        image,
        tty,
        |reason| eprintln!("debug container: {reason}"),
    )
    .await
    .map_err(|e| anyhow!("pod/{namespace}/{pod}: {}", describe_debug_error(&e)))?;
    eprintln!(
        "Attached to {name} in pod/{namespace}/{pod}. If you don't see a prompt, press Enter."
    );
    let process = attach_container(client, namespace, pod, &name, tty)
        .await
        .map_err(|e| anyhow!("pod/{namespace}/{pod}: {}", describe_debug_error(&e)))?;
    run_attached(process, tty).await
}

/// `kubeloupe node-shell <node>`: the pod is deleted however the session ends
/// (exit, error, Ctrl-C, SIGTERM).
pub async fn cli_node_shell(
    client: &Client,
    node: &str,
    namespace: &str,
    image: &str,
) -> Result<()> {
    let tty = use_tty();
    let pod = new_node_shell_name(node);
    let describe =
        |e: anyhow::Error| anyhow!("node/{node}: {}", describe_node_shell_error(&e, namespace));
    let session = async {
        create_node_shell(client, node, namespace, &pod, image, |reason| {
            eprintln!("node shell pod: {reason}")
        })
        .await
        .map_err(describe)?;
        eprintln!("Shell on node/{node} (pod {namespace}/{pod}, deleted when the shell exits).");
        let process = exec_node_shell(client, namespace, &pod, tty)
            .await
            .map_err(describe)?;
        run_attached(process, tty).await
    };
    let result = tokio::select! {
        r = session => r,
        () = interrupted() => Err(anyhow!("interrupted")),
    };
    let deleted = delete_node_shell(client, namespace, &pod).await;
    result?;
    deleted
}

/// Ctrl-C (outside raw mode) or SIGTERM.
async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
