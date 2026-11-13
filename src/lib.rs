// Table headers keep the literal column names inline with their widths.
#![allow(clippy::print_literal)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::{AsyncBufReadExt, TryStreamExt};
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::{
    Api, ApiResource, AttachParams, AttachedProcess, DynamicObject, ListParams, LogParams, Patch,
    PatchParams,
};
use kube::config::KubeConfigOptions;
use kube::core::GroupVersionKind;
use kube::Client;
use serde::Serialize;
use std::collections::BTreeMap;
use tokio::io::AsyncReadExt;

pub mod watch;

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

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContextInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EventInfo {
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub object: String,
    pub age: String,
    pub count: i32,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
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

fn created_secs(ts: &Option<Time>) -> i64 {
    ts.as_ref().map(|t| t.0.timestamp()).unwrap_or_default()
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

/// Options shared by the CLI and the desktop app for reading pod logs.
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// Container name; `None` lets the API server pick the default.
    pub container: Option<String>,
    /// Read the logs of the previous (crashed/restarted) container instance.
    pub previous: bool,
    /// Number of lines from the end; `None` = everything.
    pub tail_lines: Option<i64>,
    /// Only logs newer than this many seconds.
    pub since_seconds: Option<i64>,
    /// Prefix each line with its RFC3339 timestamp.
    pub timestamps: bool,
}

impl LogOptions {
    fn to_params(&self, follow: bool) -> LogParams {
        LogParams {
            follow,
            previous: self.previous,
            tail_lines: self.tail_lines,
            since_seconds: self.since_seconds,
            timestamps: self.timestamps,
            container: self.container.clone(),
            ..LogParams::default()
        }
    }
}

/// Fetch a pod's logs in one go (non-streaming, for the CLI).
pub async fn get_pod_logs(
    client: &Client,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
) -> Result<String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    Ok(pods.logs(name, &opts.to_params(false)).await?)
}

/// Stream a pod's logs, calling `on_line` for each line. Runs until the
/// stream ends (or the caller drops the future). With `previous` set the
/// container is no longer running, so the log is read once (no follow) and
/// the stream simply ends.
pub async fn stream_pod_logs<F>(
    client: &Client,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
    mut on_line: F,
) -> Result<()>
where
    F: FnMut(String) + Send,
{
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = opts.to_params(!opts.previous);
    let mut lines = pods.log_stream(name, &params).await?.lines();
    while let Some(line) = lines.try_next().await? {
        on_line(line);
    }
    Ok(())
}

/// Collect lines from `rx` into batches and hand each to `emit`. A batch is
/// flushed when it reaches `max_lines` or when `interval` has passed since its
/// first line, so a noisy pod costs one event per batch instead of per line.
/// Returns (after flushing the remainder) once all senders are dropped.
pub async fn batch_lines<F>(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    max_lines: usize,
    interval: std::time::Duration,
    mut emit: F,
) where
    F: FnMut(Vec<String>),
{
    let mut batch: Vec<String> = Vec::new();
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let sleep = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            line = rx.recv() => match line {
                Some(line) => {
                    if batch.is_empty() {
                        deadline = Some(tokio::time::Instant::now() + interval);
                    }
                    batch.push(line);
                    if batch.len() >= max_lines {
                        deadline = None;
                        emit(std::mem::take(&mut batch));
                    }
                }
                None => break,
            },
            _ = sleep => {
                deadline = None;
                if !batch.is_empty() {
                    emit(std::mem::take(&mut batch));
                }
            }
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
}

