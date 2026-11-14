//! Port-forwarding Pods, Services and workloads to localhost.
//!
//! kube-rs's `Portforwarder` hands out a raw stream per port and binds
//! nothing itself, so [`PortForward::start`] binds a local TCP listener and,
//! for each incoming connection, picks a backend pod that is ready right now
//! ([`resolve`]) and opens a fresh port-forward to it. That is the kubectl
//! semantics (several clients can connect at once), except that a Service or
//! workload forward follows its pods: when the pod serving it goes away the
//! next connection goes to another one.

mod resolve;
mod spec;

pub use resolve::{
    container_port, describe_service_port, find_service_port, pod_is_ready, slice_backends, Backend,
};
pub use spec::{ForwardKind, ForwardTarget, LocalPort, PortSpec, RemotePort};

use crate::actions::describe_error;
use anyhow::{bail, Context, Result};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::Api;
use kube::Client;
use resolve::Resolver;
use serde::Serialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

/// How many ready backends one connection tries before giving up.
const TRIES_PER_CONNECTION: usize = 3;
/// How long to wait for the remote error message after a connection closes.
const ERROR_GRACE: Duration = Duration::from_millis(300);

/// Live state of a forward.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ForwardStatus {
    /// The pod the last connection went to (empty before the first one).
    pub pod: String,
    /// Connections open right now.
    pub connections: u32,
    /// Connections served since the start.
    pub total_connections: u64,
    /// What went wrong last, in plain words; empty once a later connection
    /// got an answer.
    pub last_error: String,
}

#[derive(Default)]
struct Shared {
    connections: AtomicU32,
    total: AtomicU64,
    /// `(pod, last_error)`.
    text: Mutex<(String, String)>,
}

impl Shared {
    fn set_pod(&self, pod: &str) {
        self.text.lock().unwrap().0 = pod.to_string();
    }
    fn set_error(&self, err: String) {
        tracing::debug!("port-forward: {err}");
        self.text.lock().unwrap().1 = err;
    }
    fn clear_error(&self) {
        self.text.lock().unwrap().1.clear();
    }
    fn pod(&self) -> String {
        self.text.lock().unwrap().0.clone()
    }
}

/// A running forward. Dropping it (or [`PortForward::stop`]) stops the
/// listener and every open connection.
pub struct PortForward {
    pub target: ForwardTarget,
    pub remote: RemotePort,
    /// The remote port number (the Service port, or the pod port).
    pub remote_port: u16,
    pub local_port: u16,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl PortForward {
    /// Resolve the target, bind the local port, check that the API server
    /// can forward to a backend, then serve until stopped. Fails with a
    /// plain message when the target or port does not exist, no pod is
    /// ready, or the local port is taken.
    pub async fn start(
        client: &Client,
        target: ForwardTarget,
        remote: RemotePort,
        local: LocalPort,
    ) -> Result<Self> {
        let resolver = Resolver::new(client, &target, &remote)
            .await
            .map_err(|e| anyhow::anyhow!(describe_error(&e)))?;
        let ns = target.namespace.clone();
        let backends = resolver
            .backends(client, &ns)
            .await
            .with_context(|| describe_target(&target))?;
        let Some(first) = backends.first().cloned() else {
            bail!("no ready pod backs {}", describe_target(&target));
        };
        let remote_port = resolver.remote_number().unwrap_or(first.port);

        let listener = bind_local(local, remote_port).await?;
        let local_port = listener.local_addr()?.port();

        // Probe: fail now instead of on the first connection.
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        let mut probe = pods
            .portforward(&first.pod, &[first.port])
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "cannot forward to pod {ns}/{}: {}",
                    first.pod,
                    describe_error(&anyhow::Error::from(e))
                )
            })?;
        let _ = probe.take_stream(first.port);
        probe.abort();

        let shared = Arc::new(Shared::default());
        shared.set_pod(&first.pod);
        let task = tokio::spawn(serve(
            listener,
            client.clone(),
            ns,
            resolver,
            shared.clone(),
            describe_target(&target),
        ));
        Ok(Self {
            target,
            remote,
            remote_port,
            local_port,
            shared,
            task,
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub fn status(&self) -> ForwardStatus {
        let (pod, last_error) = self.shared.text.lock().unwrap().clone();
        ForwardStatus {
            pod,
            connections: self.shared.connections.load(Ordering::Relaxed),
            total_connections: self.shared.total.load(Ordering::Relaxed),
            last_error,
        }
    }

    pub fn stop(&self) {
        self.task.abort();
    }

    /// Wait until the forward stops (it only stops when aborted).
    pub async fn wait(&mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `service demo/api`, for messages.
fn describe_target(t: &ForwardTarget) -> String {
    let kind = match t.kind {
        ForwardKind::Pod => "pod",
        ForwardKind::Service => "service",
        ForwardKind::Deployment => "deployment",
        ForwardKind::StatefulSet => "statefulset",
        ForwardKind::ReplicaSet => "replicaset",
        ForwardKind::DaemonSet => "daemonset",
    };
    format!("{kind} {}/{}", t.namespace, t.name)
}

/// Bind `127.0.0.1` on the requested port, or a random one.
async fn bind_local(local: LocalPort, remote: u16) -> Result<TcpListener> {
    let Some(port) = local.preferred(remote) else {
        return Ok(TcpListener::bind(("127.0.0.1", 0)).await?);
    };
    match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => Ok(l),
        // Only the automatic choice falls back to a random port.
        Err(_) if local == LocalPort::Auto => Ok(TcpListener::bind(("127.0.0.1", 0)).await?),
        Err(e) => Err(bind_error(port, &e)),
    }
}

fn bind_error(port: u16, e: &std::io::Error) -> anyhow::Error {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => anyhow::anyhow!(
            "local port {port} is already in use: pick another one, or leave it empty for a free one"
        ),
        std::io::ErrorKind::PermissionDenied => anyhow::anyhow!(
            "not allowed to listen on local port {port}: ports below 1024 need extra privileges"
        ),
        _ => anyhow::anyhow!("cannot listen on local port {port}: {e}"),
    }
}

async fn serve(
    listener: TcpListener,
    client: Client,
    namespace: String,
    resolver: Resolver,
    shared: Arc<Shared>,
    what: String,
) {
    let resolver = Arc::new(resolver);
    let what = Arc::new(what);
    // Connection tasks are aborted with the listener.
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((socket, _)) = accepted else { break };
                conns.spawn(connect(
                    socket,
                    client.clone(),
                    namespace.clone(),
                    resolver.clone(),
                    shared.clone(),
                    what.clone(),
                ));
            }
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
}

