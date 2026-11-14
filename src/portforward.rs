//! Forwarding a pod port to localhost.

use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use kube::Client;

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
