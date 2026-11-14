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
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams};
use kube::Client;
use kubeloupe::resources::{
    self, apply_resource_yaml, delete_resource, get_resource_detail, get_resource_yaml,
    list_resources, Category, ResourceKind,
};
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
            resources::check_access(&client, &[widget.resource.clone(), kinds[0].resource.clone()])
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
