//! Local shells in a pseudo-terminal (`portable-pty`: a Unix PTY, ConPTY on
//! Windows), for the dock's "local terminal" tabs.
//!
//! The shell runs with `KUBECONFIG` pointing at a temp kubeconfig first (see
//! `kubeloupe::shell_kubeconfig`); the session owns that file and deletes it
//! when it is dropped.

use kubeloupe::TempKubeconfig;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

/// What to run in the PTY.
#[derive(Debug, Clone)]
pub(crate) struct ShellSpec {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) cwd: Option<PathBuf>,
    /// Set on top of this process's environment.
    pub(crate) env: Vec<(OsString, OsString)>,
}

impl ShellSpec {
    /// The user's shell, started in the home directory: `$SHELL -l` on
    /// macOS and Linux; `pwsh`, `powershell` or `cmd` on Windows.
    pub(crate) fn user_shell() -> Self {
        let (program, args) = user_shell_command();
        ShellSpec {
            program,
            args,
            cwd: home_dir(),
            env: vec![("TERM".into(), "xterm-256color".into())],
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn user_shell_command() -> (OsString, Vec<OsString>) {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let fallback = if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            };
            if std::path::Path::new(fallback).exists() {
                fallback.into()
            } else {
                "/bin/sh".into()
            }
        });
    (shell, vec!["-l".into()])
}

#[cfg(windows)]
fn user_shell_command() -> (OsString, Vec<OsString>) {
    for name in ["pwsh.exe", "powershell.exe"] {
        if let Some(path) = find_in_path(name) {
            return (path.into_os_string(), vec!["-NoLogo".into()]);
        }
    }
    let cmd = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
    (cmd, Vec::new())
}

#[cfg(windows)]
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// A running local shell. Dropping it kills the shell and deletes its temp
/// kubeconfig.
pub(crate) struct LocalShell {
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    _kubeconfig: Option<TempKubeconfig>,
}

impl LocalShell {
    /// Start `spec` in a new PTY of `cols` x `rows`. `on_output` gets the
    /// output as text (UTF-8 split across reads is kept whole); `on_exit`
    /// runs once when the shell has exited and its output was delivered.
    pub(crate) fn spawn(
        spec: &ShellSpec,
        cols: u16,
        rows: u16,
        kubeconfig: Option<TempKubeconfig>,
        mut on_output: impl FnMut(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| format!("cannot open a terminal: {e}"))?;
        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| {
            format!(
                "cannot start the shell {}: {e}",
                spec.program.to_string_lossy()
            )
        })?;
        // Only the shell holds the terminal's other end now, so reads end
        // when it exits.
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("cannot read from the terminal: {e}"))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("cannot write to the terminal: {e}"))?;

        // Writes can block while the shell isn't reading: keep them off
        // the command's thread.
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for bytes in input_rx {
                if writer
                    .write_all(&bytes)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });

        let (read_done, read_done_rx) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut pending = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = decode_utf8(&mut pending, &buf[..n]);
                        if !text.is_empty() {
                            on_output(text);
                        }
                    }
                }
            }
            let _ = read_done.send(());
        });

        std::thread::spawn(move || {
            let _ = child.wait();
            // Let the last output arrive first. A background job that keeps
            // the terminal open (or ConPTY, which only ends reads when the
            // console is closed) must not hold up the close.
            let _ = read_done_rx.recv_timeout(Duration::from_millis(500));
            on_exit();
        });

        Ok(LocalShell {
            input,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            _kubeconfig: kubeconfig,
        })
    }

    pub(crate) fn write(&self, data: Vec<u8>) -> Result<(), String> {
        self.input
            .send(data)
            .map_err(|_| "the shell has exited".to_string())
    }

    pub(crate) fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.master
            .lock()
            .unwrap()
            .resize(size(cols, rows))
            .map_err(|e| format!("cannot resize the terminal: {e}"))
    }
}

