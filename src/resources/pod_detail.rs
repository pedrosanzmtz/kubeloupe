//! The pod section of the detail panel: pod-level facts plus a summary of
//! every container (init, regular, ephemeral). Env values never leave here:
//! only variable names and where `valueFrom` points.

use super::columns::age_text;
use crate::LogContainer;
use k8s_openapi::api::core::v1::{
    ContainerState, ContainerStatus, EnvFromSource, EnvVar, Pod, Probe, ResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use serde::Serialize;
use std::collections::BTreeMap;

/// Pod-only part of a [`super::ResourceDetail`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PodExtras {
    pub phase: String,
    /// Why the pod is in that phase (`Evicted`, ...), if the kubelet says.
    pub reason: String,
    pub message: String,
    pub node: String,
    pub pod_ips: Vec<String>,
    pub host_ip: String,
    pub qos_class: String,
    pub priority_class: String,
    pub service_account: String,
    /// Init containers first, then regular, then ephemeral.
    pub containers: Vec<ContainerDetail>,
    /// Every init and regular container, for the log picker.
    pub log_containers: Vec<LogContainer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum ContainerKind {
    Init,
    Regular,
    Ephemeral,
}

/// A container state (current or last). `state` is empty when there is none.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct StateInfo {
    /// `running`, `waiting`, `terminated`, or empty.
    pub state: String,
    pub reason: String,
    pub message: String,
    /// Only meaningful when terminated.
    pub exit_code: i32,
    /// Age of the start (running) or the finish (terminated).
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PortInfo {
    pub name: String,
    pub port: i32,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ProbeInfo {
    /// `liveness`, `readiness` or `startup`.
    pub kind: String,
    /// Like `kubectl describe`: `http-get :8080/healthz delay=5s timeout=1s period=10s #success=1 #failure=3`.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct MountInfo {
    pub name: String,
    pub path: String,
    pub sub_path: String,
    pub read_only: bool,
}

/// An env variable. Literal values are never included.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct EnvInfo {
    /// The variable, or `<prefix>*` for a whole `envFrom` source.
    pub name: String,
    /// Empty for a literal value, else `secret:name/key`, `configmap:name/key`,
    /// `field:metadata.name`, `resource:limits.cpu`, or for `envFrom`
    /// `secret:name` / `configmap:name`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContainerDetail {
    pub name: String,
    pub kind: ContainerKind,
    pub image: String,
    pub image_id: String,
    pub state: StateInfo,
    pub last_state: StateInfo,
    pub ready: bool,
    pub restarts: i32,
    pub requests: BTreeMap<String, String>,
    pub limits: BTreeMap<String, String>,
    pub ports: Vec<PortInfo>,
    pub probes: Vec<ProbeInfo>,
    pub mounts: Vec<MountInfo>,
    pub env: Vec<EnvInfo>,
}

/// Everything the spec says about one container, whatever its kind.
struct SpecView<'a> {
    name: &'a str,
    image: Option<&'a String>,
    resources: Option<&'a ResourceRequirements>,
    ports: Vec<PortInfo>,
    probes: [(&'static str, Option<&'a Probe>); 3],
    mounts: Vec<MountInfo>,
    env: Option<&'a Vec<EnvVar>>,
    env_from: Option<&'a Vec<EnvFromSource>>,
}

macro_rules! spec_view {
    ($c:expr) => {{
        let c = $c;
        SpecView {
            name: &c.name,
            image: c.image.as_ref(),
            resources: c.resources.as_ref(),
            ports: c
                .ports
                .iter()
                .flatten()
                .map(|p| PortInfo {
                    name: p.name.clone().unwrap_or_default(),
                    port: p.container_port,
                    protocol: p.protocol.clone().unwrap_or_else(|| "TCP".into()),
                })
                .collect(),
            probes: [
                ("liveness", c.liveness_probe.as_ref()),
                ("readiness", c.readiness_probe.as_ref()),
                ("startup", c.startup_probe.as_ref()),
            ],
            mounts: c
                .volume_mounts
                .iter()
                .flatten()
                .map(|m| MountInfo {
                    name: m.name.clone(),
                    path: m.mount_path.clone(),
                    sub_path: m.sub_path.clone().unwrap_or_default(),
                    read_only: m.read_only.unwrap_or(false),
                })
                .collect(),
            env: c.env.as_ref(),
            env_from: c.env_from.as_ref(),
        }
    }};
}