/// Fetch cluster overview: counts, pods by namespace, recent events.
pub async fn get_overview(client: &Client) -> Result<Overview> {
    let (pods, namespaces, nodes, events) = tokio::join!(
        get_pods(client, None),
        get_namespaces(client),
        get_nodes(client),
        get_events(client),
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
    pods_by_namespace.sort_by_key(|b| std::cmp::Reverse(b.1));

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

async fn get_events(client: &Client) -> Result<Vec<EventInfo>> {
    let events: Api<Event> = Api::all(client.clone());
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
    infos.sort_by_key(|b| std::cmp::Reverse(b.0));
    Ok(infos.into_iter().take(15).map(|(_, info)| info).collect())
}

/// List pods, optionally scoped to one namespace (CLI output).
pub async fn list_pods(client: &Client, namespace: Option<&str>) -> Result<()> {
    let pods = get_pods(client, namespace).await?;
    println!(
        "{:<50} {:<20} {:<12} {:<10} {}",
        "NAME", "NAMESPACE", "PHASE", "NODE", "AGE"
    );
    for pod in pods {
        println!(
            "{:<50} {:<20} {:<12} {:<10} {}",
            pod.name, pod.namespace, pod.phase, pod.node, pod.age
        );
    }
    Ok(())
}

/// List all namespaces in the cluster (CLI output).
pub async fn list_namespaces(client: &Client) -> Result<()> {
    let namespaces = get_namespaces(client).await?;
    println!("{:<30} {:<10} {}", "NAME", "STATUS", "AGE");
    for ns in namespaces {
        println!("{:<30} {:<10} {}", ns.name, ns.status, ns.age);
    }
    Ok(())
}

/// List all nodes in the cluster (CLI output).
pub async fn list_nodes(client: &Client) -> Result<()> {
    let nodes = get_nodes(client).await?;
    println!("{:<40} {:<12} {:<12} {}", "NAME", "READY", "VERSION", "AGE");
    for node in nodes {
        println!(
            "{:<40} {:<12} {:<12} {}",
            node.name, node.ready, node.version, node.age
        );
    }
    Ok(())
}

/// Print a pod's logs (CLI output). With `follow`, keeps streaming until
/// interrupted.
pub async fn print_pod_logs(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
    opts: &LogOptions,
    follow: bool,
) -> Result<()> {
    let namespace = namespace.unwrap_or("default");
    if follow {
        anyhow::ensure!(
            !opts.previous,
            "--follow cannot be combined with --previous"
        );
        stream_pod_logs(client, namespace, name, opts, |line| println!("{line}")).await
    } else {
        let logs = get_pod_logs(client, namespace, name, opts).await?;
        print!("{logs}");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Kubernetes quantities ("150m", "128Mi") — parsing and formatting.
// ---------------------------------------------------------------------------

/// Parse a CPU quantity into millicores: "150m" -> 150, "1" -> 1000,
/// "2500n" -> 0, "0.5" -> 500.
pub fn parse_cpu_quantity(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(v) = s.strip_suffix('n') {
        v.parse::<f64>()
            .ok()
            .map(|n| (n / 1_000_000.0).round() as u64)
    } else if let Some(v) = s.strip_suffix('u') {
        v.parse::<f64>().ok().map(|u| (u / 1_000.0).round() as u64)
    } else if let Some(v) = s.strip_suffix('m') {
        v.parse::<f64>().ok().map(|m| m.round() as u64)
    } else {
        s.parse::<f64>().ok().map(|c| (c * 1000.0).round() as u64)
    }
}

/// Parse a memory quantity into bytes: "128Mi" -> 134217728, "1Gi",
/// "512Ki", "100M" (decimal), "123456" (plain bytes).
pub fn parse_memory_quantity(s: &str) -> Option<u64> {
    let s = s.trim();
    for (suffix, mult) in [
        ("Ki", 1024u64),
        ("Mi", 1024u64.pow(2)),
        ("Gi", 1024u64.pow(3)),
        ("Ti", 1024u64.pow(4)),
        ("Pi", 1024u64.pow(5)),
        ("Ei", 1024u64.pow(6)),
        ("K", 1000u64),
        ("M", 1000u64.pow(2)),
        ("G", 1000u64.pow(3)),
        ("T", 1000u64.pow(4)),
        ("P", 1000u64.pow(5)),
        ("E", 1000u64.pow(6)),
    ] {
        if let Some(num) = s.strip_suffix(suffix) {
            return num
                .parse::<f64>()
                .ok()
                .map(|v| (v * mult as f64).round() as u64);
        }
    }
    s.parse::<f64>().ok().map(|v| v.round() as u64)
}

/// Format millicores for display: 150 -> "150m", 1500 -> "1.5".
pub fn format_cpu(millicores: u64) -> String {
    if millicores >= 1000 {
        let cores = millicores as f64 / 1000.0;
        let text = format!("{cores:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{millicores}m")
    }
}

/// Format bytes for display: 134217728 -> "128Mi".
pub fn format_memory(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "Ki", "Mi", "Gi", "Ti", "Pi"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        let text = format!("{value:.2}");
        format!(
            "{}{}",
            text.trim_end_matches('0').trim_end_matches('.'),
            UNITS[unit]
        )
    }
}

// ---------------------------------------------------------------------------
// Metrics API (metrics.k8s.io/v1beta1, served by metrics-server).
// Returns `Ok(None)` when the metrics API is not installed on the cluster.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodUsage {
    pub namespace: String,
    pub name: String,
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NodeUsage {
    pub name: String,
    pub cpu_millicores: u64,
    pub cpu_capacity_millicores: u64,
    pub memory_bytes: u64,
    pub memory_capacity_bytes: u64,
}

fn metrics_resource(kind: &str, plural: &str) -> ApiResource {
    ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", kind),
        plural,
    )
}

fn is_not_found(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(resp) if resp.code == 404)
}

/// Fetch per-pod CPU/memory usage. `Ok(None)` = metrics API unavailable.
pub async fn get_pod_metrics(client: &Client) -> Result<Option<Vec<PodUsage>>> {
    let api: Api<DynamicObject> =
        Api::all_with(client.clone(), &metrics_resource("PodMetrics", "pods"));
    let list = match api.list(&ListParams::default()).await {
        Ok(list) => list,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut usages = Vec::new();
    for item in list.items {
        // DynamicObject splits metadata out of `data`, so the name and
        // namespace live on the item itself, not in data["metadata"].
        let name = item.metadata.name.unwrap_or_default();
        let namespace = item.metadata.namespace.unwrap_or_default();
        let data = &item.data;
        let mut cpu = 0u64;
        let mut memory = 0u64;
        if let Some(containers) = data["containers"].as_array() {
            for c in containers {
                cpu += c["usage"]["cpu"]
                    .as_str()
                    .and_then(parse_cpu_quantity)
                    .unwrap_or(0);
                memory += c["usage"]["memory"]
                    .as_str()
                    .and_then(parse_memory_quantity)
                    .unwrap_or(0);
            }
        }
        usages.push(PodUsage {
            namespace,
            name,
            cpu_millicores: cpu,
            memory_bytes: memory,
        });
    }
    Ok(Some(usages))
}

/// Fetch per-node CPU/memory usage with capacity. `Ok(None)` = metrics API
/// unavailable.
pub async fn get_node_metrics(client: &Client) -> Result<Option<Vec<NodeUsage>>> {
    let api: Api<DynamicObject> =
        Api::all_with(client.clone(), &metrics_resource("NodeMetrics", "nodes"));
    let list = match api.list(&ListParams::default()).await {
        Ok(list) => list,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let nodes: Api<Node> = Api::all(client.clone());
    let mut capacities: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    if let Ok(node_list) = nodes.list(&ListParams::default()).await {
        for node in node_list.items {
            let name = node.metadata.name.unwrap_or_default();
            let (mut cpu_cap, mut mem_cap) = (0u64, 0u64);
            if let Some(cap) = node.status.and_then(|s| s.capacity) {
                cpu_cap = cap
                    .get("cpu")
                    .map(|q| parse_cpu_quantity(&q.0).unwrap_or(0))
                    .unwrap_or(0);
                mem_cap = cap
                    .get("memory")
                    .map(|q| parse_memory_quantity(&q.0).unwrap_or(0))
                    .unwrap_or(0);
            }
            capacities.insert(name, (cpu_cap, mem_cap));
        }
    }
    let mut usages = Vec::new();
    for item in list.items {
        let name = item.metadata.name.unwrap_or_default();
        let data = &item.data;
        let cpu = data["usage"]["cpu"]
            .as_str()
            .and_then(parse_cpu_quantity)
            .unwrap_or(0);
        let memory = data["usage"]["memory"]
            .as_str()
            .and_then(parse_memory_quantity)
            .unwrap_or(0);
        let (cpu_cap, mem_cap) = capacities.get(&name).copied().unwrap_or((0, 0));
        usages.push(NodeUsage {
            name,
            cpu_millicores: cpu,
            cpu_capacity_millicores: cpu_cap,
            memory_bytes: memory,
            memory_capacity_bytes: mem_cap,
        });
    }
    Ok(Some(usages))
}

// ---------------------------------------------------------------------------
// Pod YAML: read a pod as YAML, apply edited YAML with server-side apply.
// ---------------------------------------------------------------------------

/// Fetch one pod as YAML (status and managed fields stripped, so the
/// document is what a human would edit).
pub async fn get_pod_yaml(client: &Client, namespace: &str, name: &str) -> Result<String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let mut pod = pods.get(name).await?;
    pod.metadata.managed_fields = None;
    pod.status = None;
    // k8s-openapi already serializes apiVersion/kind for the Pod.
    Ok(serde_yaml::to_string(&pod)?)
}

/// Apply edited pod YAML with server-side apply (field manager "kubeloupe").
pub async fn apply_pod_yaml(
    client: &Client,
    namespace: &str,
    name: &str,
    yaml: &str,
) -> Result<()> {
    let value: serde_json::Value = serde_yaml::from_str(yaml)?;
    let ar = ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod"));
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
    api.patch(
        name,
        &PatchParams::apply("kubeloupe"),
        &Patch::Apply(&value),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pod exec.
// ---------------------------------------------------------------------------

/// Open an interactive TTY shell (`/bin/sh`) in a pod. The caller drives
/// stdin/stdout/stderr and the terminal size on the returned process.
pub async fn exec_pod(
    client: &Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
) -> Result<AttachedProcess> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let mut params = AttachParams::interactive_tty();
    params.container = container.map(str::to_string);
    Ok(pods.exec(name, ["/bin/sh"], &params).await?)
}

/// Result of a non-interactive command run in a pod.
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Run a command in a pod (non-TTY) and collect its output.
pub async fn exec_command(
    client: &Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    command: &[String],
) -> Result<ExecResult> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = AttachParams {
        container: container.map(str::to_string),
        stdin: false,
        stdout: true,
        stderr: true,
        tty: false,
        ..AttachParams::default()
    };
    let mut attached = pods.exec(name, command, &params).await?;
    let stdout_reader = attached.stdout();
    let stderr_reader = attached.stderr();
    // join() returns Ok even when the command fails — the failure is only
    // visible on the status channel, so read it too.
    let status_fut = attached.take_status();
    let read_stdout = async {
        match stdout_reader {
            Some(mut r) => {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf).await;
                buf
            }
            None => String::new(),
        }
    };
    let read_stderr = async {
        match stderr_reader {
            Some(mut r) => {
                let mut buf = String::new();
                let _ = r.read_to_string(&mut buf).await;
                buf
            }
            None => String::new(),
        }
    };
    let read_status = async {
        match status_fut {
            Some(f) => f.await,
            None => None,
        }
    };
    let (stdout, mut stderr, joined, status) =
        tokio::join!(read_stdout, read_stderr, attached.join(), read_status);
    let mut success = joined.is_ok();
    if let Some(s) = status {
        if s.status.as_deref() == Some("Failure") {
            success = false;
            if let Some(msg) = s.message {
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(&msg);
            }
        }
    }
    Ok(ExecResult {
        stdout,
        stderr,
        success,
    })
}

// ---------------------------------------------------------------------------
// Port-forwarding. kube-rs's Portforwarder hands out a raw stream per port
// and binds nothing itself, so this binds a local TCP listener and, for each
// incoming connection, opens a fresh port-forward (the kubectl semantics,
// which also lets several clients connect at once).
// ---------------------------------------------------------------------------

/// Start forwarding a pod port to an ephemeral localhost port. Returns the
/// local port and a handle; aborting the handle stops the listener.
/// Fails fast when the API server cannot forward (e.g. no kubelet).
pub async fn serve_port_forward(
    client: &Client,
    namespace: &str,
    name: &str,
    pod_port: u16,
) -> Result<(u16, tokio::task::JoinHandle<()>)> {
    // Probe: fail fast instead of only failing when a client connects.
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let mut probe = pods.portforward(name, &[pod_port]).await?;
    let _ = probe.take_stream(pod_port);
    probe.abort();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_port = listener.local_addr()?.port();
    let client = client.clone();
    let ns_owned = namespace.to_string();
    let name_owned = name.to_string();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let client = client.clone();
            let ns = ns_owned.clone();
            let pod = name_owned.clone();
            tokio::spawn(async move {
                let pods: Api<Pod> = Api::namespaced(client, &ns);
                let Ok(mut pf) = pods.portforward(&pod, &[pod_port]).await else {
                    return;
                };
                if let Some(mut stream) = pf.take_stream(pod_port) {
                    let mut socket = socket;
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
            });
        }
    });
    Ok((local_port, handle))
}

