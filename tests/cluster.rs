//! Integration tests against a real cluster (kind, via
//! `scripts/test-cluster/setup.sh`). All `#[ignore]`d so plain `cargo test`
//! stays fast; run with `cargo test -- --ignored`.
//!
//! The context comes from `KUBELOUPE_TEST_CONTEXT` (default
//! `kind-kubeloupe-test`). Tests that mutate things use their own temporary
//! namespace and delete it afterwards.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Pod};
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams};
use kube::Client;
use kubeloupe::watch::Update;
use kubeloupe::{
    apply_pod_yaml, client_for_context, exec_command, get_namespaces, get_nodes, get_pod_details,
    get_pod_logs, get_pod_metrics, get_pod_yaml, get_pods, serve_port_forward, stream_pod_logs,
    stream_workload_logs, LogOptions, WorkloadEvent, WorkloadKind,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(90);

async fn connect() -> Result<Client> {
    let ctx = std::env::var("KUBELOUPE_TEST_CONTEXT")
        .unwrap_or_else(|_| "kind-kubeloupe-test".to_string());
    tokio::time::timeout(Duration::from_secs(20), client_for_context(Some(&ctx)))
        .await
        .context("timed out building client")?
        .with_context(|| format!("cannot connect to context {ctx}"))
}

/// Bound a future so a broken cluster fails instead of hanging.
async fn within<T>(what: &str, d: Duration, f: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(d, f)
        .await
        .map_err(|_| anyhow::anyhow!("timed out after {d:?}: {what}"))?
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
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn pod_phase(client: &Client, ns: &str, name: &str) -> Result<Option<String>> {
    let api: Api<Pod> = Api::namespaced(client.clone(), ns);
    Ok(api.get_opt(name).await?.and_then(|p| p.status?.phase))
}

async fn wait_running(client: &Client, ns: &str, name: &str) -> Result<()> {
    eventually(&format!("pod {ns}/{name} Running"), TIMEOUT, || async {
        Ok(pod_phase(client, ns, name)
            .await?
            .filter(|p| p == "Running")
            .map(|_| ()))
    })
    .await
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
    let result = body(client.clone(), ns.clone()).await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

fn sleeper_pod(name: &str) -> Pod {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": { "name": name, "labels": { "app": "it" } },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{
                "name": "c", "image": "busybox:1.36",
                "command": ["sh", "-c", "echo hello-from-it; sleep 3600"]
            }]
        }
    }))
    .unwrap()
}

async fn first_running_demo_pod(client: &Client, prefix: &str) -> Result<String> {
    eventually(
        &format!("a running demo/{prefix}* pod"),
        TIMEOUT,
        || async {
            Ok(get_pods(client, Some("demo"))
                .await?
                .into_iter()
                .find(|p| p.name.starts_with(prefix) && p.phase == "Running")
                .map(|p| p.name))
        },
    )
    .await
}

