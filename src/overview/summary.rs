//! What the overview keeps per object, and the pure functions that turn a
//! set of them into the overview's parts. No API calls here: the live
//! overview folds watch updates into maps of these and calls [`pod_counts`],
//! [`capacity`] and [`newest_events`] when something changed.

use super::{Capacity, EventInfo, NodeCounts, PodCounts, ProblemPod};
use crate::created_secs;
use crate::{parse_cpu_quantity, parse_memory_quantity};
use k8s_openapi::api::core::v1::{Container, Event, Node, Pod};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use std::collections::{BTreeMap, HashMap};

/// A pod pending longer than this is listed as a problem.
pub const PENDING_TOO_LONG_SECS: i64 = 5 * 60;
/// At most this many problem pods are listed (all are counted).
pub const MAX_PROBLEMS: usize = 50;

/// How a pod counts in the overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodState {
    Running,
    Pending,
    Succeeded,
    Failed,
    /// A container is waiting for a reason other than starting up:
    /// CrashLoopBackOff, ImagePullBackOff, CreateContainerConfigError, ...
    WaitingError,
    /// A phase this version does not know ("Unknown").
    Other,
}

/// The part of a pod the overview needs.
#[derive(Debug, Clone, PartialEq)]
pub struct PodSummary {
    pub namespace: String,
    pub name: String,
    /// As in the Pods table: the waiting reason, `Terminating`, or the phase.
    pub status: String,
    pub state: PodState,
    pub ready: bool,
    pub terminating: bool,
    pub restarts: i32,
    /// Unix seconds.
    pub created: i64,
    /// Bound to a node (`spec.nodeName` set).
    pub scheduled: bool,
    pub cpu_request_millicores: u64,
    pub memory_request_bytes: u64,
}

/// The part of a node the overview needs.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSummary {
    pub ready: bool,
    pub cpu_allocatable_millicores: u64,
    pub memory_allocatable_bytes: u64,
    pub pods_allocatable: u64,
}

/// Waiting reasons that are part of a normal start, not an error.
const STARTING: [&str; 2] = ["ContainerCreating", "PodInitializing"];

/// The pod's status the way `kubectl get pods` (and the Pods table) shows
/// it: `Terminating`, else the first waiting reason other than
/// `PodInitializing`, else `status.reason` or the phase.
pub fn pod_status(pod: &Pod) -> String {
    if pod.metadata.deletion_timestamp.is_some() {
        return "Terminating".into();
    }
    if let Some(reason) = waiting_reasons(pod).find(|r| *r != "PodInitializing") {
        return reason.to_string();
    }
    let status = pod.status.as_ref();
    status
        .and_then(|s| s.reason.clone())
        .or_else(|| status.and_then(|s| s.phase.clone()))
        .unwrap_or_default()
}

fn waiting_reasons(pod: &Pod) -> impl Iterator<Item = &str> {
    let status = pod.status.as_ref();
    let init = status.and_then(|s| s.init_container_statuses.as_deref());
    let main = status.and_then(|s| s.container_statuses.as_deref());
    init.unwrap_or_default()
        .iter()
        .chain(main.unwrap_or_default())
        .filter_map(|c| c.state.as_ref()?.waiting.as_ref()?.reason.as_deref())
}

pub fn pod_summary(pod: &Pod) -> PodSummary {
    let status = pod.status.as_ref();
    let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or_default();
    let waiting_error = waiting_reasons(pod).any(|r| !STARTING.contains(&r));
    let state = match phase {
        "Succeeded" => PodState::Succeeded,
        "Failed" => PodState::Failed,
        _ if waiting_error => PodState::WaitingError,
        "Running" => PodState::Running,
        "Pending" => PodState::Pending,
        _ => PodState::Other,
    };
    let ready = status
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or_default()
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");
    let restarts = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|c| c.restart_count)
        .sum();
    let spec = pod.spec.as_ref();
    let (cpu, memory) = spec.map(pod_requests).unwrap_or_default();
    PodSummary {
        namespace: pod.metadata.namespace.clone().unwrap_or_default(),
        name: pod.metadata.name.clone().unwrap_or_default(),
        status: pod_status(pod),
        state,
        ready,
        terminating: pod.metadata.deletion_timestamp.is_some(),
        restarts,
        created: created_secs(&pod.metadata.creation_timestamp),
        scheduled: spec.and_then(|s| s.node_name.as_ref()).is_some(),
        cpu_request_millicores: cpu,
        memory_request_bytes: memory,
    }
}

