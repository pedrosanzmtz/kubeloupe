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
use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Node, Pod};
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams};
use kube::Client;
use kubeloupe::actions::{
    self, delete_many, drain_node, force_delete_pod, rollout_restart, scale, set_suspended,
    set_unschedulable, trigger_cronjob, DrainEvent, DrainOptions,
};
use kubeloupe::resources::{
    self, apply_resource_yaml, apply_resource_yaml_with, delete_resource, get_resource_detail,
    get_resource_yaml, list_resources, ApplyOptions, ApplyOutcome, Category, DetailState,
    ResourceDetail, ResourceKind,
};
use kubeloupe::watch::Update;
use kubeloupe::watch::WatchState;
use kubeloupe::{
    apply_pod_yaml, client_for_context, default_namespace, exec_command, get_namespaces, get_nodes,
    get_overview, get_pod_logs, get_pod_metrics, get_pod_yaml, get_pods, is_forbidden,
    list_namespaces_if_allowed, pod_ref, serve_port_forward, stream_pod_logs, stream_workload_logs,
    LogOptions, WorkloadEvent, WorkloadKind,
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

/// A pod's detail (with its `pod` section).
async fn get_pod_details(client: &Client, ns: &str, name: &str) -> Result<ResourceDetail> {
    get_resource_detail(client, &pod_ref(), ns, name).await
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
        .pod
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

        // Straight from the editor: includes metadata.resourceVersion.
        let original = get_pod_yaml(&client, &ns, "p").await?;
        ensure!(original.contains("app: it"), "{original}");
        ensure!(
            original.contains("resourceVersion:"),
            "this test is about a YAML that carries a resourceVersion:\n{original}"
        );

        // Two saves in a row from the same editor content. The first apply
        // bumps the resourceVersion, so the second carries a stale one; this
        // used to fail with 409 Conflict.
        let edited = original.replacen("app: it", "app: it\n    edited: \"one\"", 1);
        within(
            "apply 1",
            TIMEOUT,
            apply_pod_yaml(&client, &ns, "p", &edited),
        )
        .await?;
        let d = get_pod_details(&client, &ns, "p").await?;
        ensure!(
            d.labels.get("edited").map(String::as_str) == Some("one"),
            "{:?}",
            d.labels
        );
        let edited = original.replacen("app: it", "app: it\n    edited: \"two\"", 1);
        within(
            "apply 2",
            TIMEOUT,
            apply_pod_yaml(&client, &ns, "p", &edited),
        )
        .await?;
        let d = get_pod_details(&client, &ns, "p").await?;
        ensure!(
            d.labels.get("edited").map(String::as_str) == Some("two"),
            "{:?}",
            d.labels
        );

        // And the revert, again from the original (now very stale) text.
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

async fn kind_named(client: &Client, name: &str) -> Result<ResourceKind> {
    let kinds = within("discover", TIMEOUT, resources::discover(client)).await?;
    resources::resolve_kind(&kinds, name)
        .cloned()
        .with_context(|| format!("discovery has no kind {name}"))
}

const CRD_GROUP: &str = "kltest.example.com";

fn widget_crd() -> Result<CustomResourceDefinition> {
    Ok(serde_json::from_value(serde_json::json!({
        "apiVersion": "apiextensions.k8s.io/v1",
        "kind": "CustomResourceDefinition",
        "metadata": { "name": format!("widgets.{CRD_GROUP}") },
        "spec": {
            "group": CRD_GROUP,
            "scope": "Namespaced",
            "names": { "plural": "widgets", "singular": "widget", "kind": "Widget",
                       "shortNames": ["wdg"] },
            "versions": [{
                "name": "v1", "served": true, "storage": true,
                "schema": { "openAPIV3Schema": {
                    "type": "object", "x-kubernetes-preserve-unknown-fields": true
                }},
                "additionalPrinterColumns": [
                    { "name": "Size", "type": "integer", "jsonPath": ".spec.size" },
                    { "name": "Color", "type": "string", "jsonPath": ".spec.color" },
                    { "name": "Hidden", "type": "string", "jsonPath": ".spec.hidden",
                      "priority": 1 },
                    { "name": "Age", "type": "date", "jsonPath": ".metadata.creationTimestamp" }
                ]
            }]
        }
    }))?)
}

/// Install the test CRD, run `body`, and remove the CRD again.
async fn with_widget_crd<F, Fut>(body: F) -> Result<()>
where
    F: FnOnce(Client, ResourceKind) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let client = connect().await?;
    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
    let crd = widget_crd()?;
    let name = format!("widgets.{CRD_GROUP}");
    crds.patch(
        &name,
        &PatchParams::apply("kubeloupe-it").force(),
        &Patch::Apply(&crd),
    )
    .await?;
    let result = async {
        let kind = eventually("Widget CRD served", TIMEOUT, || async {
            Ok(resources::discover(&client)
                .await?
                .into_iter()
                .find(|k| k.resource.group == CRD_GROUP && k.resource.kind == "Widget"))
        })
        .await?;
        body(client.clone(), kind).await
    }
    .await;
    let _ = crds.delete(&name, &DeleteParams::default()).await;
    result
}

#[tokio::test]
#[ignore]
async fn discovery_finds_builtins_and_a_crd() -> Result<()> {
    with_widget_crd(|client, widget| async move {
        let kinds = resources::discover(&client).await?;
        for (name, category, namespaced) in [
            ("pods", Category::Workloads, true),
            ("deployments", Category::Workloads, true),
            ("services", Category::Network, true),
            ("configmaps", Category::Config, true),
            ("secrets", Category::Config, true),
            ("persistentvolumes", Category::Storage, false),
            ("nodes", Category::Cluster, false),
            ("events", Category::Cluster, true),
        ] {
            let k = resources::resolve_kind(&kinds, name)
                .with_context(|| format!("discovery is missing {name}"))?;
            ensure!(k.category == category, "{name}: {:?}", k.category);
            ensure!(k.resource.namespaced == namespaced, "{name} scope");
            ensure!(!k.crd, "{name} is not a CRD");
            ensure!(k.verbs.iter().any(|v| v == "list"), "{name} verbs");
        }
        // events.k8s.io must not shadow the core Events.
        ensure!(
            kinds.iter().filter(|k| k.resource.kind == "Event").count() == 1,
            "duplicate Event kinds"
        );

        // The CRD, with its scope, category and printer columns (priority 0).
        ensure!(widget.crd && widget.category == Category::Custom);
        ensure!(widget.resource.namespaced);
        let labels: Vec<_> = widget.columns.iter().map(|c| c.label.as_str()).collect();
        ensure!(
            labels == ["Name", "Namespace", "Size", "Color", "Age"],
            "{labels:?}"
        );
        // Resolvable by plural, short name, kind and plural.group.
        for q in ["widgets", "wdg", "Widget", "widgets.kltest.example.com"] {
            ensure!(
                resources::resolve_kind(&kinds, q).map(|k| &k.id) == Some(&widget.id),
                "cannot resolve {q}"
            );
        }

        // RBAC check answers for admin on the kind-cluster.
        let access =
            resources::check_access(&client, &[widget.resource.clone(), kinds[0].resource.clone()], None)
                .await;
        ensure!(
            access.len() == 2 && access.iter().all(|a| a.allowed),
            "{access:?}"
        );

        // A custom object shows its printer-column values.
        let c = client.clone();
        in_temp_ns_with(&client, "kl-it-crd", |ns| async move {
            let yaml = format!(
                "apiVersion: {CRD_GROUP}/v1\nkind: Widget\nmetadata:\n  name: w1\n  namespace: {ns}\nspec:\n  size: 3\n  color: blue\n"
            );
            apply_resource_yaml(&c, &widget.resource, &ns, "w1", &yaml).await?;
            let rows = list_resources(&c, &widget, Some(&ns)).await?;
            ensure!(rows.len() == 1, "{rows:?}");
            ensure!(
                rows[0].cells[..4] == ["w1", ns.as_str(), "3", "blue"],
                "{rows:?}"
            );
            Ok(())
        })
        .await
    })
    .await
}

/// Like `in_temp_ns`, for a caller that already has a client.
async fn in_temp_ns_with<F, Fut>(client: &Client, prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("{prefix}-{suffix:x}");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": ns }
    }))?;
    namespaces.create(&PostParams::default(), &obj).await?;
    let result = body(ns.clone()).await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

