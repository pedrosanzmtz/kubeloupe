//! Building a client for a kubeconfig context, and listing contexts.

use anyhow::Result;
use kube::config::KubeConfigOptions;
use kube::Client;
use serde::Serialize;

/// Build a client for the given context (`None` = the kubeconfig's
/// current-context), or from in-cluster config.
pub async fn client_for_context(context: Option<&str>) -> Result<Client> {
    let mut config = match context {
        Some(name) => {
            kube::Config::from_kubeconfig(&KubeConfigOptions {
                context: Some(name.to_string()),
                cluster: None,
                user: None,
            })
            .await?
        }
        None => kube::Config::infer().await?,
    };
    if !kubeconfig_sets_proxy(context) {
        // kube-rs takes HTTPS_PROXY from the environment as the cluster
        // proxy and has no no_proxy support, so on a machine with a proxy
        // configured it would tunnel even local cluster addresses through
        // the proxy and fail. Only honour a proxy the kubeconfig sets
        // explicitly for this cluster.
        config.proxy_url = None;
    }
    Ok(Client::try_from(config)?)
}

/// Build a client from the current kubeconfig (or in-cluster config).
pub async fn client() -> Result<Client> {
    client_for_context(None).await
}

/// Whether the kubeconfig sets a proxy-url for the given context's cluster
/// (`None` = the kubeconfig's current-context).
fn kubeconfig_sets_proxy(context: Option<&str>) -> bool {
    match kube::config::Kubeconfig::read() {
        Ok(kubeconfig) => kubeconfig_has_proxy(&kubeconfig, context),
        Err(_) => false,
    }
}

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
