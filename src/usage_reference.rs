//! Reference lines for usage charts: a pod's requests and limits, a node's
//! allocatable.

use crate::{parse_cpu_quantity, parse_memory_quantity};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::Api;
use kube::Client;
use serde::Serialize;
use serde_json::Value;

/// What usage is compared against. 0 = not set (no request, no limit, or
/// not a pod / node).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct UsageReference {
    pub cpu_request: u64,
    pub cpu_limit: u64,
    pub memory_request: u64,
    pub memory_limit: u64,
    pub cpu_allocatable: u64,
    pub memory_allocatable: u64,
}

/// The containers that run alongside each other: the regular ones and
/// sidecars (init containers with `restartPolicy: Always`). metrics-server
/// reports these.
fn running_containers(pod: &Value) -> Vec<&Value> {
    let regular = pod
        .pointer("/spec/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let sidecars = pod
        .pointer("/spec/initContainers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c["restartPolicy"].as_str() == Some("Always"));
    regular.chain(sidecars).collect()
}

/// The pod's `field` (requests or limits) of resource `r`, summed over its
/// running containers. For limits, 0 unless every container sets one: a
/// single container without a limit makes the pod unbounded.
fn pod_total(pod: &Value, field: &str, r: &str, parse: fn(&str) -> Option<u64>) -> u64 {
    let containers = running_containers(pod);
    let mut total = 0;
    for c in &containers {
        match c["resources"][field][r].as_str().and_then(parse) {
            Some(v) => total += v,
            None if field == "limits" => return 0,
            None => {}
        }
    }
    total
}

/// A pod's requests and limits (from its spec as JSON).
pub fn pod_reference(pod: &Value) -> UsageReference {
    UsageReference {
        cpu_request: pod_total(pod, "requests", "cpu", parse_cpu_quantity),
        cpu_limit: pod_total(pod, "limits", "cpu", parse_cpu_quantity),
        memory_request: pod_total(pod, "requests", "memory", parse_memory_quantity),
        memory_limit: pod_total(pod, "limits", "memory", parse_memory_quantity),
        ..Default::default()
    }
}

/// A node's allocatable CPU and memory (from the Node as JSON).
pub fn node_reference(node: &Value) -> UsageReference {
    let alloc = |r: &str, parse: fn(&str) -> Option<u64>| {
        node.pointer(&format!("/status/allocatable/{r}"))
            .and_then(Value::as_str)
            .and_then(parse)
            .unwrap_or(0)
    };
    UsageReference {
        cpu_allocatable: alloc("cpu", parse_cpu_quantity),
        memory_allocatable: alloc("memory", parse_memory_quantity),
        ..Default::default()
    }
}

/// Fetch the reference lines of a pod (`namespace` set) or a node. A pod or
/// node that is gone has none.
pub async fn get_usage_reference(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
) -> Result<UsageReference> {
    Ok(match namespace {
        Some(ns) => match Api::<Pod>::namespaced(client.clone(), ns)
            .get_opt(name)
            .await?
        {
            Some(pod) => pod_reference(&serde_json::to_value(pod)?),
            None => UsageReference::default(),
        },
        None => match Api::<Node>::all(client.clone()).get_opt(name).await? {
            Some(node) => node_reference(&serde_json::to_value(node)?),
            None => UsageReference::default(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MI: u64 = 1024 * 1024;

    #[test]
    fn pod_requests_and_limits_are_summed() {
        let pod = json!({ "spec": {
            "containers": [
                { "name": "app", "resources": {
                    "requests": { "cpu": "250m", "memory": "64Mi" },
                    "limits": { "cpu": "1", "memory": "128Mi" } } },
                { "name": "proxy", "resources": {
                    "requests": { "cpu": "0.1", "memory": "32Mi" },
                    "limits": { "cpu": "500m", "memory": "0.25Gi" } } }
            ],
            "initContainers": [
                // Runs to completion first: not counted.
                { "name": "migrate", "resources": { "requests": { "cpu": "4" } } },
                // A sidecar runs alongside: counted.
                { "name": "log", "restartPolicy": "Always", "resources": {
                    "requests": { "cpu": "50m", "memory": "16Mi" },
                    "limits": { "cpu": "50m", "memory": "16Mi" } } }
            ]
        }});
        assert_eq!(
            pod_reference(&pod),
            UsageReference {
                cpu_request: 400,
                cpu_limit: 1550,
                memory_request: 112 * MI,
                memory_limit: 400 * MI,
                ..Default::default()
            }
        );
    }

    #[test]
    fn one_container_without_a_limit_makes_the_pod_unbounded() {
        let pod = json!({ "spec": { "containers": [
            { "name": "a", "resources": { "requests": { "memory": "64Mi" }, "limits": { "memory": "128Mi", "cpu": "1" } } },
            { "name": "b", "resources": { "limits": { "cpu": "1" } } }
        ]}});
        let r = pod_reference(&pod);
        assert_eq!(r.memory_request, 64 * MI);
        assert_eq!(r.memory_limit, 0);
        assert_eq!(r.cpu_limit, 2000);
        assert_eq!(r.cpu_request, 0);
        assert_eq!(pod_reference(&json!({})), UsageReference::default());
    }

    #[test]
    fn node_allocatable() {
        let node = json!({ "status": { "allocatable": { "cpu": "3800m", "memory": "7Gi" } } });
        let r = node_reference(&node);
        assert_eq!(r.cpu_allocatable, 3800);
        assert_eq!(r.memory_allocatable, 7 * 1024 * MI);
        assert_eq!(r.cpu_request, 0);
    }
}