fn quantity(map: Option<&BTreeMap<String, Quantity>>, key: &str, cpu: bool) -> u64 {
    let Some(q) = map.and_then(|m| m.get(key)) else {
        return 0;
    };
    let parsed = if cpu {
        parse_cpu_quantity(&q.0)
    } else {
        parse_memory_quantity(&q.0)
    };
    parsed.unwrap_or(0)
}

fn container_requests(c: &Container) -> (u64, u64) {
    let requests = c.resources.as_ref().and_then(|r| r.requests.as_ref());
    (
        quantity(requests, "cpu", true),
        quantity(requests, "memory", false),
    )
}

/// What the scheduler reserves for a pod: the sum of its containers'
/// requests or the largest init container's, whichever is larger, plus the
/// pod overhead.
fn pod_requests(spec: &k8s_openapi::api::core::v1::PodSpec) -> (u64, u64) {
    let (mut cpu, mut memory) = spec
        .containers
        .iter()
        .map(container_requests)
        .fold((0, 0), |(a, b), (c, m)| (a + c, b + m));
    for init in spec.init_containers.as_deref().unwrap_or_default() {
        let (c, m) = container_requests(init);
        cpu = cpu.max(c);
        memory = memory.max(m);
    }
    let overhead = spec.overhead.as_ref();
    cpu += quantity(overhead, "cpu", true);
    memory += quantity(overhead, "memory", false);
    (cpu, memory)
}

pub fn node_summary(node: &Node) -> NodeSummary {
    let status = node.status.as_ref();
    let ready = status
        .and_then(|s| s.conditions.as_deref())
        .unwrap_or_default()
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");
    let alloc = status.and_then(|s| s.allocatable.as_ref());
    let pods = alloc
        .and_then(|a| a.get("pods"))
        .and_then(|q| q.0.parse().ok())
        .unwrap_or(0);
    NodeSummary {
        ready,
        cpu_allocatable_millicores: quantity(alloc, "cpu", true),
        memory_allocatable_bytes: quantity(alloc, "memory", false),
        pods_allocatable: pods,
    }
}

/// An event as the overview lists it, with its time in unix seconds.
pub fn event_info(e: &Event) -> EventInfo {
    let time = e
        .last_timestamp
        .as_ref()
        .map(|t| t.0)
        .or_else(|| e.event_time.as_ref().map(|t| t.0))
        .or_else(|| e.first_timestamp.as_ref().map(|t| t.0))
        .or_else(|| e.metadata.creation_timestamp.as_ref().map(|t| t.0))
        .map(|t| t.timestamp())
        .unwrap_or_default();
    let o = &e.involved_object;
    let kind = o.kind.clone().unwrap_or_default();
    let name = o.name.clone().unwrap_or_default();
    EventInfo {
        key: format!(
            "{}/{}",
            e.metadata.namespace.as_deref().unwrap_or_default(),
            e.metadata.name.as_deref().unwrap_or_default()
        ),
        kind: e.type_.clone().unwrap_or_default(),
        reason: e.reason.clone().unwrap_or_default(),
        message: e.message.clone().unwrap_or_default(),
        object: format!("{kind}/{name}"),
        object_api_version: o.api_version.clone().unwrap_or_default(),
        object_kind: kind,
        object_namespace: o.namespace.clone().unwrap_or_default(),
        object_name: name,
        time,
        count: e.count.unwrap_or(1),
    }
}