/// The pod section of the detail panel.
pub fn pod_extras(pod: &Pod) -> PodExtras {
    let spec = pod.spec.as_ref();
    let status = pod.status.as_ref();
    let statuses = |kind: ContainerKind| -> &[ContainerStatus] {
        let list = status.and_then(|s| match kind {
            ContainerKind::Init => s.init_container_statuses.as_ref(),
            ContainerKind::Regular => s.container_statuses.as_ref(),
            ContainerKind::Ephemeral => s.ephemeral_container_statuses.as_ref(),
        });
        list.map(Vec::as_slice).unwrap_or(&[])
    };

    let mut containers = Vec::new();
    if let Some(spec) = spec {
        let init = spec.init_containers.iter().flatten();
        let ephemeral = spec.ephemeral_containers.iter().flatten();
        for c in init {
            containers.push(container_detail(
                spec_view!(c),
                ContainerKind::Init,
                statuses(ContainerKind::Init),
            ));
        }
        for c in &spec.containers {
            containers.push(container_detail(
                spec_view!(c),
                ContainerKind::Regular,
                statuses(ContainerKind::Regular),
            ));
        }
        for c in ephemeral {
            containers.push(container_detail(
                spec_view!(c),
                ContainerKind::Ephemeral,
                statuses(ContainerKind::Ephemeral),
            ));
        }
    }
    let log_containers = containers
        .iter()
        .filter(|c| c.kind != ContainerKind::Ephemeral)
        .map(|c| LogContainer {
            name: c.name.clone(),
            init: c.kind == ContainerKind::Init,
        })
        .collect();

    let s = |f: fn(&k8s_openapi::api::core::v1::PodStatus) -> Option<&String>| {
        status.and_then(f).cloned().unwrap_or_default()
    };
    let mut pod_ips: Vec<String> = status
        .and_then(|s| s.pod_ips.as_ref())
        .map(|ips| ips.iter().map(|i| i.ip.clone()).collect())
        .unwrap_or_default();
    if pod_ips.is_empty() {
        pod_ips.extend(status.and_then(|s| s.pod_ip.clone()));
    }
    PodExtras {
        phase: s(|s| s.phase.as_ref()),
        reason: s(|s| s.reason.as_ref()),
        message: s(|s| s.message.as_ref()),
        node: spec.and_then(|s| s.node_name.clone()).unwrap_or_default(),
        pod_ips,
        host_ip: s(|s| s.host_ip.as_ref()),
        qos_class: s(|s| s.qos_class.as_ref()),
        priority_class: spec
            .and_then(|s| s.priority_class_name.clone())
            .unwrap_or_default(),
        service_account: spec
            .and_then(|s| s.service_account_name.clone())
            .unwrap_or_default(),
        containers,
        log_containers,
    }
}

fn container_detail(
    spec: SpecView<'_>,
    kind: ContainerKind,
    statuses: &[ContainerStatus],
) -> ContainerDetail {
    let st = statuses.iter().find(|s| s.name == spec.name);
    let quantities = |pick: fn(&ResourceRequirements) -> Option<&BTreeMap<String, _>>| {
        spec.resources
            .and_then(pick)
            .map(|m| {
                m.iter()
                    .map(
                        |(k, v): (
                            &String,
                            &k8s_openapi::apimachinery::pkg::api::resource::Quantity,
                        )| { (k.clone(), v.0.clone()) },
                    )
                    .collect()
            })
            .unwrap_or_default()
    };
    ContainerDetail {
        name: spec.name.to_string(),
        kind,
        image: spec.image.cloned().unwrap_or_default(),
        image_id: st.map(|s| s.image_id.clone()).unwrap_or_default(),
        state: st
            .and_then(|s| s.state.as_ref())
            .map(state_info)
            .unwrap_or_default(),
        last_state: st
            .and_then(|s| s.last_state.as_ref())
            .map(state_info)
            .unwrap_or_default(),
        ready: st.is_some_and(|s| s.ready),
        restarts: st.map(|s| s.restart_count).unwrap_or(0),
        requests: quantities(|r| r.requests.as_ref()),
        limits: quantities(|r| r.limits.as_ref()),
        ports: spec.ports,
        probes: spec
            .probes
            .iter()
            .filter_map(|(k, p)| {
                p.map(|p| ProbeInfo {
                    kind: (*k).to_string(),
                    summary: probe_summary(p),
                })
            })
            .collect(),
        mounts: spec.mounts,
        env: env_refs(spec.env, spec.env_from),
    }
}

