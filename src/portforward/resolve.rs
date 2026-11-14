//! Which pod and port a forward connects to right now.
//!
//! A pod target always uses that pod. A Service maps its port to the
//! `targetPort` and picks a ready endpoint from its EndpointSlices; a
//! workload picks a ready pod by its selector. Both are asked again for
//! every new local connection, so a forward follows pod restarts.

use super::spec::{ForwardKind, ForwardTarget, RemotePort};
use crate::workload_selector;
use anyhow::{bail, Context, Result};
use k8s_openapi::api::core::v1::{Pod, Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, ListParams};
use kube::Client;

/// A pod port to connect a local connection to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    pub pod: String,
    pub port: u16,
}

/// How to find the backends of a forward.
#[derive(Debug, Clone)]
pub(crate) enum Resolver {
    Pod {
        name: String,
        port: u16,
    },
    Service {
        name: String,
        port: ServicePort,
    },
    Workload {
        selector: String,
        remote: RemotePort,
    },
}

impl Resolver {
    /// Look the target up once. Fails when it does not exist or has no such
    /// port.
    pub(crate) async fn new(
        client: &Client,
        target: &ForwardTarget,
        remote: &RemotePort,
    ) -> Result<Self> {
        let ns = &target.namespace;
        let name = &target.name;
        match target.kind {
            ForwardKind::Pod => {
                let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
                let pod = pods
                    .get_opt(name)
                    .await?
                    .with_context(|| format!("pod {ns}/{name} not found"))?;
                let port = container_port(&pod, remote).with_context(|| {
                    format!(
                        "pod {ns}/{name} has no port named '{remote}' (ports: {})",
                        list_or_none(&container_ports(&pod))
                    )
                })?;
                Ok(Self::Pod {
                    name: name.clone(),
                    port,
                })
            }
            ForwardKind::Service => {
                let services: Api<Service> = Api::namespaced(client.clone(), ns);
                let svc = services
                    .get_opt(name)
                    .await?
                    .with_context(|| format!("service {ns}/{name} not found"))?;
                let spec = svc.spec.unwrap_or_default();
                if spec.type_.as_deref() == Some("ExternalName") {
                    bail!("service {ns}/{name} is an ExternalName service: no pod backs it");
                }
                let port = find_service_port(spec.ports.as_deref().unwrap_or_default(), remote)
                    .with_context(|| format!("service {ns}/{name}"))?;
                Ok(Self::Service {
                    name: name.clone(),
                    port,
                })
            }
            kind => {
                let wk = kind.workload().expect("a workload kind");
                let selector = workload_selector(client, wk, ns, name).await?;
                Ok(Self::Workload {
                    selector,
                    remote: remote.clone(),
                })
            }
        }
    }

    /// The port number on the target's own side: the Service port, or the
    /// container port. `None` for a named workload port until a pod is seen.
    pub(crate) fn remote_number(&self) -> Option<u16> {
        match self {
            Self::Pod { port, .. } => Some(*port),
            Self::Service { port, .. } => u16::try_from(port.port).ok(),
            Self::Workload {
                remote: RemotePort::Number(n),
                ..
            } => Some(*n),
            Self::Workload { .. } => None,
        }
    }

    /// The ready backends right now, best first. Empty when none is ready.
    pub(crate) async fn backends(&self, client: &Client, namespace: &str) -> Result<Vec<Backend>> {
        match self {
            Self::Pod { name, port } => Ok(vec![Backend {
                pod: name.clone(),
                port: *port,
            }]),
            Self::Service { name, port } => {
                let slices: Api<EndpointSlice> = Api::namespaced(client.clone(), namespace);
                let lp =
                    ListParams::default().labels(&format!("kubernetes.io/service-name={name}"));
                let list = slices.list(&lp).await?;
                Ok(slice_backends(&list.items, port))
            }
            Self::Workload { selector, remote } => {
                let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
                let list = pods.list(&ListParams::default().labels(selector)).await?;
                let mut out: Vec<Backend> = list
                    .items
                    .iter()
                    .filter(|p| pod_is_ready(p))
                    .filter_map(|p| {
                        Some(Backend {
                            pod: p.metadata.name.clone()?,
                            port: container_port(p, remote)?,
                        })
                    })
                    .collect();
                out.sort_by(|a, b| a.pod.cmp(&b.pod));
                Ok(out)
            }
        }
    }
}

