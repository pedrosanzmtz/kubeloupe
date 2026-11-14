//! The pure parts of [`super::related`]: row cells, selectors, what a pod
//! references, and the groups computed from the object alone (a Service's
//! ports, an Ingress's rules, a Node's taints and allocated resources...).

use super::columns::{
    abbreviate, age_text, pod_ready, pod_restarts, pod_status, service_ports as ports_text,
};
use super::jsonpath::{array_at, cell_text, first_of, path_i64, path_str};
use super::related::{RelatedGroup, RelatedItem};
use crate::{format_cpu, format_memory, parse_cpu_quantity, parse_memory_quantity};
use serde_json::Value;

/// The string at a JSON pointer, or "".
pub(super) fn str_at(v: &Value, pointer: &str) -> String {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn age_of(v: &Value) -> String {
    let ts = str_at(v, "/metadata/creationTimestamp");
    if ts.is_empty() {
        ts
    } else {
        age_text(&ts)
    }
}

fn name_of(v: &Value) -> String {
    str_at(v, "/metadata/name")
}

// --- Row cells ----------------------------------------------------------------

pub(super) fn pod_cells(v: &Value) -> Vec<String> {
    vec![
        name_of(v),
        pod_ready(v),
        pod_status(v),
        pod_restarts(v),
        age_of(v),
    ]
}

pub(super) fn node_pod_cells(v: &Value) -> Vec<String> {
    vec![
        name_of(v),
        str_at(v, "/metadata/namespace"),
        pod_ready(v),
        pod_status(v),
        pod_restarts(v),
    ]
}

const REVISION: &str = "/metadata/annotations/deployment.kubernetes.io~1revision";

pub(super) fn replicaset_cells(v: &Value) -> Vec<String> {
    vec![
        name_of(v),
        str_at(v, REVISION),
        format!(
            "{}/{}",
            path_i64(v, ".status.readyReplicas"),
            path_i64(v, ".spec.replicas")
        ),
        age_of(v),
    ]
}

/// Newest revision first.
pub(super) fn by_revision_desc(objs: &mut [Value]) {
    objs.sort_by_key(|o| std::cmp::Reverse(str_at(o, REVISION).parse::<i64>().unwrap_or(0)));
}

pub(super) fn newest_first(objs: &mut [Value]) {
    objs.sort_by_key(|o| std::cmp::Reverse(str_at(o, "/metadata/creationTimestamp")));
}

fn condition_true(v: &Value, type_: &str) -> bool {
    array_at(v, "/status/conditions")
        .any(|c| path_str(c, ".type") == type_ && path_str(c, ".status") == "True")
}

/// Complete, Failed, Suspended or Running.
pub(super) fn job_state(v: &Value) -> &'static str {
    if condition_true(v, "Complete") {
        "Complete"
    } else if condition_true(v, "Failed") {
        "Failed"
    } else if v.pointer("/spec/suspend") == Some(&Value::Bool(true)) {
        "Suspended"
    } else {
        "Running"
    }
}

fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    s.parse().ok()
}

