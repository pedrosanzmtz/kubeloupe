//! Exec credential plugins: a fake `kubeloupe-test-auth` plugin in a temp dir,
//! and a kubeconfig whose user logs in with it.
//!
//! The plugin runs before the first request, so the failure cases need no
//! cluster and run in plain `cargo test`. The `#[ignore]`d test logs in to the
//! kind cluster (`KUBELOUPE_TEST_CONTEXT`, default `kind-kubeloupe-test`) with
//! a ServiceAccount token the plugin prints.
//!
//! The plugin's directory is put on the `PATH` the kubeconfig's `exec.env`
//! gives the plugin (the command is looked up there), rather than on this
//! process's `PATH`, which the tests running in parallel share.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result};
use kube::config::Kubeconfig;
use kubeloupe::client_from_kubeconfig;

const PLUGIN: &str = "kubeloupe-test-auth";
/// Nothing listens here, so a plugin that works ends in "Cannot reach".
const NO_SERVER: &str = "https://127.0.0.1:1";

/// A fresh temp dir holding an executable `kubeloupe-test-auth` script.
fn plugin_dir(script: &str) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kubeloupe-auth-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(PLUGIN);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(dir)
}

fn credential_script(token: &str) -> String {
    format!(
        r#"cat <<'EOF'
{{"apiVersion":"client.authentication.k8s.io/v1","kind":"ExecCredential","status":{{"token":"{token}"}}}}
EOF"#
    )
}

/// A kubeconfig with one context `fake` whose user runs the plugin, with
/// `path` as its `PATH`.
fn kubeconfig(
    cluster: serde_json::Value,
    path: &Path,
    exec_extra: serde_json::Value,
) -> Kubeconfig {
    let mut exec = serde_json::json!({
        "apiVersion": "client.authentication.k8s.io/v1",
        "command": PLUGIN,
        "env": [{"name": "PATH", "value": format!("{}:/usr/bin:/bin", path.display())}],
    });
    if let (Some(exec), Some(extra)) = (exec.as_object_mut(), exec_extra.as_object()) {
        exec.extend(extra.clone());
    }
    serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "fake",
        "contexts": [{"name": "fake", "context": {"cluster": "c", "user": "u"}}],
        "clusters": [{"name": "c", "cluster": cluster}],
        "users": [{"name": "u", "user": {"exec": exec}}],
    }))
    .expect("valid kubeconfig")
}

fn no_server() -> serde_json::Value {
    serde_json::json!({"server": NO_SERVER, "insecure-skip-tls-verify": true})
}

