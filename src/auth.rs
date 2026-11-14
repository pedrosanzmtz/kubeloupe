//! Readable messages for the ways connecting to a cluster fails: an exec
//! credential plugin that is missing, exits with an error or wants an
//! interactive login, rejected credentials, and certificate problems.
//!
//! The desktop app picks the status text by the prefix of these messages
//! (see `ui-src/src/lib/clusterError.ts`), so keep [`LOGIN_FAILED`],
//! [`CERTIFICATE_PROBLEM`] and [`UNREACHABLE`] in sync with it.

use kube::client::AuthError;
use kube::config::{ExecConfig, ExecInteractiveMode};
use std::error::Error as StdError;
use std::process::ExitStatus;

/// Start of every message about a failed login.
pub const LOGIN_FAILED: &str = "Login to `";
/// Start of every message about a TLS / certificate failure.
pub const CERTIFICATE_PROBLEM: &str = "Certificate problem";
/// Start of every message about a cluster that could not be reached.
pub const UNREACHABLE: &str = "Cannot reach the cluster";

/// What a message is about: the context, its API server and the exec
/// plugin it logs in with.
#[derive(Debug, Clone, Default)]
pub struct AuthTarget {
    pub context: String,
    pub server: String,
    /// The exec plugin's `command`, if the user logs in with one.
    pub exec_command: Option<String>,
    /// The `PATH` the plugin's own `env` sets, if any; the command is looked
    /// up there instead of in ours.
    pub exec_path: Option<String>,
}

impl AuthTarget {
    fn command(&self) -> &str {
        self.exec_command.as_deref().unwrap_or("credential plugin")
    }
}

/// Make an exec plugin fit a process that can't prompt (`interactive` is
/// false: the desktop app, or a CLI without a terminal), like kubectl does.
///
/// A plugin that always needs a prompt is refused with a readable message.
/// Otherwise it is told it is not interactive, which also makes kube capture
/// its stderr (where hints like `aws sso login` appear) instead of passing
/// it through to ours.
pub fn prepare_exec(
    exec: &mut ExecConfig,
    interactive: bool,
    target: &AuthTarget,
) -> Result<(), String> {
    if interactive {
        return Ok(());
    }
    if exec.interactive_mode == Some(ExecInteractiveMode::Always) {
        return Err(interactive_message(target));
    }
    exec.interactive_mode = Some(ExecInteractiveMode::Never);
    Ok(())
}

fn interactive_message(target: &AuthTarget) -> String {
    format!(
        "{LOGIN_FAILED}{ctx}` failed: `{cmd}` needs an interactive login (interactiveMode: Always), \
         which kubeloupe can't show. Log in from a terminal first, for example with \
         `kubectl --context {ctx} get namespaces`, then retry.",
        ctx = target.context,
        cmd = target.command(),
    )
}

/// The readable message for an error from building a client or from its
/// first request. Errors that are none of the cases above are reported as
/// an unreachable cluster, with the full error chain.
pub fn describe_client_error(err: &(dyn StdError + 'static), target: &AuthTarget) -> String {
    let ctx = &target.context;
    if let Some(auth) = find::<AuthError>(err) {
        return describe_auth_error(auth, target);
    }
    if let Some(kube::Error::Api(resp)) = find::<kube::Error>(err) {
        if resp.code == 401 {
            let hint = match &target.exec_command {
                Some(cmd) => format!(" Check that `{cmd}` still has a valid login session."),
                None => String::new(),
            };
            return format!(
                "{LOGIN_FAILED}{ctx}` failed: the cluster rejected the credentials (expired token?).{hint}"
            );
        }
    }
    let chain = error_chain(err);
    if is_tls_error(&chain) {
        return format!(
            "{CERTIFICATE_PROBLEM} connecting to `{ctx}` ({}): {chain}. Check the cluster's \
             certificate-authority data in the kubeconfig.",
            target.server
        );
    }
    format!("{UNREACHABLE} for `{ctx}` ({}): {chain}", target.server)
}

fn describe_auth_error(err: &AuthError, target: &AuthTarget) -> String {
    let ctx = &target.context;
    let cmd = target.command();
    match err {
        AuthError::AuthExecStart(io) if io.kind() == std::io::ErrorKind::NotFound => {
            let path = match &target.exec_path {
                Some(path) => path.clone(),
                None => std::env::var("PATH").unwrap_or_default(),
            };
            not_found_message(cmd, ctx, &path)
        }
        AuthError::AuthExecRun { status, out, .. } => {
            exec_failed_message(cmd, ctx, *status, &out.stderr)
        }
        AuthError::AuthExecStart(io) => {
            format!("{LOGIN_FAILED}{ctx}` failed: could not run `{cmd}`: {io}")
        }
        AuthError::AuthExecParse(_) | AuthError::ExecPluginFailed => format!(
            "{LOGIN_FAILED}{ctx}` failed: `{cmd}` returned something that is not a valid \
             ExecCredential: {}",
            error_chain(err)
        ),
        _ => format!("{LOGIN_FAILED}{ctx}` failed: {}", error_chain(err)),
    }
}

/// The message for an exec plugin that is not on `PATH`.
pub fn not_found_message(cmd: &str, ctx: &str, path: &str) -> String {
    let searched = if path.is_empty() {
        "(empty)".to_string()
    } else {
        std::env::split_paths(path)
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{LOGIN_FAILED}{ctx}` failed: the `{cmd}` command it logs in with was not found. \
         Searched PATH: {searched}. Install it, or start kubeloupe from a terminal."
    )
}

/// The message for an exec plugin that exited with an error, with its stderr.
pub fn exec_failed_message(cmd: &str, ctx: &str, status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    let output = if stderr.is_empty() {
        "it printed no error message".to_string()
    } else {
        stderr.to_string()
    };
    let status = match status.code() {
        Some(code) => format!("exit code {code}"),
        None => status.to_string(),
    };
    format!("{LOGIN_FAILED}{ctx}` failed: `{cmd}` exited with {status}: {output}")
}

/// The first error in the chain of type `T`.
fn find<'a, T: StdError + 'static>(err: &'a (dyn StdError + 'static)) -> Option<&'a T> {
    let mut cur: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(t) = e.downcast_ref::<T>() {
            return Some(t);
        }
        cur = e.source();
    }
    None
}

