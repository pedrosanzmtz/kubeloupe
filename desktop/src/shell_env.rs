//! Import the login shell's environment when the app starts from the Dock,
//! Finder or a desktop launcher.
//!
//! GUI apps don't get the environment set up in shell rc files: on macOS
//! `PATH` is launchd's `/usr/bin:/bin:/usr/sbin:/sbin`, so exec credential
//! plugins like `aws`, `gke-gcloud-auth-plugin` or `kubelogin` are not found,
//! and a `KUBECONFIG` exported in `~/.zshrc` is not seen. This runs
//! `$SHELL -ilc 'env -0'` once at start-up and imports `PATH` (merged),
//! `KUBECONFIG` and a few cloud CLI variables.
//!
//! Unix only: Windows apps get the user's environment. Skipped when started
//! from a terminal (stdin is a TTY, so the environment is already the
//! shell's), or when `KUBELOUPE_NO_SHELL_ENV` is set.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Set to any value to skip importing the login shell's environment.
pub(crate) const OPT_OUT: &str = "KUBELOUPE_NO_SHELL_ENV";

const TIMEOUT: Duration = Duration::from_secs(3);

/// Printed around the environment, so whatever the rc files print to stdout
/// is ignored.
const START: &str = "__KUBELOUPE_ENV_START__";
const END: &str = "__KUBELOUPE_ENV_END__";

/// Imported when the process doesn't already set them (besides `PATH`, which
/// is merged).
const IMPORTED: &[&str] = &[
    "KUBECONFIG",
    "KUBECACHEDIR",
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
    "CLOUDSDK_CONFIG",
    "CLOUDSDK_CORE_PROJECT",
    "USE_GKE_GCLOUD_AUTH_PLUGIN",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "AZURE_CONFIG_DIR",
    "DIGITALOCEAN_ACCESS_TOKEN",
];

/// Import the login shell's environment into this process. Must run before
/// any other thread starts, since it changes the process environment.
/// Failures (no shell, timeout, odd output) are ignored.
pub(crate) fn import_login_shell_env() {
    use std::io::IsTerminal;
    if std::env::var_os(OPT_OUT).is_some() || std::io::stdin().is_terminal() {
        return;
    }
    let Some(output) = capture(&login_shell(), TIMEOUT) else {
        return;
    };
    let Some(shell_env) = parse(&output) else {
        return;
    };
    let current: HashMap<String, String> = std::env::vars().collect();
    for (key, value) in changes(&current, &shell_env) {
        // Single-threaded here (start of `main`), so this is sound.
        std::env::set_var(key, value);
    }
}

fn login_shell() -> PathBuf {
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return PathBuf::from(shell);
    }
    let fallback = if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    };
    if Path::new(fallback).exists() {
        PathBuf::from(fallback)
    } else {
        PathBuf::from("/bin/sh")
    }
}

/// The script run by the login shell. Works in sh, bash, zsh, fish and csh.
/// Falls back to plain `env` where `env -0` is not supported.
fn script() -> String {
    format!("printf '%s' {START}; env -0 2>/dev/null || env; printf '%s' {END}")
}

/// Run `shell -ilc <script>` and return its stdout, or `None` if it fails or
/// takes longer than `timeout` (it is killed then).
fn capture(shell: &Path, timeout: Duration) -> Option<Vec<u8>> {
    capture_command(shell, &["-ilc".into(), script()], timeout)
}

fn capture_command(program: &Path, args: &[String], timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    // Read on a thread: something the rc files started in the background
    // may hold stdout open after the shell exits.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(timeout) {
        Ok(buf) => {
            let _ = child.wait();
            Some(buf)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Parse the environment between the markers: NUL-separated `KEY=value`
/// entries (`env -0`), or newline-separated ones if this `env` doesn't
/// support `-0`. `None` if the markers are missing or nothing parses.
fn parse(output: &[u8]) -> Option<HashMap<String, String>> {
    let text = String::from_utf8_lossy(output);
    let start = text.find(START)? + START.len();
    let end = start + text[start..].find(END)?;
    let body = &text[start..end];
    let sep = if body.contains('\0') { '\0' } else { '\n' };
    let env: HashMap<String, String> = body
        .split(sep)
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            valid_name(key).then(|| (key.to_string(), value.to_string()))
        })
        .collect();
    (!env.is_empty()).then_some(env)
}