// ---------------------------------------------------------------------------
// CLI output for the new commands.
// ---------------------------------------------------------------------------

/// Print `kubectl top pods`-style usage (CLI output).
pub async fn print_top_pods(client: &Client) -> Result<()> {
    match get_pod_metrics(client).await? {
        None => {
            println!("Metrics API not available on this cluster (is metrics-server installed?)")
        }
        Some(usages) => {
            println!(
                "{:<50} {:<20} {:<10} {}",
                "NAME", "NAMESPACE", "CPU", "MEMORY"
            );
            for u in usages {
                println!(
                    "{:<50} {:<20} {:<10} {}",
                    u.name,
                    u.namespace,
                    format_cpu(u.cpu_millicores),
                    format_memory(u.memory_bytes)
                );
            }
        }
    }
    Ok(())
}

/// Print `kubectl top nodes`-style usage (CLI output).
pub async fn print_top_nodes(client: &Client) -> Result<()> {
    match get_node_metrics(client).await? {
        None => {
            println!("Metrics API not available on this cluster (is metrics-server installed?)")
        }
        Some(usages) => {
            println!("{:<40} {:<16} {}", "NAME", "CPU", "MEMORY");
            for u in usages {
                let cpu = if u.cpu_capacity_millicores > 0 {
                    format!(
                        "{}/{}",
                        format_cpu(u.cpu_millicores),
                        format_cpu(u.cpu_capacity_millicores)
                    )
                } else {
                    format_cpu(u.cpu_millicores)
                };
                let mem = if u.memory_capacity_bytes > 0 {
                    format!(
                        "{}/{}",
                        format_memory(u.memory_bytes),
                        format_memory(u.memory_capacity_bytes)
                    )
                } else {
                    format_memory(u.memory_bytes)
                };
                println!("{:<40} {:<16} {}", u.name, cpu, mem);
            }
        }
    }
    Ok(())
}

