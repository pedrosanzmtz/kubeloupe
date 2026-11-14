//! Tests of the `kubeloupe` binary: exit codes, stdout/stderr, and (the
//! `#[ignore]`d ones) its output against the kind cluster. Run those with
//! `cargo test --test cli -- --ignored`; the context comes from
//! `KUBELOUPE_TEST_CONTEXT` (default `kind-kubeloupe-test`).

use std::process::{Command, Output};

use anyhow::Result;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Secret};
use kube::api::{Api, DeleteParams, PostParams};
use serde_json::json;

fn context() -> String {
    std::env::var("KUBELOUPE_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubeloupe-test".into())
}

fn kubeloupe(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kubeloupe"))
        .args(args)
        .output()
        .expect("run kubeloupe")
}

/// Run against the test cluster's context; panics unless it succeeds.
fn on_cluster(args: &[&str]) -> String {
    let ctx = context();
    let mut all = vec!["--context", &ctx];
    all.extend_from_slice(args);
    let out = kubeloupe(&all);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn completions_need_no_cluster() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = kubeloupe(&["--kubeconfig", "/nonexistent", "completions", shell]);
        assert!(out.status.success(), "{shell}");
        let script = String::from_utf8_lossy(&out.stdout);
        assert!(script.contains("kubeloupe"), "{shell}");
        assert!(script.contains("describe"), "{shell}");
    }
}

#[test]
fn errors_go_to_stderr_with_a_failing_exit_code() {
    let out = kubeloupe(&["--kubeconfig", "/nonexistent/kubeconfig", "get", "pods"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.starts_with("error: cannot read kubeconfig /nonexistent/kubeconfig"),
        "{err}"
    );
    assert!(
        !err.contains('\x1b'),
        "no colour when not a terminal: {err}"
    );
}

#[test]
fn bad_arguments_exit_with_usage_error() {
    let out = kubeloupe(&["get", "pods", "-o", "xml"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
#[ignore]
fn cli_context_and_names() {
    let out = on_cluster(&["get", "pods", "-n", "demo", "-o", "name"]);
    let names: Vec<_> = out.lines().collect();
    assert!(names.contains(&"pod/crashed"), "{out}");
    assert!(names.iter().all(|n| n.starts_with("pod/")), "{out}");
    let out = on_cluster(&["get", "deploy", "-n", "demo", "-o", "name"]);
    assert!(out.lines().any(|l| l == "deployment.apps/web"), "{out}");
}

#[test]
#[ignore]
fn cli_kubeconfig_flag_reads_that_file() {
    let kc = kube::config::Kubeconfig::read().expect("a kubeconfig");
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-kubeconfig.yaml");
    std::fs::write(&path, serde_yaml::to_string(&kc).unwrap()).unwrap();
    let ctx = context();
    let out = Command::new(env!("CARGO_BIN_EXE_kubeloupe"))
        .args(["--kubeconfig", path.to_str().unwrap(), "--context", &ctx])
        .args(["get", "ns", "demo", "-o", "name"])
        // Make sure the file given is the only one it could have read.
        .env("KUBECONFIG", "/nonexistent")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "namespace/demo\n");
}

#[test]
#[ignore]
fn cli_yaml_of_a_deployment() {
    let out = on_cluster(&["yaml", "deploy/web", "-n", "demo"]);
    assert!(
        out.starts_with("apiVersion: apps/v1\nkind: Deployment\n"),
        "{out}"
    );
    assert!(!out.contains("managedFields"));
    // A bare name is still a pod.
    let out = on_cluster(&["yaml", "crashed", "-n", "demo"]);
    assert!(out.starts_with("apiVersion: v1\nkind: Pod\n"), "{out}");
}

#[test]
#[ignore]
fn cli_describe_shows_failed_scheduling() {
    let out = on_cluster(&["describe", "pod/hungry", "-n", "default"]);
    assert!(out.starts_with("Name:           hungry\n"), "{out}");
    assert!(out.contains("PodScheduled  False"), "{out}");
    assert!(out.contains("FailedScheduling"), "{out}");
}

#[test]
#[ignore]
fn cli_not_found_is_a_plain_error() {
    let ctx = context();
    let out = kubeloupe(&[
        "--context",
        &ctx,
        "describe",
        "pod",
        "nothere",
        "-n",
        "demo",
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "error: pods \"nothere\" not found\n"
    );
}

#[tokio::test]
#[ignore]
async fn cli_selectors_formats_and_secret_reveal() -> Result<()> {
    let client = kubeloupe::client_for_context(Some(&context())).await?;
    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("kl-it-cli-{suffix:x}");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces
        .create(
            &PostParams::default(),
            &serde_json::from_value(json!({ "metadata": { "name": ns } }))?,
        )
        .await?;
    let result = async {
        let cms: Api<ConfigMap> = Api::namespaced(client.clone(), &ns);
        for (name, tier) in [("a", "fe"), ("b", "be")] {
            let cm = json!({ "metadata": { "name": name, "labels": { "tier": tier } } });
            cms.create(&PostParams::default(), &serde_json::from_value(cm)?)
                .await?;
        }
        let secrets: Api<Secret> = Api::namespaced(client.clone(), &ns);
        let secret = json!({ "metadata": { "name": "creds" }, "data": { "pw": "aHVudGVyMg==" } });
        secrets
            .create(&PostParams::default(), &serde_json::from_value(secret)?)
            .await?;

        let out = on_cluster(&["get", "cm", "-n", &ns, "-l", "tier=fe", "-o", "name"]);
        assert_eq!(out, "configmap/a\n");
        let out = on_cluster(&[
            "get",
            "cm",
            "-n",
            &ns,
            "--field-selector",
            "metadata.name=b",
            "-o",
            "json",
        ]);
        let list: serde_json::Value = serde_json::from_str(&out)?;
        assert_eq!(list["kind"], "List");
        assert_eq!(list["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(list["items"][0]["metadata"]["name"], "b");
        let out = on_cluster(&["get", "cm/a", "-n", &ns, "-o", "wide"]);
        assert!(out.contains("tier=fe"), "{out}");

        let hidden = on_cluster(&["yaml", "secret/creds", "-n", &ns]);
        assert!(!hidden.contains("aHVudGVyMg=="), "{hidden}");
        let shown = on_cluster(&["yaml", "secret/creds", "-n", &ns, "--reveal"]);
        assert!(shown.contains("aHVudGVyMg=="), "{shown}");
        let hidden = on_cluster(&["get", "secret", "creds", "-n", &ns, "-o", "json"]);
        assert!(!hidden.contains("aHVudGVyMg=="), "{hidden}");
        let described = on_cluster(&["describe", "secret", "creds", "-n", &ns]);
        assert!(described.contains("pw: 7 bytes"), "{described}");
        assert!(!described.contains("aHVudGVyMg=="), "{described}");

        // An empty list says so on stderr and leaves stdout empty.
        let ctx = context();
        let out = kubeloupe(&["--context", &ctx, "get", "cm", "-n", &ns, "-l", "tier=none"]);
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("No resources found"));
        Ok(())
    }
    .await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}