/// The Service port named or numbered `remote`.
pub fn find_service_port(ports: &[ServicePort], remote: &RemotePort) -> Result<ServicePort> {
    let found = ports.iter().find(|p| match remote {
        RemotePort::Number(n) => p.port == i32::from(*n),
        RemotePort::Name(s) => p.name.as_deref() == Some(s.as_str()),
    });
    let Some(port) = found else {
        let listed: Vec<String> = ports.iter().map(describe_service_port).collect();
        bail!("has no port {remote} (ports: {})", list_or_none(&listed));
    };
    if port.protocol.as_deref().is_some_and(|p| p != "TCP") {
        bail!(
            "port {remote} is {}: port-forward only supports TCP",
            port.protocol.as_deref().unwrap_or_default()
        );
    }
    Ok(port.clone())
}

/// `80/http -> 8080`, for messages and the UI.
pub fn describe_service_port(p: &ServicePort) -> String {
    let target = match &p.target_port {
        Some(IntOrString::Int(n)) => n.to_string(),
        Some(IntOrString::String(s)) => s.clone(),
        None => p.port.to_string(),
    };
    match p.name.as_deref() {
        Some(name) if !name.is_empty() => format!("{}/{name} -> {target}", p.port),
        _ => format!("{} -> {target}", p.port),
    }
}

/// The ready pod endpoints of a Service port in its EndpointSlices, sorted
/// by pod name. The slice's port (matched by the Service port's name) is
/// the resolved `targetPort`, so a named `targetPort` needs no pod lookup;
/// a numeric one is used as is.
pub fn slice_backends(slices: &[EndpointSlice], port: &ServicePort) -> Vec<Backend> {
    let want = port.name.as_deref().unwrap_or("");
    let fallback = match &port.target_port {
        None => u16::try_from(port.port).ok(),
        Some(IntOrString::Int(n)) => u16::try_from(*n).ok(),
        Some(IntOrString::String(_)) => None,
    };
    let mut out = Vec::new();
    for slice in slices {
        let from_slice = slice
            .ports
            .iter()
            .flatten()
            .find(|p| p.name.as_deref().unwrap_or("") == want)
            .and_then(|p| p.port)
            .and_then(|n| u16::try_from(n).ok());
        let Some(number) = from_slice.or(fallback) else {
            continue;
        };
        for ep in &slice.endpoints {
            // `ready` unset means ready; terminating endpoints are not ready.
            let ready = ep.conditions.as_ref().and_then(|c| c.ready).unwrap_or(true);
            let pod = ep
                .target_ref
                .as_ref()
                .filter(|r| r.kind.as_deref() == Some("Pod"))
                .and_then(|r| r.name.clone());
            if let (true, Some(pod)) = (ready, pod) {
                if !out.iter().any(|b: &Backend| b.pod == pod) {
                    out.push(Backend { pod, port: number });
                }
            }
        }
    }
    out.sort_by(|a, b| a.pod.cmp(&b.pod));
    out
}

/// The pod's port for `remote`: a number as is, a name looked up in the
/// containers' ports.
pub fn container_port(pod: &Pod, remote: &RemotePort) -> Option<u16> {
    match remote {
        RemotePort::Number(n) => Some(*n),
        RemotePort::Name(name) => pod
            .spec
            .as_ref()?
            .containers
            .iter()
            .flat_map(|c| c.ports.iter().flatten())
            .find(|p| p.name.as_deref() == Some(name.as_str()))
            .and_then(|p| u16::try_from(p.container_port).ok()),
    }
}

fn container_ports(pod: &Pod) -> Vec<String> {
    pod.spec
        .iter()
        .flat_map(|s| s.containers.iter())
        .flat_map(|c| c.ports.iter().flatten())
        .map(|p| match p.name.as_deref() {
            Some(n) => format!("{}/{n}", p.container_port),
            None => p.container_port.to_string(),
        })
        .collect()
}

