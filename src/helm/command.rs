//! Changing releases through the `helm` CLI: rollback and uninstall.
//!
//! Reading releases is native (see the parent module); writing them is left
//! to Helm itself, which knows how to render, hook and record a revision.
//! The binary is `helm` on `PATH`, or `KUBELOUPE_HELM` if set.

use super::check_release_name;
use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Give up on a helm command after this long (it does not `--wait`).
const HELM_TIMEOUT: Duration = Duration::from_secs(300);

/// The cluster helm should talk to: the same kubeconfig file and context as
/// kubeloupe. `None` leaves helm's default (`KUBECONFIG`, then
/// `~/.kube/config`; the current context).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelmTarget {
    pub kubeconfig: Option<PathBuf>,
    pub context: Option<String>,
}

impl HelmTarget {
    fn args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(ctx) = &self.context {
            out.push("--kube-context".to_string());
            out.push(ctx.clone());
        }
        if let Some(path) = &self.kubeconfig {
            out.push("--kubeconfig".to_string());
            out.push(path.display().to_string());
        }
        out
    }
}

fn helm_binary() -> String {
    std::env::var("KUBELOUPE_HELM")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "helm".to_string())
}

const NOT_INSTALLED: &str = "helm is not installed or not on PATH. Install it (https://helm.sh/docs/intro/install/) to roll back or uninstall releases.";

/// Helm's own error text without its `Error: ` prefix, else the exit status.
fn failure_text(stderr: &str, status: std::process::ExitStatus) -> String {
    let text = stderr.trim();
    let text = text.strip_prefix("Error: ").unwrap_or(text);
    if text.is_empty() {
        format!("helm exited with {status}")
    } else {
        text.to_string()
    }
}

async fn run_helm(args: &[String]) -> Result<String> {
    let mut cmd = Command::new(helm_binary());
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(HELM_TIMEOUT, cmd.output()).await {
        Err(_) => bail!("helm did not finish within {} s", HELM_TIMEOUT.as_secs()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => bail!(NOT_INSTALLED),
        Ok(Err(e)) => return Err(anyhow!(e).context("cannot run helm")),
        Ok(Ok(out)) => out,
    };
    if !out.status.success() {
        bail!(failure_text(
            &String::from_utf8_lossy(&out.stderr),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The installed helm's version (`v3.16.4+g7877b45`), or why there is none.
pub async fn helm_version() -> Result<String> {
    run_helm(&["version".into(), "--short".into()]).await
}

fn rollback_args(target: &HelmTarget, namespace: &str, name: &str, revision: u32) -> Vec<String> {
    let mut args: Vec<String> = vec!["rollback".into(), name.into(), revision.to_string()];
    args.extend(["--namespace".into(), namespace.into()]);
    args.extend(target.args());
    args
}

fn uninstall_args(target: &HelmTarget, namespace: &str, name: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["uninstall".into(), name.into()];
    args.extend(["--namespace".into(), namespace.into()]);
    args.extend(target.args());
    args
}

fn check_namespace(namespace: &str) -> Result<()> {
    if namespace.is_empty() || namespace.starts_with('-') {
        bail!("{namespace:?} is not a valid namespace");
    }
    Ok(())
}

/// `helm rollback <name> <revision> -n <namespace>`. Returns a short
/// summary.
pub async fn helm_rollback(
    target: &HelmTarget,
    namespace: &str,
    name: &str,
    revision: u32,
) -> Result<String> {
    check_release_name(name)?;
    check_namespace(namespace)?;
    if revision == 0 {
        bail!("revision must be 1 or more");
    }
    run_helm(&rollback_args(target, namespace, name, revision))
        .await
        .map_err(|e| anyhow!("Rollback of {name} failed: {e}"))?;
    Ok(format!(
        "Rolled back {namespace}/{name} to revision {revision}"
    ))
}

/// `helm uninstall <name> -n <namespace>`. Returns a short summary.
pub async fn helm_uninstall(target: &HelmTarget, namespace: &str, name: &str) -> Result<String> {
    check_release_name(name)?;
    check_namespace(namespace)?;
    run_helm(&uninstall_args(target, namespace, name))
        .await
        .map_err(|e| anyhow!("Uninstall of {name} failed: {e}"))?;
    Ok(format!("Uninstalled {namespace}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_the_context_and_kubeconfig() {
        let t = HelmTarget {
            kubeconfig: Some("/k/config".into()),
            context: Some("kind-x".into()),
        };
        assert_eq!(
            rollback_args(&t, "apps", "web", 2).join(" "),
            "rollback web 2 --namespace apps --kube-context kind-x --kubeconfig /k/config"
        );
        assert_eq!(
            uninstall_args(&HelmTarget::default(), "apps", "web").join(" "),
            "uninstall web --namespace apps"
        );
    }

    #[cfg(unix)]
    #[test]
    fn helm_errors_lose_their_prefix() {
        use std::os::unix::process::ExitStatusExt;
        let status = std::process::ExitStatus::from_raw(1 << 8);
        assert_eq!(
            failure_text("Error: release: not found\n", status),
            "release: not found"
        );
        assert_eq!(
            failure_text("  ", status),
            "helm exited with exit status: 1"
        );
    }

    #[tokio::test]
    async fn rejects_bad_input_before_running_helm() {
        let t = HelmTarget::default();
        assert!(helm_rollback(&t, "ns", "-x", 1).await.is_err());
        assert!(helm_rollback(&t, "ns", "web", 0).await.is_err());
        assert!(helm_uninstall(&t, "--all", "web").await.is_err());
    }
}