impl Drop for LocalShell {
    fn drop(&mut self) {
        let _ = self.killer.lock().unwrap().kill();
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Decode `chunk` after the bytes left over from the last read. An
/// incomplete character at the end stays in `pending` for the next read;
/// invalid bytes become U+FFFD.
fn decode_utf8(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let mut out = String::new();
    loop {
        let (valid, bad) = match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => (e.valid_up_to(), e.error_len()),
        };
        out.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or_default());
        match bad {
            Some(len) => {
                out.push('\u{FFFD}');
                pending.drain(..valid + len);
            }
            None => {
                pending.drain(..valid);
                return out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_reads() {
        let mut pending = Vec::new();
        let bytes = "aé€".as_bytes();
        assert_eq!(decode_utf8(&mut pending, &bytes[..2]), "a");
        assert_eq!(decode_utf8(&mut pending, &bytes[2..4]), "é");
        assert_eq!(decode_utf8(&mut pending, &bytes[4..]), "€");
        assert!(pending.is_empty());
        assert_eq!(decode_utf8(&mut pending, b"x\xffy"), "x\u{FFFD}y");
    }

    /// Run `script` with `sh -c` in a PTY until it exits, then close the
    /// session, and return its output.
    #[cfg(unix)]
    fn run_in_pty(
        script: &str,
        env: Vec<(OsString, OsString)>,
        kubeconfig: Option<TempKubeconfig>,
    ) -> String {
        let spec = ShellSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: None,
            env,
        };
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (exit_tx, exit_rx) = mpsc::channel::<()>();
        let shell = LocalShell::spawn(
            &spec,
            80,
            24,
            kubeconfig,
            move |text| {
                let _ = out_tx.send(text);
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .unwrap();
        exit_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the shell did not exit");
        drop(shell);
        out_rx.try_iter().collect()
    }

    #[cfg(unix)]
    #[test]
    fn pty_runs_with_input_and_size() {
        let spec = ShellSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "stty size; read line; echo \"got $line\"".into(),
            ],
            cwd: None,
            env: vec![],
        };
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (exit_tx, exit_rx) = mpsc::channel::<()>();
        let shell = LocalShell::spawn(
            &spec,
            100,
            30,
            None,
            move |t| {
                let _ = out_tx.send(t);
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .unwrap();
        shell.write(b"hello\n".to_vec()).unwrap();
        exit_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let out: String = out_rx.try_iter().collect();
        assert!(out.contains("30 100"), "{out:?}");
        assert!(out.contains("got hello"), "{out:?}");
    }

    /// The test the issue asks for: kubectl in the PTY sees the app's
    /// context and namespace, the user's kubeconfig is byte-identical
    /// afterwards, and the temp file is gone. Runs in a temp HOME; skipped
    /// where kubectl is not installed.
    #[cfg(unix)]
    #[test]
    fn kubectl_in_the_pty_uses_the_app_context() {
        use kubeloupe::{shell_kubeconfig, shell_kubeconfig_env};
        use std::process::Command;

        if Command::new("kubectl")
            .args(["version", "--client"])
            .output()
            .is_err()
        {
            // GitHub's runners have kubectl: don't let CI skip this quietly.
            assert!(std::env::var_os("CI").is_none(), "kubectl not found on CI");
            eprintln!("kubectl not found: skipping");
            return;
        }
        let home = std::env::temp_dir().join(format!(
            "kubeloupe-pty-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let kube_dir = home.join(".kube");
        std::fs::create_dir_all(&kube_dir).unwrap();
        let user_config = kube_dir.join("config");
        let original = "apiVersion: v1\nkind: Config\ncurrent-context: one\n\
            clusters:\n- name: c1\n  cluster: {server: \"https://127.0.0.1:1\"}\n\
            - name: c2\n  cluster: {server: \"https://127.0.0.1:2\"}\n\
            users:\n- name: u1\n  user: {token: t1}\n- name: u2\n  user: {token: t2}\n\
            contexts:\n- name: one\n  context: {cluster: c1, user: u1, namespace: orig}\n\
            - name: two\n  context: {cluster: c2, user: u2}\n";
        std::fs::write(&user_config, original).unwrap();

        let kc = kube::config::Kubeconfig::read_from(&user_config).unwrap();
        let generated = shell_kubeconfig(&kc, Some("two"), Some("team-a")).unwrap();
        let temp = TempKubeconfig::create_in(&home, &generated.yaml).unwrap();
        let temp_path = temp.path().to_path_buf();
        let kubeconfig = shell_kubeconfig_env(temp.path(), None, Some(&user_config), false);
        let out = run_in_pty(
            "kubectl config current-context; \
             kubectl config view --minify -o jsonpath='ns={..namespace}'; echo; \
             kubectl config view --minify -o jsonpath='server={.clusters[0].cluster.server}'; echo",
            vec![
                ("HOME".into(), home.clone().into_os_string()),
                ("KUBECONFIG".into(), kubeconfig),
            ],
            Some(temp),
        );
        let lines: Vec<&str> = out.lines().map(str::trim).collect();
        assert!(lines.contains(&"two"), "{out:?}");
        assert!(lines.contains(&"ns=team-a"), "{out:?}");
        assert!(lines.contains(&"server=https://127.0.0.1:2"), "{out:?}");
        assert_eq!(std::fs::read_to_string(&user_config).unwrap(), original);
        assert!(!temp_path.exists(), "the temp kubeconfig was not deleted");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
