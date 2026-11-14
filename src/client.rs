//! Building a client for a kubeconfig context, and listing contexts.

use crate::auth::{describe_client_error, prepare_exec, AuthTarget};
use anyhow::{anyhow, Result};
use kube::config::KubeConfigOptions;
use kube::Client;
use serde::Serialize;
use std::io::IsTerminal;

/// Build a client for the given context (`None` = the kubeconfig's
/// current-context), or from in-cluster config.
///
/// Exec credential plugins may prompt only when stdin is a terminal; use
/// [`client_for_context_with`] to decide that yourself.
pub async fn client_for_context(context: Option<&str>) -> Result<Client> {
    client_for_context_with(context, std::io::stdin().is_terminal()).await
}

/// Build a client for the given context and check that it can log in.
///
/// `interactive` says whether an exec credential plugin may prompt the user
/// (it then inherits our stdin and stderr). The desktop app passes false.
///
/// The client makes one request (`/version`) before it is returned, so a
/// missing plugin, rejected credentials or a certificate problem fail here,
/// with a readable message naming the context (see [`crate::auth`]).
pub async fn client_for_context_with(context: Option<&str>, interactive: bool) -> Result<Client> {
    match (kube::config::Kubeconfig::read(), context) {
        (Ok(kubeconfig), _) => client_from_kubeconfig(kubeconfig, context, interactive).await,
        // A named context needs a kubeconfig.
        (Err(e), Some(_)) => Err(e.into()),
        // No kubeconfig: in-cluster config.
        (Err(_), None) => {
            let config = kube::Config::infer().await?;
            connect(config, "in-cluster".to_string(), interactive).await
        }
    }
}

/// [`client_for_context_with`] for a kubeconfig the caller loaded.
pub async fn client_from_kubeconfig(
    kubeconfig: kube::config::Kubeconfig,
    context: Option<&str>,
    interactive: bool,
) -> Result<Client> {
    let context_name = match context {
        Some(name) => name.to_string(),
        None => kubeconfig.current_context.clone().unwrap_or_default(),
    };
    let has_proxy = kubeconfig_has_proxy(&kubeconfig, context);
    let mut config = kube::Config::from_custom_kubeconfig(
        kubeconfig,
        &KubeConfigOptions {
            context: context.map(str::to_string),
            cluster: None,
            user: None,
        },
    )
    .await?;
    if !has_proxy {
        // kube-rs takes HTTPS_PROXY from the environment as the cluster
        // proxy and has no no_proxy support, so on a machine with a proxy
        // configured it would tunnel even local cluster addresses through
        // the proxy and fail. Only honour a proxy the kubeconfig sets
        // explicitly for this cluster.
        config.proxy_url = None;
    }
    connect(config, context_name, interactive).await
}

/// Build the client, run its exec plugin and make the first request.
async fn connect(mut config: kube::Config, context: String, interactive: bool) -> Result<Client> {
    let exec = config.auth_info.exec.as_ref();
    let target = AuthTarget {
        context,
        server: config.cluster_url.to_string(),
        exec_command: exec.and_then(|e| e.command.clone()),
        // The plugin's own `env` may set the PATH it is looked up in.
        exec_path: exec
            .and_then(|e| e.env.as_ref())
            .and_then(|env| {
                env.iter()
                    .find(|v| v.get("name").map(String::as_str) == Some("PATH"))
            })
            .and_then(|v| v.get("value").cloned()),
    };
    if let Some(exec) = config.auth_info.exec.as_mut() {
        prepare_exec(exec, interactive, &target).map_err(|msg| anyhow!(msg))?;
    }
    // Building the client runs the exec plugin, if any.
    let client =
        Client::try_from(config).map_err(|e| anyhow!(describe_client_error(&e, &target)))?;
    client
        .apiserver_version()
        .await
        .map_err(|e| anyhow!(describe_client_error(&e, &target)))?;
    Ok(client)
}

/// Build a client from the current kubeconfig (or in-cluster config).
pub async fn client() -> Result<Client> {
    client_for_context(None).await
}

/// Whether the kubeconfig sets a proxy-url for the given context's cluster
/// (`None` = the kubeconfig's current-context).
fn kubeconfig_has_proxy(kubeconfig: &kube::config::Kubeconfig, context: Option<&str>) -> bool {
    let current = match context {
        Some(name) => name.to_string(),
        None => match kubeconfig.current_context.clone() {
            Some(name) => name,
            None => return false,
        },
    };
    let Some(cluster_name) = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == current)
        .and_then(|c| c.context.as_ref())
        .map(|ctx| ctx.cluster.clone())
    else {
        return false;
    };
    kubeconfig
        .clusters
        .iter()
        .find(|c| c.name == cluster_name)
        .and_then(|c| c.cluster.as_ref())
        .map(|cluster| cluster.proxy_url.is_some())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContextInfo {
    pub name: String,
    pub current: bool,
    /// The context's default namespace (`contexts[].context.namespace`).
    pub namespace: Option<String>,
}

/// List the contexts in the kubeconfig and which one is current.
pub async fn list_contexts() -> Result<Vec<ContextInfo>> {
    let kubeconfig = kube::config::Kubeconfig::read()?;
    let current = kubeconfig.current_context.clone().unwrap_or_default();
    Ok(kubeconfig
        .contexts
        .iter()
        .map(|c| ContextInfo {
            name: c.name.clone(),
            current: c.name == current,
            namespace: c.context.as_ref().and_then(|ctx| ctx.namespace.clone()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_kubeconfig() -> kube::config::Kubeconfig {
        kube::config::Kubeconfig::from_yaml(
            r#"
apiVersion: v1
kind: Config
current-context: proxied
contexts:
- name: proxied
  context: {cluster: with-proxy, user: u}
- name: direct
  context: {cluster: no-proxy, user: u}
clusters:
- name: with-proxy
  cluster: {server: "https://a.example", proxy-url: "http://127.0.0.1:8080"}
- name: no-proxy
  cluster: {server: "https://b.example"}
users:
- name: u
  user: {}
"#,
        )
        .unwrap()
    }

    #[test]
    fn proxy_detection() {
        let kc = sample_kubeconfig();
        assert!(kubeconfig_has_proxy(&kc, None));
        assert!(kubeconfig_has_proxy(&kc, Some("proxied")));
        assert!(!kubeconfig_has_proxy(&kc, Some("direct")));
        assert!(!kubeconfig_has_proxy(&kc, Some("missing")));
    }
}
