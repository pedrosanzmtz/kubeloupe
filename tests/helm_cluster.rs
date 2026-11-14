//! Helm releases against the kind cluster (`scripts/test-cluster/setup.sh`),
//! with the local chart in `scripts/test-cluster/charts/demo-chart`. All
//! `#[ignore]`d; run with `cargo test --test helm_cluster -- --ignored`.
//!
//! They need the `helm` binary (on `PATH`, or `KUBELOUPE_HELM`) and skip
//! without it, except in CI, where a missing helm fails them. Each test
//! installs its release in its own temporary namespace and deletes it.

use std::future::Future;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, DeleteParams, PostParams};
use kube::Client;
use kubeloupe::client_for_context;
use kubeloupe::helm::{
    get_helm_release, helm_rollback, helm_uninstall, helm_version, list_helm_releases,
    release_charts, HelmRevisionKey, HelmTarget,
};
use kubeloupe::resources::REDACTED;

fn context() -> String {
    std::env::var("KUBELOUPE_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubeloupe-test".to_string())
}

fn helm_bin() -> String {
    std::env::var("KUBELOUPE_HELM")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "helm".into())
}

const CHART: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/test-cluster/charts/demo-chart"
);

/// Whether helm is there; in CI its absence is an error.
async fn have_helm() -> Result<bool> {
    match helm_version().await {
        Ok(_) => Ok(true),
        Err(e) if std::env::var_os("CI").is_some() => bail!("helm is required in CI: {e:#}"),
        Err(e) => {
            eprintln!("skipping: {e:#}");
            Ok(false)
        }
    }
}