/// Why `pod` is a problem, if it is one: a waiting error, running but not
/// ready, or pending for too long.
fn problem(pod: &PodSummary, now: i64) -> Option<String> {
    if pod.terminating {
        return None;
    }
    match pod.state {
        PodState::WaitingError => Some(pod.status.clone()),
        PodState::Running if !pod.ready => Some("Not ready".into()),
        PodState::Pending if pod.created > 0 && now - pod.created > PENDING_TOO_LONG_SECS => {
            Some(format!("Pending for {}m", (now - pod.created) / 60))
        }
        _ => None,
    }
}

/// Pod counts by state, pods per namespace and the problem pods, as of
/// `now` (unix seconds; it decides which pending pods are overdue).
pub fn pod_counts<'a>(
    namespace: Option<String>,
    pods: impl IntoIterator<Item = &'a PodSummary>,
    now: i64,
) -> PodCounts {
    let mut c = PodCounts {
        namespace,
        total: 0,
        running: 0,
        pending: 0,
        succeeded: 0,
        failed: 0,
        waiting_errors: 0,
        by_namespace: Vec::new(),
        problems: Vec::new(),
        problems_total: 0,
    };
    let mut by_namespace: HashMap<&str, usize> = HashMap::new();
    let mut problems: Vec<(&PodSummary, String)> = Vec::new();
    for pod in pods {
        c.total += 1;
        *by_namespace.entry(&pod.namespace).or_insert(0) += 1;
        match pod.state {
            PodState::Running => c.running += 1,
            PodState::Pending => c.pending += 1,
            PodState::Succeeded => c.succeeded += 1,
            PodState::Failed => c.failed += 1,
            PodState::WaitingError => c.waiting_errors += 1,
            PodState::Other => {}
        }
        if let Some(why) = problem(pod, now) {
            problems.push((pod, why));
        }
    }
    let mut by_namespace: Vec<(String, usize)> = by_namespace
        .into_iter()
        .map(|(ns, n)| (ns.to_string(), n))
        .collect();
    by_namespace.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    c.by_namespace = by_namespace;
    // Waiting errors first, then the most restarts, then the oldest.
    problems.sort_by(|(a, _), (b, _)| {
        let err = |p: &PodSummary| p.state != PodState::WaitingError;
        (
            err(a),
            std::cmp::Reverse(a.restarts),
            a.created,
            &a.namespace,
            &a.name,
        )
            .cmp(&(
                err(b),
                std::cmp::Reverse(b.restarts),
                b.created,
                &b.namespace,
                &b.name,
            ))
    });
    c.problems_total = problems.len();
    c.problems = problems
        .into_iter()
        .take(MAX_PROBLEMS)
        .map(|(p, reason)| ProblemPod {
            namespace: p.namespace.clone(),
            name: p.name.clone(),
            status: p.status.clone(),
            reason,
            restarts: p.restarts,
            created: p.created,
        })
        .collect();
    c
}

pub fn node_counts<'a>(nodes: impl IntoIterator<Item = &'a NodeSummary>) -> NodeCounts {
    let mut c = NodeCounts { total: 0, ready: 0 };
    for n in nodes {
        c.total += 1;
        c.ready += usize::from(n.ready);
    }
    c
}

/// Requests and usage against what the nodes can allocate. Requests count
/// the pods bound to a node that have not finished, like the scheduler.
/// `usage` is the nodes' (cpu millicores, memory bytes) from the metrics
/// API, if it is available.
pub fn capacity<'a, 'b>(
    pods: impl IntoIterator<Item = &'a PodSummary>,
    nodes: impl IntoIterator<Item = &'b NodeSummary>,
    usage: Option<(u64, u64)>,
) -> Capacity {
    let mut c = Capacity {
        cpu_requests_millicores: 0,
        cpu_allocatable_millicores: 0,
        cpu_usage_millicores: usage.map(|u| u.0).unwrap_or(0),
        memory_requests_bytes: 0,
        memory_allocatable_bytes: 0,
        memory_usage_bytes: usage.map(|u| u.1).unwrap_or(0),
        metrics: usage.is_some(),
        pods: 0,
        pods_allocatable: 0,
    };
    for n in nodes {
        c.cpu_allocatable_millicores += n.cpu_allocatable_millicores;
        c.memory_allocatable_bytes += n.memory_allocatable_bytes;
        c.pods_allocatable += n.pods_allocatable;
    }
    for p in pods {
        if p.scheduled && !matches!(p.state, PodState::Succeeded | PodState::Failed) {
            c.pods += 1;
            c.cpu_requests_millicores += p.cpu_request_millicores;
            c.memory_requests_bytes += p.memory_request_bytes;
        }
    }
    c
}

