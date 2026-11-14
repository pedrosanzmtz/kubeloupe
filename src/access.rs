//! Working with partial RBAC: users who may only see some namespaces.
//!
//! Many accounts only have a RoleBinding in one or a few namespaces. For
//! them a cluster-wide list is forbidden (403) while the same list in their
//! own namespace works, so callers ask "forbidden, or broken?" and fall back
//! to a namespace instead of failing.

use crate::resources::can_list;
use crate::{get_namespaces, NamespaceInfo};
use anyhow::Result;
use kube::Client;

/// Whether `e` is the API server refusing the request (HTTP 403).
pub fn is_forbidden(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| match cause.downcast_ref::<kube::Error>() {
            Some(kube::Error::Api(resp)) => resp.code == 403,
            _ => false,
        })
}

/// The namespaces in the cluster, or `None` if the user may not list them.
pub async fn list_namespaces_if_allowed(client: &Client) -> Result<Option<Vec<NamespaceInfo>>> {
    match get_namespaces(client).await {
        Ok(list) => Ok(Some(list)),
        Err(e) if is_forbidden(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// How many listed namespaces [`default_namespace`] probes at most.
const MAX_PROBED_NAMESPACES: usize = 20;

/// The namespace a UI should start on: `None` (all namespaces) if the user
/// may list pods across the cluster. Otherwise the context's namespace
/// (`client.default_namespace()`, which is "default" when the kubeconfig
/// sets none) if pods may be listed there, else the first listable
/// namespace where they may, else the context's namespace anyway.
pub async fn default_namespace(client: &Client) -> Option<String> {
    if can_list(client, "", "pods", None).await {
        return None;
    }
    let own = client.default_namespace().to_string();
    if can_list(client, "", "pods", Some(&own)).await {
        return Some(own);
    }
    if let Ok(Some(list)) = list_namespaces_if_allowed(client).await {
        for ns in list.iter().take(MAX_PROBED_NAMESPACES) {
            if can_list(client, "", "pods", Some(&ns.name)).await {
                return Some(ns.name.clone());
            }
        }
    }
    Some(own)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::core::ErrorResponse;

    fn api_error(code: u16) -> anyhow::Error {
        kube::Error::Api(ErrorResponse {
            status: "Failure".into(),
            message: "nope".into(),
            reason: "Forbidden".into(),
            code,
        })
        .into()
    }

    #[test]
    fn forbidden_detection() {
        assert!(is_forbidden(&api_error(403)));
        assert!(is_forbidden(&api_error(403).context("listing pods")));
        assert!(!is_forbidden(&api_error(404)));
        assert!(!is_forbidden(&anyhow::anyhow!("connection refused")));
    }
}