/// Decrements the open-connection count when a connection ends.
struct Open(Arc<Shared>);
impl Drop for Open {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Serve one local connection: pick a ready backend (the current one if it
/// still is), forward, record what went wrong.
async fn connect(
    mut socket: tokio::net::TcpStream,
    client: Client,
    namespace: String,
    resolver: Arc<Resolver>,
    shared: Arc<Shared>,
    what: Arc<String>,
) {
    let mut backends = match resolver.backends(&client, &namespace).await {
        Ok(b) if b.is_empty() => {
            shared.set_error(format!("no ready pod backs {what}"));
            return;
        }
        Ok(b) => b,
        Err(e) => {
            shared.set_error(format!(
                "cannot find a pod for {what}: {}",
                describe_error(&e)
            ));
            return;
        }
    };
    let current = shared.pod();
    if let Some(i) = backends.iter().position(|b| b.pod == current) {
        backends.swap(0, i);
    }
    let pods: Api<Pod> = Api::namespaced(client, &namespace);
    for b in backends.into_iter().take(TRIES_PER_CONNECTION) {
        let mut pf = match pods.portforward(&b.pod, &[b.port]).await {
            Ok(pf) => pf,
            Err(e) => {
                shared.set_error(format!(
                    "pod {}: {}",
                    b.pod,
                    describe_error(&anyhow::Error::from(e))
                ));
                continue;
            }
        };
        let (Some(mut stream), Some(remote_error)) =
            (pf.take_stream(b.port), pf.take_error(b.port))
        else {
            pf.abort();
            continue;
        };
        shared.set_pod(&b.pod);
        shared.connections.fetch_add(1, Ordering::Relaxed);
        shared.total.fetch_add(1, Ordering::Relaxed);
        let _open = Open(shared.clone());
        let copied = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        drop(stream);
        let message = tokio::time::timeout(ERROR_GRACE, remote_error)
            .await
            .ok()
            .flatten();
        pf.abort();
        match (message, copied) {
            (Some(msg), _) => shared.set_error(format!("pod {} port {}: {msg}", b.pod, b.port)),
            (None, Ok((_, received))) if received > 0 => shared.clear_error(),
            (None, Ok(_)) => {}
            (None, Err(e)) => shared.set_error(format!("pod {} port {}: {e}", b.pod, b.port)),
        }
        return;
    }
}

/// One port of a Service, for the UI's per-port Forward buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ServicePortInfo {
    /// Empty when the port has no name.
    pub name: String,
    pub port: u16,
    pub protocol: String,
    /// The `targetPort`: a number or a container port name.
    pub target_port: String,
}

/// The ports of a Service.
pub async fn service_ports(
    client: &Client,
    namespace: &str,
    name: &str,
) -> Result<Vec<ServicePortInfo>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let svc = services
        .get_opt(name)
        .await?
        .with_context(|| format!("service {namespace}/{name} not found"))?;
    Ok(svc
        .spec
        .and_then(|s| s.ports)
        .unwrap_or_default()
        .into_iter()
        .map(|p| ServicePortInfo {
            name: p.name.clone().unwrap_or_default(),
            port: u16::try_from(p.port).unwrap_or(0),
            protocol: p.protocol.clone().unwrap_or_else(|| "TCP".to_string()),
            target_port: match &p.target_port {
                Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(n)) => {
                    n.to_string()
                }
                Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s)) => {
                    s.clone()
                }
                None => p.port.to_string(),
            },
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_taken_local_port_is_reported_plainly() {
        let taken = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let err = bind_local(LocalPort::Exactly(port), 80).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "local port {port} is already in use: pick another one, or leave it empty for a free one"
            )
        );
        // The automatic choice falls back to a free port instead.
        let l = bind_local(LocalPort::Auto, port).await.unwrap();
        assert_ne!(l.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn auto_uses_the_remote_port_when_free() {
        // Find a free port >= 1024, release it, then ask for it.
        let probe = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let l = bind_local(LocalPort::Auto, port).await.unwrap();
        assert_eq!(l.local_addr().unwrap().port(), port);
        // Below 1024 it never tries the remote port.
        let l = bind_local(LocalPort::Auto, 80).await.unwrap();
        assert_ne!(l.local_addr().unwrap().port(), 80);
    }
}