/// Print one pod as YAML (CLI output).
pub async fn print_pod_yaml(client: &Client, namespace: Option<&str>, name: &str) -> Result<()> {
    let yaml = get_pod_yaml(client, namespace.unwrap_or("default"), name).await?;
    print!("{yaml}");
    Ok(())
}

/// Forward a pod port to localhost until Ctrl-C (CLI output).
pub async fn run_port_forward(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
    port: u16,
) -> Result<()> {
    let (local_port, handle) =
        serve_port_forward(client, namespace.unwrap_or("default"), name, port).await?;
    println!("Forwarding from 127.0.0.1:{local_port} -> {name}:{port} (Ctrl-C to stop)");
    tokio::signal::ctrl_c().await?;
    handle.abort();
    println!("Port-forward stopped.");
    Ok(())
}

/// Run a command in a pod and print its output (CLI output).
pub async fn exec_in_pod(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
    container: Option<&str>,
    command: &[String],
) -> Result<()> {
    let result = exec_command(
        client,
        namespace.unwrap_or("default"),
        name,
        container,
        command,
    )
    .await?;
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    if !result.success {
        anyhow::bail!("command exited with a non-zero status");
    }
    Ok(())
}

/// Print live changes to pods, nodes or namespaces until interrupted.
pub async fn watch_and_print(
    client: &Client,
    resource: &str,
    namespace: Option<&str>,
) -> Result<()> {
    use watch::{run_batched, Output, DEFAULT_FLUSH};
    fn print<T: std::fmt::Debug>(out: Output<T>) {
        match out {
            Output::State(s) => println!("state: {s:?}"),
            Output::Batch(b) => {
                println!(
                    "{}: {} applied, {} deleted",
                    if b.snapshot { "snapshot" } else { "batch" },
                    b.applied.len(),
                    b.deleted.len()
                );
                for item in &b.applied {
                    println!("  + {item:?}");
                }
                for key in &b.deleted {
                    println!("  - {key}");
                }
            }
        }
    }
    match resource {
        "pods" => run_batched(watch::watch_pods(client, namespace).1, DEFAULT_FLUSH, print).await,
        "nodes" => run_batched(watch::watch_nodes(client).1, DEFAULT_FLUSH, print).await,
        "namespaces" => run_batched(watch::watch_namespaces(client).1, DEFAULT_FLUSH, print).await,
        other => {
            anyhow::bail!("unknown resource for watch: {other} (use pods, nodes or namespaces)")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test(start_paused = true)]
    async fn batch_lines_flushes_on_size_and_on_close() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        for i in 0..5 {
            tx.send(i.to_string()).unwrap();
        }
        drop(tx);
        let mut out: Vec<Vec<String>> = Vec::new();
        batch_lines(rx, 2, std::time::Duration::from_millis(50), |b| out.push(b)).await;
        let sizes: Vec<usize> = out.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
        assert_eq!(out[2], vec!["4".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn batch_lines_flushes_on_interval() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(batch_lines(
            rx,
            500,
            std::time::Duration::from_millis(50),
            move |b| {
                let _ = out_tx.send(b);
            },
        ));
        tx.send("a".into()).unwrap();
        tx.send("b".into()).unwrap();
        // Sender still open: only the timer can flush.
        let first = out_rx.recv().await.unwrap();
        assert_eq!(first, vec!["a".to_string(), "b".to_string()]);
        tx.send("c".into()).unwrap();
        drop(tx);
        task.await.unwrap();
        assert_eq!(out_rx.recv().await.unwrap(), vec!["c".to_string()]);
    }

    #[test]
    fn log_options_map_to_params() {
        let opts = LogOptions {
            previous: true,
            container: Some("app".into()),
            since_seconds: Some(300),
            ..LogOptions::default()
        };
        let p = opts.to_params(!opts.previous);
        assert!(p.previous && !p.follow);
        assert_eq!(p.container.as_deref(), Some("app"));
        assert_eq!(p.since_seconds, Some(300));
    }
    use super::*;

    #[test]
    fn cpu_quantities() {
        assert_eq!(parse_cpu_quantity("150m"), Some(150));
        assert_eq!(parse_cpu_quantity("1"), Some(1000));
        assert_eq!(parse_cpu_quantity("0.5"), Some(500));
        assert_eq!(parse_cpu_quantity("2"), Some(2000));
        assert_eq!(parse_cpu_quantity("2500n"), Some(0));
        assert_eq!(parse_cpu_quantity("1500u"), Some(2));
        assert_eq!(parse_cpu_quantity("bogus"), None);
    }

    #[test]
    fn memory_quantities() {
        assert_eq!(parse_memory_quantity("128Mi"), Some(128 * 1024 * 1024));
        assert_eq!(parse_memory_quantity("1Gi"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory_quantity("512Ki"), Some(512 * 1024));
        assert_eq!(parse_memory_quantity("100M"), Some(100 * 1000 * 1000));
        assert_eq!(parse_memory_quantity("123456"), Some(123456));
        assert_eq!(parse_memory_quantity("1.5Gi"), Some(1610612736));
        assert_eq!(parse_memory_quantity("bogus"), None);
    }

    #[test]
    fn quantity_formatting() {
        assert_eq!(format_cpu(150), "150m");
        assert_eq!(format_cpu(1500), "1.5");
        assert_eq!(format_cpu(2000), "2");
        assert_eq!(format_memory(128 * 1024 * 1024), "128Mi");
        assert_eq!(format_memory(1024 * 1024 * 1024), "1Gi");
        assert_eq!(format_memory(512), "512B");
    }

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

    #[test]
    fn age_formatting() {
        let ago = |s: i64| format_age(Utc::now() - chrono::Duration::seconds(s));
        assert_eq!(ago(5), "5s");
        assert_eq!(ago(120), "2m");
        assert_eq!(ago(7_300), "2h");
        assert_eq!(ago(3 * 86_400 + 10), "3d");
        // Timestamps in the future clamp to zero.
        assert_eq!(ago(-100), "0s");
        assert_eq!(age_of(None), "");
    }
}
