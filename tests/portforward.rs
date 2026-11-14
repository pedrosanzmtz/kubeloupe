//! Integration tests of port-forwarding (`PortForward`) against the kind
//! cluster. `#[ignore]`d like `cluster.rs`; run with
//! `cargo test --test portforward -- --ignored`. Each test works in its own
//! temporary namespace and binds only random local ports, so tests (and
//! other worktrees) never fight over a port.

use std::future::Future;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::Client;
use kubeloupe::{
    client_for_context, pod_is_ready, ForwardKind, ForwardTarget, LocalPort, PortForward,
    RemotePort,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(120);

async fn connect() -> Result<Client> {
    let ctx = std::env::var("KUBELOUPE_TEST_CONTEXT")
        .unwrap_or_else(|_| "kind-kubeloupe-test".to_string());
    tokio::time::timeout(Duration::from_secs(20), client_for_context(Some(&ctx)))
        .await
        .context("timed out building client")?
        .with_context(|| format!("cannot connect to context {ctx}"))
}

/// Run `body` in a fresh namespace, deleting it afterwards even on error.
async fn in_temp_ns<F, Fut>(prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(Client, String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let client = connect().await?;
    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("{prefix}-{suffix:x}");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": ns }
    }))?;
    namespaces.create(&PostParams::default(), &obj).await?;
    let result = tokio::time::timeout(Duration::from_secs(300), body(client.clone(), ns.clone()))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("test timed out")));
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

/// Poll `f` until it returns `Some`, or fail after `d`.
async fn eventually<T, Fut: Future<Output = Result<Option<T>>>>(
    what: &str,
    d: Duration,
    mut f: impl FnMut() -> Fut,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + d;
    loop {
        if let Some(v) = f().await? {
            return Ok(v);
        }
        if tokio::time::Instant::now() > deadline {
            bail!("timed out after {d:?} waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// A 2-replica nginx Deployment with a named container port, and a Service
/// whose `targetPort` is that name.
async fn create_web(client: &Client, ns: &str) -> Result<()> {
    let deploy: Deployment = serde_json::from_value(serde_json::json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": { "name": "web" },
        "spec": {
            "replicas": 2,
            "selector": { "matchLabels": { "app": "pf-web" } },
            "template": {
                "metadata": { "labels": { "app": "pf-web" } },
                "spec": {
                    "terminationGracePeriodSeconds": 0,
                    "containers": [{
                        "name": "nginx", "image": "nginx:1.27",
                        "ports": [{ "name": "web", "containerPort": 80 }],
                        "readinessProbe": { "httpGet": { "path": "/", "port": "web" }, "periodSeconds": 1 }
                    }]
                }
            }
        }
    }))?;
    Api::<Deployment>::namespaced(client.clone(), ns)
        .create(&PostParams::default(), &deploy)
        .await?;
    let svc: Service = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Service",
        "metadata": { "name": "web" },
        "spec": {
            "selector": { "app": "pf-web" },
            "ports": [{ "name": "http", "port": 80, "targetPort": "web" }]
        }
    }))?;
    Api::<Service>::namespaced(client.clone(), ns)
        .create(&PostParams::default(), &svc)
        .await?;
    wait_ready_pods(client, ns, 2).await?;
    Ok(())
}

async fn ready_pods(client: &Client, ns: &str) -> Result<Vec<String>> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let list = pods
        .list(&ListParams::default().labels("app=pf-web"))
        .await?;
    Ok(list
        .items
        .iter()
        .filter(|p| pod_is_ready(p))
        .filter_map(|p| p.metadata.name.clone())
        .collect())
}

async fn wait_ready_pods(client: &Client, ns: &str, n: usize) -> Result<Vec<String>> {
    eventually(&format!("{n} ready web pods"), TIMEOUT, || async {
        let ready = ready_pods(client, ns).await?;
        Ok((ready.len() >= n).then_some(ready))
    })
    .await
}

async fn http_get(port: u16) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut buf = String::new();
        s.read_to_string(&mut buf).await?;
        Ok(buf)
    })
    .await
    .context("http request timed out")?
}

