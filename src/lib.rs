use anyhow::Result;
use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::{Api, Client};
use kube::api::ListParams;
use serde::Serialize;

/// Build a client from the current kubeconfig (or in-cluster config).
pub async fn client() -> Result<Client> {
    Ok(Client::try_default().await?)
}

fn age_of(ts: Option<Time>) -> String {
    ts.map(|t| format_age(t.0)).unwrap_or_default()
}

fn format_age(t: DateTime<Utc>) -> String {
    let secs = (Utc::now() - t).num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PodInfo {
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub node: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NamespaceInfo {
    pub name: String,
    pub status: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeInfo {
    pub name: String,
    pub ready: String,
    pub version: String,
    pub age: String,
}

/// Fetch pods, optionally scoped to one namespace.
pub async fn get_pods(namespace: Option<&str>) -> Result<Vec<PodInfo>> {
    let client = client().await?;
    let pods: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client, ns),
        None => Api::all(client),
    };
    let list = pods.list(&ListParams::default()).await?;
    Ok(list
        .items
        .into_iter()
        .map(|pod| {
            let meta = pod.metadata;
            PodInfo {
                name: meta.name.unwrap_or_default(),
                namespace: meta.namespace.unwrap_or_default(),
                phase: pod
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_default(),
                node: pod
                    .spec
                    .as_ref()
                    .and_then(|s| s.node_name.clone())
                    .unwrap_or_default(),
                age: age_of(meta.creation_timestamp),
            }
        })
        .collect())
}

/// Fetch all namespaces in the cluster.
pub async fn get_namespaces() -> Result<Vec<NamespaceInfo>> {
    let client = client().await?;
    let namespaces: Api<Namespace> = Api::all(client);
    let list = namespaces.list(&ListParams::default()).await?;
    Ok(list
        .items
        .into_iter()
        .map(|ns| {
            let meta = ns.metadata;
            NamespaceInfo {
                name: meta.name.unwrap_or_default(),
                status: ns
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_default(),
                age: age_of(meta.creation_timestamp),
            }
        })
        .collect())
}

/// Fetch all nodes in the cluster.
pub async fn get_nodes() -> Result<Vec<NodeInfo>> {
    let client = client().await?;
    let nodes: Api<Node> = Api::all(client);
    let list = nodes.list(&ListParams::default()).await?;
    Ok(list
        .items
        .into_iter()
        .map(|node| {
            let meta = node.metadata;
            let ready = node
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|conds| conds.iter().find(|c| c.type_ == "Ready"))
                .map(|c| c.status.clone())
                .unwrap_or_default();
            let version = node
                .status
                .as_ref()
                .and_then(|s| s.node_info.as_ref())
                .map(|info| info.kubelet_version.clone())
                .unwrap_or_default();
            NodeInfo {
                name: meta.name.unwrap_or_default(),
                ready,
                version,
                age: age_of(meta.creation_timestamp),
            }
        })
        .collect())
}

/// List pods, optionally scoped to one namespace (CLI output).
pub async fn list_pods(namespace: Option<&str>) -> Result<()> {
    let pods = get_pods(namespace).await?;
    println!("{:<50} {:<20} {:<12} {:<10} {}", "NAME", "NAMESPACE", "PHASE", "NODE", "AGE");
    for pod in pods {
        println!(
            "{:<50} {:<20} {:<12} {:<10} {}",
            pod.name, pod.namespace, pod.phase, pod.node, pod.age
        );
    }
    Ok(())
}

/// List all namespaces in the cluster (CLI output).
pub async fn list_namespaces() -> Result<()> {
    let namespaces = get_namespaces().await?;
    println!("{:<30} {:<10} {}", "NAME", "STATUS", "AGE");
    for ns in namespaces {
        println!("{:<30} {:<10} {}", ns.name, ns.status, ns.age);
    }
    Ok(())
}

/// List all nodes in the cluster (CLI output).
pub async fn list_nodes() -> Result<()> {
    let nodes = get_nodes().await?;
    println!("{:<40} {:<12} {:<12} {}", "NAME", "READY", "VERSION", "AGE");
    for node in nodes {
        println!(
            "{:<40} {:<12} {:<12} {}",
            node.name, node.ready, node.version, node.age
        );
    }
    Ok(())
}