/// Run helm against the test context, failing with its stderr.
fn helm(args: &[&str]) -> Result<()> {
    let out = Command::new(helm_bin())
        .args(args)
        .args(["--kube-context", &context()])
        .output()
        .context("cannot run helm")?;
    ensure!(
        out.status.success(),
        "helm {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

fn kubeloupe(args: &[&str]) -> Result<(String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_kubeloupe"))
        .args(args)
        .args(["--context", &context()])
        .output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    ensure!(out.status.success(), "kubeloupe {args:?}: {stderr}");
    Ok((stdout, stderr))
}

async fn in_temp_ns<F, Fut>(prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(Client, String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let client = tokio::time::timeout(
        Duration::from_secs(20),
        client_for_context(Some(&context())),
    )
    .await
    .context("timed out building client")??;
    let suffix = std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros();
    let ns = format!("{prefix}-{suffix:x}");
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": ns }
    }))?;
    namespaces.create(&PostParams::default(), &obj).await?;
    let result = tokio::time::timeout(Duration::from_secs(240), body(client.clone(), ns.clone()))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out")));
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

#[tokio::test]
#[ignore]
async fn helm_releases_list_decode_roll_back_and_uninstall() -> Result<()> {
    if !have_helm().await? {
        return Ok(());
    }
    in_temp_ns("kl-it-helm", |client, ns| async move {
        let rel = "kl-demo";
        helm(&["install", rel, CHART, "-n", &ns])?;
        helm(&["upgrade", rel, CHART, "-n", &ns, "--set", "greeting=bye"])?;

        // The list comes from labels: two revisions, the newest current.
        let list = list_helm_releases(&client, std::slice::from_ref(&ns)).await?;
        ensure!(list.warnings.is_empty(), "{:?}", list.warnings);
        ensure!(list.releases.len() == 1, "{:?}", list.releases);
        let row = &list.releases[0];
        ensure!(
            row.name == rel && row.namespace == ns && row.revision == 2,
            "{row:?}"
        );
        ensure!(row.status == "deployed" && row.updated > 0, "{row:?}");
        let history: Vec<(u32, &str)> = row
            .history
            .iter()
            .map(|h| (h.revision, h.status.as_str()))
            .collect();
        ensure!(
            history == [(2, "deployed"), (1, "superseded")],
            "{history:?}"
        );

        // The chart columns, from the payload.
        let charts = release_charts(
            &client,
            &[HelmRevisionKey {
                namespace: ns.clone(),
                name: rel.into(),
                revision: 2,
                storage: row.storage,
            }],
        )
        .await;
        let c = &charts[0];
        ensure!(c.error.is_empty(), "{c:?}");
        ensure!(
            (
                c.chart.as_str(),
                c.chart_version.as_str(),
                c.app_version.as_str()
            ) == ("demo-chart", "0.1.0", "1.36"),
            "{c:?}"
        );

        // The newest revision, decoded, with the Secret hidden.
        let r = get_helm_release(&client, &ns, rel, None, false).await?;
        ensure!(r.revision == 2 && r.status == "deployed", "{r:?}");
        ensure!(r.values == "greeting: bye\n", "{}", r.values);
        ensure!(
            r.computed_values.contains("replicas: 1"),
            "{}",
            r.computed_values
        );
        ensure!(
            r.computed_values.contains("greeting: bye"),
            "{}",
            r.computed_values
        );
        ensure!(r.notes.trim() == format!("{rel} says bye."), "{}", r.notes);
        ensure!(r.description == "Upgrade complete", "{}", r.description);
        ensure!(r.secrets_hidden, "secrets not hidden");
        ensure!(!r.manifest.contains("not-a-real-secret"), "{}", r.manifest);
        ensure!(r.manifest.contains(REDACTED), "{}", r.manifest);
        ensure!(r.manifest.contains("greeting: \"bye\""), "{}", r.manifest);
        let kinds: Vec<String> = r
            .resources
            .iter()
            .map(|o| format!("{}/{}", o.kind, o.name))
            .collect();
        for want in [
            format!("Secret/{rel}-secret"),
            format!("ConfigMap/{rel}-config"),
            format!("Deployment/{rel}-app"),
        ] {
            ensure!(kinds.contains(&want), "{want} not in {kinds:?}");
        }

        // An older revision, revealed.
        let r1 = get_helm_release(&client, &ns, rel, Some(1), true).await?;
        ensure!(r1.revision == 1 && r1.status == "superseded", "{r1:?}");
        ensure!(r1.values.is_empty(), "{}", r1.values);
        ensure!(!r1.secrets_hidden && r1.manifest.contains("not-a-real-secret"));
        ensure!(
            r1.notes.trim() == format!("{rel} says hello."),
            "{}",
            r1.notes
        );
        let e = get_helm_release(&client, &ns, rel, Some(9), false)
            .await
            .unwrap_err();
        ensure!(e.to_string().contains("no revision 9"), "{e}");
        let e = get_helm_release(&client, &ns, "missing", None, false)
            .await
            .unwrap_err();
        ensure!(
            e.to_string().contains("No Helm release named missing"),
            "{e}"
        );

        // The CLI.
        let (out, _) = kubeloupe(&["helm", "list", "-n", &ns])?;
        ensure!(
            out.starts_with("NAME") && out.contains("demo-chart-0.1.0"),
            "{out}"
        );
        let (out, _) = kubeloupe(&["helm", "get", rel, "values", "-n", &ns])?;
        ensure!(out == "greeting: bye\n", "{out}");
        let (out, err) = kubeloupe(&["helm", "get", rel, "manifest", "-n", &ns])?;
        ensure!(
            !out.contains("not-a-real-secret") && err.contains("--reveal"),
            "{out}"
        );
        let (out, _) = kubeloupe(&["helm", "get", rel, "notes", "-n", &ns, "--revision", "1"])?;
        ensure!(out.trim() == format!("{rel} says hello."), "{out}");
        let (out, _) = kubeloupe(&["helm", "get", rel, "-n", &ns])?;
        ensure!(
            out.contains("STATUS: deployed") && out.contains("HISTORY:"),
            "{out}"
        );

        // Phase 2, through the helm CLI.
        let target = HelmTarget {
            kubeconfig: None,
            context: Some(context()),
        };
        let msg = helm_rollback(&target, &ns, rel, 1).await?;
        ensure!(msg.contains("revision 1"), "{msg}");
        let r = get_helm_release(&client, &ns, rel, None, false).await?;
        ensure!(r.revision == 3 && r.status == "deployed", "{r:?}");
        ensure!(r.values.is_empty(), "rollback kept values: {}", r.values);
        ensure!(
            r.notes.trim() == format!("{rel} says hello."),
            "{}",
            r.notes
        );

        let e = helm_rollback(&target, &ns, rel, 42).await.unwrap_err();
        ensure!(
            e.to_string().starts_with("Rollback of kl-demo failed:"),
            "{e}"
        );

        helm_uninstall(&target, &ns, rel).await?;
        let list = list_helm_releases(&client, std::slice::from_ref(&ns)).await?;
        ensure!(list.releases.is_empty(), "{:?}", list.releases);
        Ok(())
    })
    .await
}

/// The limited user (`limited-user.sh`) may read Secrets in team-a only:
/// other scopes become warnings, not errors.
#[tokio::test]
#[ignore]
async fn limited_user_helm_listing_degrades_per_namespace() -> Result<()> {
    let Ok(path) = std::env::var("KUBELOUPE_LIMITED_KUBECONFIG") else {
        eprintln!("KUBELOUPE_LIMITED_KUBECONFIG not set; skipping");
        return Ok(());
    };
    let kubeconfig = kube::config::Kubeconfig::read_from(&path)?;
    let mut config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default()).await?;
    config.proxy_url = None;
    let client = Client::try_from(config)?;

    let all = list_helm_releases(&client, &[]).await?;
    ensure!(
        all.warnings.len() == 1 && all.warnings[0].contains("across all namespaces"),
        "{:?}",
        all.warnings
    );
    let some =
        list_helm_releases(&client, &["team-a".to_string(), "kube-system".to_string()]).await?;
    ensure!(
        some.warnings.len() == 1 && some.warnings[0].contains("namespace kube-system"),
        "{:?}",
        some.warnings
    );
    let e = get_helm_release(&client, "kube-system", "anything", None, false)
        .await
        .unwrap_err();
    ensure!(
        e.to_string().starts_with("Not allowed to list Secrets"),
        "{e}"
    );
    Ok(())
}