fn target(kind: ForwardKind, ns: &str, name: &str) -> ForwardTarget {
    ForwardTarget {
        kind,
        namespace: ns.to_string(),
        name: name.to_string(),
    }
}

#[tokio::test]
#[ignore]
async fn service_forward_survives_deleting_the_serving_pod() -> Result<()> {
    in_temp_ns("kl-it-pf", |client, ns| async move {
        create_web(&client, &ns).await?;
        // EndpointSlices lag the pods a little.
        let svc = target(ForwardKind::Service, &ns, "web");
        let fwd = eventually("service forward starts", TIMEOUT, || async {
            match PortForward::start(
                &client,
                svc.clone(),
                RemotePort::Number(80),
                LocalPort::Random,
            )
            .await
            {
                Ok(f) => Ok(Some(f)),
                Err(e) if e.to_string().contains("no ready pod") => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await?;
        ensure!(fwd.remote_port == 80);
        let body = http_get(fwd.local_port).await?;
        ensure!(body.contains("Welcome to nginx"), "{body}");
        let first = fwd.status().pod;
        ensure!(!first.is_empty(), "{:?}", fwd.status());

        // Delete the pod that served it: the next request goes elsewhere.
        Api::<Pod>::namespaced(client.clone(), &ns)
            .delete(&first, &DeleteParams::default())
            .await?;
        let body = eventually("a request after the pod is gone", TIMEOUT, || async {
            let body = http_get(fwd.local_port).await.unwrap_or_default();
            Ok(body.contains("Welcome to nginx").then_some(body))
        })
        .await?;
        ensure!(body.contains(" 200 OK"), "{body}");
        let now = fwd.status();
        ensure!(now.pod != first, "still served by the deleted pod: {now:?}");
        ensure!(now.total_connections >= 2, "{now:?}");

        // By name, through the Deployment as well.
        let deploy = target(ForwardKind::Deployment, &ns, "web");
        let fwd = PortForward::start(
            &client,
            deploy,
            RemotePort::Name("web".into()),
            LocalPort::Random,
        )
        .await?;
        ensure!(fwd.remote_port == 80, "{}", fwd.remote_port);
        let body = http_get(fwd.local_port).await?;
        ensure!(body.contains("Welcome to nginx"), "{body}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn pod_forward_by_port_name_and_clear_errors() -> Result<()> {
    in_temp_ns("kl-it-pf", |client, ns| async move {
        create_web(&client, &ns).await?;
        let pod = ready_pods(&client, &ns).await?.remove(0);

        let fwd = PortForward::start(
            &client,
            target(ForwardKind::Pod, &ns, &pod),
            RemotePort::Name("web".into()),
            LocalPort::Random,
        )
        .await?;
        let body = http_get(fwd.local_port).await?;
        ensure!(body.contains("Welcome to nginx"), "{body}");

        // Asking for the local port that is now taken.
        let err = PortForward::start(
            &client,
            target(ForwardKind::Service, &ns, "web"),
            RemotePort::Number(80),
            LocalPort::Exactly(fwd.local_port),
        )
        .await
        .err()
        .context("a busy local port was accepted")?;
        ensure!(
            err.to_string()
                .contains(&format!("local port {} is already in use", fwd.local_port)),
            "{err}"
        );

        let err = PortForward::start(
            &client,
            target(ForwardKind::Service, &ns, "web"),
            RemotePort::Number(81),
            LocalPort::Random,
        )
        .await
        .err()
        .context("a missing service port was accepted")?;
        ensure!(
            err.to_string().contains(&format!(
                "service {ns}/web: has no port 81 (ports: 80/http -> web)"
            )),
            "{err}"
        );

        let err = PortForward::start(
            &client,
            target(ForwardKind::Service, &ns, "nope"),
            RemotePort::Number(80),
            LocalPort::Random,
        )
        .await
        .err()
        .context("a missing service was accepted")?;
        ensure!(
            err.to_string()
                .contains(&format!("service {ns}/nope not found")),
            "{err}"
        );
        Ok(())
    })
    .await
}
