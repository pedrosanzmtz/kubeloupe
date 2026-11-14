//! Integration tests of `apply_manifests` (create from YAML) against the kind
//! cluster. `#[ignore]`d like `cluster.rs`; run with
//! `cargo test --test apply -- --ignored`. Everything is created in
//! temporary namespaces, with a CRD group unique to the run, and removed.

use std::future::Future;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, DeleteParams, DynamicObject, PostParams};
use kube::discovery::ApiResource;
use kube::Client;
use kubeloupe::client_for_context;
use kubeloupe::resources::{
    apply_manifests, discover, ApplyResult, ApplyStatus, ManifestOptions, ResourceKind,
};

async fn connect() -> Result<Client> {
    let ctx = std::env::var("KUBELOUPE_TEST_CONTEXT")
        .unwrap_or_else(|_| "kind-kubeloupe-test".to_string());
    tokio::time::timeout(Duration::from_secs(20), client_for_context(Some(&ctx)))
        .await
        .context("timed out building client")?
        .with_context(|| format!("cannot connect to context {ctx}"))
}

fn suffix() -> String {
    format!(
        "{:x}",
        std::process::id() ^ chrono::Utc::now().timestamp_subsec_micros()
    )
}

/// Run `body` in a fresh namespace, deleting it afterwards even on error.
async fn in_temp_ns<F, Fut>(prefix: &str, body: F) -> Result<()>
where
    F: FnOnce(Client, String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let client = connect().await?;
    let ns = format!("{prefix}-{}", suffix());
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let obj: Namespace = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": ns }
    }))?;
    namespaces.create(&PostParams::default(), &obj).await?;
    let result = body(client.clone(), ns.clone()).await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}

async fn kinds(client: &Client) -> Result<Vec<ResourceKind>> {
    tokio::time::timeout(Duration::from_secs(60), discover(client))
        .await
        .context("discovery timed out")?
}

async fn apply(
    client: &Client,
    yaml: &str,
    ns: &str,
    dry_run: bool,
    skip_invalid: bool,
) -> Result<Vec<ApplyResult>> {
    let kinds = kinds(client).await?;
    let opts = ManifestOptions {
        dry_run,
        skip_invalid,
    };
    tokio::time::timeout(
        Duration::from_secs(90),
        apply_manifests(client, &kinds, yaml, ns, opts),
    )
    .await
    .context("apply timed out")
}

fn statuses(results: &[ApplyResult]) -> Vec<(String, ApplyStatus)> {
    results
        .iter()
        .map(|r| (format!("{}/{}", r.kind, r.name), r.status))
        .collect()
}

async fn configmap_exists(client: &Client, ns: &str, name: &str) -> Result<bool> {
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), ns);
    Ok(api.get_opt(name).await?.is_some())
}

#[tokio::test]
#[ignore]
async fn apply_crd_and_instance_in_one_manifest_dry_run_first_then_unchanged() -> Result<()> {
    in_temp_ns("kl-it-apply-crd", |client, ns| async move {
        let group = format!("klapply{}.example.com", suffix());
        let crd_name = format!("gadgets.{group}");
        let yaml = format!(
            "apiVersion: {group}/v1
kind: Gadget
metadata:
  name: g1
spec:
  size: 3
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
data:
  a: \"1\"
---
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: {crd_name}
spec:
  group: {group}
  scope: Namespaced
  names: {{plural: gadgets, singular: gadget, kind: Gadget}}
  versions:
    - name: v1
      served: true
      storage: true
      schema:
        openAPIV3Schema: {{type: object, x-kubernetes-preserve-unknown-fields: true}}
"
        );
        let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
        let result = async {
            // A dry run validates what it can and stores nothing; the Gadget
            // waits for its CRD.
            let dry = apply(&client, &yaml, &ns, true, false).await?;
            ensure!(
                statuses(&dry)
                    == vec![
                        ("Gadget/g1".to_string(), ApplyStatus::Pending),
                        ("ConfigMap/settings".to_string(), ApplyStatus::Created),
                        (
                            format!("CustomResourceDefinition/{crd_name}"),
                            ApplyStatus::Created
                        ),
                    ],
                "{dry:?}"
            );
            ensure!(dry[1].namespace == ns, "namespace defaulted: {dry:?}");
            ensure!(
                crds.get_opt(&crd_name).await?.is_none(),
                "dry run created the CRD"
            );
            ensure!(!configmap_exists(&client, &ns, "settings").await?);

            // The real apply: CRD first, then the Gadget once it is served.
            let done = apply(&client, &yaml, &ns, false, false).await?;
            ensure!(
                done.iter().all(|r| r.status == ApplyStatus::Created),
                "{done:?}"
            );
            let ar = ApiResource {
                group: group.clone(),
                version: "v1".into(),
                api_version: format!("{group}/v1"),
                kind: "Gadget".into(),
                plural: "gadgets".into(),
            };
            let gadgets: Api<DynamicObject> = Api::namespaced_with(client.clone(), &ns, &ar);
            let g = gadgets.get("g1").await?;
            ensure!(g.data["spec"]["size"] == 3, "{g:?}");
            ensure!(configmap_exists(&client, &ns, "settings").await?);

            // Again: nothing changes.
            let again = apply(&client, &yaml, &ns, false, false).await?;
            ensure!(
                again.iter().all(|r| r.status == ApplyStatus::Unchanged),
                "{again:?}"
            );
            // A change is "configured".
            let changed = yaml.replace("a: \"1\"", "a: \"2\"");
            let third = apply(&client, &changed, &ns, false, false).await?;
            ensure!(third[1].status == ApplyStatus::Configured, "{third:?}");
            ensure!(third[0].status == ApplyStatus::Unchanged, "{third:?}");
            Ok(())
        }
        .await;
        let _ = crds.delete(&crd_name, &DeleteParams::default()).await;
        result
    })
    .await
}