/// A container state as the panel shows it.
pub fn state_info(s: &ContainerState) -> StateInfo {
    let ts = |t: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>| {
        t.map(|t| age_text(&t.0.to_rfc3339())).unwrap_or_default()
    };
    if let Some(r) = &s.running {
        StateInfo {
            state: "running".into(),
            since: ts(r.started_at.as_ref()),
            ..Default::default()
        }
    } else if let Some(t) = &s.terminated {
        StateInfo {
            state: "terminated".into(),
            reason: t.reason.clone().unwrap_or_default(),
            message: t.message.clone().unwrap_or_default(),
            exit_code: t.exit_code,
            since: ts(t.finished_at.as_ref()),
        }
    } else if let Some(w) = &s.waiting {
        StateInfo {
            state: "waiting".into(),
            reason: w.reason.clone().unwrap_or_default(),
            message: w.message.clone().unwrap_or_default(),
            ..Default::default()
        }
    } else {
        StateInfo::default()
    }
}

fn port_text(p: &IntOrString) -> String {
    match p {
        IntOrString::Int(i) => i.to_string(),
        IntOrString::String(s) => s.clone(),
    }
}

/// A probe the way `kubectl describe` prints it.
pub fn probe_summary(p: &Probe) -> String {
    let action = if let Some(h) = &p.http_get {
        let scheme = h.scheme.as_deref().unwrap_or("HTTP");
        let prefix = if scheme.eq_ignore_ascii_case("HTTP") {
            String::new()
        } else {
            format!("{}://", scheme.to_lowercase())
        };
        format!(
            "http-get {prefix}{}:{}{}",
            h.host.as_deref().unwrap_or(""),
            port_text(&h.port),
            h.path.as_deref().unwrap_or("/")
        )
    } else if let Some(t) = &p.tcp_socket {
        format!(
            "tcp-socket {}:{}",
            t.host.as_deref().unwrap_or(""),
            port_text(&t.port)
        )
    } else if let Some(e) = &p.exec {
        format!(
            "exec [{}]",
            e.command
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        )
    } else if let Some(g) = &p.grpc {
        match g.service.as_deref().filter(|s| !s.is_empty()) {
            Some(svc) => format!("grpc :{} {svc}", g.port),
            None => format!("grpc :{}", g.port),
        }
    } else {
        "unknown".to_string()
    };
    format!(
        "{action} delay={}s timeout={}s period={}s #success={} #failure={}",
        p.initial_delay_seconds.unwrap_or(0),
        p.timeout_seconds.unwrap_or(1),
        p.period_seconds.unwrap_or(10),
        p.success_threshold.unwrap_or(1),
        p.failure_threshold.unwrap_or(3),
    )
}