/// The `n` newest events, newest first.
pub fn newest_events<'a>(
    events: impl IntoIterator<Item = &'a EventInfo>,
    n: usize,
) -> Vec<EventInfo> {
    let mut all: Vec<&EventInfo> = events.into_iter().collect();
    // Ties by key, the reverse of the order `trim_events` drops them in.
    all.sort_by(|a, b| (b.time, &b.key).cmp(&(a.time, &a.key)));
    all.into_iter().take(n).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000;

    fn ts(secs: i64) -> String {
        chrono::DateTime::from_timestamp(secs, 0)
            .unwrap()
            .to_rfc3339()
    }

    fn pod(name: &str, phase: &str, extra: serde_json::Value) -> Pod {
        let mut v = json!({
            "metadata": { "name": name, "namespace": "demo", "creationTimestamp": ts(NOW - 3600) },
            "spec": {
                "nodeName": "node-1",
                "containers": [{ "name": "c", "resources": { "requests": { "cpu": "100m", "memory": "64Mi" } } }]
            },
            "status": {
                "phase": phase,
                "conditions": [{ "type": "Ready", "status": if phase == "Running" { "True" } else { "False" } }]
            }
        });
        merge(&mut v, extra);
        serde_json::from_value(v).unwrap()
    }

    fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
        match (a, b) {
            (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                for (k, v) in b {
                    merge(a.entry(k).or_insert(serde_json::Value::Null), v);
                }
            }
            (a, b) => *a = b,
        }
    }

    fn crash_looping(name: &str) -> Pod {
        pod(
            name,
            "Running",
            json!({ "status": {
                "conditions": [{ "type": "Ready", "status": "False" }],
                "containerStatuses": [{
                    "name": "c", "image": "busybox", "imageID": "", "ready": false, "restartCount": 7,
                    "state": { "waiting": { "reason": "CrashLoopBackOff" } }
                }]
            }}),
        )
    }

    #[test]
    fn crash_looping_pods_are_not_running() {
        let p = pod_summary(&crash_looping("crashed"));
        assert_eq!(p.status, "CrashLoopBackOff");
        assert_eq!(p.state, PodState::WaitingError);
        assert_eq!(p.restarts, 7);
    }

    #[test]
    fn starting_containers_are_not_errors() {
        let p = pod_summary(&pod(
            "new",
            "Pending",
            json!({ "status": { "containerStatuses": [{
                "name": "c", "image": "busybox", "imageID": "", "ready": false, "restartCount": 0,
                "state": { "waiting": { "reason": "ContainerCreating" } }
            }]}}),
        ));
        assert_eq!(p.state, PodState::Pending);
        assert_eq!(p.status, "ContainerCreating");
    }

    #[test]
    fn counts_and_problems() {
        let pods: Vec<PodSummary> = [
            pod("web-1", "Running", json!({})),
            crash_looping("crashed"),
            pod("job", "Succeeded", json!({})),
            pod("evicted", "Failed", json!({ "status": { "reason": "Evicted" } })),
            // Pending for an hour, and one just created.
            pod("stuck", "Pending", json!({ "spec": { "nodeName": null } })),
            pod(
                "fresh",
                "Pending",
                json!({ "metadata": { "creationTimestamp": ts(NOW - 30) }, "spec": { "nodeName": null } }),
            ),
            pod(
                "unready",
                "Running",
                json!({ "status": { "conditions": [{ "type": "Ready", "status": "False" }] } }),
            ),
            pod(
                "going",
                "Running",
                json!({ "metadata": { "deletionTimestamp": ts(NOW) },
                         "status": { "conditions": [{ "type": "Ready", "status": "False" }] } }),
            ),
        ]
        .iter()
        .map(pod_summary)
        .collect();

        let c = pod_counts(None, &pods, NOW);
        assert_eq!(c.total, 8);
        assert_eq!(c.running, 3); // web-1, unready, going
        assert_eq!(c.waiting_errors, 1);
        assert_eq!(c.pending, 2);
        assert_eq!(c.succeeded, 1);
        assert_eq!(c.failed, 1);
        assert_eq!(c.by_namespace, vec![("demo".to_string(), 8)]);

        let problems: Vec<(&str, &str)> = c
            .problems
            .iter()
            .map(|p| (p.name.as_str(), p.reason.as_str()))
            .collect();
        assert_eq!(
            problems,
            vec![
                ("crashed", "CrashLoopBackOff"),
                ("stuck", "Pending for 60m"),
                ("unready", "Not ready"),
            ]
        );
        assert_eq!(c.problems_total, 3);
        assert_eq!(c.problems[0].status, "CrashLoopBackOff");
    }

    #[test]
    fn capacity_counts_requests_of_scheduled_unfinished_pods() {
        let pods: Vec<PodSummary> = [
            pod("a", "Running", json!({})),
            crash_looping("b"),
            pod("done", "Succeeded", json!({})),
            pod(
                "unscheduled",
                "Pending",
                json!({ "spec": { "nodeName": null } }),
            ),
            pod(
                "init",
                "Running",
                json!({ "spec": { "initContainers": [
                    { "name": "i", "resources": { "requests": { "cpu": "1", "memory": "32Mi" } } }
                ]}}),
            ),
        ]
        .iter()
        .map(pod_summary)
        .collect();
        let node: Node = serde_json::from_value(json!({
            "metadata": { "name": "node-1" },
            "status": {
                "allocatable": { "cpu": "4", "memory": "8Gi", "pods": "110" },
                "conditions": [{ "type": "Ready", "status": "True" }]
            }
        }))
        .unwrap();
        let nodes = [node_summary(&node), node_summary(&node)];
        let c = capacity(&pods, &nodes, Some((900, 1 << 30)));
        assert_eq!(c.pods, 3);
        // 100m + 100m + max(100m, 1 init core).
        assert_eq!(c.cpu_requests_millicores, 1200);
        assert_eq!(c.memory_requests_bytes, 3 * 64 * 1024 * 1024);
        assert_eq!(c.cpu_allocatable_millicores, 8000);
        assert_eq!(c.memory_allocatable_bytes, 16 << 30);
        assert_eq!(c.pods_allocatable, 220);
        assert!(c.metrics);
        assert_eq!(c.cpu_usage_millicores, 900);

        assert_eq!(node_counts(&nodes), NodeCounts { total: 2, ready: 2 });
        assert!(!capacity(&pods, &nodes, None).metrics);
    }

    #[test]
    fn events_link_to_their_object_and_sort_newest_first() {
        let event = |name: &str, at: i64| -> Event {
            serde_json::from_value(json!({
                "metadata": { "name": name, "namespace": "demo" },
                "involvedObject": { "apiVersion": "apps/v1", "kind": "Deployment", "name": "web", "namespace": "demo" },
                "reason": "FailedCreate", "message": "quota", "type": "Warning",
                "lastTimestamp": ts(at), "count": 3
            }))
            .unwrap()
        };
        let infos: Vec<EventInfo> = [event("old", NOW - 600), event("new", NOW)]
            .iter()
            .map(event_info)
            .collect();
        let newest = newest_events(&infos, 1);
        assert_eq!(newest.len(), 1);
        let e = &newest[0];
        assert_eq!(e.key, "demo/new");
        assert_eq!(e.time, NOW);
        assert_eq!(
            (
                e.object.as_str(),
                e.object_api_version.as_str(),
                e.object_kind.as_str(),
                e.object_namespace.as_str(),
                e.object_name.as_str()
            ),
            ("Deployment/web", "apps/v1", "Deployment", "demo", "web")
        );
        assert_eq!(e.count, 3);
    }
}
