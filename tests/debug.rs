//! Integration tests of debug containers and node shells against the kind
//! cluster. `#[ignore]`d like `cluster.rs`; run with
//! `cargo test --test debug -- --ignored`.
//!
//! Ephemeral containers cannot be removed, so the debug tests only debug
//! pods they create in a temporary namespace. The node shell tests delete
//! their pod whatever happens, and are the only node-level tests in this
//! binary (test binaries run one after another).

use std::future::Future;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::Client;
use kubeloupe::debug::{add_debug_container, attach_container, describe_debug_error};
use kubeloupe::node_shell::{
    create_node_shell, delete_node_shell, describe_node_shell_error, exec_node_shell,
    leftover_node_shells, new_node_shell_name, NODE_SHELL_IMAGE, NODE_SHELL_LABEL,
    NODE_SHELL_NAMESPACE,
};
use kubeloupe::{client_for_context, exec_command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(180);
const PAUSE: &str = "registry.k8s.io/pause:3.10";

fn context() -> String {
    std::env::var("KUBELOUPE_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubeloupe-test".to_string())
}

async fn connect() -> Result<Client> {
    let ctx = context();
    tokio::time::timeout(Duration::from_secs(20), client_for_context(Some(&ctx)))
        .await
        .context("timed out building client")?
        .with_context(|| format!("cannot connect to context {ctx}"))
}

/// Run `body` in a fresh namespace (with `labels`), deleting it afterwards
/// even on error or timeout.
async fn in_temp_ns<F, Fut>(prefix: &str, labels: serde_json::Value, body: F) -> Result<()>
where
    F: FnOnce(Client, String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let client = connect().await?;
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("{prefix}-{suffix:x}-{seq}");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace",
        "metadata": { "name": ns, "labels": labels }
    }))?;
    namespaces.create(&PostParams::default(), &obj).await?;
    let result = tokio::time::timeout(Duration::from_secs(300), body(client.clone(), ns.clone()))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("test timed out")));
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

/// A pod whose only container has no shell and runs forever.
async fn pause_pod(client: &Client, ns: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let pod: Pod = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": { "name": "noshell" },
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{ "name": "app", "image": PAUSE }]
        }
    }))?;
    pods.create(&PostParams::default(), &pod).await?;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let p = pods.get("noshell").await?;
        if p.status.and_then(|s| s.phase).as_deref() == Some("Running") {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            bail!("pod {ns}/noshell did not start");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Send `input` to an attached process without a TTY and collect stdout
/// until it exits.
async fn converse(mut p: kube::api::AttachedProcess, input: &str) -> Result<String> {
    let mut stdin = p.stdin().context("no stdin")?;
    let mut stdout = p.stdout().context("no stdout")?;
    stdin.write_all(input.as_bytes()).await?;
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(60), stdout.read_to_string(&mut out))
        .await
        .context("timed out reading the session")??;
    drop(stdin);
    Ok(out)
}

async fn worker_node(client: &Client) -> Result<String> {
    let nodes: Api<Node> = Api::all(client.clone());
    let list = nodes.list(&ListParams::default()).await?;
    let names: Vec<String> = list
        .items
        .into_iter()
        .filter_map(|n| n.metadata.name)
        .collect();
    names
        .iter()
        .find(|n| n.ends_with("-worker"))
        .or(names.first())
        .cloned()
        .context("no nodes")
}