/// Variable names and where `valueFrom` / `envFrom` point. Never values.
pub fn env_refs(env: Option<&Vec<EnvVar>>, env_from: Option<&Vec<EnvFromSource>>) -> Vec<EnvInfo> {
    let mut out: Vec<EnvInfo> = env_from
        .into_iter()
        .flatten()
        .map(|f| {
            let prefix = f.prefix.clone().unwrap_or_default();
            let source = if let Some(s) = &f.secret_ref {
                format!("secret:{}", s.name)
            } else if let Some(c) = &f.config_map_ref {
                format!("configmap:{}", c.name)
            } else {
                String::new()
            };
            EnvInfo {
                name: format!("{prefix}*"),
                source,
            }
        })
        .collect();
    out.extend(env.into_iter().flatten().map(|e| {
        let source = match &e.value_from {
            None => String::new(),
            Some(v) => {
                if let Some(s) = &v.secret_key_ref {
                    format!("secret:{}/{}", s.name, s.key)
                } else if let Some(c) = &v.config_map_key_ref {
                    format!("configmap:{}/{}", c.name, c.key)
                } else if let Some(f) = &v.field_ref {
                    format!("field:{}", f.field_path)
                } else if let Some(r) = &v.resource_field_ref {
                    format!("resource:{}", r.resource)
                } else {
                    "unknown".to_string()
                }
            }
        };
        EnvInfo {
            name: e.name.clone(),
            source,
        }
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(v: serde_json::Value) -> Pod {
        serde_json::from_value(v).unwrap()
    }

    fn probe(v: serde_json::Value) -> Probe {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn probe_text_matches_kubectl_describe() {
        let http = probe(json!({
            "httpGet": { "path": "/healthz", "port": 8080 },
            "initialDelaySeconds": 5, "periodSeconds": 10
        }));
        assert_eq!(
            probe_summary(&http),
            "http-get :8080/healthz delay=5s timeout=1s period=10s #success=1 #failure=3"
        );
        let https = probe(json!({
            "httpGet": { "path": "/ready", "port": "web", "scheme": "HTTPS", "host": "10.0.0.1" },
            "timeoutSeconds": 3, "failureThreshold": 5
        }));
        assert_eq!(
            probe_summary(&https),
            "http-get https://10.0.0.1:web/ready delay=0s timeout=3s period=10s #success=1 #failure=5"
        );
        let tcp = probe(json!({ "tcpSocket": { "port": 5432 } }));
        assert!(probe_summary(&tcp).starts_with("tcp-socket :5432 delay=0s"));
        let exec = probe(json!({ "exec": { "command": ["cat", "/tmp/healthy"] } }));
        assert!(probe_summary(&exec).starts_with("exec [cat /tmp/healthy] "));
        let grpc = probe(json!({ "grpc": { "port": 9090, "service": "health" } }));
        assert!(probe_summary(&grpc).starts_with("grpc :9090 health "));
    }

    #[test]
    fn env_shows_names_and_references_never_values() {
        let p = pod(json!({
            "spec": { "containers": [{
                "name": "app",
                "env": [
                    { "name": "PLAIN", "value": "s3cr3t-literal" },
                    { "name": "DB_PASS", "valueFrom": { "secretKeyRef": { "name": "db", "key": "password" } } },
                    { "name": "MODE", "valueFrom": { "configMapKeyRef": { "name": "cfg", "key": "mode" } } },
                    { "name": "POD", "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } } },
                    { "name": "CPU", "valueFrom": { "resourceFieldRef": { "resource": "limits.cpu" } } }
                ],
                "envFrom": [
                    { "secretRef": { "name": "all-secrets" } },
                    { "configMapRef": { "name": "all-cfg" }, "prefix": "CFG_" }
                ]
            }]}
        }));
        let x = pod_extras(&p);
        let env: Vec<(&str, &str)> = x.containers[0]
            .env
            .iter()
            .map(|e| (e.name.as_str(), e.source.as_str()))
            .collect();
        assert_eq!(
            env,
            [
                ("*", "secret:all-secrets"),
                ("CFG_*", "configmap:all-cfg"),
                ("PLAIN", ""),
                ("DB_PASS", "secret:db/password"),
                ("MODE", "configmap:cfg/mode"),
                ("POD", "field:metadata.name"),
                ("CPU", "resource:limits.cpu"),
            ]
        );
        let text = serde_json::to_string(&x).unwrap();
        assert!(!text.contains("s3cr3t-literal"), "{text}");
    }

    /// A pod like `demo/crashed`: an init container that finished, a
    /// crash-looping app whose last run was OOM-killed, and an ephemeral
    /// debug container.
    fn crashing_pod() -> Pod {
        pod(json!({
            "metadata": { "name": "crashed", "namespace": "demo" },
            "spec": {
                "nodeName": "node-1",
                "serviceAccountName": "default",
                "priorityClassName": "high",
                "initContainers": [{ "name": "init", "image": "busybox:1.36" }],
                "containers": [{
                    "name": "app", "image": "busybox:1.36",
                    "resources": {
                        "requests": { "cpu": "100m", "memory": "64Mi" },
                        "limits": { "memory": "128Mi" }
                    },
                    "ports": [{ "name": "http", "containerPort": 8080 }, { "containerPort": 53, "protocol": "UDP" }],
                    "livenessProbe": { "httpGet": { "path": "/healthz", "port": 8080 } },
                    "volumeMounts": [{ "name": "data", "mountPath": "/data", "readOnly": true, "subPath": "x" }]
                }],
                "ephemeralContainers": [{ "name": "debugger", "image": "busybox:1.36" }]
            },
            "status": {
                "phase": "Running",
                "qosClass": "Burstable",
                "hostIP": "172.18.0.2",
                "podIP": "10.244.1.5",
                "podIPs": [{ "ip": "10.244.1.5" }, { "ip": "fd00::5" }],
                "initContainerStatuses": [{
                    "name": "init", "image": "busybox:1.36", "imageID": "sha256:init",
                    "ready": true, "restartCount": 0,
                    "state": { "terminated": { "exitCode": 0, "reason": "Completed",
                        "finishedAt": "2024-01-01T00:00:00Z" } }
                }],
                "containerStatuses": [{
                    "name": "app", "image": "busybox:1.36", "imageID": "docker.io/library/busybox@sha256:abc",
                    "ready": false, "restartCount": 7,
                    "state": { "waiting": { "reason": "CrashLoopBackOff",
                        "message": "back-off 5m0s restarting failed container" } },
                    "lastState": { "terminated": { "exitCode": 137, "reason": "OOMKilled",
                        "finishedAt": "2024-01-01T00:00:00Z" } }
                }]
            }
        }))
    }

    #[test]
    fn containers_cover_every_kind_with_state_and_last_termination() {
        let x = pod_extras(&crashing_pod());
        let kinds: Vec<(&str, ContainerKind)> = x
            .containers
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("init", ContainerKind::Init),
                ("app", ContainerKind::Regular),
                ("debugger", ContainerKind::Ephemeral)
            ]
        );
        let init = &x.containers[0];
        assert_eq!(init.state.state, "terminated");
        assert_eq!(init.state.reason, "Completed");
        assert_eq!(init.image_id, "sha256:init");

        let app = &x.containers[1];
        assert_eq!(app.state.state, "waiting");
        assert_eq!(app.state.reason, "CrashLoopBackOff");
        assert!(app.state.message.starts_with("back-off"));
        assert_eq!(app.last_state.state, "terminated");
        assert_eq!(app.last_state.reason, "OOMKilled");
        assert_eq!(app.last_state.exit_code, 137);
        assert!(app.last_state.since.ends_with('d'), "{:?}", app.last_state);
        assert_eq!(app.restarts, 7);
        assert!(!app.ready);
        assert_eq!(app.requests.get("cpu").map(String::as_str), Some("100m"));
        assert_eq!(app.limits.get("memory").map(String::as_str), Some("128Mi"));
        assert!(!app.limits.contains_key("cpu"));
        assert_eq!(
            app.ports,
            [
                PortInfo {
                    name: "http".into(),
                    port: 8080,
                    protocol: "TCP".into()
                },
                PortInfo {
                    name: String::new(),
                    port: 53,
                    protocol: "UDP".into()
                },
            ]
        );
        assert_eq!(app.probes.len(), 1);
        assert_eq!(app.probes[0].kind, "liveness");
        assert_eq!(
            app.mounts,
            [MountInfo {
                name: "data".into(),
                path: "/data".into(),
                sub_path: "x".into(),
                read_only: true
            }]
        );

        // No status yet for the ephemeral container.
        let dbg = &x.containers[2];
        assert_eq!(dbg.state, StateInfo::default());
        assert_eq!(dbg.last_state.state, "");

        // Ephemeral containers have no log picker entry.
        let logs: Vec<(&str, bool)> = x
            .log_containers
            .iter()
            .map(|c| (c.name.as_str(), c.init))
            .collect();
        assert_eq!(logs, [("init", true), ("app", false)]);
    }

    #[test]
    fn pod_level_fields() {
        let x = pod_extras(&crashing_pod());
        assert_eq!(x.phase, "Running");
        assert_eq!(x.node, "node-1");
        assert_eq!(x.pod_ips, ["10.244.1.5", "fd00::5"]);
        assert_eq!(x.host_ip, "172.18.0.2");
        assert_eq!(x.qos_class, "Burstable");
        assert_eq!(x.priority_class, "high");
        assert_eq!(x.service_account, "default");

        // A pending pod has no status beyond its phase.
        let p = pod(
            json!({ "spec": { "containers": [{ "name": "c" }] }, "status": { "phase": "Pending" } }),
        );
        let x = pod_extras(&p);
        assert_eq!(x.phase, "Pending");
        assert!(x.pod_ips.is_empty());
        assert_eq!(x.containers[0].state.state, "");
        assert_eq!(x.containers[0].restarts, 0);
    }
}