/// "45s", "3m12s", "2h5m", "3d4h".
pub(super) fn duration_text(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s if s < 86_400 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        s => format!("{}d{}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// From start to completion, or until now while it runs.
pub(super) fn job_duration(v: &Value) -> String {
    let Some(start) = parse_time(&str_at(v, "/status/startTime")) else {
        return String::new();
    };
    let end = parse_time(&str_at(v, "/status/completionTime")).unwrap_or_else(chrono::Utc::now);
    duration_text((end - start).num_seconds())
}

pub(super) fn job_cells(v: &Value) -> Vec<String> {
    let completions = v
        .pointer("/spec/completions")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    vec![
        name_of(v),
        job_state(v).into(),
        format!("{}/{completions}", path_i64(v, ".status.succeeded")),
        job_duration(v),
        age_of(v),
    ]
}

pub(super) fn slice_cells(v: &Value) -> Vec<String> {
    let endpoints: Vec<&Value> = array_at(v, "/endpoints").collect();
    // An endpoint without a `ready` condition counts as ready.
    let ready = endpoints
        .iter()
        .filter(|e| e.pointer("/conditions/ready") != Some(&Value::Bool(false)))
        .count();
    let addrs = endpoints
        .iter()
        .filter_map(|e| e.pointer("/addresses/0").and_then(Value::as_str))
        .map(String::from)
        .collect();
    let ports = array_at(v, "/ports")
        .map(|p| {
            let port = path_i64(p, ".port").to_string();
            match p
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
            {
                Some(n) => format!("{n}:{port}"),
                None => port,
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    vec![
        name_of(v),
        format!("{ready}/{}", endpoints.len()),
        abbreviate(addrs),
        ports,
    ]
}

pub(super) fn ingress_cells(v: &Value) -> Vec<String> {
    let hosts: Vec<String> = array_at(v, "/spec/rules")
        .map(|r| first_of(&[path_str(r, ".host"), "*".into()]))
        .collect();
    let paths: Vec<String> = array_at(v, "/spec/rules")
        .flat_map(|r| array_at(r, "/http/paths"))
        .map(|p| first_of(&[path_str(p, ".path"), "/".into()]))
        .collect();
    vec![name_of(v), abbreviate(hosts), abbreviate(paths)]
}

pub(super) fn service_cells(v: &Value) -> Vec<String> {
    vec![
        name_of(v),
        path_str(v, ".spec.type"),
        path_str(v, ".spec.clusterIP"),
        ports_text(v),
    ]
}

// --- Selectors and ownership ----------------------------------------------------

/// A LabelSelector (`matchLabels`, `matchExpressions`) as a label selector
/// string. `None` when it is missing or empty: an empty selector would
/// match every object in the namespace.
pub(super) fn label_selector(sel: &Value) -> Option<String> {
    let mut parts: Vec<String> = sel
        .get("matchLabels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", cell_text(v)))
        .collect();
    for e in array_at(sel, "/matchExpressions") {
        let key = path_str(e, ".key");
        let values: Vec<String> = array_at(e, "/values").map(cell_text).collect();
        parts.push(match path_str(e, ".operator").as_str() {
            "In" => format!("{key} in ({})", values.join(",")),
            "NotIn" => format!("{key} notin ({})", values.join(",")),
            "Exists" => key,
            "DoesNotExist" => format!("!{key}"),
            _ => continue,
        });
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

/// A plain label map (a Service's `spec.selector`) as a selector string;
/// `None` if it is missing or empty.
pub(super) fn map_selector(sel: &Value) -> Option<String> {
    label_selector(&serde_json::json!({ "matchLabels": sel }))
}

/// Whether the label map `selector` selects an object with `labels`. A
/// missing or empty selector selects nothing.
pub(super) fn selects(selector: Option<&Value>, labels: Option<&Value>) -> bool {
    let Some(sel) = selector
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    else {
        return false;
    };
    sel.iter()
        .all(|(k, v)| labels.and_then(|l| l.get(k)) == Some(v))
}

/// Whether `obj` has an owner reference to `uid`.
pub(super) fn owned_by(obj: &Value, uid: &str) -> bool {
    !uid.is_empty()
        && array_at(obj, "/metadata/ownerReferences").any(|o| path_str(o, ".uid") == uid)
}

/// The Services an Ingress routes to (its default backend and every path).
pub(super) fn ingress_services(v: &Value) -> Vec<String> {
    ingress_backends(v).into_iter().map(|b| b.service).collect()
}

// --- What a pod references -----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    ConfigMap,
    Secret,
    /// A PersistentVolumeClaim.
    Claim,
}

/// An object a pod uses, and how ("volume data", "envFrom (app)").
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PodRef {
    pub kind: RefKind,
    pub name: String,
    pub how: Vec<String>,
}

/// The ConfigMaps, Secrets and PVCs `pod` mounts or references, in order.
pub(super) fn pod_refs(pod: &Value) -> Vec<PodRef> {
    let mut out: Vec<PodRef> = Vec::new();
    let mut add = |kind: RefKind, name: String, how: String| {
        if name.is_empty() {
            return;
        }
        match out.iter_mut().find(|r| r.kind == kind && r.name == name) {
            Some(r) if !r.how.contains(&how) => r.how.push(how),
            Some(_) => {}
            None => out.push(PodRef {
                kind,
                name,
                how: vec![how],
            }),
        }
    };
    for vol in array_at(pod, "/spec/volumes") {
        let how = format!("volume {}", path_str(vol, ".name"));
        add(
            RefKind::ConfigMap,
            path_str(vol, ".configMap.name"),
            how.clone(),
        );
        add(
            RefKind::Secret,
            path_str(vol, ".secret.secretName"),
            how.clone(),
        );
        add(
            RefKind::Claim,
            path_str(vol, ".persistentVolumeClaim.claimName"),
            how.clone(),
        );
        for src in array_at(vol, "/projected/sources") {
            add(
                RefKind::ConfigMap,
                path_str(src, ".configMap.name"),
                how.clone(),
            );
            add(RefKind::Secret, path_str(src, ".secret.name"), how.clone());
        }
    }
    let containers = array_at(pod, "/spec/initContainers")
        .chain(array_at(pod, "/spec/containers"))
        .chain(array_at(pod, "/spec/ephemeralContainers"));
    for c in containers {
        let cname = path_str(c, ".name");
        for from in array_at(c, "/envFrom") {
            let how = format!("envFrom ({cname})");
            add(
                RefKind::ConfigMap,
                path_str(from, ".configMapRef.name"),
                how.clone(),
            );
            add(RefKind::Secret, path_str(from, ".secretRef.name"), how);
        }
        for env in array_at(c, "/env") {
            let how = format!("env {} ({cname})", path_str(env, ".name"));
            add(
                RefKind::ConfigMap,
                path_str(env, ".valueFrom.configMapKeyRef.name"),
                how.clone(),
            );
            add(
                RefKind::Secret,
                path_str(env, ".valueFrom.secretKeyRef.name"),
                how,
            );
        }
    }
    for s in array_at(pod, "/spec/imagePullSecrets") {
        add(
            RefKind::Secret,
            path_str(s, ".name"),
            "imagePullSecret".into(),
        );
    }
    out
}

/// The pods among `pods` that use the `kind` named `name`, with how.
pub(super) fn pods_using(pods: Vec<Value>, kind: RefKind, name: &str) -> Vec<(Value, String)> {
    pods.into_iter()
        .filter_map(|p| {
            let r = pod_refs(&p)
                .into_iter()
                .find(|r| r.kind == kind && r.name == name)?;
            Some((p, r.how.join(", ")))
        })
        .collect()
}

/// The pod's ConfigMaps, Secrets and PVCs, one group per kind that has any.
pub(super) fn pod_mounts(pod: &Value) -> Vec<RelatedGroup> {
    let ns = str_at(pod, "/metadata/namespace");
    let refs = pod_refs(pod);
    [
        (RefKind::ConfigMap, "ConfigMaps", "ConfigMap"),
        (RefKind::Secret, "Secrets", "Secret"),
        (
            RefKind::Claim,
            "PersistentVolumeClaims",
            "PersistentVolumeClaim",
        ),
    ]
    .into_iter()
    .filter_map(|(kind, title, k)| {
        let items: Vec<RelatedItem> = refs
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| RelatedItem::object(&ns, &r.name, vec![r.name.clone(), r.how.join(", ")]))
            .collect();
        (!items.is_empty())
            .then(|| RelatedGroup::objects(title, "v1", k, &["Name", "Used as"], items))
    })
    .collect()
}

// --- Groups from the object alone ------------------------------------------------

/// A Service's ports: port → targetPort (a named target is resolved by the
/// pods; it is shown as named).
pub(super) fn service_ports(v: &Value) -> Vec<RelatedGroup> {
    let rows = array_at(v, "/spec/ports")
        .map(|p| {
            let port = path_i64(p, ".port").to_string();
            let target = p
                .get("targetPort")
                .map(cell_text)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| port.clone());
            vec![
                path_str(p, ".name"),
                port,
                target,
                first_of(&[path_str(p, ".protocol"), "TCP".into()]),
                path_str(p, ".nodePort"),
            ]
        })
        .collect();
    vec![RelatedGroup::plain(
        "Ports",
        &["Name", "Port", "Target", "Protocol", "NodePort"],
        rows,
    )]
}

struct Backend {
    host: String,
    path: String,
    service: String,
    port: String,
}

fn backend_of(b: &Value, host: String, path: String) -> Option<Backend> {
    let service = path_str(b, ".service.name");
    if service.is_empty() {
        // A `resource` backend: not a Service.
        return None;
    }
    let port = first_of(&[
        path_str(b, ".service.port.number"),
        path_str(b, ".service.port.name"),
    ]);
    Some(Backend {
        host,
        path,
        service,
        port,
    })
}

fn ingress_backends(v: &Value) -> Vec<Backend> {
    let mut out: Vec<Backend> = v
        .pointer("/spec/defaultBackend")
        .and_then(|b| backend_of(b, "*".into(), "(default)".into()))
        .into_iter()
        .collect();
    for rule in array_at(v, "/spec/rules") {
        let host = first_of(&[path_str(rule, ".host"), "*".into()]);
        for p in array_at(rule, "/http/paths") {
            let path = first_of(&[path_str(p, ".path"), "/".into()]);
            if let Some(b) = p
                .get("backend")
                .and_then(|b| backend_of(b, host.clone(), path))
            {
                out.push(b);
            }
        }
    }
    out
}

/// An Ingress's rules (host/path → Service:port) and its TLS Secrets.
pub(super) fn ingress_rules(v: &Value) -> Vec<RelatedGroup> {
    let ns = str_at(v, "/metadata/namespace");
    let rules = ingress_backends(v)
        .into_iter()
        .map(|b| {
            RelatedItem::object(
                &ns,
                &b.service,
                vec![b.host, b.path, b.service.clone(), b.port],
            )
        })
        .collect();
    let mut groups = vec![RelatedGroup::objects(
        "Rules",
        "v1",
        "Service",
        &["Host", "Path", "Service", "Port"],
        rules,
    )
    .name_in(2)];
    let tls: Vec<RelatedItem> = array_at(v, "/spec/tls")
        .filter_map(|t| {
            let secret = path_str(t, ".secretName");
            let hosts = array_at(t, "/hosts").map(cell_text).collect::<Vec<_>>();
            (!secret.is_empty())
                .then(|| RelatedItem::object(&ns, &secret, vec![secret.clone(), hosts.join(",")]))
        })
        .collect();
    if !tls.is_empty() {
        groups.push(RelatedGroup::objects(
            "TLS",
            "v1",
            "Secret",
            &["Secret", "Hosts"],
            tls,
        ));
    }
    groups
}

pub(super) fn node_taints(v: &Value) -> Vec<RelatedGroup> {
    let rows = array_at(v, "/spec/taints")
        .map(|t| {
            vec![
                path_str(t, ".key"),
                path_str(t, ".value"),
                path_str(t, ".effect"),
            ]
        })
        .collect();
    vec![RelatedGroup::plain(
        "Taints",
        &["Key", "Value", "Effect"],
        rows,
    )]
}

/// Requests or limits of one pod for `resource`, the way the scheduler
/// counts them: the larger of the containers' sum and any init container.
fn pod_amount(pod: &Value, field: &str, resource: &str, parse: fn(&str) -> Option<u64>) -> u64 {
    let of = |c: &Value| {
        c.pointer(&format!("/resources/{field}/{resource}"))
            .and_then(Value::as_str)
            .and_then(parse)
            .unwrap_or(0)
    };
    let sum: u64 = array_at(pod, "/spec/containers").map(of).sum();
    let init = array_at(pod, "/spec/initContainers")
        .map(of)
        .max()
        .unwrap_or(0);
    sum.max(init)
}

fn terminated(pod: &Value) -> bool {
    matches!(
        path_str(pod, ".status.phase").as_str(),
        "Succeeded" | "Failed"
    )
}

fn with_percent(amount: String, used: u64, total: u64) -> String {
    match (used * 100).checked_div(total) {
        Some(pct) => format!("{amount} ({pct}%)"),
        None => amount,
    }
}

/// A Node's requests and limits (of its running pods) vs. allocatable.
pub(super) fn node_capacity(node: &Value, pods: &[Value]) -> RelatedGroup {
    let running: Vec<&Value> = pods.iter().filter(|p| !terminated(p)).collect();
    let alloc = |r: &str, parse: fn(&str) -> Option<u64>| {
        node.pointer(&format!("/status/allocatable/{r}"))
            .and_then(Value::as_str)
            .and_then(parse)
            .unwrap_or(0)
    };
    let sum = |field: &str, r: &str, parse: fn(&str) -> Option<u64>| -> u64 {
        running.iter().map(|p| pod_amount(p, field, r, parse)).sum()
    };
    let mut rows = Vec::new();
    for (r, parse, fmt) in [
        (
            "cpu",
            parse_cpu_quantity as fn(&str) -> Option<u64>,
            format_cpu as fn(u64) -> String,
        ),
        ("memory", parse_memory_quantity, format_memory),
    ] {
        let total = alloc(r, parse);
        let req = sum("requests", r, parse);
        let lim = sum("limits", r, parse);
        rows.push(vec![
            r.to_string(),
            with_percent(fmt(req), req, total),
            with_percent(fmt(lim), lim, total),
            fmt(total),
        ]);
    }
    let max_pods = alloc("pods", |s| s.parse().ok());
    let n = running.len() as u64;
    rows.push(vec![
        "pods".into(),
        with_percent(n.to_string(), n, max_pods),
        String::new(),
        max_pods.to_string(),
    ]);
    RelatedGroup::plain(
        "Allocated resources",
        &["Resource", "Requests", "Limits", "Allocatable"],
        rows,
    )
}

/// A PVC's PersistentVolume and StorageClass, as links.
pub(super) fn pvc_volume(v: &Value) -> Vec<RelatedGroup> {
    let mut groups = Vec::new();
    let pv = path_str(v, ".spec.volumeName");
    if !pv.is_empty() {
        let cells = vec![
            pv.clone(),
            path_str(v, ".status.capacity.storage"),
            path_str(v, ".status.accessModes"),
            path_str(v, ".status.phase"),
        ];
        groups.push(RelatedGroup::objects(
            "PersistentVolume",
            "v1",
            "PersistentVolume",
            &["Name", "Capacity", "Access modes", "Status"],
            vec![RelatedItem::object("", &pv, cells)],
        ));
    }
    let class = path_str(v, ".spec.storageClassName");
    if !class.is_empty() {
        groups.push(RelatedGroup::objects(
            "StorageClass",
            "storage.k8s.io/v1",
            "StorageClass",
            &["Name"],
            vec![RelatedItem::object("", &class, vec![class.clone()])],
        ));
    }
    groups
}

/// A StatefulSet's governing Service.
pub(super) fn statefulset_service(v: &Value) -> Vec<RelatedGroup> {
    let svc = path_str(v, ".spec.serviceName");
    if svc.is_empty() {
        return Vec::new();
    }
    let ns = str_at(v, "/metadata/namespace");
    vec![RelatedGroup::objects(
        "Service",
        "v1",
        "Service",
        &["Name"],
        vec![RelatedItem::object(&ns, &svc, vec![svc.clone()])],
    )]
}

/// An HPA's scale target, with its current and desired replicas.
pub(super) fn hpa_target(v: &Value) -> Vec<RelatedGroup> {
    let name = path_str(v, ".spec.scaleTargetRef.name");
    if name.is_empty() {
        return Vec::new();
    }
    let kind = path_str(v, ".spec.scaleTargetRef.kind");
    let ns = str_at(v, "/metadata/namespace");
    let cells = vec![
        name.clone(),
        kind.clone(),
        format!(
            "{}/{}",
            path_i64(v, ".status.currentReplicas"),
            path_i64(v, ".status.desiredReplicas")
        ),
    ];
    vec![RelatedGroup::objects(
        "Scale target",
        &path_str(v, ".spec.scaleTargetRef.apiVersion"),
        &kind,
        &["Name", "Kind", "Current/desired"],
        vec![RelatedItem::object(&ns, &name, cells)],
    )]
}

/// The name of a metric (spec or status entry of autoscaling/v2).
fn metric_name(m: &Value) -> String {
    match path_str(m, ".type").as_str() {
        "Resource" => path_str(m, ".resource.name"),
        "ContainerResource" => format!(
            "{} ({})",
            path_str(m, ".containerResource.name"),
            path_str(m, ".containerResource.container")
        ),
        "Pods" => path_str(m, ".pods.metric.name"),
        "Object" => format!(
            "{} on {}/{}",
            path_str(m, ".object.metric.name"),
            path_str(m, ".object.describedObject.kind"),
            path_str(m, ".object.describedObject.name")
        ),
        "External" => path_str(m, ".external.metric.name"),
        other => other.to_string(),
    }
}

/// The source object of a metric: `resource`, `pods`, ...
fn metric_body(m: &Value) -> Option<&Value> {
    let key = match path_str(m, ".type").as_str() {
        "Resource" => "resource",
        "ContainerResource" => "containerResource",
        "Pods" => "pods",
        "Object" => "object",
        "External" => "external",
        _ => return None,
    };
    m.get(key)
}

fn value_text(t: &Value) -> String {
    if let Some(u) = t.get("averageUtilization").and_then(Value::as_i64) {
        return format!("{u}%");
    }
    if let Some(a) = t.get("averageValue") {
        return format!("{} (avg)", cell_text(a));
    }
    t.get("value").map(cell_text).unwrap_or_default()
}

/// Each metric's current value vs. its target.
pub(super) fn hpa_metrics(v: &Value) -> Vec<RelatedGroup> {
    let current: Vec<&Value> = array_at(v, "/status/currentMetrics").collect();
    let rows = array_at(v, "/spec/metrics")
        .map(|m| {
            let name = metric_name(m);
            let now = current
                .iter()
                .find(|c| path_str(c, ".type") == path_str(m, ".type") && metric_name(c) == name)
                .and_then(|c| metric_body(c)?.get("current"))
                .map(value_text)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "<unknown>".into());
            let target = metric_body(m)
                .and_then(|b| b.get("target"))
                .map(value_text)
                .unwrap_or_default();
            vec![name, now, target]
        })
        .collect();
    vec![RelatedGroup::plain(
        "Metrics",
        &["Metric", "Current", "Target"],
        rows,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn label_selectors_become_selector_strings() {
        let sel = json!({
            "matchLabels": { "app": "web" },
            "matchExpressions": [
                { "key": "tier", "operator": "In", "values": ["a", "b"] },
                { "key": "env", "operator": "NotIn", "values": ["dev"] },
                { "key": "canary", "operator": "Exists" },
                { "key": "legacy", "operator": "DoesNotExist" }
            ]
        });
        assert_eq!(
            label_selector(&sel).unwrap(),
            "app=web,tier in (a,b),env notin (dev),canary,!legacy"
        );
        assert_eq!(label_selector(&json!({})), None);
        assert_eq!(label_selector(&json!({ "matchLabels": {} })), None);
        assert_eq!(map_selector(&json!({ "app": "web" })).unwrap(), "app=web");
        assert_eq!(map_selector(&json!(null)), None);
    }

    #[test]
    fn service_selector_matching() {
        let labels = json!({ "app": "web", "tier": "fe" });
        assert!(selects(Some(&json!({ "app": "web" })), Some(&labels)));
        assert!(!selects(Some(&json!({ "app": "db" })), Some(&labels)));
        assert!(!selects(
            Some(&json!({ "app": "web", "x": "y" })),
            Some(&labels)
        ));
        // No selector, or an empty one, selects nothing.
        assert!(!selects(None, Some(&labels)));
        assert!(!selects(Some(&json!({})), Some(&labels)));
        assert!(!selects(Some(&json!({ "app": "web" })), None));
    }

    #[test]
    fn ownership_by_uid() {
        let rs =
            json!({ "metadata": { "ownerReferences": [{ "uid": "d1", "kind": "Deployment" }] } });
        assert!(owned_by(&rs, "d1"));
        assert!(!owned_by(&rs, "d2"));
        assert!(!owned_by(&rs, ""));
    }

    fn pod() -> Value {
        json!({
            "metadata": { "name": "p", "namespace": "ns" },
            "spec": {
                "volumes": [
                    { "name": "cfg", "configMap": { "name": "settings" } },
                    { "name": "certs", "secret": { "secretName": "tls" } },
                    { "name": "data", "persistentVolumeClaim": { "claimName": "pvc-1" } },
                    { "name": "all", "projected": { "sources": [
                        { "configMap": { "name": "extra" } }, { "secret": { "name": "tls" } }
                    ] } }
                ],
                "initContainers": [{ "name": "init", "envFrom": [{ "secretRef": { "name": "db" } }] }],
                "containers": [{
                    "name": "app",
                    "envFrom": [{ "configMapRef": { "name": "settings" } }],
                    "env": [
                        { "name": "PASS", "valueFrom": { "secretKeyRef": { "name": "db", "key": "p" } } },
                        { "name": "MODE", "valueFrom": { "configMapKeyRef": { "name": "flags", "key": "m" } } },
                        { "name": "PLAIN", "value": "x" }
                    ]
                }],
                "imagePullSecrets": [{ "name": "registry" }]
            }
        })
    }

    #[test]
    fn detects_configmap_secret_and_claim_usage() {
        let refs = pod_refs(&pod());
        let find = |k: RefKind, n: &str| {
            refs.iter()
                .find(|r| r.kind == k && r.name == n)
                .map(|r| r.how.clone())
        };
        assert_eq!(
            find(RefKind::ConfigMap, "settings").unwrap(),
            ["volume cfg", "envFrom (app)"]
        );
        assert_eq!(
            find(RefKind::ConfigMap, "flags").unwrap(),
            ["env MODE (app)"]
        );
        assert_eq!(find(RefKind::ConfigMap, "extra").unwrap(), ["volume all"]);
        assert_eq!(
            find(RefKind::Secret, "tls").unwrap(),
            ["volume certs", "volume all"]
        );
        assert_eq!(
            find(RefKind::Secret, "db").unwrap(),
            ["envFrom (init)", "env PASS (app)"]
        );
        assert_eq!(
            find(RefKind::Secret, "registry").unwrap(),
            ["imagePullSecret"]
        );
        assert_eq!(find(RefKind::Claim, "pvc-1").unwrap(), ["volume data"]);
        assert_eq!(find(RefKind::Secret, "settings"), None);

        let other =
            json!({ "metadata": { "name": "q" }, "spec": { "containers": [{ "name": "c" }] } });
        let using = pods_using(vec![pod(), other], RefKind::Secret, "db");
        assert_eq!(using.len(), 1);
        assert_eq!(using[0].1, "envFrom (init), env PASS (app)");

        let groups = pod_mounts(&pod());
        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["ConfigMaps", "Secrets", "PersistentVolumeClaims"]);
        assert_eq!(groups[1].items[0].namespace, "ns");
        assert_eq!(groups[2].kind, "PersistentVolumeClaim");
    }

    #[test]
    fn service_port_mapping() {
        let svc = json!({ "spec": { "ports": [
            { "name": "http", "port": 80, "targetPort": 8080, "protocol": "TCP" },
            { "name": "metrics", "port": 9090, "targetPort": "metrics", "nodePort": 30090 },
            { "port": 53, "protocol": "UDP" }
        ] } });
        let g = &service_ports(&svc)[0];
        let rows: Vec<Vec<String>> = g.items.iter().map(|i| i.cells.clone()).collect();
        assert_eq!(rows[0], ["http", "80", "8080", "TCP", ""]);
        assert_eq!(rows[1], ["metrics", "9090", "metrics", "TCP", "30090"]);
        // No targetPort: the same as the port.
        assert_eq!(rows[2], ["", "53", "53", "UDP", ""]);
        assert!(g.kind.is_empty());
    }

    #[test]
    fn ingress_rules_link_services_and_tls_secrets() {
        let ing = json!({
            "metadata": { "name": "i", "namespace": "ns" },
            "spec": {
                "defaultBackend": { "service": { "name": "fallback", "port": { "number": 80 } } },
                "rules": [
                    { "host": "a.example.com", "http": { "paths": [
                        { "path": "/api", "backend": { "service": { "name": "api", "port": { "name": "http" } } } },
                        { "path": "/static", "backend": { "resource": { "kind": "Bucket", "name": "b" } } }
                    ] } },
                    { "http": { "paths": [{ "backend": { "service": { "name": "web", "port": { "number": 8080 } } } }] } }
                ],
                "tls": [{ "hosts": ["a.example.com"], "secretName": "a-tls" }]
            }
        });
        let groups = ingress_rules(&ing);
        let rules = &groups[0];
        assert_eq!(rules.kind, "Service");
        assert_eq!(rules.name_column, 2);
        let rows: Vec<Vec<String>> = rules.items.iter().map(|i| i.cells.clone()).collect();
        assert_eq!(rows[0], ["*", "(default)", "fallback", "80"]);
        assert_eq!(rows[1], ["a.example.com", "/api", "api", "http"]);
        assert_eq!(rows[2], ["*", "/", "web", "8080"]);
        assert_eq!(rules.items[1].namespace, "ns");
        assert_eq!(groups[1].items[0].name, "a-tls");
        assert_eq!(ingress_services(&ing), ["fallback", "api", "web"]);
    }

    #[test]
    fn node_capacity_counts_running_pods() {
        let node =
            json!({ "status": { "allocatable": { "cpu": "2", "memory": "4Gi", "pods": "110" } } });
        let p = |cpu: &str, mem: &str, phase: &str| {
            json!({
                "spec": {
                    "initContainers": [{ "resources": { "requests": { "cpu": "1" } } }],
                    "containers": [
                        { "resources": { "requests": { "cpu": cpu, "memory": mem }, "limits": { "memory": mem } } },
                        { "resources": { "requests": { "cpu": cpu } } }
                    ]
                },
                "status": { "phase": phase }
            })
        };
        // Pod 1: containers ask 2x250m = 500m, the init container 1: 1 counts.
        let pods = [
            p("250m", "1Gi", "Running"),
            p("100m", "1Gi", "Running"),
            p("1", "1Gi", "Succeeded"),
        ];
        let g = node_capacity(&node, &pods);
        let rows: Vec<Vec<String>> = g.items.iter().map(|i| i.cells.clone()).collect();
        assert_eq!(rows[0], ["cpu", "2 (100%)", "0m (0%)", "2"]);
        assert_eq!(rows[1], ["memory", "2Gi (50%)", "2Gi (50%)", "4Gi"]);
        assert_eq!(rows[2], ["pods", "2 (1%)", "", "110"]);
    }

    #[test]
    fn hpa_metrics_current_vs_target() {
        let hpa = json!({
            "metadata": { "namespace": "ns" },
            "spec": {
                "scaleTargetRef": { "apiVersion": "apps/v1", "kind": "Deployment", "name": "web" },
                "metrics": [
                    { "type": "Resource", "resource": { "name": "cpu", "target": { "type": "Utilization", "averageUtilization": 80 } } },
                    { "type": "Pods", "pods": { "metric": { "name": "rps" }, "target": { "type": "AverageValue", "averageValue": "100" } } },
                    { "type": "Resource", "resource": { "name": "memory", "target": { "type": "AverageValue", "averageValue": "500Mi" } } }
                ]
            },
            "status": {
                "currentReplicas": 2, "desiredReplicas": 3,
                "currentMetrics": [
                    { "type": "Resource", "resource": { "name": "cpu", "current": { "averageUtilization": 95, "averageValue": "190m" } } },
                    { "type": "Pods", "pods": { "metric": { "name": "rps" }, "current": { "averageValue": "42" } } }
                ]
            }
        });
        let rows: Vec<Vec<String>> = hpa_metrics(&hpa)[0]
            .items
            .iter()
            .map(|i| i.cells.clone())
            .collect();
        assert_eq!(rows[0], ["cpu", "95%", "80%"]);
        assert_eq!(rows[1], ["rps", "42 (avg)", "100 (avg)"]);
        assert_eq!(rows[2], ["memory", "<unknown>", "500Mi (avg)"]);
        let t = &hpa_target(&hpa)[0];
        assert_eq!(
            (t.api_version.as_str(), t.kind.as_str()),
            ("apps/v1", "Deployment")
        );
        assert_eq!(t.items[0].cells, ["web", "Deployment", "2/3"]);
    }

    #[test]
    fn workload_row_cells() {
        let rs = json!({
            "metadata": { "name": "web-1", "annotations": { "deployment.kubernetes.io/revision": "4" } },
            "spec": { "replicas": 3 }, "status": { "readyReplicas": 2 }
        });
        assert_eq!(&replicaset_cells(&rs)[..3], ["web-1", "4", "2/3"]);
        let mut list = vec![
            json!({ "metadata": { "annotations": { "deployment.kubernetes.io/revision": "2" } } }),
            rs.clone(),
            json!({ "metadata": { "annotations": { "deployment.kubernetes.io/revision": "10" } } }),
        ];
        by_revision_desc(&mut list);
        let revs: Vec<String> = list.iter().map(|o| str_at(o, REVISION)).collect();
        assert_eq!(revs, ["10", "4", "2"]);

        let job = json!({
            "metadata": { "name": "j" },
            "spec": { "completions": 2 },
            "status": {
                "succeeded": 2, "startTime": "2024-01-01T00:00:00Z", "completionTime": "2024-01-01T00:03:12Z",
                "conditions": [{ "type": "Complete", "status": "True" }]
            }
        });
        assert_eq!(&job_cells(&job)[..4], ["j", "Complete", "2/2", "3m12s"]);

        let slice = json!({
            "metadata": { "name": "web-abc" },
            "endpoints": [
                { "addresses": ["10.0.0.1"], "conditions": { "ready": true } },
                { "addresses": ["10.0.0.2"], "conditions": { "ready": false } }
            ],
            "ports": [{ "name": "http", "port": 8080 }]
        });
        assert_eq!(
            slice_cells(&slice),
            ["web-abc", "1/2", "10.0.0.1,10.0.0.2", "http:8080"]
        );
    }
}
