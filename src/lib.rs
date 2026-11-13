use anyhow::Result;
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use kube::{Api, Client};
use kube::api::ListParams;

/// Build a client from the current kubeconfig (or in-cluster config).
pub async fn client() -> Result<Client> {
    Ok(Client::try_default().await?)
}

/// List pods, optionally scoped to one namespace.
pub async fn list_pods(namespace: Option<&str>) -> Result<()> {
    let client = client().await?;
    let pods: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client, ns),
        None => Api::all(client),
    };
    let list = pods.list(&ListParams::default()).await?;
    println!("{:<50} {:<20} {:<12} {}", "NAME", "NAMESPACE", "PHASE", "NODE");
    for pod in list {
        let meta = pod.metadata;
        let name = meta.name.unwrap_or_default();
        let ns = meta.namespace.unwrap_or_default();
        let phase = pod.status.and_then(|s| s.phase).unwrap_or_default();
        let node = pod.spec.and_then(|s| s.node_name).unwrap_or_default();
        println!("{:<50} {:<20} {:<12} {}", name, ns, phase, node);
    }
    Ok(())
}

/// List all namespaces in the cluster.
pub async fn list_namespaces() -> Result<()> {
    let client = client().await?;
    let namespaces: Api<Namespace> = Api::all(client);
    let list = namespaces.list(&ListParams::default()).await?;
    println!("{:<30} {}", "NAME", "STATUS");
    for ns in list {
        let name = ns.metadata.name.unwrap_or_default();
        let status = ns
            .status
            .and_then(|s| s.phase)
            .unwrap_or_default();
        println!("{:<30} {}", name, status);
    }
    Ok(())
}

/// List all nodes in the cluster.
pub async fn list_nodes() -> Result<()> {
    let client = client().await?;
    let nodes: Api<Node> = Api::all(client);
    let list = nodes.list(&ListParams::default()).await?;
    println!("{:<40} {:<12} {}", "NAME", "STATUS", "VERSION");
    for node in list {
        let name = node.metadata.name.unwrap_or_default();
        let ready = node
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .and_then(|conds| conds.iter().find(|c| c.type_ == "Ready"))
            .map(|c| c.status.clone())
            .unwrap_or_default();
        let version = node
            .status
            .and_then(|s| s.node_info)
            .map(|info| info.kubelet_version)
            .unwrap_or_default();
        println!("{:<40} {:<12} {}", name, ready, version);
    }
    Ok(())
}