async fn connect_error(kc: Kubeconfig) -> String {
    match client_from_kubeconfig(kc, None, false).await {
        Ok(_) => panic!("expected the connection to fail"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn plugin_on_path_is_run() -> Result<()> {
    let dir = plugin_dir(&credential_script("t0ken"))?;
    let msg = connect_error(kubeconfig(no_server(), &dir, serde_json::json!({}))).await;
    // The plugin ran and returned a token; only the server is missing.
    assert!(
        msg.starts_with("Cannot reach the cluster for `fake`"),
        "{msg}"
    );
    assert!(msg.contains(NO_SERVER), "{msg}");
    Ok(())
}

#[tokio::test]
async fn missing_plugin_is_reported() -> Result<()> {
    let empty = std::env::temp_dir().join("kubeloupe-auth-does-not-exist");
    let msg = connect_error(kubeconfig(no_server(), &empty, serde_json::json!({}))).await;
    assert!(msg.starts_with("Login to `fake` failed"), "{msg}");
    assert!(
        msg.contains("the `kubeloupe-test-auth` command it logs in with was not found"),
        "{msg}"
    );
    assert!(msg.contains(&empty.display().to_string()), "{msg}");
    assert!(msg.contains("start kubeloupe from a terminal"), "{msg}");
    Ok(())
}

#[tokio::test]
async fn failing_plugin_stderr_is_shown() -> Result<()> {
    let dir =
        plugin_dir("echo 'Error: the SSO session has expired. Run: aws sso login' >&2\nexit 1")?;
    let msg = connect_error(kubeconfig(no_server(), &dir, serde_json::json!({}))).await;
    assert!(msg.starts_with("Login to `fake` failed"), "{msg}");
    assert!(msg.contains("exit code 1"), "{msg}");
    assert!(msg.contains("Run: aws sso login"), "{msg}");
    Ok(())
}

#[tokio::test]
async fn interactive_plugin_is_refused() -> Result<()> {
    let dir = plugin_dir(&credential_script("t0ken"))?;
    let kc = kubeconfig(
        no_server(),
        &dir,
        serde_json::json!({"interactiveMode": "Always"}),
    );
    let msg = connect_error(kc).await;
    assert!(msg.contains("needs an interactive login"), "{msg}");
    assert!(msg.contains("Log in from a terminal first"), "{msg}");
    Ok(())
}

/// Logs in to the kind cluster with a ServiceAccount token from the plugin,
/// and gets a readable message once the token is wrong.
#[tokio::test]
#[ignore]
async fn plugin_token_lists_pods_on_kind() -> Result<()> {
    use k8s_openapi::api::authentication::v1::TokenRequest;
    use k8s_openapi::api::core::v1::{Namespace, Pod, ServiceAccount};
    use k8s_openapi::api::rbac::v1::RoleBinding;
    use kube::api::{Api, DeleteParams, ListParams, PostParams};

    let ctx = std::env::var("KUBELOUPE_TEST_CONTEXT")
        .unwrap_or_else(|_| "kind-kubeloupe-test".to_string());
    let admin = kubeloupe::client_for_context(Some(&ctx)).await?;

    // The real cluster entry (server and CA) for the temp kubeconfig.
    let real = Kubeconfig::read()?;
    let cluster_name = real
        .contexts
        .iter()
        .find(|c| c.name == ctx)
        .and_then(|c| c.context.as_ref())
        .map(|c| c.cluster.clone())
        .context("test context not in kubeconfig")?;
    let cluster = real
        .clusters
        .iter()
        .find(|c| c.name == cluster_name)
        .and_then(|c| c.cluster.clone())
        .context("test cluster not in kubeconfig")?;
    let cluster = serde_json::to_value(cluster)?;

    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("kl-auth-{suffix:x}");
    let namespaces: Api<Namespace> = Api::all(admin.clone());
    namespaces
        .create(
            &PostParams::default(),
            &serde_json::from_value(serde_json::json!({"metadata": {"name": ns}}))?,
        )
        .await?;

    let result = async {
        let pp = PostParams::default();
        let sas: Api<ServiceAccount> = Api::namespaced(admin.clone(), &ns);
        sas.create(
            &pp,
            &serde_json::from_value(serde_json::json!({"metadata": {"name": "viewer"}}))?,
        )
        .await?;
        let bindings: Api<RoleBinding> = Api::namespaced(admin.clone(), &ns);
        bindings
            .create(
                &pp,
                &serde_json::from_value(serde_json::json!({
                    "metadata": {"name": "viewer"},
                    "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": "view"},
                    "subjects": [{"kind": "ServiceAccount", "name": "viewer", "namespace": ns}],
                }))?,
            )
            .await?;
        let token = sas
            .create_token_request(
                "viewer",
                &pp,
                &serde_json::from_value::<TokenRequest>(
                    serde_json::json!({"spec": {"audiences": [], "expirationSeconds": 600}}),
                )?,
            )
            .await?
            .status
            .context("token request has no status")?
            .token;

        let dir = plugin_dir(&credential_script(&token))?;
        let client = client_from_kubeconfig(
            kubeconfig(cluster.clone(), &dir, serde_json::json!({})),
            None,
            false,
        )
        .await?;
        let pods: Api<Pod> = Api::namespaced(client, &ns);
        pods.list(&ListParams::default()).await?;

        // A token the cluster rejects.
        let bad = plugin_dir(&credential_script("not-a-valid-token"))?;
        let msg = connect_error(kubeconfig(cluster.clone(), &bad, serde_json::json!({}))).await;
        assert!(
            msg.contains("the cluster rejected the credentials"),
            "{msg}"
        );
        anyhow::Ok(())
    }
    .await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}