fn configmap_yaml(ns: &str, data: &str) -> String {
    format!(
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cm\n  namespace: {ns}\n  labels:\n    app: it\ndata:\n{data}"
    )
}

#[tokio::test]
#[ignore]
async fn generic_list_get_apply_delete_on_a_configmap() -> Result<()> {
    in_temp_ns("kl-it-generic", |client, ns| async move {
        let kind = kind_named(&client, "configmaps").await?;
        let r = &kind.resource;

        // Apply creates.
        let first = configmap_yaml(&ns, "  a: \"1\"\n");
        apply_resource_yaml(&client, r, &ns, "cm", &first).await?;
        let rows = list_resources(&client, &kind, Some(&ns)).await?;
        ensure!(rows.iter().any(|row| row.name == "cm"), "{rows:?}");
        let table = resources::render_table(&kind, &rows);
        ensure!(table.starts_with("NAME") && table.contains("cm"), "{table}");

        // Get as YAML: identity first, no managedFields.
        let yaml = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(
            yaml.starts_with("apiVersion: v1\nkind: ConfigMap\nmetadata:"),
            "{yaml}"
        );
        ensure!(!yaml.contains("managedFields"), "{yaml}");

        // Edit: add a key, from the YAML we just read (resourceVersion and all).
        let edited = yaml.replacen("\ndata:", "\ndata:\n  b: \"2\"", 1);
        apply_resource_yaml(&client, r, &ns, "cm", &edited).await?;
        let after = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(
            after.contains("b: '2'") || after.contains("b: \"2\""),
            "{after}"
        );

        // Detail: metadata and labels.
        let d = get_resource_detail(&client, r, &ns, "cm").await?;
        ensure!(d.name == "cm" && d.namespace == ns && d.kind == "ConfigMap");
        ensure!(d.labels.get("app").map(String::as_str) == Some("it"));

        // The YAML must describe the same kind it is applied to.
        let wrong = yaml.replace("kind: ConfigMap", "kind: Secret");
        ensure!(apply_resource_yaml(&client, r, &ns, "cm", &wrong)
            .await
            .is_err());

        // Namespaced kinds need a namespace.
        ensure!(get_resource_yaml(&client, r, "", "cm", false)
            .await
            .is_err());

        // Delete.
        delete_resource(&client, r, &ns, "cm").await?;
        eventually("configmap gone", TIMEOUT, || async {
            let rows = list_resources(&client, &kind, Some(&ns)).await?;
            Ok(rows.iter().all(|row| row.name != "cm").then_some(()))
        })
        .await
    })
    .await
}