async fn node_shell_pod_gone(client: &Client, ns: &str, name: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    for _ in 0..60 {
        if pods.get_opt(name).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("node shell pod {ns}/{name} was not deleted")
}

#[tokio::test]
#[ignore]
async fn debug_container_sees_the_target_containers_processes() -> Result<()> {
    in_temp_ns(
        "kl-it-debug",
        serde_json::json!({}),
        |client, ns| async move {
            pause_pod(&client, &ns).await?;
            // No shell in the image: exec fails.
            let sh = exec_command(&client, &ns, "noshell", Some("app"), &["sh".to_string()]).await;
            ensure!(
                sh.as_ref().map(|r| !r.success).unwrap_or(true),
                "exec sh unexpectedly worked"
            );

            let name =
                add_debug_container(&client, &ns, "noshell", "app", "", false, |_| {}).await?;
            ensure!(name.starts_with("debugger-"), "{name}");
            let attached = attach_container(&client, &ns, "noshell", &name, false).await?;
            let out = converse(attached, "ps; exit\n").await?;
            ensure!(out.contains("/pause"), "ps did not show /pause: {out}");

            let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
            let pod = pods.get("noshell").await?;
            let eph = pod.spec.unwrap().ephemeral_containers.unwrap_or_default();
            ensure!(
                eph.iter().any(|c| c.name == name
                    && c.target_container_name.as_deref() == Some("app")
                    && c.image.as_deref() == Some("busybox:1.37")),
                "{eph:?}"
            );
            Ok(())
        },
    )
    .await
}

#[tokio::test]
#[ignore]
async fn debug_container_reports_a_bad_image_and_a_missing_container() -> Result<()> {
    in_temp_ns(
        "kl-it-debugimg",
        serde_json::json!({}),
        |client, ns| async move {
            pause_pod(&client, &ns).await?;
            let err = add_debug_container(
                &client,
                &ns,
                "noshell",
                "app",
                "kubeloupe.invalid/no-such-image:1",
                false,
                |_| {},
            )
            .await
            .err()
            .context("a bad image should fail")?;
            let text = describe_debug_error(&err);
            ensure!(text.contains("the image could not be pulled"), "{text}");

            let err = add_debug_container(&client, &ns, "noshell", "nope", "", false, |_| {})
                .await
                .err()
                .context("a missing container should fail")?;
            let text = describe_debug_error(&err);
            ensure!(text.contains("no container named nope"), "{text}");
            Ok(())
        },
    )
    .await
}

#[tokio::test]
#[ignore]
async fn cli_debug_runs_piped_commands() -> Result<()> {
    in_temp_ns(
        "kl-it-clidebug",
        serde_json::json!({}),
        |client, ns| async move {
            pause_pod(&client, &ns).await?;
            let out = run_cli(&["debug", "noshell", "-n", &ns, "-c", "app"], "ps; exit\n")?;
            ensure!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            ensure!(stdout.contains("/pause"), "{stdout}");
            let stderr = String::from_utf8_lossy(&out.stderr);
            ensure!(stderr.contains("cannot be removed"), "{stderr}");
            drop(client);
            Ok(())
        },
    )
    .await
}

fn run_cli(args: &[&str], stdin: &str) -> Result<std::process::Output> {
    let ctx = context();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kubeloupe"))
        .arg("--context")
        .arg(&ctx)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(stdin.as_bytes())?;
    Ok(child.wait_with_output()?)
}

/// Node-level: opens a shell on the worker, checks it is the host, and
/// deletes the pod whatever happens.
#[tokio::test]
#[ignore]
async fn node_shell_runs_on_the_host_and_is_deleted() -> Result<()> {
    let client = connect().await?;
    let node = worker_node(&client).await?;
    let ns = NODE_SHELL_NAMESPACE;
    let pod = new_node_shell_name(&node);
    let session = async {
        create_node_shell(&client, &node, ns, &pod, NODE_SHELL_IMAGE, |_| {}).await?;
        let p = exec_node_shell(&client, ns, &pod, false).await?;
        converse(p, "cat /etc/hostname; exit\n").await
    };
    let result = tokio::time::timeout(TIMEOUT, session)
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
    let deleted = delete_node_shell(&client, ns, &pod).await;
    let out = result?;
    deleted?;
    ensure!(out.trim() == node, "hostname {out:?}, node {node}");
    node_shell_pod_gone(&client, ns, &pod).await?;

    // The CLI cleans up after itself too.
    let out = run_cli(&["node-shell", &node], "cat /etc/hostname; exit\n")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(stdout.trim() == node, "{stdout:?}");
    let left = leftover_node_shells(&client).await?;
    for p in left.iter().filter(|p| p.node == node) {
        // Deleted with grace 0; give it a moment to disappear.
        node_shell_pod_gone(&client, &p.namespace, &p.name).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn node_shell_forbidden_by_pod_security_says_so() -> Result<()> {
    let labels = serde_json::json!({ "pod-security.kubernetes.io/enforce": "baseline" });
    in_temp_ns("kl-it-psa", labels, |client, ns| async move {
        let node = worker_node(&client).await?;
        let pod = new_node_shell_name(&node);
        let err = create_node_shell(&client, &node, &ns, &pod, NODE_SHELL_IMAGE, |_| {})
            .await
            .err();
        let _ = delete_node_shell(&client, &ns, &pod).await;
        let err = err.context("a privileged pod should be rejected")?;
        let text = describe_node_shell_error(&err, &ns);
        ensure!(
            text.starts_with("Pod Security Admission does not allow"),
            "{text}"
        );
        ensure!(text.contains("allows privileged pods"), "{text}");
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        let left = pods
            .list(&ListParams::default().labels(&format!("{NODE_SHELL_LABEL}=true")))
            .await?;
        ensure!(left.items.is_empty(), "a pod was left behind");
        Ok(())
    })
    .await
}

/// The limited user (`edit` in team-a) may not create pods in kube-system.
#[tokio::test]
#[ignore]
async fn limited_user_node_shell_is_forbidden() -> Result<()> {
    let Ok(path) = std::env::var("KUBELOUPE_LIMITED_KUBECONFIG") else {
        eprintln!("KUBELOUPE_LIMITED_KUBECONFIG not set; skipping");
        return Ok(());
    };
    let kubeconfig = kube::config::Kubeconfig::read_from(&path)?;
    let mut config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default()).await?;
    config.proxy_url = None;
    let client = Client::try_from(config)?;
    let pod = new_node_shell_name("kubeloupe-test-worker");
    let err = create_node_shell(
        &client,
        "kubeloupe-test-worker",
        NODE_SHELL_NAMESPACE,
        &pod,
        NODE_SHELL_IMAGE,
        |_| {},
    )
    .await
    .err();
    // Should it have worked after all, remove the pod as the admin.
    let _ = delete_node_shell(&connect().await?, NODE_SHELL_NAMESPACE, &pod).await;
    let err = err.context("the limited user should not create pods in kube-system")?;
    let text = describe_node_shell_error(&err, NODE_SHELL_NAMESPACE);
    ensure!(text.starts_with("Not allowed"), "{text}");
    ensure!(text.contains("kube-system"), "{text}");
    Ok(())
}