#[tokio::test]
#[ignore]
async fn apply_an_invalid_document_blocks_the_batch_unless_skipped() -> Result<()> {
    in_temp_ns("kl-it-apply-err", |client, ns| async move {
        let yaml = format!(
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: good\n  namespace: {ns}\n\
             ---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: Not_Valid\n  namespace: {ns}\n\
             ---\napiVersion: v1\nkind: Nonsense\nmetadata:\n  name: x\n"
        );
        let dry = apply(&client, &yaml, &ns, true, false).await?;
        ensure!(dry[0].status == ApplyStatus::Created, "{dry:?}");
        ensure!(dry[1].status == ApplyStatus::Failed, "{dry:?}");
        ensure!(
            dry[1].message.contains("invalid") && dry[1].line == 7,
            "{dry:?}"
        );
        ensure!(
            dry[2].status == ApplyStatus::Failed
                && dry[2].message.contains("does not serve Nonsense"),
            "{dry:?}"
        );

        // Not applied while a document fails.
        let blocked = apply(&client, &yaml, &ns, false, false).await?;
        ensure!(blocked[0].status == ApplyStatus::Skipped, "{blocked:?}");
        ensure!(!configmap_exists(&client, &ns, "good").await?);

        // Unless the user applies just the valid ones.
        let partial = apply(&client, &yaml, &ns, false, true).await?;
        ensure!(partial[0].status == ApplyStatus::Created, "{partial:?}");
        ensure!(partial[1].status == ApplyStatus::Failed, "{partial:?}");
        ensure!(configmap_exists(&client, &ns, "good").await?);
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore]
async fn apply_a_namespace_and_its_objects_together() -> Result<()> {
    let client = connect().await?;
    let ns = format!("kl-it-apply-ns-{}", suffix());
    // The ConfigMap comes first in the file; the Namespace is applied first.
    let yaml = format!(
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: inside\n  namespace: {ns}\n\
         ---\napiVersion: v1\nkind: Namespace\nmetadata:\n  name: {ns}\n"
    );
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let result = async {
        let dry = apply(&client, &yaml, "default", true, false).await?;
        ensure!(
            statuses(&dry)
                == vec![
                    ("ConfigMap/inside".to_string(), ApplyStatus::Pending),
                    (format!("Namespace/{ns}"), ApplyStatus::Created),
                ],
            "{dry:?}"
        );
        ensure!(
            namespaces.get_opt(&ns).await?.is_none(),
            "dry run created it"
        );
        let done = apply(&client, &yaml, "default", false, false).await?;
        ensure!(
            done.iter().all(|r| r.status == ApplyStatus::Created),
            "{done:?}"
        );
        ensure!(configmap_exists(&client, &ns, "inside").await?);
        Ok(())
    }
    .await;
    let _ = namespaces.delete(&ns, &DeleteParams::background()).await;
    result
}
