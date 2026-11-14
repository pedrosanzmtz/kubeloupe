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
    fn quantity_formatting() {
        assert_eq!(format_cpu(150), "150m");
        assert_eq!(format_cpu(1500), "1.5");
        assert_eq!(format_cpu(2000), "2");
        assert_eq!(format_memory(128 * 1024 * 1024), "128Mi");
        assert_eq!(format_memory(1024 * 1024 * 1024), "1Gi");
        assert_eq!(format_memory(512), "512B");
    }
}