#[tokio::test]
#[ignore]
async fn generic_yaml_can_be_saved_twice_in_a_row() -> Result<()> {
    in_temp_ns("kl-it-twice", |client, ns| async move {
        let kind = kind_named(&client, "configmaps").await?;
        let r = &kind.resource;
        let first = configmap_yaml(&ns, "  a: \"1\"\n");
        apply_resource_yaml(&client, r, &ns, "cm", &first).await?;

        // One editor session: load once, save twice without reloading.
        let loaded = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(loaded.contains("resourceVersion:"), "{loaded}");
        for n in ["2", "3"] {
            let text = loaded.replacen("\ndata:", &format!("\ndata:\n  x{n}: v"), 1);
            apply_resource_yaml(&client, r, &ns, "cm", &text).await?;
        }
        // Applying the same document is a field-ownership update, not an
        // accumulation: only the last set of keys remains.
        let now = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(now.contains("x3:") && !now.contains("x2:"), "{now}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn yaml_dry_run_validates_and_a_stale_version_is_a_conflict() -> Result<()> {
    in_temp_ns("kl-it-dryrun", |client, ns| async move {
        let kind = kind_named(&client, "configmaps").await?;
        let r = &kind.resource;
        apply_resource_yaml(&client, r, &ns, "cm", &configmap_yaml(&ns, "  a: \"1\"\n")).await?;
        let loaded = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        let rv = resources::get_resource(&client, r, &ns, "cm")
            .await?
            .metadata
            .resource_version
            .unwrap_or_default();
        let opts = |dry_run: bool, rv: &str| ApplyOptions {
            dry_run,
            resource_version: Some(rv.to_string()),
        };

        // A dry run validates and stores nothing.
        let edited = loaded.replacen("\ndata:", "\ndata:\n  b: \"2\"", 1);
        let out =
            apply_resource_yaml_with(&client, r, &ns, "cm", &edited, &opts(true, &rv)).await?;
        ensure!(
            out == ApplyOutcome::Applied { takeovers: vec![] },
            "{out:?}"
        );
        let after = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(!after.contains("b:"), "{after}");

        // A field the schema does not have is rejected by the dry run.
        let invalid = format!("{loaded}spec:\n  foo: 1\n");
        let err = apply_resource_yaml_with(&client, r, &ns, "cm", &invalid, &opts(true, &rv))
            .await
            .expect_err("unknown field accepted");
        ensure!(format!("{err:#}").contains("spec"), "{err:#}");

        // Someone else writes: the edit loaded at `rv` is not applied.
        apply_resource_yaml(&client, r, &ns, "cm", &configmap_yaml(&ns, "  a: \"9\"\n")).await?;
        let out =
            apply_resource_yaml_with(&client, r, &ns, "cm", &edited, &opts(false, &rv)).await?;
        let ApplyOutcome::Conflict {
            resource_version: latest,
        } = out
        else {
            bail!("expected a conflict, got {out:?}");
        };
        ensure!(latest != rv);
        let now = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(!now.contains("b:"), "{now}");

        // Against the latest version it applies.
        let out =
            apply_resource_yaml_with(&client, r, &ns, "cm", &edited, &opts(false, &latest)).await?;
        ensure!(matches!(out, ApplyOutcome::Applied { .. }), "{out:?}");
        let now = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        ensure!(now.contains("b:"), "{now}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn yaml_apply_reports_fields_taken_over() -> Result<()> {
    in_temp_ns("kl-it-takeover", |client, ns| async move {
        let kind = kind_named(&client, "configmaps").await?;
        let r = &kind.resource;

        // Another tool creates the object (like `kubectl create` would).
        let cms: Api<k8s_openapi::api::core::v1::ConfigMap> = Api::namespaced(client.clone(), &ns);
        let cm: k8s_openapi::api::core::v1::ConfigMap =
            serde_json::from_value(serde_json::json!({
                "apiVersion": "v1", "kind": "ConfigMap",
                "metadata": { "name": "cm" }, "data": { "a": "1", "b": "1" }
            }))?;
        let pp = PostParams {
            field_manager: Some("other-tool".into()),
            ..PostParams::default()
        };
        cms.create(&pp, &cm).await?;

        // Editing a field it owns still saves (forced), and says so.
        let yaml = get_resource_yaml(&client, r, &ns, "cm", false).await?;
        let edited = yaml
            .replace("a: '1'", "a: '2'")
            .replace("a: \"1\"", "a: \"2\"");
        ensure!(edited != yaml, "edit did not apply to: {yaml}");
        let took = apply_resource_yaml(&client, r, &ns, "cm", &edited).await?;
        ensure!(
            took.iter()
                .any(|t| t.manager == "other-tool" && t.fields.iter().any(|f| f == ".data.a")),
            "expected .data.a from other-tool, got {took:?}"
        );
        let now = cms.get("cm").await?;
        ensure!(now.data.unwrap_or_default().get("a").map(String::as_str) == Some("2"));

        // kubeloupe owns those fields now: saving again takes nothing over.
        let took = apply_resource_yaml(&client, r, &ns, "cm", &edited).await?;
        ensure!(took.is_empty(), "second save took over {took:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn secret_values_are_hidden_until_revealed() -> Result<()> {
    in_temp_ns("kl-it-secret", |client, ns| async move {
        let kind = kind_named(&client, "secrets").await?;
        let r = &kind.resource;
        let yaml = format!(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: s\n  namespace: {ns}\nstringData:\n  password: hunter2\n"
        );
        apply_resource_yaml(&client, r, &ns, "s", &yaml).await?;

        let hidden = get_resource_yaml(&client, r, &ns, "s", false).await?;
        ensure!(hidden.contains(resources::REDACTED), "{hidden}");
        ensure!(!hidden.contains("aHVudGVyMg=="), "base64 leaked:\n{hidden}");
        // A redacted document must not be saved over the real values.
        ensure!(apply_resource_yaml(&client, r, &ns, "s", &hidden)
            .await
            .is_err());

        let shown = get_resource_yaml(&client, r, &ns, "s", true).await?;
        ensure!(shown.contains("aHVudGVyMg=="), "{shown}");
        apply_resource_yaml(&client, r, &ns, "s", &shown).await?;

        // The detail lists keys and sizes, never values.
        let d = get_resource_detail(&client, r, &ns, "s").await?;
        ensure!(d.secret_keys.len() == 1 && d.secret_keys[0].key == "password");
        ensure!(d.secret_keys[0].bytes == 7, "{:?}", d.secret_keys);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn generic_watch_sees_configmap_changes() -> Result<()> {
    in_temp_ns("kl-it-gwatch", |client, ns| async move {
        let kind = kind_named(&client, "configmaps").await?;
        let r = &kind.resource;
        let (_store, mut updates) = kubeloupe::watch::watch_resource(&client, &kind, Some(&ns));
        // Wait for the initial snapshot so the changes below are live events.
        within("initial snapshot", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if matches!(u, Update::Snapshot(_)) {
                    return Ok(());
                }
            }
            bail!("watch ended")
        })
        .await?;

        let data_col = kind.columns.iter().position(|c| c.key == "data").unwrap();
        let first = configmap_yaml(&ns, "  a: \"1\"\n");
        apply_resource_yaml(&client, r, &ns, "cm", &first).await?;
        within("applied", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if let Update::Applied(row) = u {
                    if row.name == "cm" {
                        ensure!(row.key == format!("{ns}/cm"), "{row:?}");
                        ensure!(row.cells[data_col] == "1", "{row:?}");
                        return Ok(());
                    }
                }
            }
            bail!("watch ended")
        })
        .await?;

        // A change that shows in a cell arrives as another Applied.
        let second = configmap_yaml(&ns, "  a: \"1\"\n  b: \"2\"\n");
        apply_resource_yaml(&client, r, &ns, "cm", &second).await?;
        within("updated", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if let Update::Applied(row) = u {
                    if row.name == "cm" && row.cells[data_col] == "2" {
                        return Ok(());
                    }
                }
            }
            bail!("watch ended")
        })
        .await?;

        delete_resource(&client, r, &ns, "cm").await?;
        within("deleted", TIMEOUT, async {
            while let Some(u) = updates.next().await {
                if let Update::Deleted(row) = u {
                    if row.name == "cm" {
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

// ---------------------------------------------------------------------------
// Resource actions (#4).
// ---------------------------------------------------------------------------

fn sleeper_deployment(name: &str, replicas: i32, extra_labels: serde_json::Value) -> Deployment {
    let mut labels = serde_json::json!({ "app": name });
    for (k, v) in extra_labels.as_object().into_iter().flatten() {
        labels[k] = v.clone();
    }
    serde_json::from_value(serde_json::json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": { "name": name },
        "spec": {
            "replicas": replicas,
            "selector": { "matchLabels": { "app": name } },
            "template": {
                "metadata": { "labels": labels },
                "spec": {
                    "terminationGracePeriodSeconds": 0,
                    "containers": [{
                        "name": "c", "image": "busybox:1.36",
                        "command": ["sh", "-c", "sleep 3600"]
                    }]
                }
            }
        }
    }))
    .unwrap()
}

fn cronjob_named(name: &str) -> CronJob {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "batch/v1", "kind": "CronJob",
        "metadata": { "name": name },
        "spec": {
            // Never fires on its own during a test.
            "schedule": "0 0 1 1 *",
            "jobTemplate": { "spec": { "template": { "spec": {
                "restartPolicy": "Never",
                "containers": [{ "name": "c", "image": "busybox:1.36", "command": ["true"] }]
            }}}}
        }
    }))
    .unwrap()
}

#[tokio::test]
#[ignore]
async fn action_scale_changes_replicas() -> Result<()> {
    in_temp_ns("kl-it-scale", |client, ns| async move {
        let deployments: Api<Deployment> = Api::namespaced(client.clone(), &ns);
        deployments
            .create(
                &PostParams::default(),
                &sleeper_deployment("web", 1, serde_json::json!({})),
            )
            .await?;
        let kind = kind_named(&client, "deployments").await?;
        scale(&client, &kind.resource, &ns, "web", 2).await?;
        eventually("spec.replicas == 2", TIMEOUT, || async {
            let d = deployments.get("web").await?;
            Ok((d.spec.and_then(|s| s.replicas) == Some(2)).then_some(()))
        })
        .await?;
        // Scale to zero removes the pods.
        scale(&client, &kind.resource, &ns, "web", 0).await?;
        eventually("no pods", TIMEOUT, || async {
            let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
            Ok(pods
                .list(&Default::default())
                .await?
                .items
                .is_empty()
                .then_some(()))
        })
        .await?;
        // A negative count and a kind without /scale are refused.
        ensure!(scale(&client, &kind.resource, &ns, "web", -1)
            .await
            .is_err());
        let cms = kind_named(&client, "configmaps").await?;
        ensure!(scale(&client, &cms.resource, &ns, "x", 1).await.is_err());
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn action_rollout_restart_adds_annotation_and_replicaset() -> Result<()> {
    in_temp_ns("kl-it-restart", |client, ns| async move {
        let deployments: Api<Deployment> = Api::namespaced(client.clone(), &ns);
        deployments
            .create(
                &PostParams::default(),
                &sleeper_deployment("web", 1, serde_json::json!({})),
            )
            .await?;
        let replicasets: Api<ReplicaSet> = Api::namespaced(client.clone(), &ns);
        eventually("first ReplicaSet", TIMEOUT, || async {
            Ok((replicasets.list(&Default::default()).await?.items.len() == 1).then_some(()))
        })
        .await?;

        let kind = kind_named(&client, "deployments").await?;
        rollout_restart(&client, &kind.resource, &ns, "web").await?;

        let d = deployments.get("web").await?;
        let ann = d
            .spec
            .and_then(|s| s.template.metadata)
            .and_then(|m| m.annotations)
            .unwrap_or_default();
        let stamp = ann
            .get(actions::RESTARTED_AT)
            .context("restartedAt annotation missing")?;
        ensure!(
            chrono::DateTime::parse_from_rfc3339(stamp).is_ok(),
            "not RFC 3339: {stamp}"
        );
        eventually("a second ReplicaSet", TIMEOUT, || async {
            Ok((replicasets.list(&Default::default()).await?.items.len() >= 2).then_some(()))
        })
        .await
    })
    .await
}

#[tokio::test]
#[ignore]
async fn action_trigger_cronjob_creates_owned_job() -> Result<()> {
    in_temp_ns("kl-it-trigger", |client, ns| async move {
        let cronjobs: Api<CronJob> = Api::namespaced(client.clone(), &ns);
        let cj = cronjobs
            .create(&PostParams::default(), &cronjob_named("nightly"))
            .await?;
        let job_name = trigger_cronjob(&client, &ns, "nightly").await?;
        ensure!(job_name.starts_with("nightly-manual-"), "{job_name}");

        let jobs: Api<Job> = Api::namespaced(client.clone(), &ns);
        let job = jobs.get(&job_name).await?;
        let owner = job
            .metadata
            .owner_references
            .as_ref()
            .and_then(|o| o.first())
            .context("job has no owner reference")?;
        ensure!(owner.kind == "CronJob" && owner.name == "nightly");
        ensure!(Some(&owner.uid) == cj.metadata.uid.as_ref());
        ensure!(
            job.metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("cronjob.kubernetes.io/instantiate"))
                .map(String::as_str)
                == Some("manual")
        );
        // Two triggers make two distinct jobs.
        let second = trigger_cronjob(&client, &ns, "nightly").await?;
        ensure!(second != job_name);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn action_suspend_and_resume_cronjob() -> Result<()> {
    in_temp_ns("kl-it-suspend", |client, ns| async move {
        let cronjobs: Api<CronJob> = Api::namespaced(client.clone(), &ns);
        cronjobs
            .create(&PostParams::default(), &cronjob_named("nightly"))
            .await?;
        let suspended = || async {
            Ok::<_, anyhow::Error>(
                cronjobs
                    .get("nightly")
                    .await?
                    .spec
                    .and_then(|s| s.suspend)
                    .unwrap_or(false),
            )
        };
        set_suspended(&client, &ns, "nightly", true).await?;
        ensure!(suspended().await?);
        set_suspended(&client, &ns, "nightly", false).await?;
        ensure!(!suspended().await?);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn action_bulk_delete_reports_each_object() -> Result<()> {
    in_temp_ns("kl-it-bulk", |client, ns| async move {
        let cms: Api<ConfigMap> = Api::namespaced(client.clone(), &ns);
        for n in ["a", "b", "c"] {
            let cm: ConfigMap = serde_json::from_value(
                serde_json::json!({"metadata": {"name": n}, "data": {"k": "v"}}),
            )?;
            cms.create(&PostParams::default(), &cm).await?;
        }
        let kind = kind_named(&client, "configmaps").await?;
        let items: Vec<(String, String)> = ["a", "b", "c", "never-existed"]
            .iter()
            .map(|n| (ns.clone(), n.to_string()))
            .collect();
        let results = delete_many(&client, &kind.resource, &items).await;
        ensure!(results.len() == 4, "{results:?}");
        for r in &results[..3] {
            ensure!(r.error.is_empty(), "{r:?}");
        }
        let missing = results[3].error.as_str();
        ensure!(missing.starts_with("Already gone"), "{missing}");
        let left: Vec<String> = cms
            .list(&Default::default())
            .await?
            .items
            .into_iter()
            .filter_map(|c| c.metadata.name)
            .filter(|n| ["a", "b", "c"].contains(&n.as_str()))
            .collect();
        ensure!(left.is_empty(), "still there: {left:?}");
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn action_force_delete_removes_a_stuck_pod() -> Result<()> {
    in_temp_ns("kl-it-force", |client, ns| async move {
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        // A long grace period and a shell that ignores SIGTERM: a normal
        // delete leaves the pod Terminating.
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": "stuck" },
            "spec": {
                "terminationGracePeriodSeconds": 600,
                "containers": [{
                    "name": "c", "image": "busybox:1.36",
                    "command": ["sh", "-c", "trap '' TERM; while true; do sleep 1; done"]
                }]
            }
        }))?;
        pods.create(&PostParams::default(), &pod).await?;
        wait_running(&client, &ns, "stuck").await?;

        let kind = kind_named(&client, "pods").await?;
        delete_resource(&client, &kind.resource, &ns, "stuck").await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let p = pods.get("stuck").await?;
        ensure!(
            p.metadata.deletion_timestamp.is_some(),
            "a plain delete should leave the pod terminating"
        );

        force_delete_pod(&client, &ns, "stuck").await?;
        eventually("pod gone", Duration::from_secs(30), || async {
            Ok(pods.get_opt("stuck").await?.is_none().then_some(()))
        })
        .await?;
        // Deleting what is already gone is a plain-words 404.
        let err = force_delete_pod(&client, &ns, "stuck").await.unwrap_err();
        ensure!(
            actions::describe_error(&err).starts_with("Already gone"),
            "{err:#}"
        );
        Ok(())
    })
    .await
}

/// The worker node: the one that is not a control plane.
async fn worker_node(client: &Client) -> Result<String> {
    let nodes: Api<Node> = Api::all(client.clone());
    nodes
        .list(&Default::default())
        .await?
        .items
        .into_iter()
        .find(|n| {
            !n.metadata
                .labels
                .iter()
                .flatten()
                .any(|(k, _)| k == "node-role.kubernetes.io/control-plane")
        })
        .and_then(|n| n.metadata.name)
        .context("no worker node in the test cluster")
}

async fn unschedulable(client: &Client, node: &str) -> Result<bool> {
    let nodes: Api<Node> = Api::all(client.clone());
    Ok(nodes
        .get(node)
        .await?
        .spec
        .and_then(|s| s.unschedulable)
        .unwrap_or(false))
}

fn pinned_deployment(name: &str, node: &str, replicas: i32, drain_label: &str) -> Deployment {
    let mut d = sleeper_deployment(name, replicas, serde_json::json!({ "kl-it": drain_label }));
    d.spec
        .as_mut()
        .unwrap()
        .template
        .spec
        .as_mut()
        .unwrap()
        .node_selector = Some([("kubernetes.io/hostname".to_string(), node.to_string())].into());
    d
}

/// Cordon, uncordon and drain the shared kind cluster's worker. Named `zz_`
/// so it runs last in a single-threaded run; the drain is limited to this
/// test's own pods with a label selector so the demo workload is never
/// evicted. The worker is made schedulable again whatever happens.
#[tokio::test]
#[ignore]
async fn zz_node_cordon_uncordon_and_drain_respects_pdb() -> Result<()> {
    let client = connect().await?;
    let node = worker_node(&client).await?;
    let result = in_temp_ns_with(&client, "kl-it-drain", |ns| {
        let client = client.clone();
        let node = node.clone();
        async move { drain_scenario(&client, &ns, &node).await }
    })
    .await;
    // Restore the shared node even if the scenario failed half way.
    let restore = set_unschedulable(&client, &node, false).await;
    result?;
    restore?;
    ensure!(
        !unschedulable(&client, &node).await?,
        "{node} is still cordoned"
    );
    Ok(())
}

async fn drain_scenario(client: &Client, ns: &str, node: &str) -> Result<()> {
    // Cordon and uncordon.
    set_unschedulable(client, node, true).await?;
    ensure!(unschedulable(client, node).await?);
    set_unschedulable(client, node, false).await?;
    ensure!(!unschedulable(client, node).await?);

    // Part 1: a 2-replica deployment behind a PDB that needs 1 available.
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), ns);
    deployments
        .create(
            &PostParams::default(),
            &pinned_deployment("guarded", node, 2, "drain-pdb"),
        )
        .await?;
    let pdbs: Api<PodDisruptionBudget> = Api::namespaced(client.clone(), ns);
    let pdb: PodDisruptionBudget = serde_json::from_value(serde_json::json!({
        "apiVersion": "policy/v1", "kind": "PodDisruptionBudget",
        "metadata": { "name": "guarded" },
        "spec": { "minAvailable": 1, "selector": { "matchLabels": { "app": "guarded" } } }
    }))?;
    pdbs.create(&PostParams::default(), &pdb).await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let running = |app: &'static str| {
        let pods = pods.clone();
        async move {
            let list = pods
                .list(&kube::api::ListParams::default().labels(&format!("app={app}")))
                .await?;
            Ok::<_, anyhow::Error>(
                list.items
                    .iter()
                    .filter(|p| {
                        p.metadata.deletion_timestamp.is_none()
                            && p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
                    })
                    .count(),
            )
        }
    };
    eventually("2 guarded pods running", TIMEOUT, || async {
        Ok((running("guarded").await? == 2).then_some(()))
    })
    .await?;

    let events: Arc<Mutex<Vec<DrainEvent>>> = Arc::default();
    let sink = events.clone();
    let opts = DrainOptions {
        timeout: Duration::from_secs(20),
        pod_selector: Some("kl-it=drain-pdb".to_string()),
    };
    let summary = within(
        "drain with PDB",
        Duration::from_secs(60),
        drain_node(client, node, &opts, move |e| sink.lock().unwrap().push(e)),
    )
    .await?;
    let seen = events.lock().unwrap().clone();
    ensure!(
        matches!(seen.first(), Some(DrainEvent::Cordoned { .. })),
        "{seen:?}"
    );
    ensure!(
        seen.iter()
            .any(|e| matches!(e, DrainEvent::Planned { to_evict: 2, .. })),
        "{seen:?}"
    );
    ensure!(
        seen.iter()
            .any(|e| matches!(e, DrainEvent::WaitingOnPdb { .. })),
        "the PDB never blocked an eviction: {seen:?}"
    );
    // One pod could go; the PDB held the other back until the timeout.
    ensure!(
        summary.evicted == 1 && summary.failed == 1,
        "{summary:?} {seen:?}"
    );
    ensure!(!summary.complete());
    ensure!(
        seen.iter()
            .any(|e| matches!(e, DrainEvent::Failed { error, .. }
            if error.contains("PodDisruptionBudget"))),
        "{seen:?}"
    );
    ensure!(
        running("guarded").await? >= 1,
        "the PDB did not keep a pod up"
    );
    ensure!(unschedulable(client, node).await?, "drain should cordon");

    // Part 2: no PDB, everything goes.
    set_unschedulable(client, node, false).await?;
    deployments
        .create(
            &PostParams::default(),
            &pinned_deployment("free", node, 1, "drain-free"),
        )
        .await?;
    eventually("free pod running", TIMEOUT, || async {
        Ok((running("free").await? == 1).then_some(()))
    })
    .await?;
    let opts = DrainOptions {
        timeout: Duration::from_secs(60),
        pod_selector: Some("kl-it=drain-free".to_string()),
    };
    let summary = within(
        "free drain",
        Duration::from_secs(90),
        drain_node(client, node, &opts, |_| {}),
    )
    .await?;
    ensure!(summary.complete() && summary.evicted == 1, "{summary:?}");
    Ok(())
}

// ---------------------------------------------------------------------------
// A namespace-scoped user (scripts/test-cluster/limited-user.sh): `edit` in
// team-a only. These tests skip unless KUBELOUPE_LIMITED_KUBECONFIG points
// at the kubeconfig that script writes.
// ---------------------------------------------------------------------------

const LIMITED_NS: &str = "team-a";

/// A client for the limited user, or `None` (skip) when not configured.
async fn connect_limited() -> Result<Option<Client>> {
    let Ok(path) = std::env::var("KUBELOUPE_LIMITED_KUBECONFIG") else {
        eprintln!("KUBELOUPE_LIMITED_KUBECONFIG not set; skipping");
        return Ok(None);
    };
    let kubeconfig = kube::config::Kubeconfig::read_from(&path)
        .with_context(|| format!("cannot read {path}"))?;
    let mut config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default()).await?;
    // As client_for_context: never tunnel the local cluster through an
    // environment proxy.
    config.proxy_url = None;
    Ok(Some(Client::try_from(config)?))
}

#[tokio::test]
#[ignore]
async fn limited_user_access_is_checked_per_namespace() -> Result<()> {
    let Some(client) = connect_limited().await? else {
        return Ok(());
    };
    let pods = kind_named(&client, "pods").await?.resource;
    let deployments = kind_named(&client, "deployments").await?.resource;
    let nodes = kind_named(&client, "nodes").await?.resource;
    let kinds = [pods.clone(), deployments.clone(), nodes.clone()];
    let allowed = |access: &[resources::AccessResult], r: &resources::ResourceRef| {
        access.iter().find(|a| a.id == r.id()).map(|a| a.allowed)
    };

    let in_ns = within("check_access(team-a)", TIMEOUT, async {
        Ok(resources::check_access(&client, &kinds, Some(LIMITED_NS)).await)
    })
    .await?;
    ensure!(allowed(&in_ns, &pods) == Some(true), "{in_ns:?}");
    ensure!(allowed(&in_ns, &deployments) == Some(true), "{in_ns:?}");
    // Cluster-scoped: the namespace does not help.
    ensure!(allowed(&in_ns, &nodes) == Some(false), "{in_ns:?}");

    let all = within("check_access(all)", TIMEOUT, async {
        Ok(resources::check_access(&client, &kinds, None).await)
    })
    .await?;
    ensure!(all.iter().all(|a| !a.allowed), "{all:?}");

    let other = within("check_access(default)", TIMEOUT, async {
        Ok(resources::check_access(&client, &kinds, Some("default")).await)
    })
    .await?;
    ensure!(allowed(&other, &pods) == Some(false), "{other:?}");

    // Starts on its own namespace, and cannot list namespaces.
    ensure!(default_namespace(&client).await.as_deref() == Some(LIMITED_NS));
    ensure!(list_namespaces_if_allowed(&client).await?.is_none());
    Ok(())
}

#[tokio::test]
#[ignore]
async fn limited_user_lists_pods_in_its_namespace_only() -> Result<()> {
    let Some(client) = connect_limited().await? else {
        return Ok(());
    };
    let pods = within(
        "get_pods(team-a)",
        TIMEOUT,
        get_pods(&client, Some(LIMITED_NS)),
    )
    .await?;
    ensure!(pods.iter().all(|p| p.namespace == LIMITED_NS), "{pods:?}");
    let all = tokio::time::timeout(TIMEOUT, get_pods(&client, None)).await?;
    match all {
        Err(e) => ensure!(is_forbidden(&e), "expected a 403, got {e:#}"),
        Ok(p) => bail!("listing pods cluster-wide should be forbidden, got {p:?}"),
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn limited_user_gets_a_partial_overview() -> Result<()> {
    let Some(client) = connect_limited().await? else {
        return Ok(());
    };
    let ov = within("overview", TIMEOUT, get_overview(&client, None)).await?;
    let pods = ov.pods.as_ref().context("pods missing from the overview")?;
    ensure!(pods.namespace.as_deref() == Some(LIMITED_NS), "{ov:?}");
    ensure!(
        pods.by_namespace.iter().all(|(ns, _)| ns == LIMITED_NS),
        "{ov:?}"
    );
    let events = ov.events.as_ref().context("events missing")?;
    ensure!(events.namespace.as_deref() == Some(LIMITED_NS), "{ov:?}");
    ensure!(ov.nodes.is_none() && ov.namespaces.is_none(), "{ov:?}");
    ensure!(ov.notes.len() >= 3, "{:?}", ov.notes);

    // An explicit namespace the user has no access to: pods are left out,
    // the call still succeeds.
    let ov = within(
        "overview(default)",
        TIMEOUT,
        get_overview(&client, Some("default")),
    )
    .await?;
    ensure!(ov.pods.is_none() && ov.events.is_none(), "{ov:?}");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn limited_user_forbidden_watch_stops() -> Result<()> {
    let Some(client) = connect_limited().await? else {
        return Ok(());
    };
    let pods = kind_named(&client, "pods").await?;

    // In its namespace: the watch goes live.
    let (_store, mut updates) = kubeloupe::watch::watch_resource(&client, &pods, Some(LIMITED_NS));
    within("live watch in team-a", TIMEOUT, async {
        while let Some(u) = updates.next().await {
            match u {
                Update::State(WatchState::Live) => return Ok(()),
                Update::State(WatchState::Forbidden { error }) => bail!("forbidden: {error}"),
                _ => {}
            }
        }
        bail!("watch ended")
    })
    .await?;

    // Across the cluster: one Forbidden, then the stream ends (no retries).
    let (_store, updates) = kubeloupe::watch::watch_resource(&client, &pods, None);
    let all: Vec<_> = within("forbidden watch ends", TIMEOUT, async {
        Ok(updates.collect::<Vec<_>>().await)
    })
    .await?;
    ensure!(
        matches!(
            all.as_slice(),
            [
                Update::State(WatchState::Connecting),
                Update::State(WatchState::Forbidden { .. })
            ]
        ),
        "{all:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn admin_sees_everything_unscoped() -> Result<()> {
    let client = connect().await?;
    ensure!(default_namespace(&client).await.is_none());
    ensure!(list_namespaces_if_allowed(&client).await?.is_some());
    let ov = within("overview", TIMEOUT, get_overview(&client, None)).await?;
    ensure!(ov.notes.is_empty(), "{:?}", ov.notes);
    ensure!(ov.namespaces.is_some() && ov.nodes.is_some(), "{ov:?}");
    let pods = ov.pods.context("pods")?;
    ensure!(pods.namespace.is_none() && pods.total > 0);
    ensure!(ov.events.context("events")?.namespace.is_none());
    Ok(())
}

#[tokio::test]
#[ignore]
async fn pending_pod_detail_has_failed_scheduling_event() -> Result<()> {
    let client = connect().await?;
    // The scheduler retries `default/hungry` with backoff; its event is
    // refreshed on every retry.
    let d = eventually("a FailedScheduling event", TIMEOUT, || async {
        let d = get_pod_details(&client, "default", "hungry").await?;
        Ok(d.events
            .iter()
            .any(|e| e.reason == "FailedScheduling" && e.type_ == "Warning")
            .then_some(d))
    })
    .await?;
    let pod = d.pod;
    ensure!(pod.phase == "Pending", "{pod:?}");
    ensure!(pod.node.is_empty(), "{pod:?}");
    ensure!(
        d.conditions
            .iter()
            .any(|c| c.type_ == "PodScheduled" && c.status == "False"),
        "{:?}",
        d.conditions
    );
    let c = &pod.containers[0];
    ensure!(
        c.requests.get("cpu").map(String::as_str) == Some("1k"),
        "{c:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn crashed_pod_detail_has_last_termination() -> Result<()> {
    let client = connect().await?;
    // `demo/crashed` exits 1 every few seconds: right after a restart only
    // `last_state` has the termination, in between `state` has it.
    let c = eventually(
        "a non-zero termination of demo/crashed",
        TIMEOUT,
        || async {
            let d = get_pod_details(&client, "demo", "crashed").await?;
            let c = d.pod.containers.into_iter().find(|c| c.name == "app");
            Ok(c.filter(|c| c.last_state.state == "terminated"))
        },
    )
    .await?;
    ensure!(c.last_state.exit_code != 0, "{c:?}");
    ensure!(c.last_state.reason == "Error", "{c:?}");
    ensure!(!c.last_state.since.is_empty(), "{c:?}");
    ensure!(c.restarts > 0, "{c:?}");
    ensure!(c.image == "busybox:1.36" && !c.image_id.is_empty(), "{c:?}");
    ensure!(c.env.is_empty() && c.probes.is_empty(), "{c:?}");
    Ok(())
}

/// The next detail-watch state that passes `f`.
async fn next_detail(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<DetailState>,
    want: &str,
    f: fn(&DetailState) -> bool,
) -> Result<DetailState> {
    within(want, TIMEOUT, async {
        while let Some(s) = rx.recv().await {
            if f(&s) {
                return Ok(s);
            }
        }
        bail!("watch ended")
    })
    .await
}

#[tokio::test]
#[ignore]
async fn detail_watch_sends_updates_and_deleted() -> Result<()> {
    in_temp_ns("kl-it-dwatch", |client, ns| async move {
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        let mut pod = sleeper_pod("p");
        pod.spec.as_mut().unwrap().containers[0].env = Some(serde_json::from_value(
            serde_json::json!([
                { "name": "LITERAL", "value": "do-not-show-me" },
                { "name": "FROM_SECRET", "valueFrom": { "secretKeyRef": { "name": "s", "key": "k", "optional": true } } }
            ]),
        )?);
        pods.create(&PostParams::default(), &pod).await?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watch = tokio::spawn(resources::run_detail_watch(
            client.clone(),
            pod_ref(),
            ns.clone(),
            "p".into(),
            Duration::from_millis(200),
            move |s| {
                let _ = tx.send(s);
            },
        ));
        let first = next_detail(&mut rx, "the first detail", |s| {
            matches!(s, DetailState::Detail { .. })
        })
        .await?;
        let text = serde_json::to_string(&first)?;
        ensure!(!text.contains("do-not-show-me"), "{text}");
        let DetailState::Detail { detail } = first else {
            unreachable!()
        };
        let env = &detail.pod.containers[0].env;
        ensure!(
            env.iter()
                .any(|e| e.name == "FROM_SECRET" && e.source == "secret:s/k"),
            "{env:?}"
        );

        pods.patch(
            "p",
            &PatchParams::default(),
            &Patch::Merge(serde_json::json!({ "metadata": { "labels": { "watched": "yes" } } })),
        )
        .await?;
        next_detail(&mut rx, "the label update", |s| {
            matches!(s, DetailState::Detail { detail } if detail.labels.get("watched").is_some_and(|v| v == "yes"))
        })
        .await?;

        pods.delete("p", &DeleteParams::default().grace_period(0)).await?;
        next_detail(&mut rx, "deleted", |s| matches!(s, DetailState::Deleted)).await?;
        watch.abort();
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Related objects (#35).
// ---------------------------------------------------------------------------

/// The group titled `title`, failing if it is missing or could not load.
fn related_group<'a>(
    related: &'a resources::Related,
    title: &str,
) -> Result<&'a resources::RelatedGroup> {
    let g = related
        .groups
        .iter()
        .find(|g| g.title == title)
        .with_context(|| format!("no {title} group in {related:?}"))?;
    ensure!(g.error.is_empty(), "{title}: {}", g.error);
    Ok(g)
}

fn related_names(g: &resources::RelatedGroup) -> Vec<String> {
    g.items.iter().map(|i| i.name.clone()).collect()
}

#[tokio::test]
#[ignore]
async fn related_deployment_lists_its_replicaset_and_pods() -> Result<()> {
    let client = connect().await?;
    let kind = kind_named(&client, "deployment").await?;
    let related = within(
        "related of demo/web",
        TIMEOUT,
        resources::get_related(&client, &kind.resource, "demo", "web"),
    )
    .await?;
    ensure!(
        related
            .summary
            .first()
            .is_some_and(|s| s.ends_with(" ready")),
        "{:?}",
        related.summary
    );
    ensure!(
        related.summary.iter().any(|s| s.starts_with("revision ")),
        "{:?}",
        related.summary
    );
    let rs = related_group(&related, "ReplicaSets")?;
    ensure!(rs.kind == "ReplicaSet" && rs.api_version == "apps/v1");
    ensure!(
        !rs.items.is_empty() && rs.items.iter().all(|i| i.name.starts_with("web-")),
        "{rs:?}"
    );
    let pods = related_group(&related, "Pods")?;
    let names = related_names(pods);
    ensure!(names.len() >= 2, "{names:?}");
    // Only the Deployment's own pods, and each one under its ReplicaSet's name.
    ensure!(
        names.iter().all(|n| rs
            .items
            .iter()
            .any(|r| n.starts_with(&format!("{}-", r.name)))),
        "{names:?} vs {rs:?}"
    );
    ensure!(
        related.facts.iter().any(|f| f.label == "Rollout"),
        "{:?}",
        related.facts
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn related_service_lists_its_pods_ports_and_endpoints() -> Result<()> {
    in_temp_ns("kl-it-rel-svc", |client, ns| async move {
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        pods.create(&PostParams::default(), &sleeper_pod("backend"))
            .await?;
        // Not selected by the Service.
        let mut other = sleeper_pod("other");
        other.metadata.labels = Some([("app".to_string(), "else".to_string())].into());
        pods.create(&PostParams::default(), &other).await?;

        let svc: k8s_openapi::api::core::v1::Service = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1", "kind": "Service",
            "metadata": { "name": "front" },
            "spec": {
                "selector": { "app": "it" },
                "ports": [{ "name": "http", "port": 80, "targetPort": 8080 }]
            }
        }))?;
        Api::<k8s_openapi::api::core::v1::Service>::namespaced(client.clone(), &ns)
            .create(&PostParams::default(), &svc)
            .await?;
        wait_running(&client, &ns, "backend").await?;

        let kind = kind_named(&client, "service").await?;
        let related = eventually("the Service's EndpointSlice", TIMEOUT, || async {
            let r = resources::get_related(&client, &kind.resource, &ns, "front").await?;
            let slices = related_group(&r, "EndpointSlices")?;
            Ok((!slices.items.is_empty()).then_some(r))
        })
        .await?;
        let pods = related_group(&related, "Pods")?;
        ensure!(related_names(pods) == ["backend"], "{pods:?}");
        let ports = related_group(&related, "Ports")?;
        ensure!(
            ports.items[0].cells[..3] == ["http", "80", "8080"],
            "{ports:?}"
        );
        // No Ingress routes to it.
        ensure!(related_group(&related, "Ingresses")?.items.is_empty());
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn related_node_lists_the_demo_pods_on_it() -> Result<()> {
    let client = connect().await?;
    let web = first_running_demo_pod(&client, "web-").await?;
    let node = Api::<Pod>::namespaced(client.clone(), "demo")
        .get(&web)
        .await?
        .spec
        .and_then(|s| s.node_name)
        .context("the demo pod has no node")?;
    let kind = kind_named(&client, "node").await?;
    let related = within(
        "related of the node",
        TIMEOUT,
        resources::get_related(&client, &kind.resource, "", &node),
    )
    .await?;
    let pods = related_group(&related, "Pods")?;
    ensure!(
        pods.items
            .iter()
            .any(|i| i.namespace == "demo" && i.name == web),
        "{web} not in {:?}",
        related_names(pods)
    );
    // Only this node's pods: every one of them reports this node.
    let all = get_pods(&client, None).await?;
    for item in &pods.items {
        let p = all
            .iter()
            .find(|p| p.namespace == item.namespace && p.name == item.name);
        ensure!(
            p.is_none_or(|p| p.node == node),
            "{}/{} is not on {node}",
            item.namespace,
            item.name
        );
    }
    let alloc = related_group(&related, "Allocated resources")?;
    ensure!(
        alloc
            .items
            .iter()
            .map(|i| i.cells[0].as_str())
            .collect::<Vec<_>>()
            == ["cpu", "memory", "pods"],
        "{alloc:?}"
    );
    related_group(&related, "Taints")?;
    ensure!(
        related.summary.first().is_some_and(|s| s == "Ready"),
        "{:?}",
        related.summary
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn related_configmap_is_used_by_the_pod_that_mounts_it() -> Result<()> {
    in_temp_ns("kl-it-rel-cm", |client, ns| async move {
        let cm: ConfigMap = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": { "name": "settings" },
            "data": { "a": "1" }
        }))?;
        Api::<ConfigMap>::namespaced(client.clone(), &ns)
            .create(&PostParams::default(), &cm)
            .await?;
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": "reader" },
            "spec": {
                "terminationGracePeriodSeconds": 0,
                "volumes": [{ "name": "cfg", "configMap": { "name": "settings" } }],
                "containers": [{
                    "name": "c", "image": "busybox:1.36", "command": ["sleep", "3600"],
                    "volumeMounts": [{ "name": "cfg", "mountPath": "/cfg" }]
                }]
            }
        }))?;
        let pods: Api<Pod> = Api::namespaced(client.clone(), &ns);
        pods.create(&PostParams::default(), &pod).await?;
        // A pod that does not use it.
        pods.create(&PostParams::default(), &sleeper_pod("bystander"))
            .await?;

        let cm_kind = kind_named(&client, "configmap").await?;
        let related = within(
            "related of the ConfigMap",
            TIMEOUT,
            resources::get_related(&client, &cm_kind.resource, &ns, "settings"),
        )
        .await?;
        let used = related_group(&related, "Used by pods")?;
        ensure!(related_names(used) == ["reader"], "{used:?}");
        ensure!(
            used.items[0]
                .cells
                .last()
                .is_some_and(|c| c == "volume cfg"),
            "{used:?}"
        );
        ensure!(related.summary == ["1 key"], "{:?}", related.summary);

        // The reverse, in the pod's panel.
        let from_pod = within(
            "related of the pod",
            TIMEOUT,
            resources::get_related(&client, &pod_ref(), &ns, "reader"),
        )
        .await?;
        let cms = related_group(&from_pod, "ConfigMaps")?;
        // Besides `kube-root-ca.crt`, which the service account volume projects.
        ensure!(
            related_names(cms).contains(&"settings".to_string()),
            "{cms:?}"
        );
        Ok(())
    })
    .await
}