#[tokio::test]
#[ignore]
async fn lists_pods_namespaces_and_nodes() -> Result<()> {
    let client = connect().await?;
    let pods = within("get_pods", TIMEOUT, get_pods(&client, Some("demo"))).await?;
    for want in ["crashed", "multi"] {
        ensure!(
            pods.iter().any(|p| p.name == want),
            "missing demo pod {want}"
        );
    }
    ensure!(
        pods.iter().any(|p| p.name.starts_with("web-")),
        "no web pod"
    );
    ensure!(
        pods.iter().any(|p| p.name.starts_with("logger-")),
        "no logger pod"
    );
    ensure!(pods.iter().all(|p| p.namespace == "demo"));

    let all = within("get_pods(all)", TIMEOUT, get_pods(&client, None)).await?;
    ensure!(all.iter().any(|p| p.namespace == "kube-system"));

    let namespaces = within("get_namespaces", TIMEOUT, get_namespaces(&client)).await?;
    for want in ["default", "kube-system", "demo"] {
        ensure!(
            namespaces.iter().any(|n| n.name == want),
            "missing ns {want}"
        );
    }

    let nodes = within("get_nodes", TIMEOUT, get_nodes(&client)).await?;
    ensure!(nodes.len() >= 2, "expected 2 nodes, got {}", nodes.len());
    ensure!(nodes.iter().all(|n| n.ready == "True"), "{nodes:?}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn pod_details_lists_log_containers() -> Result<()> {
    let client = connect().await?;
    let d = within(
        "details",
        TIMEOUT,
        get_pod_details(&client, "demo", "multi"),
    )
    .await?;
    let names: Vec<_> = d
        .log_containers
        .iter()
        .map(|c| (c.name.as_str(), c.init))
        .collect();
    ensure!(names.contains(&("init-setup", true)), "{names:?}");
    ensure!(names.contains(&("app", false)), "{names:?}");
    ensure!(names.contains(&("sidecar", false)), "{names:?}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn get_logs_specific_container() -> Result<()> {
    let client = connect().await?;
    wait_running(&client, "demo", "multi").await?;
    for (container, needle, other) in [
        ("app", "app: serving request", "sidecar:"),
        ("sidecar", "sidecar: proxy heartbeat", "app:"),
    ] {
        let opts = LogOptions {
            container: Some(container.into()),
            ..LogOptions::default()
        };
        let logs = eventually(&format!("{container} logs"), TIMEOUT, || async {
            let l = get_pod_logs(&client, "demo", "multi", &opts).await?;
            Ok(l.contains(needle).then_some(l))
        })
        .await?;
        ensure!(!logs.contains(other), "{container} logs leaked {other}");
    }
    let init = LogOptions {
        container: Some("init-setup".into()),
        ..LogOptions::default()
    };
    let logs = get_pod_logs(&client, "demo", "multi", &init).await?;
    ensure!(logs.contains("init-setup: done"), "{logs}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn get_logs_previous_container_of_crashed_pod() -> Result<()> {
    let client = connect().await?;
    let opts = LogOptions {
        container: Some("app".into()),
        previous: true,
        ..LogOptions::default()
    };
    // The first crash can take a moment on a fresh cluster.
    let logs = eventually(
        "previous logs of demo/crashed",
        Duration::from_secs(120),
        || async {
            match get_pod_logs(&client, "demo", "crashed", &opts).await {
                Ok(l) if l.contains("FATAL") => Ok(Some(l)),
                _ => Ok(None),
            }
        },
    )
    .await?;
    ensure!(logs.contains("starting up at"), "{logs}");
    ensure!(logs.contains("FATAL: cannot reach database"), "{logs}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn stream_logs_yields_lines() -> Result<()> {
    let client = connect().await?;
    let pod = first_running_demo_pod(&client, "logger-").await?;
    let opts = LogOptions {
        tail_lines: Some(5),
        ..LogOptions::default()
    };
    let mut lines = Vec::new();
    // The follow stream never ends on its own; give it a bounded time and
    // check what arrived.
    let _ = tokio::time::timeout(
        Duration::from_secs(8),
        stream_pod_logs(&client, "demo", &pod, &opts, |l| lines.push(l)),
    )
    .await;
    ensure!(!lines.is_empty(), "no lines streamed");
    ensure!(
        lines.iter().any(|l| l.contains("hello from logger")),
        "{lines:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn stream_logs_previous_ends() -> Result<()> {
    let client = connect().await?;
    let opts = LogOptions {
        container: Some("app".into()),
        previous: true,
        ..LogOptions::default()
    };
    let lines = eventually(
        "streamed previous logs",
        Duration::from_secs(120),
        || async {
            let mut lines = Vec::new();
            let r = within("stream previous", Duration::from_secs(20), async {
                stream_pod_logs(&client, "demo", "crashed", &opts, |l| lines.push(l)).await
            })
            .await;
            Ok((r.is_ok() && lines.iter().any(|l| l.contains("FATAL"))).then_some(lines))
        },
    )
    .await?;
    ensure!(lines.iter().any(|l| l.contains("FATAL")), "{lines:?}");
    Ok(())
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
#[ignore]
async fn exec_runs_echo() -> Result<()> {
    let client = connect().await?;
    let pod = first_running_demo_pod(&client, "logger-").await?;
    let cmd = strings(&["echo", "kubeloupe-exec"]);
    let r = within(
        "exec echo",
        TIMEOUT,
        exec_command(&client, "demo", &pod, None, &cmd),
    )
    .await?;
    ensure!(r.success, "exec failed: {}", r.stderr);
    ensure!(
        r.stdout.trim() == "kubeloupe-exec",
        "stdout = {:?}",
        r.stdout
    );

    let fail = strings(&["sh", "-c", "echo oops >&2; exit 3"]);
    let r = within(
        "exec fail",
        TIMEOUT,
        exec_command(&client, "demo", &pod, None, &fail),
    )
    .await?;
    ensure!(!r.success, "exit 3 reported as success");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn exec_in_named_container() -> Result<()> {
    let client = connect().await?;
    wait_running(&client, "demo", "multi").await?;
    let cmd = strings(&["hostname"]);
    let r = within(
        "exec sidecar",
        TIMEOUT,
        exec_command(&client, "demo", "multi", Some("sidecar"), &cmd),
    )
    .await?;
    ensure!(
        r.success && r.stdout.trim() == "multi",
        "{:?} {:?}",
        r.stdout,
        r.stderr
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn port_forward_serves_http() -> Result<()> {
    let client = connect().await?;
    let pod = first_running_demo_pod(&client, "web-").await?;
    let (port, handle) = within(
        "serve_port_forward",
        TIMEOUT,
        serve_port_forward(&client, "demo", &pod, 80),
    )
    .await?;
    let res = within("http request", Duration::from_secs(30), async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut buf = String::new();
        s.read_to_string(&mut buf).await?;
        Ok(buf)
    })
    .await;
    handle.abort();
    let body = res?;
    ensure!(body.contains(" 200 OK"), "{body}");
    ensure!(body.contains("Welcome to nginx"), "{body}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn yaml_apply_changes_label_and_reverts() -> Result<()> {
    in_temp_ns("kl-it-yaml", |client, ns| async move {
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        pods.create(&PostParams::default(), &sleeper_pod("p"))
            .await?;
        wait_running(&client, &ns, "p").await?;

        // Drop resourceVersion: it goes stale as soon as the first apply
        // lands, and SSA would then reject the revert with a conflict.
        let original: String = get_pod_yaml(&client, &ns, "p")
            .await?
            .lines()
            .filter(|l| !l.trim_start().starts_with("resourceVersion:"))
            .map(|l| format!("{l}\n"))
            .collect();
        ensure!(original.contains("app: it"), "{original}");

        let edited = original.replacen("app: it", "app: it\n    edited: \"yes\"", 1);
        within("apply", TIMEOUT, apply_pod_yaml(&client, &ns, "p", &edited)).await?;
        let d = get_pod_details(&client, &ns, "p").await?;
        ensure!(
            d.labels.get("edited").map(String::as_str) == Some("yes"),
            "{:?}",
            d.labels
        );

        within(
            "revert",
            TIMEOUT,
            apply_pod_yaml(&client, &ns, "p", &original),
        )
        .await?;
        let d = get_pod_details(&client, &ns, "p").await?;
        ensure!(!d.labels.contains_key("edited"), "{:?}", d.labels);
        ensure!(d.labels.get("app").map(String::as_str) == Some("it"));
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn metrics_become_available() -> Result<()> {
    let client = connect().await?;
    // metrics-server needs a while after start before it reports anything.
    let pods = eventually("pod metrics", Duration::from_secs(240), || async {
        Ok(get_pod_metrics(&client).await?.filter(|m| !m.is_empty()))
    })
    .await?;
    ensure!(pods.iter().any(|m| m.namespace == "kube-system"));
    let nodes = eventually("node metrics", Duration::from_secs(120), || async {
        Ok(kubeloupe::get_node_metrics(&client)
            .await?
            .filter(|m| !m.is_empty()))
    })
    .await?;
    ensure!(nodes
        .iter()
        .all(|n| n.cpu_capacity_millicores > 0 && n.memory_bytes > 0));
    Ok(())
}

#[tokio::test]
#[ignore]
async fn watch_pods_sees_create_and_delete() -> Result<()> {
    in_temp_ns("kl-it-watch", |client, ns| async move {
        let (_store, mut updates) = kubeloupe::watch::watch_pods(&client, Some(&ns));
        // Wait for the initial (empty) snapshot so the pod is a live event.
        within("initial snapshot", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if matches!(u, Update::Snapshot(_)) {
                    return Ok(());
                }
            }
            bail!("watch ended")
        })
        .await?;

        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        pods.create(&PostParams::default(), &sleeper_pod("w"))
            .await?;
        within("applied", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if let Update::Applied(p) = u {
                    if p.name == "w" {
                        return Ok(());
                    }
                }
            }
            bail!("watch ended")
        })
        .await?;

        pods.delete("w", &DeleteParams::default().grace_period(0))
            .await?;
        within("deleted", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if let Update::Deleted(p) = u {
                    if p.name == "w" {
                        return Ok(());
                    }
                }
            }
            bail!("watch ended")
        })
        .await
    })
    .await
}

/// Distinct pod names that logged a line, from the collected events.
fn pods_with_lines(events: &[WorkloadEvent]) -> BTreeSet<String> {
    let mut pods = BTreeSet::new();
    for ev in events {
        if let WorkloadEvent::Lines(lines) = ev {
            for l in lines {
                // Each pod prints its own name (HOSTNAME), so the tag must agree.
                assert!(
                    l.line.contains(&l.pod),
                    "line {:?} tagged with pod {}",
                    l.line,
                    l.pod
                );
                pods.insert(l.pod.clone());
            }
        }
    }
    pods
}

fn stopped_pods(events: &[WorkloadEvent]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkloadEvent::Stopped { pod, .. } => Some(pod.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
#[ignore]
async fn workload_logs_follow_a_rollout_restart() -> Result<()> {
    in_temp_ns("kl-it-wl", |client, ns| async move {
        let deployments: Api<Deployment> = Api::namespaced(client.clone(), &ns);
        let dep: Deployment = serde_json::from_value(serde_json::json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": { "name": "ticker" },
            "spec": {
                "replicas": 2,
                "selector": { "matchLabels": { "app": "ticker" } },
                "template": {
                    "metadata": { "labels": { "app": "ticker" } },
                    "spec": {
                        "terminationGracePeriodSeconds": 0,
                        "containers": [{
                            "name": "c", "image": "busybox:1.36",
                            "command": ["sh", "-c",
                                "while true; do echo \"tick $HOSTNAME\"; sleep 1; done"]
                        }]
                    }
                }
            }
        }))?;
        deployments.create(&PostParams::default(), &dep).await?;

        let events: Arc<Mutex<Vec<WorkloadEvent>>> = Arc::default();
        let sink = events.clone();
        let stream_client = client.clone();
        let stream_ns = ns.clone();
        let task = tokio::spawn(async move {
            let opts = LogOptions {
                tail_lines: Some(10),
                ..LogOptions::default()
            };
            stream_workload_logs(
                &stream_client,
                WorkloadKind::Deployment,
                &stream_ns,
                "ticker",
                &opts,
                |ev| sink.lock().unwrap().push(ev),
            )
            .await
        });

        let result = async {
            let old = eventually("lines from both replicas", TIMEOUT, || async {
                let pods = pods_with_lines(&events.lock().unwrap());
                Ok((pods.len() >= 2).then_some(pods))
            })
            .await?;
            ensure!(old.len() == 2, "expected 2 pods, saw {old:?}");

            deployments
                .patch(
                    "ticker",
                    &PatchParams::default(),
                    &Patch::Merge(serde_json::json!({"spec": {"template": {"metadata": {
                        "annotations": {"kubectl.kubernetes.io/restartedAt": "2099-01-01T00:00:00Z"}
                    }}}})),
                )
                .await?;

            // Lines from two pods that did not exist before the restart.
            let new = eventually("lines from the new pods", TIMEOUT, || async {
                let pods: Vec<_> = pods_with_lines(&events.lock().unwrap())
                    .difference(&old)
                    .cloned()
                    .collect();
                Ok((pods.len() >= 2).then_some(pods))
            })
            .await?;
            eventually("old streams to end", TIMEOUT, || async {
                Ok(old
                    .is_subset(&stopped_pods(&events.lock().unwrap()))
                    .then_some(()))
            })
            .await?;
            // The streams of the new pods are still running.
            let stopped = stopped_pods(&events.lock().unwrap());
            for pod in &new {
                ensure!(!stopped.contains(pod), "stream of new pod {pod} stopped");
            }
            Ok(())
        }
        .await;
        task.abort();
        result
    })
    .await
}