fn valid_name(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The variables to set, given the process's environment and the shell's.
///
/// `PATH` becomes the shell's entries in its order (what a terminal would
/// find first), followed by the process's entries the shell lacks. The
/// [`IMPORTED`] variables are taken from the shell only when the process
/// doesn't set them already.
fn changes(
    current: &HashMap<String, String>,
    shell: &HashMap<String, String>,
) -> Vec<(String, OsString)> {
    let mut out = Vec::new();
    if let Some(shell_path) = shell.get("PATH") {
        let merged = merge_paths(shell_path, current.get("PATH").map(String::as_str));
        if current.get("PATH").map(OsString::from) != Some(merged.clone()) {
            out.push(("PATH".to_string(), merged));
        }
    }
    for key in IMPORTED {
        if current.contains_key(*key) {
            continue;
        }
        if let Some(value) = shell.get(*key) {
            out.push((key.to_string(), value.into()));
        }
    }
    out
}

fn merge_paths(shell: &str, current: Option<&str>) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    let all =
        std::env::split_paths(shell).chain(current.into_iter().flat_map(std::env::split_paths));
    for entry in all {
        if !entry.as_os_str().is_empty() && !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    std::env::join_paths(entries).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(body: &str) -> Vec<u8> {
        format!("motd noise\n{START}{body}{END}logout\n").into_bytes()
    }

    #[test]
    fn parses_nul_separated_env() {
        let env = parse(&wrap(
            "PATH=/opt/homebrew/bin:/usr/bin\0KUBECONFIG=/k/a:/k/b\0MULTI=line1\nline2\0",
        ))
        .unwrap();
        assert_eq!(env["PATH"], "/opt/homebrew/bin:/usr/bin");
        assert_eq!(env["KUBECONFIG"], "/k/a:/k/b");
        assert_eq!(env["MULTI"], "line1\nline2");
    }

    #[test]
    fn parses_newline_separated_env() {
        let env = parse(&wrap("PATH=/usr/bin\nAWS_PROFILE=dev\n")).unwrap();
        assert_eq!(env["PATH"], "/usr/bin");
        assert_eq!(env["AWS_PROFILE"], "dev");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"").is_none());
        assert!(parse(b"\xff\xfe random bytes \0 no markers").is_none());
        assert!(parse(format!("{START}no end marker").as_bytes()).is_none());
        assert!(parse(&wrap("just text\0=nokey\0\x31BAD=x\0")).is_none());
        let env = parse(&wrap("junk\0GOOD=1\0")).unwrap();
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn merges_path_and_imports_missing_vars() {
        let current: HashMap<String, String> = [
            ("PATH", "/usr/bin:/bin:/app/bin"),
            ("AWS_PROFILE", "from-launcher"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let shell: HashMap<String, String> = [
            ("PATH", "/opt/homebrew/bin:/usr/bin:/bin"),
            ("AWS_PROFILE", "from-shell"),
            ("KUBECONFIG", "/home/u/.kube/a"),
            ("SECRET_TOKEN", "not imported"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let changes: HashMap<String, OsString> = changes(&current, &shell).into_iter().collect();
        assert_eq!(
            changes["PATH"],
            OsString::from("/opt/homebrew/bin:/usr/bin:/bin:/app/bin")
        );
        assert_eq!(changes["KUBECONFIG"], OsString::from("/home/u/.kube/a"));
        assert!(!changes.contains_key("AWS_PROFILE"));
        assert!(!changes.contains_key("SECRET_TOKEN"));
    }

    #[test]
    fn capture_times_out() {
        let started = std::time::Instant::now();
        let out = capture_command(
            Path::new("/bin/sh"),
            &["-c".into(), "sleep 10".into()],
            Duration::from_millis(200),
        );
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn captures_a_real_shell() {
        // `sh -c` (no -il) runs the same script, without the user's rc files.
        let out = capture_command(
            Path::new("/bin/sh"),
            &["-c".into(), script()],
            Duration::from_secs(5),
        )
        .unwrap();
        let env = parse(&out).unwrap();
        assert!(env.contains_key("PATH"));
    }
}
