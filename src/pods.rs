//! Pod, namespace and node summaries, pod details and pod YAML.

use crate::{age_of, created_secs, resources};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use kube::api::{Api, ListParams};
use kube::Client;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodInfo {
    /// Creation time (unix seconds) so a UI can keep `age` fresh itself.
    pub created: i64,
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub node: String,
    pub age: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NamespaceInfo {
    /// Creation time (unix seconds) so a UI can keep `age` fresh itself.
    pub created: i64,
    pub name: String,
    pub status: String,
    pub age: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NodeInfo {
    /// Creation time (unix seconds) so a UI can keep `age` fresh itself.
    pub created: i64,
    pub name: String,
    pub ready: String,
    pub version: String,
    pub age: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restarts: i32,
    pub state: String,
}

/// A container (regular or init) whose logs can be read.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct LogContainer {
    pub name: String,
    pub init: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodDetail {
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub node: String,
    pub ip: String,
    pub age: String,
    pub labels: BTreeMap<String, String>,
    pub containers: Vec<ContainerInfo>,
    /// Every container in the spec, init containers first, for the log picker.
    pub log_containers: Vec<LogContainer>,
}

/// Summarize a pod for the list view.
pub fn pod_info(pod: &Pod) -> PodInfo {
    let meta = &pod.metadata;
    PodInfo {
        created: created_secs(&meta.creation_timestamp),
        name: meta.name.clone().unwrap_or_default(),
        namespace: meta.namespace.clone().unwrap_or_default(),
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
        age: age_of(meta.creation_timestamp.clone()),
    }
}

/// Summarize a namespace for the list view.
pub fn namespace_info(ns: &Namespace) -> NamespaceInfo {
    let meta = &ns.metadata;
    NamespaceInfo {
        created: created_secs(&meta.creation_timestamp),
        name: meta.name.clone().unwrap_or_default(),
        status: ns
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_default(),
        age: age_of(meta.creation_timestamp.clone()),
    }
}

/// Summarize a node for the list view.
pub fn node_info(node: &Node) -> NodeInfo {
    let meta = &node.metadata;
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
        created: created_secs(&meta.creation_timestamp),
        name: meta.name.clone().unwrap_or_default(),
        ready,
        version,
        age: age_of(meta.creation_timestamp.clone()),
    }
}

/// Fetch pods, optionally scoped to one namespace.
pub async fn get_pods(client: &Client, namespace: Option<&str>) -> Result<Vec<PodInfo>> {
    let pods: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let list = pods.list(&ListParams::default()).await?;
    Ok(list.items.iter().map(pod_info).collect())
}

/// Fetch all namespaces in the cluster.
pub async fn get_namespaces(client: &Client) -> Result<Vec<NamespaceInfo>> {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let list = namespaces.list(&ListParams::default()).await?;
    Ok(list.items.iter().map(namespace_info).collect())
}

/// Fetch all nodes in the cluster.
pub async fn get_nodes(client: &Client) -> Result<Vec<NodeInfo>> {
    let nodes: Api<Node> = Api::all(client.clone());
    let list = nodes.list(&ListParams::default()).await?;
    Ok(list.items.iter().map(node_info).collect())
}

/// Fetch one pod's details: identity, labels and per-container state.
pub async fn get_pod_details(client: &Client, namespace: &str, name: &str) -> Result<PodDetail> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pod = pods.get(name).await?;
    let meta = pod.metadata;
    let images: BTreeMap<String, String> = pod
        .spec
        .as_ref()
        .map(|s| {
            s.containers
                .iter()
                .map(|c| (c.name.clone(), c.image.clone().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let log_containers: Vec<LogContainer> = pod
        .spec
        .as_ref()
        .map(|s| {
            let init = s.init_containers.iter().flatten().map(|c| LogContainer {
                name: c.name.clone(),
                init: true,
            });
            let main = s.containers.iter().map(|c| LogContainer {
                name: c.name.clone(),
                init: false,
            });
            init.chain(main).collect()
        })
        .unwrap_or_default();
    let containers = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref())
        .map(|statuses| {
            statuses
                .iter()
                .map(|cs| {
                    let state = cs
                        .state
                        .as_ref()
                        .map(|st| {
                            if st.running.is_some() {
                                "Running".to_string()
                            } else if let Some(w) = st.waiting.as_ref() {
                                format!(
                                    "Waiting{}",
                                    w.reason
                                        .as_ref()
                                        .map(|r| format!(": {r}"))
                                        .unwrap_or_default()
                                )
                            } else if let Some(t) = st.terminated.as_ref() {
                                format!(
                                    "Terminated{}",
                                    t.reason
                                        .as_ref()
                                        .map(|r| format!(": {r}"))
                                        .unwrap_or_default()
                                )
                            } else {
                                "Unknown".to_string()
                            }
                        })
                        .unwrap_or_default();
                    ContainerInfo {
                        image: images.get(&cs.name).cloned().unwrap_or_default(),
                        name: cs.name.clone(),
                        ready: cs.ready,
                        restarts: cs.restart_count,
                        state,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(PodDetail {
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
        ip: pod
            .status
            .as_ref()
            .and_then(|s| s.pod_ip.clone())
            .unwrap_or_default(),
        age: age_of(meta.creation_timestamp),
        labels: meta.labels.unwrap_or_default(),
        containers,
        log_containers,
    })
}

// ---------------------------------------------------------------------------
// Pod YAML: thin wrappers over the generic resource YAML.
// ---------------------------------------------------------------------------

fn pod_ref() -> resources::ResourceRef {
    resources::ResourceRef {
        group: String::new(),
        version: "v1".into(),
        kind: "Pod".into(),
        plural: "pods".into(),
        namespaced: true,
    }
}

/// Fetch one pod as YAML (without managed fields).
pub async fn get_pod_yaml(client: &Client, namespace: &str, name: &str) -> Result<String> {
    resources::get_resource_yaml(client, &pod_ref(), namespace, name, false).await
}

/// Apply edited pod YAML with server-side apply (field manager "kubeloupe").
pub async fn apply_pod_yaml(
    client: &Client,
    namespace: &str,
    name: &str,
    yaml: &str,
) -> Result<Vec<resources::FieldTakeover>> {
    resources::apply_resource_yaml(client, &pod_ref(), namespace, name, yaml).await
}