/// The error and its sources joined with ": ", skipping a source whose text
/// the previous message already contains.
fn error_chain(err: &(dyn StdError + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(e) = cur {
        let text = e.to_string();
        if !parts.last().is_some_and(|last| last.contains(&text)) {
            parts.push(text);
        }
        cur = e.source();
    }
    parts.join(": ")
}

fn is_tls_error(chain: &str) -> bool {
    let lower = chain.to_lowercase();
    ["certificate", "x509", "tls handshake", "unknownissuer"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    fn target() -> AuthTarget {
        AuthTarget {
            context: "prod".into(),
            server: "https://k.example".into(),
            exec_command: Some("aws".into()),
            exec_path: None,
        }
    }

    #[test]
    fn not_found_lists_path() {
        let msg = not_found_message("aws", "prod", "/usr/bin:/bin");
        assert!(msg.starts_with(LOGIN_FAILED), "{msg}");
        assert!(msg.contains("`aws`"), "{msg}");
        assert!(msg.contains("`prod`"), "{msg}");
        assert!(msg.contains("Searched PATH: /usr/bin, /bin."), "{msg}");
    }

    #[cfg(unix)]
    #[test]
    fn exec_failure_includes_stderr() {
        let msg = exec_failed_message(
            "aws",
            "prod",
            exit_status(255),
            b"\nError: SSO session expired. Run aws sso login\n",
        );
        assert!(msg.starts_with(LOGIN_FAILED), "{msg}");
        assert!(msg.contains("exit code 255"), "{msg}");
        assert!(msg.ends_with("Run aws sso login"), "{msg}");
        let empty = exec_failed_message("aws", "prod", exit_status(1), b"  ");
        assert!(empty.contains("no error message"), "{empty}");
    }

    #[test]
    fn interactive_plugins() {
        let mut exec: ExecConfig = serde_json::from_value(serde_json::json!({
            "command": "kubelogin",
            "interactiveMode": "Always",
        }))
        .unwrap();
        // A terminal can prompt: left alone.
        assert!(prepare_exec(&mut exec, true, &target()).is_ok());
        assert_eq!(exec.interactive_mode, Some(ExecInteractiveMode::Always));
        // Without one, refused.
        let err = prepare_exec(&mut exec, false, &target()).unwrap_err();
        assert!(err.contains("interactive login"), "{err}");

        // IfAvailable (and unset) become Never so stderr is captured.
        exec.interactive_mode = Some(ExecInteractiveMode::IfAvailable);
        prepare_exec(&mut exec, false, &target()).unwrap();
        assert_eq!(exec.interactive_mode, Some(ExecInteractiveMode::Never));
        exec.interactive_mode = None;
        prepare_exec(&mut exec, false, &target()).unwrap();
        assert_eq!(exec.interactive_mode, Some(ExecInteractiveMode::Never));
    }

    #[test]
    fn classifies_kube_errors() {
        let not_found = kube::Error::Auth(AuthError::AuthExecStart(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
        let msg = describe_client_error(&not_found, &target());
        assert!(msg.contains("was not found"), "{msg}");

        let unauthorized = kube::Error::Api(
            serde_json::from_value(serde_json::json!({
                "status": "Failure", "message": "Unauthorized", "reason": "Unauthorized", "code": 401
            }))
            .unwrap(),
        );
        let msg = describe_client_error(&unauthorized, &target());
        assert!(msg.starts_with(LOGIN_FAILED), "{msg}");
        assert!(msg.contains("rejected the credentials"), "{msg}");

        // Wrapped in anyhow, as the rest of the crate returns them.
        let wrapped = anyhow::Error::new(unauthorized);
        let msg = describe_client_error(wrapped.as_ref(), &target());
        assert!(msg.contains("rejected the credentials"), "{msg}");

        let tls = anyhow::anyhow!("invalid peer certificate: UnknownIssuer");
        let msg = describe_client_error(tls.as_ref(), &target());
        assert!(msg.starts_with(CERTIFICATE_PROBLEM), "{msg}");

        let refused = anyhow::anyhow!("tcp connect error: Connection refused");
        let msg = describe_client_error(refused.as_ref(), &target());
        assert!(msg.starts_with(UNREACHABLE), "{msg}");
        assert!(msg.contains("https://k.example"), "{msg}");
    }

    #[test]
    fn chain_skips_repeats() {
        let err = anyhow::anyhow!("inner failure").context("outer: inner failure");
        assert_eq!(error_chain(err.as_ref()), "outer: inner failure");
        let err = anyhow::anyhow!("inner").context("outer");
        assert_eq!(error_chain(err.as_ref()), "outer: inner");
    }
}
