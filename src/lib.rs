use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::{AsyncBufReadExt, TryStreamExt};
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::{Api, ListParams, LogParams};
use kube::config::KubeConfigOptions;
use kube::Client;
use serde::Serialize;
use std::collections::BTreeMap;

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
    let Ok(kubeconfig) = kube::config::Kubeconfig::read() else {
        return false;
    };
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

#[derive(Debug, Clone, Serialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restarts: i32,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PodDetail {
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub node: String,
    pub ip: String,
    pub age: String,
    pub labels: BTreeMap<String, String>,
    pub containers: Vec<ContainerInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventInfo {
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub object: String,
    pub age: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub namespaces: usize,
    pub nodes: usize,
    pub nodes_ready: usize,
    pub pods_total: usize,
    pub pods_running: usize,
    pub pods_pending: usize,
    pub pods_failed: usize,
    pub pods_by_namespace: Vec<(String, usize)>,
    pub events: Vec<EventInfo>,
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
        })
        .collect())
}

/// Fetch pods, optionally scoped to one namespace.
pub async fn get_pods(context: Option<&str>, namespace: Option<&str>) -> Result<Vec<PodInfo>> {
    let client = client_for_context(context).await?;
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
pub async fn get_namespaces(context: Option<&str>) -> Result<Vec<NamespaceInfo>> {
    let client = client_for_context(context).await?;
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
pub async fn get_nodes(context: Option<&str>) -> Result<Vec<NodeInfo>> {
    let client = client_for_context(context).await?;
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

/// Fetch one pod's details: identity, labels and per-container state.
pub async fn get_pod_details(
    context: Option<&str>,
    namespace: &str,
    name: &str,
) -> Result<PodDetail> {
    let client = client_for_context(context).await?;
    let pods: Api<Pod> = Api::namespaced(client, namespace);
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
    })
}

/// Fetch the last lines of a pod's logs (non-streaming, for the CLI).
pub async fn get_pod_logs(
    context: Option<&str>,
    namespace: &str,
    name: &str,
    tail_lines: i64,
) -> Result<String> {
    let client = client_for_context(context).await?;
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let params = LogParams {
        tail_lines: Some(tail_lines),
        ..LogParams::default()
    };
    Ok(pods.logs(name, &params).await?)
}

/// Stream a pod's logs, calling `on_line` for each line. Runs until the
/// stream ends (or the caller drops the future).
pub async fn stream_pod_logs<F>(
    context: Option<&str>,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    mut on_line: F,
) -> Result<()>
where
    F: FnMut(String) + Send,
{
    let client = client_for_context(context).await?;
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let params = LogParams {
        follow: true,
        tail_lines: Some(200),
        container: container.map(str::to_string),
        ..LogParams::default()
    };
    let mut lines = pods.log_stream(name, &params).await?.lines();
    while let Some(line) = lines.try_next().await? {
        on_line(line);
    }
    Ok(())
}

/// Fetch cluster overview: counts, pods by namespace, recent events.
pub async fn get_overview(context: Option<&str>) -> Result<Overview> {
    let (pods, namespaces, nodes, events) = tokio::join!(
        get_pods(context, None),
        get_namespaces(context),
        get_nodes(context),
        get_events(context),
    );
    let pods = pods?;
    let namespaces = namespaces?;
    let nodes = nodes?;
    let events = events?;

    let mut by_namespace: BTreeMap<String, usize> = BTreeMap::new();
    for pod in &pods {
        *by_namespace.entry(pod.namespace.clone()).or_insert(0) += 1;
    }
    let mut pods_by_namespace: Vec<(String, usize)> = by_namespace.into_iter().collect();
    pods_by_namespace.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(Overview {
        namespaces: namespaces.len(),
        nodes_ready: nodes.iter().filter(|n| n.ready == "True").count(),
        nodes: nodes.len(),
        pods_running: pods.iter().filter(|p| p.phase == "Running").count(),
        pods_pending: pods.iter().filter(|p| p.phase == "Pending").count(),
        pods_failed: pods
            .iter()
            .filter(|p| p.phase == "Failed" || p.phase == "CrashLoopBackOff")
            .count(),
        pods_total: pods.len(),
        pods_by_namespace,
        events,
    })
}

async fn get_events(context: Option<&str>) -> Result<Vec<EventInfo>> {
    let client = client_for_context(context).await?;
    let events: Api<Event> = Api::all(client);
    let list = events.list(&ListParams::default()).await?;
    let mut infos: Vec<(DateTime<Utc>, EventInfo)> = list
        .items
        .into_iter()
        .map(|e| {
            let ts = e
                .last_timestamp
                .as_ref()
                .map(|t| t.0)
                .or_else(|| e.event_time.as_ref().map(|t| t.0))
                .or_else(|| e.first_timestamp.as_ref().map(|t| t.0))
                .unwrap_or_else(Utc::now);
            let object = format!(
                "{}/{}",
                e.involved_object.kind.clone().unwrap_or_default(),
                e.involved_object.name.clone().unwrap_or_default()
            );
            (
                ts,
                EventInfo {
                    kind: e.type_.clone().unwrap_or_default(),
                    reason: e.reason.clone().unwrap_or_default(),
                    message: e.message.clone().unwrap_or_default(),
                    object,
                    age: format_age(ts),
                    count: e.count.unwrap_or(1),
                },
            )
        })
        .collect();
    infos.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(infos.into_iter().take(15).map(|(_, info)| info).collect())
}

/// List pods, optionally scoped to one namespace (CLI output).
pub async fn list_pods(namespace: Option<&str>) -> Result<()> {
    let pods = get_pods(None, namespace).await?;
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
    let namespaces = get_namespaces(None).await?;
    println!("{:<30} {:<10} {}", "NAME", "STATUS", "AGE");
    for ns in namespaces {
        println!("{:<30} {:<10} {}", ns.name, ns.status, ns.age);
    }
    Ok(())
}

/// List all nodes in the cluster (CLI output).
pub async fn list_nodes() -> Result<()> {
    let nodes = get_nodes(None).await?;
    println!("{:<40} {:<12} {:<12} {}", "NAME", "READY", "VERSION", "AGE");
    for node in nodes {
        println!(
            "{:<40} {:<12} {:<12} {}",
            node.name, node.ready, node.version, node.age
        );
    }
    Ok(())
}

/// Print the last lines of a pod's logs (CLI output).
pub async fn print_pod_logs(namespace: Option<&str>, name: &str, tail: i64) -> Result<()> {
    let logs = get_pod_logs(None, namespace.unwrap_or("default"), name, tail).await?;
    print!("{logs}");
    Ok(())
}