/// Running, Ready and not being deleted.
pub fn pod_is_ready(pod: &Pod) -> bool {
    pod.metadata.deletion_timestamp.is_none()
        && pod
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc_port(v: serde_json::Value) -> ServicePort {
        serde_json::from_value(v).unwrap()
    }

    fn slice(v: serde_json::Value) -> EndpointSlice {
        let mut base = json!({
            "apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
            "metadata": { "name": "s" }, "addressType": "IPv4",
        });
        base.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn endpoint(pod: &str, ready: Option<bool>) -> serde_json::Value {
        json!({
            "addresses": ["10.0.0.1"],
            "conditions": { "ready": ready },
            "targetRef": { "kind": "Pod", "name": pod },
        })
    }

    #[test]
    fn service_port_by_number_or_name() {
        let ports = vec![
            svc_port(json!({ "name": "http", "port": 80, "targetPort": "web" })),
            svc_port(json!({ "name": "metrics", "port": 9090, "targetPort": 9091 })),
            svc_port(json!({ "name": "dns", "port": 53, "protocol": "UDP" })),
        ];
        let p = find_service_port(&ports, &RemotePort::Number(80)).unwrap();
        assert_eq!(p.name.as_deref(), Some("http"));
        let p = find_service_port(&ports, &RemotePort::Name("metrics".into())).unwrap();
        assert_eq!(p.port, 9090);
        let err = find_service_port(&ports, &RemotePort::Number(81)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "has no port 81 (ports: 80/http -> web, 9090/metrics -> 9091, 53/dns -> 53)"
        );
        let err = find_service_port(&ports, &RemotePort::Number(53)).unwrap_err();
        assert!(err.to_string().contains("only supports TCP"), "{err}");
    }

    #[test]
    fn named_target_port_resolves_from_the_slice() {
        let port = svc_port(json!({ "name": "http", "port": 80, "targetPort": "web" }));
        let slices = vec![
            slice(json!({
                "ports": [{ "name": "http", "port": 8080 }, { "name": "metrics", "port": 9091 }],
                "endpoints": [endpoint("b", Some(true)), endpoint("a", None), endpoint("gone", Some(false))],
            })),
            // Pods whose named port has another number are in their own slice.
            slice(json!({
                "ports": [{ "name": "http", "port": 8081 }],
                "endpoints": [endpoint("c", Some(true))],
            })),
        ];
        assert_eq!(
            slice_backends(&slices, &port),
            vec![
                Backend {
                    pod: "a".into(),
                    port: 8080
                },
                Backend {
                    pod: "b".into(),
                    port: 8080
                },
                Backend {
                    pod: "c".into(),
                    port: 8081
                },
            ]
        );
    }

    #[test]
    fn numeric_target_port_and_unnamed_service_port() {
        let port = svc_port(json!({ "port": 80, "targetPort": 8080 }));
        let s =
            slice(json!({ "ports": [{ "port": 8080 }], "endpoints": [endpoint("a", Some(true))] }));
        assert_eq!(slice_backends(&[s], &port)[0].port, 8080);
        // No ports in the slice: the numeric targetPort is used.
        let s = slice(json!({ "endpoints": [endpoint("a", Some(true))] }));
        assert_eq!(slice_backends(&[s], &port)[0].port, 8080);
        // A named targetPort with nothing in the slice cannot be resolved.
        let named = svc_port(json!({ "port": 80, "targetPort": "web" }));
        let s = slice(json!({ "endpoints": [endpoint("a", Some(true))] }));
        assert!(slice_backends(&[s], &named).is_empty());
    }

    #[test]
    fn named_container_port() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "p" },
            "spec": { "containers": [
                { "name": "a", "ports": [{ "containerPort": 8080, "name": "web" }] },
                { "name": "b", "ports": [{ "containerPort": 9000, "name": "admin" }] },
            ]}
        }))
        .unwrap();
        assert_eq!(
            container_port(&pod, &RemotePort::Name("admin".into())),
            Some(9000)
        );
        assert_eq!(container_port(&pod, &RemotePort::Name("nope".into())), None);
        assert_eq!(container_port(&pod, &RemotePort::Number(1234)), Some(1234));
        assert_eq!(container_ports(&pod), vec!["8080/web", "9000/admin"]);
    }

    #[test]
    fn readiness() {
        let mut pod: Pod = serde_json::from_value(json!({
            "metadata": { "name": "p" },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }] }
        }))
        .unwrap();
        assert!(pod_is_ready(&pod));
        pod.metadata.deletion_timestamp = Some(
            k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(Default::default()),
        );
        assert!(!pod_is_ready(&pod));
    }
}
