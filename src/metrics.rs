//! Kubernetes quantities and the metrics API (metrics.k8s.io).

use anyhow::Result;
use k8s_openapi::api::core::v1::Node;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use kube::core::GroupVersionKind;
use kube::Client;
use serde::Serialize;
use std::collections::BTreeMap;

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
    /// Per-container usage; the pod's numbers are their sum.
    pub containers: Vec<ContainerUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContainerUsage {
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

/// Fetch per-pod CPU/memory usage in `namespace` (`None` = all
/// namespaces). `Ok(None)` = metrics API unavailable.
pub async fn get_pod_metrics(
    client: &Client,
    namespace: Option<&str>,
) -> Result<Option<Vec<PodUsage>>> {
    let resource = metrics_resource("PodMetrics", "pods");
    let api: Api<DynamicObject> = match namespace {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &resource),
        None => Api::all_with(client.clone(), &resource),
    };
    let list = match api.list(&ListParams::default()).await {
        Ok(list) => list,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(list.items.into_iter().map(pod_usage).collect()))
}

/// One PodMetrics object as usage: per container, and summed.
fn pod_usage(item: DynamicObject) -> PodUsage {
    // DynamicObject splits metadata out of `data`, so the name and
    // namespace live on the item itself, not in data["metadata"].
    let name = item.metadata.name.unwrap_or_default();
    let namespace = item.metadata.namespace.unwrap_or_default();
    let containers: Vec<ContainerUsage> = item.data["containers"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| ContainerUsage {
                    name: c["name"].as_str().unwrap_or_default().to_string(),
                    cpu_millicores: c["usage"]["cpu"]
                        .as_str()
                        .and_then(parse_cpu_quantity)
                        .unwrap_or(0),
                    memory_bytes: c["usage"]["memory"]
                        .as_str()
                        .and_then(parse_memory_quantity)
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    PodUsage {
        namespace,
        name,
        cpu_millicores: containers.iter().map(|c| c.cpu_millicores).sum(),
        memory_bytes: containers.iter().map(|c| c.memory_bytes).sum(),
        containers,
    }
}

/// Usage of one pod. `Ok(None)` = the metrics API is unavailable or has no
/// sample for it yet (a new pod).
pub async fn get_one_pod_metrics(
    client: &Client,
    namespace: &str,
    name: &str,
) -> Result<Option<PodUsage>> {
    let api: Api<DynamicObject> = Api::namespaced_with(
        client.clone(),
        namespace,
        &metrics_resource("PodMetrics", "pods"),
    );
    match api.get_opt(name).await {
        Ok(item) => Ok(item.map(pod_usage)),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Usage of one node, with its capacity. `Ok(None)` = the metrics API is
/// unavailable or has no sample for it.
pub async fn get_one_node_metrics(client: &Client, name: &str) -> Result<Option<NodeUsage>> {
    let api: Api<DynamicObject> =
        Api::all_with(client.clone(), &metrics_resource("NodeMetrics", "nodes"));
    let item = match api.get_opt(name).await {
        Ok(Some(item)) => item,
        Ok(None) => return Ok(None),
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (_, cpu, memory) = node_usage_of(item);
    let (cpu_cap, mem_cap) = match Api::<Node>::all(client.clone()).get_opt(name).await {
        Ok(Some(node)) => node_capacity(&node),
        _ => (0, 0),
    };
    Ok(Some(NodeUsage {
        name: name.to_string(),
        cpu_millicores: cpu,
        cpu_capacity_millicores: cpu_cap,
        memory_bytes: memory,
        memory_capacity_bytes: mem_cap,
    }))
}

/// A NodeMetrics object as (name, cpu millicores, memory bytes).
fn node_usage_of(item: DynamicObject) -> (String, u64, u64) {
    let usage = &item.data["usage"];
    let cpu = usage["cpu"]
        .as_str()
        .and_then(parse_cpu_quantity)
        .unwrap_or(0);
    let memory = usage["memory"]
        .as_str()
        .and_then(parse_memory_quantity)
        .unwrap_or(0);
    (item.metadata.name.unwrap_or_default(), cpu, memory)
}

/// A node's (cpu millicores, memory bytes) capacity.
fn node_capacity(node: &Node) -> (u64, u64) {
    let Some(cap) = node.status.as_ref().and_then(|s| s.capacity.as_ref()) else {
        return (0, 0);
    };
    let cpu = cap
        .get("cpu")
        .and_then(|q| parse_cpu_quantity(&q.0))
        .unwrap_or(0);
    let memory = cap
        .get("memory")
        .and_then(|q| parse_memory_quantity(&q.0))
        .unwrap_or(0);
    (cpu, memory)
}

/// Each node's (name, cpu millicores, memory bytes) from NodeMetrics only.
/// `Ok(None)` = metrics API unavailable.
async fn list_node_usage(client: &Client) -> Result<Option<Vec<(String, u64, u64)>>> {
    let api: Api<DynamicObject> =
        Api::all_with(client.clone(), &metrics_resource("NodeMetrics", "nodes"));
    let list = match api.list(&ListParams::default()).await {
        Ok(list) => list,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let usage = list.items.into_iter().map(node_usage_of).collect();
    Ok(Some(usage))
}

/// Total (cpu millicores, memory bytes) used by all nodes, from NodeMetrics
/// alone (one small object per node). `Ok(None)` = metrics API unavailable.
pub async fn get_node_usage_total(client: &Client) -> Result<Option<(u64, u64)>> {
    Ok(list_node_usage(client).await?.map(|nodes| {
        nodes
            .iter()
            .fold((0, 0), |(c, m), (_, cpu, mem)| (c + cpu, m + mem))
    }))
}

/// Fetch per-node CPU/memory usage with capacity. `Ok(None)` = metrics API
/// unavailable.
pub async fn get_node_metrics(client: &Client) -> Result<Option<Vec<NodeUsage>>> {
    let Some(usage) = list_node_usage(client).await? else {
        return Ok(None);
    };
    let nodes: Api<Node> = Api::all(client.clone());
    let mut capacities: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    if let Ok(node_list) = nodes.list(&ListParams::default()).await {
        for node in node_list.items {
            let caps = node_capacity(&node);
            capacities.insert(node.metadata.name.unwrap_or_default(), caps);
        }
    }
    let usages = usage
        .into_iter()
        .map(|(name, cpu, memory)| {
            let (cpu_cap, mem_cap) = capacities.get(&name).copied().unwrap_or((0, 0));
            NodeUsage {
                name,
                cpu_millicores: cpu,
                cpu_capacity_millicores: cpu_cap,
                memory_bytes: memory,
                memory_capacity_bytes: mem_cap,
            }
        })
        .collect();
    Ok(Some(usages))
}

#[cfg(test)]
mod tests {
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
    fn pod_metrics_keep_their_containers() {
        let item: DynamicObject = serde_json::from_value(serde_json::json!({
            "apiVersion": "metrics.k8s.io/v1beta1",
            "kind": "PodMetrics",
            "metadata": { "name": "web", "namespace": "demo" },
            "containers": [
                { "name": "app", "usage": { "cpu": "120m", "memory": "64Mi" } },
                { "name": "proxy", "usage": { "cpu": "3000000n", "memory": "1Mi" } }
            ]
        }))
        .unwrap();
        let u = pod_usage(item);
        assert_eq!((u.namespace.as_str(), u.name.as_str()), ("demo", "web"));
        assert_eq!(u.cpu_millicores, 123);
        assert_eq!(u.memory_bytes, 65 * 1024 * 1024);
        assert_eq!(u.containers.len(), 2);
        assert_eq!(u.containers[1].name, "proxy");
        assert_eq!(u.containers[1].cpu_millicores, 3);
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
}
