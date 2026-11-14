//! The one-line summary and the facts at the top of a detail panel, per
//! kind ([`super::related`]'s registry points here). Pure functions of the
//! object.

use super::columns::age_text;
use super::jsonpath::{array_at, cell_text, count_keys, first_of, path_i64, path_str};
use super::related::Fact;
use super::related_local::{job_duration, job_state, str_at};
use serde_json::Value;

fn ago(ts: &str) -> String {
    if ts.is_empty() {
        "never".into()
    } else {
        format!("{} ago", age_text(ts))
    }
}

/// `spec.replicas`, which defaults to 1.
fn desired(v: &Value) -> i64 {
    v.pointer("/spec/replicas")
        .and_then(Value::as_i64)
        .unwrap_or(1)
}

fn revision(v: &Value) -> Option<String> {
    let r = str_at(
        v,
        "/metadata/annotations/deployment.kubernetes.io~1revision",
    );
    (!r.is_empty()).then(|| format!("revision {r}"))
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

// --- Workloads ---------------------------------------------------------------

pub(super) fn deployment(v: &Value) -> Vec<String> {
    let mut out = vec![format!(
        "{}/{} ready",
        path_i64(v, ".status.readyReplicas"),
        desired(v)
    )];
    let strategy = first_of(&[path_str(v, ".spec.strategy.type"), "RollingUpdate".into()]);
    if strategy == "RollingUpdate" {
        let surge = first_of(&[
            path_str(v, ".spec.strategy.rollingUpdate.maxSurge"),
            "25%".into(),
        ]);
        let unavailable = first_of(&[
            path_str(v, ".spec.strategy.rollingUpdate.maxUnavailable"),
            "25%".into(),
        ]);
        out.push(format!("RollingUpdate {surge}/{unavailable}"));
    } else {
        out.push(strategy);
    }
    out.extend(revision(v));
    if v.pointer("/spec/paused") == Some(&Value::Bool(true)) {
        out.push("paused".into());
    }
    out
}

/// What `kubectl rollout status` would say.
pub(super) fn rollout_status(v: &Value) -> String {
    let generation = path_i64(v, ".metadata.generation");
    let observed = path_i64(v, ".status.observedGeneration");
    if observed < generation {
        return "waiting for the new spec to be observed".into();
    }
    let deadline = array_at(v, "/status/conditions").any(|c| {
        path_str(c, ".type") == "Progressing"
            && path_str(c, ".reason") == "ProgressDeadlineExceeded"
    });
    if deadline {
        return "progress deadline exceeded".into();
    }
    let want = desired(v);
    let updated = path_i64(v, ".status.updatedReplicas");
    let replicas = path_i64(v, ".status.replicas");
    let available = path_i64(v, ".status.availableReplicas");
    if updated < want {
        format!("{updated} of {want} new replicas updated")
    } else if replicas > updated {
        format!("{} old replicas pending termination", replicas - updated)
    } else if available < updated {
        format!("{available} of {updated} updated replicas available")
    } else {
        "complete".into()
    }
}

pub(super) fn deployment_facts(v: &Value) -> Vec<Fact> {
    let want = desired(v);
    vec![
        Fact::new("Rollout", rollout_status(v)),
        Fact::new(
            "Replicas",
            format!(
                "{} updated · {} ready · {} available · {want} desired",
                path_i64(v, ".status.updatedReplicas"),
                path_i64(v, ".status.readyReplicas"),
                path_i64(v, ".status.availableReplicas"),
            ),
        ),
        Fact::new(
            "Generation",
            format!(
                "observed {} of {}",
                path_i64(v, ".status.observedGeneration"),
                path_i64(v, ".metadata.generation")
            ),
        ),
    ]
}

pub(super) fn statefulset(v: &Value) -> Vec<String> {
    vec![
        format!(
            "{}/{} ready",
            path_i64(v, ".status.readyReplicas"),
            desired(v)
        ),
        first_of(&[
            path_str(v, ".spec.updateStrategy.type"),
            "RollingUpdate".into(),
        ]),
    ]
}

pub(super) fn statefulset_facts(v: &Value) -> Vec<Fact> {
    let current = path_str(v, ".status.currentRevision");
    let update = path_str(v, ".status.updateRevision");
    let rollout = if update.is_empty() || current == update {
        "complete".to_string()
    } else {
        format!(
            "{} of {} updated to {update}",
            path_i64(v, ".status.updatedReplicas"),
            desired(v)
        )
    };
    vec![
        Fact::new("Rollout", rollout),
        Fact::new(
            "Pod management",
            first_of(&[
                path_str(v, ".spec.podManagementPolicy"),
                "OrderedReady".into(),
            ]),
        ),
    ]
}

pub(super) fn daemonset(v: &Value) -> Vec<String> {
    vec![
        format!(
            "{}/{} ready",
            path_i64(v, ".status.numberReady"),
            path_i64(v, ".status.desiredNumberScheduled")
        ),
        format!(
            "{} up to date",
            path_i64(v, ".status.updatedNumberScheduled")
        ),
        first_of(&[
            path_str(v, ".spec.updateStrategy.type"),
            "RollingUpdate".into(),
        ]),
    ]
}

pub(super) fn replicaset(v: &Value) -> Vec<String> {
    let mut out = vec![format!(
        "{}/{} ready",
        path_i64(v, ".status.readyReplicas"),
        desired(v)
    )];
    out.extend(revision(v));
    out
}

pub(super) fn job(v: &Value) -> Vec<String> {
    let completions = v
        .pointer("/spec/completions")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let mut out = vec![
        format!(
            "{}/{completions} succeeded",
            path_i64(v, ".status.succeeded")
        ),
        job_state(v).to_string(),
    ];
    let failed = path_i64(v, ".status.failed");
    if failed > 0 {
        out.push(format!("{failed} failed"));
    }
    let duration = job_duration(v);
    if !duration.is_empty() {
        out.push(duration);
    }
    out
}

pub(super) fn job_facts(v: &Value) -> Vec<Fact> {
    let completed = str_at(v, "/status/completionTime");
    vec![
        Fact::new("Started", ago(&str_at(v, "/status/startTime"))),
        Fact::new(
            "Completed",
            if completed.is_empty() {
                "—".to_string()
            } else {
                ago(&completed)
            },
        ),
        Fact::new(
            "Parallelism",
            v.pointer("/spec/parallelism")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .to_string(),
        ),
        Fact::new(
            "Backoff limit",
            v.pointer("/spec/backoffLimit")
                .and_then(Value::as_i64)
                .unwrap_or(6)
                .to_string(),
        ),
    ]
}

fn suspended(v: &Value) -> bool {
    v.pointer("/spec/suspend") == Some(&Value::Bool(true))
}

pub(super) fn cronjob(v: &Value) -> Vec<String> {
    let mut out = vec![
        path_str(v, ".spec.schedule"),
        if suspended(v) { "suspended" } else { "active" }.to_string(),
    ];
    let running = array_at(v, "/status/active").count();
    if running > 0 {
        out.push(format!("{running} running"));
    }
    out.push(format!(
        "last run {}",
        ago(&str_at(v, "/status/lastScheduleTime"))
    ));
    out
}

pub(super) fn cronjob_facts(v: &Value) -> Vec<Fact> {
    let mut out = vec![Fact::new("Schedule", path_str(v, ".spec.schedule"))];
    let tz = path_str(v, ".spec.timeZone");
    if !tz.is_empty() {
        out.push(Fact::new("Time zone", tz));
    }
    out.extend([
        Fact::new("Suspended", if suspended(v) { "yes" } else { "no" }),
        Fact::new("Last schedule", ago(&str_at(v, "/status/lastScheduleTime"))),
        Fact::new(
            "Last success",
            ago(&str_at(v, "/status/lastSuccessfulTime")),
        ),
        Fact::new(
            "Concurrency",
            first_of(&[path_str(v, ".spec.concurrencyPolicy"), "Allow".into()]),
        ),
    ]);
    out
}

// --- Network -----------------------------------------------------------------

pub(super) fn service(v: &Value) -> Vec<String> {
    let mut out = vec![first_of(&[path_str(v, ".spec.type"), "ClusterIP".into()])];
    let ip = path_str(v, ".spec.clusterIP");
    if !ip.is_empty() {
        out.push(ip);
    }
    let external: Vec<String> = array_at(v, "/status/loadBalancer/ingress")
        .map(|i| first_of(&[path_str(i, ".ip"), path_str(i, ".hostname")]))
        .collect();
    if !external.is_empty() {
        out.push(format!("external {}", external.join(",")));
    }
    out.push(plural(array_at(v, "/spec/ports").count(), "port"));
    out
}

pub(super) fn service_facts(v: &Value) -> Vec<Fact> {
    let selector = v
        .pointer("/spec/selector")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", cell_text(v)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "none (endpoints are managed by hand)".into());
    vec![
        Fact::new("Selector", selector),
        Fact::new(
            "Session affinity",
            first_of(&[path_str(v, ".spec.sessionAffinity"), "None".into()]),
        ),
    ]
}

pub(super) fn ingress(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let class = path_str(v, ".spec.ingressClassName");
    if !class.is_empty() {
        out.push(format!("class {class}"));
    }
    out.push(plural(array_at(v, "/spec/rules").count(), "rule"));
    if array_at(v, "/spec/tls").next().is_some() {
        out.push("TLS".into());
    }
    out
}

pub(super) fn ingress_facts(v: &Value) -> Vec<Fact> {
    let addrs: Vec<String> = array_at(v, "/status/loadBalancer/ingress")
        .map(|i| first_of(&[path_str(i, ".ip"), path_str(i, ".hostname")]))
        .collect();
    vec![Fact::new(
        "Address",
        if addrs.is_empty() {
            "—".into()
        } else {
            addrs.join(", ")
        },
    )]
}

// --- Cluster and config ------------------------------------------------------

pub(super) fn node(v: &Value) -> Vec<String> {
    let ready = array_at(v, "/status/conditions")
        .find(|c| path_str(c, ".type") == "Ready")
        .map(|c| path_str(c, ".status"));
    let mut out = vec![match ready.as_deref() {
        Some("True") => "Ready".to_string(),
        Some(_) => "NotReady".to_string(),
        None => "Unknown".to_string(),
    }];
    if v.pointer("/spec/unschedulable") == Some(&Value::Bool(true)) {
        out.push("SchedulingDisabled".into());
    }
    let roles: Vec<String> = v
        .pointer("/metadata/labels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, _)| k.strip_prefix("node-role.kubernetes.io/"))
        .map(String::from)
        .collect();
    if !roles.is_empty() {
        out.push(roles.join(","));
    }
    out.push(path_str(v, ".status.nodeInfo.kubeletVersion"));
    out.retain(|s| !s.is_empty());
    out
}

pub(super) fn node_facts(v: &Value) -> Vec<Fact> {
    let mut out: Vec<Fact> = array_at(v, "/status/addresses")
        .map(|a| Fact::new(&path_str(a, ".type"), path_str(a, ".address")))
        .collect();
    for (label, path) in [
        ("OS image", ".status.nodeInfo.osImage"),
        ("Kernel", ".status.nodeInfo.kernelVersion"),
        ("Architecture", ".status.nodeInfo.architecture"),
        ("Runtime", ".status.nodeInfo.containerRuntimeVersion"),
        ("Kubelet", ".status.nodeInfo.kubeletVersion"),
        ("Pod CIDR", ".spec.podCIDR"),
    ] {
        let value = path_str(v, path);
        if !value.is_empty() {
            out.push(Fact::new(label, value));
        }
    }
    out
}

pub(super) fn pvc(v: &Value) -> Vec<String> {
    let capacity = first_of(&[
        path_str(v, ".status.capacity.storage"),
        path_str(v, ".spec.resources.requests.storage"),
    ]);
    let mut out = vec![
        path_str(v, ".status.phase"),
        capacity,
        path_str(v, ".spec.accessModes"),
        path_str(v, ".spec.storageClassName"),
    ];
    out.retain(|s| !s.is_empty());
    out
}

pub(super) fn configmap(v: &Value) -> Vec<String> {
    vec![plural(
        count_keys(v, "/data") + count_keys(v, "/binaryData"),
        "key",
    )]
}

pub(super) fn secret(v: &Value) -> Vec<String> {
    vec![
        first_of(&[path_str(v, ".type"), "Opaque".into()]),
        plural(count_keys(v, "/data"), "key"),
    ]
}

pub(super) fn hpa(v: &Value) -> Vec<String> {
    vec![
        format!(
            "{} replicas (desired {})",
            path_i64(v, ".status.currentReplicas"),
            path_i64(v, ".status.desiredReplicas")
        ),
        format!(
            "min {} · max {}",
            v.pointer("/spec/minReplicas")
                .and_then(Value::as_i64)
                .unwrap_or(1),
            path_i64(v, ".spec.maxReplicas")
        ),
    ]
}

pub(super) fn hpa_facts(v: &Value) -> Vec<Fact> {
    vec![Fact::new(
        "Last scaled",
        ago(&str_at(v, "/status/lastScaleTime")),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deploy(status: Value) -> Value {
        json!({
            "metadata": { "generation": 4, "annotations": { "deployment.kubernetes.io/revision": "4" } },
            "spec": { "replicas": 3 },
            "status": status
        })
    }

    #[test]
    fn deployment_summary_and_rollout() {
        let done = deploy(json!({
            "observedGeneration": 4, "replicas": 3, "updatedReplicas": 3,
            "readyReplicas": 3, "availableReplicas": 3
        }));
        assert_eq!(
            deployment(&done),
            ["3/3 ready", "RollingUpdate 25%/25%", "revision 4"]
        );
        assert_eq!(rollout_status(&done), "complete");

        let mut recreate = done.clone();
        recreate["spec"]["strategy"] = json!({ "type": "Recreate" });
        recreate["spec"]["paused"] = json!(true);
        assert_eq!(
            deployment(&recreate)[1..],
            ["Recreate", "revision 4", "paused"]
        );

        let stale = deploy(json!({ "observedGeneration": 3 }));
        assert_eq!(
            rollout_status(&stale),
            "waiting for the new spec to be observed"
        );
        let rolling =
            deploy(json!({ "observedGeneration": 4, "replicas": 4, "updatedReplicas": 1 }));
        assert_eq!(rollout_status(&rolling), "1 of 3 new replicas updated");
        let old = deploy(json!({ "observedGeneration": 4, "replicas": 4, "updatedReplicas": 3 }));
        assert_eq!(rollout_status(&old), "1 old replicas pending termination");
        let unavailable = deploy(json!({
            "observedGeneration": 4, "replicas": 3, "updatedReplicas": 3, "availableReplicas": 2
        }));
        assert_eq!(
            rollout_status(&unavailable),
            "2 of 3 updated replicas available"
        );
        let stuck = deploy(json!({
            "observedGeneration": 4,
            "conditions": [{ "type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded" }]
        }));
        assert_eq!(rollout_status(&stuck), "progress deadline exceeded");
    }

    #[test]
    fn job_and_cronjob_summaries() {
        let j = json!({
            "spec": { "completions": 3 },
            "status": { "succeeded": 1, "failed": 2, "startTime": "2024-01-01T00:00:00Z" }
        });
        let s = job(&j);
        assert_eq!(s[..3], ["1/3 succeeded", "Running", "2 failed"]);

        let cj = json!({
            "spec": { "schedule": "*/5 * * * *", "suspend": true },
            "status": { "active": [{ "name": "a" }] }
        });
        assert_eq!(
            cronjob(&cj),
            ["*/5 * * * *", "suspended", "1 running", "last run never"]
        );
        let facts = cronjob_facts(&cj);
        assert_eq!(facts[1], Fact::new("Suspended", "yes"));
        assert_eq!(facts[3], Fact::new("Last success", "never"));
    }

    #[test]
    fn service_node_and_config_summaries() {
        let svc = json!({ "spec": { "type": "NodePort", "clusterIP": "10.96.0.5", "ports": [{ "port": 80 }] } });
        assert_eq!(service(&svc), ["NodePort", "10.96.0.5", "1 port"]);
        assert_eq!(
            service_facts(&svc)[0].value,
            "none (endpoints are managed by hand)"
        );

        let nd = json!({
            "metadata": { "labels": { "node-role.kubernetes.io/control-plane": "" } },
            "spec": { "unschedulable": true },
            "status": {
                "conditions": [{ "type": "Ready", "status": "True" }],
                "nodeInfo": { "kubeletVersion": "v1.31.0", "osImage": "Debian" },
                "addresses": [{ "type": "InternalIP", "address": "172.18.0.2" }]
            }
        });
        assert_eq!(
            node(&nd),
            ["Ready", "SchedulingDisabled", "control-plane", "v1.31.0"]
        );
        let facts = node_facts(&nd);
        assert_eq!(facts[0], Fact::new("InternalIP", "172.18.0.2"));
        assert_eq!(facts[1], Fact::new("OS image", "Debian"));

        assert_eq!(
            configmap(&json!({ "data": { "a": "1", "b": "2" } })),
            ["2 keys"]
        );
        assert_eq!(
            secret(&json!({ "type": "kubernetes.io/tls", "data": { "tls.crt": "" } })),
            ["kubernetes.io/tls", "1 key"]
        );
        let claim = json!({
            "spec": { "accessModes": ["ReadWriteOnce"], "storageClassName": "standard", "resources": { "requests": { "storage": "1Gi" } } },
            "status": { "phase": "Pending" }
        });
        assert_eq!(pvc(&claim), ["Pending", "1Gi", "ReadWriteOnce", "standard"]);
    }
}
