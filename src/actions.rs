//! Resource actions: scale, rollout restart, cordon/uncordon, drain, trigger a
//! CronJob, suspend/resume, delete (graceful, forced, bulk).
//!
//! One registry ([`actions_for`]) says which actions apply to which kind. The
//! CLI and the desktop UI (which receives the list with each discovered
//! [`ResourceKind`](crate::resources::ResourceKind)) both read it, so the
//! context menu, the detail panel and the command line cannot drift apart.
//!
//! Everything the cluster is asked to do goes through the generic layer in
//! [`crate::resources`]. Results reach the UI through the live watch.

use crate::resources::{self, dyn_api, ResourceKind, ResourceRef};
use anyhow::{bail, Context, Result};
use futures::StreamExt;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, EvictParams, ListParams, Patch, PatchParams, PostParams};
use kube::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

const FIELD_MANAGER: &str = "kubeloupe";

/// What `kubectl rollout restart` patches.
pub const RESTARTED_AT: &str = "kubectl.kubernetes.io/restartedAt";
/// Marks a Job created by hand from a CronJob.
pub const INSTANTIATE: &str = "cronjob.kubernetes.io/instantiate";
const MIRROR_POD: &str = "kubernetes.io/config.mirror";

// ---------------------------------------------------------------------------
// The registry.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ActionId {
    Delete,
    ForceDelete,
    Scale,
    Restart,
    Cordon,
    Uncordon,
    Drain,
    Trigger,
    Suspend,
    Resume,
}

/// An extra value the action needs before it can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ActionInput {
    /// Nothing more is needed.
    None,
    /// A non-negative whole number of replicas.
    Replicas,
}

/// One action offered for a kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ActionDef {
    pub id: ActionId,
    pub label: String,
    /// Needs a confirmation naming the object(s). (Scale asks again when the
    /// target is 0, see [`scale_is_destructive`].)
    pub destructive: bool,
    /// The confirmation also makes the user type the object's name.
    pub type_to_confirm: bool,
    pub input: ActionInput,
}

fn def(id: ActionId, label: &str) -> ActionDef {
    ActionDef {
        id,
        label: label.to_string(),
        destructive: false,
        type_to_confirm: false,
        input: ActionInput::None,
    }
}

/// The actions that apply to a kind, in menu order. `verbs` are what the API
/// server serves for the kind: Delete needs the `delete` verb. The other
/// actions are patches and are offered whenever the kind matches; the API
/// server's answer (403) is shown if the user may not.
pub fn actions_for(r: &ResourceRef, verbs: &[String]) -> Vec<ActionDef> {
    use ActionId::*;
    let mut out = Vec::new();
    match (r.group.as_str(), r.kind.as_str()) {
        ("apps", "Deployment" | "StatefulSet" | "ReplicaSet") => {
            out.push(ActionDef {
                input: ActionInput::Replicas,
                ..def(Scale, "Scale…")
            });
            if r.kind != "ReplicaSet" {
                out.push(def(Restart, "Rollout restart"));
            }
        }
        ("apps", "DaemonSet") => out.push(def(Restart, "Rollout restart")),
        ("", "Node") => {
            out.push(def(Cordon, "Cordon"));
            out.push(def(Uncordon, "Uncordon"));
            out.push(ActionDef {
                destructive: true,
                ..def(Drain, "Drain…")
            });
        }
        ("batch", "CronJob") => {
            out.push(def(Trigger, "Trigger now"));
            out.push(def(Suspend, "Suspend"));
            out.push(def(Resume, "Resume"));
        }
        _ => {}
    }
    if verbs.iter().any(|v| v == "delete") {
        out.push(ActionDef {
            destructive: true,
            ..def(Delete, "Delete")
        });
        if (r.group.as_str(), r.kind.as_str()) == ("", "Pod") {
            out.push(ActionDef {
                destructive: true,
                type_to_confirm: true,
                ..def(ForceDelete, "Force delete…")
            });
        }
    }
    out
}

/// Scaling to zero stops the workload, so it is confirmed like a delete.
pub fn scale_is_destructive(replicas: i32) -> bool {
    replicas == 0
}

// ---------------------------------------------------------------------------
// Error messages.
// ---------------------------------------------------------------------------

/// The API error behind `err`, if any, as `(http code, message)`.
fn api_error(err: &anyhow::Error) -> Option<(u16, String)> {
    err.chain()
        .find_map(|e| match e.downcast_ref::<kube::Error>() {
            Some(kube::Error::Api(r)) => Some((r.code, r.message.clone())),
            _ => None,
        })
}

/// Plain-words text for an API error code. Pure, so it is unit tested.
pub fn describe_status(code: u16, message: &str) -> String {
    let lower = message.to_lowercase();
    match code {
        403 => format!("Not allowed: your account lacks permission for this action. {message}"),
        401 => "Not signed in: the cluster rejected your credentials.".to_string(),
        404 => "Already gone: the object no longer exists.".to_string(),
        409 => format!("Conflict: the object changed while the action ran. Try again. {message}"),
        422 => format!("The cluster rejected the request as invalid. {message}"),
        429 if lower.contains("disruption budget") => format!(
            "Blocked by a PodDisruptionBudget: evicting would leave too few pods running. {message}"
        ),
        429 => format!("The cluster is asking clients to slow down (429). {message}"),
        500 if lower.contains("disruption budget") => format!(
            "Blocked by a PodDisruptionBudget (misconfigured, several budgets match the pod). {message}"
        ),
        c if c >= 500 => format!("The API server failed ({c}). {message}"),
        _ => message.to_string(),
    }
}

/// Turn any error from an action into the sentence shown inline.
pub fn describe_error(err: &anyhow::Error) -> String {
    match api_error(err) {
        Some((code, msg)) => describe_status(code, &msg),
        None => format!("{err:#}"),
    }
}

// ---------------------------------------------------------------------------
// Simple patches.
// ---------------------------------------------------------------------------

fn patch_params() -> PatchParams {
    PatchParams {
        field_manager: Some(FIELD_MANAGER.to_string()),
        ..PatchParams::default()
    }
}

fn namespaced_api(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
) -> Result<Api<kube::api::DynamicObject>> {
    if r.namespaced && namespace.is_empty() {
        bail!("{} is namespaced: a namespace is required", r.kind);
    }
    Ok(dyn_api(client, r, Some(namespace)))
}

/// Set the replica count through the `/scale` subresource.
pub async fn scale(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    replicas: i32,
) -> Result<()> {
    if replicas < 0 {
        bail!("replicas must be 0 or more");
    }
    namespaced_api(client, r, namespace)?
        .patch_scale(
            name,
            &patch_params(),
            &Patch::Merge(json!({ "spec": { "replicas": replicas } })),
        )
        .await?;
    Ok(())
}

/// The merge patch of a rollout restart.
pub fn restart_patch(timestamp: &str) -> serde_json::Value {
    json!({ "spec": { "template": { "metadata": { "annotations": { RESTARTED_AT: timestamp } } } } })
}

/// `kubectl rollout restart`: stamp the pod template so the controller rolls
/// the pods.
pub async fn rollout_restart(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
) -> Result<()> {
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    namespaced_api(client, r, namespace)?
        .patch(name, &patch_params(), &Patch::Merge(restart_patch(&ts)))
        .await?;
    Ok(())
}

/// Mark a node (un)schedulable.
pub async fn set_unschedulable(client: &Client, node: &str, unschedulable: bool) -> Result<()> {
    let api: Api<k8s_openapi::api::core::v1::Node> = Api::all(client.clone());
    api.patch(
        node,
        &patch_params(),
        &Patch::Merge(json!({ "spec": { "unschedulable": unschedulable } })),
    )
    .await?;
    Ok(())
}

/// Suspend or resume a CronJob.
pub async fn set_suspended(
    client: &Client,
    namespace: &str,
    name: &str,
    suspend: bool,
) -> Result<()> {
    let api: Api<CronJob> = Api::namespaced(client.clone(), namespace);
    api.patch(
        name,
        &patch_params(),
        &Patch::Merge(json!({ "spec": { "suspend": suspend } })),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Trigger a CronJob.
// ---------------------------------------------------------------------------

/// Longest Job name: the job controller copies it into a 63 character label.
const MAX_JOB_NAME: usize = 63;

/// `<cronjob>-manual-<suffix>`, shortening the CronJob part if the whole
/// would not fit a Job name.
pub fn manual_job_name(cronjob: &str, suffix: &str) -> String {
    let tail = format!("-manual-{suffix}");
    let keep = MAX_JOB_NAME.saturating_sub(tail.len());
    let head: String = cronjob.chars().take(keep).collect();
    format!("{}{tail}", head.trim_end_matches(['-', '.']))
}

/// A random 5 character suffix from the alphabet Kubernetes uses for
/// generated names (no vowels, no look-alikes).
fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    const ALPHABET: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
    let mut n = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let mut out = String::new();
    for _ in 0..5 {
        out.push(ALPHABET[(n % ALPHABET.len() as u64) as usize] as char);
        n /= ALPHABET.len() as u64;
    }
    out
}

/// The Job `kubectl create job --from=cronjob/<name>` would create: the
/// CronJob's `jobTemplate`, owned by the CronJob and annotated as manual.
pub fn job_from_cronjob(cj: &CronJob, suffix: &str) -> Result<Job> {
    let name = cj.metadata.name.clone().context("CronJob has no name")?;
    let template = cj
        .spec
        .as_ref()
        .context("CronJob has no spec")?
        .job_template
        .clone();
    let tmeta = template.metadata.unwrap_or_default();
    let mut annotations: BTreeMap<String, String> = tmeta.annotations.unwrap_or_default();
    annotations.insert(INSTANTIATE.to_string(), "manual".to_string());
    let mut job = Job::default();
    job.metadata.name = Some(manual_job_name(&name, suffix));
    job.metadata.namespace = cj.metadata.namespace.clone();
    job.metadata.labels = tmeta.labels;
    job.metadata.annotations = Some(annotations);
    job.metadata.owner_references = Some(vec![OwnerReference {
        api_version: "batch/v1".to_string(),
        kind: "CronJob".to_string(),
        name,
        uid: cj.metadata.uid.clone().context("CronJob has no uid")?,
        controller: Some(true),
        block_owner_deletion: Some(true),
    }]);
    job.spec = template.spec;
    Ok(job)
}

/// Create a Job from the CronJob's template. Returns the Job's name.
pub async fn trigger_cronjob(client: &Client, namespace: &str, name: &str) -> Result<String> {
    let cjs: Api<CronJob> = Api::namespaced(client.clone(), namespace);
    let cj = cjs.get(name).await?;
    let job = job_from_cronjob(&cj, &random_suffix())?;
    let jobs: Api<Job> = Api::namespaced(client.clone(), namespace);
    let created = jobs.create(&PostParams::default(), &job).await?;
    Ok(created.metadata.name.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Delete.
// ---------------------------------------------------------------------------

/// Delete with grace period 0, for pods stuck terminating.
pub async fn force_delete_pod(client: &Client, namespace: &str, name: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    pods.delete(name, &DeleteParams::default().grace_period(0))
        .await?;
    Ok(())
}

/// Outcome of one object in a bulk delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct BulkItemResult {
    /// `namespace/name`, or `name` if cluster scoped.
    pub key: String,
    /// Plain-words reason; empty when the delete was accepted.
    pub error: String,
}

/// Delete several objects of one kind. Every object is attempted; failures
/// are reported per object rather than aborting the rest.
pub async fn delete_many(
    client: &Client,
    r: &ResourceRef,
    items: &[(String, String)],
) -> Vec<BulkItemResult> {
    futures::stream::iter(items.iter().cloned())
        .map(|(namespace, name)| async move {
            let key = if namespace.is_empty() {
                name.clone()
            } else {
                format!("{namespace}/{name}")
            };
            let error = resources::delete_resource(client, r, &namespace, &name)
                .await
                .err()
                .map(|e| describe_error(&e))
                .unwrap_or_default();
            BulkItemResult { key, error }
        })
        .buffered(8)
        .collect()
        .await
}

// ---------------------------------------------------------------------------
// Dispatcher shared by the CLI and the desktop app.
// ---------------------------------------------------------------------------

/// Run one non-drain action on one object. Returns a short summary for the
/// toast or CLI output.
pub async fn run_action(
    client: &Client,
    r: &ResourceRef,
    namespace: &str,
    name: &str,
    action: ActionId,
    replicas: Option<i32>,
) -> Result<String> {
    // Kind check, ignoring verbs: a server that lacks `delete` just answers 405.
    let verbs = ["delete".to_string()];
    if !actions_for(r, &verbs).iter().any(|a| a.id == action) {
        bail!("{action:?} does not apply to {}", r.kind);
    }
    let what = if namespace.is_empty() {
        format!("{}/{name}", r.kind)
    } else {
        format!("{}/{namespace}/{name}", r.kind)
    };
    Ok(match action {
        ActionId::Scale => {
            let n = replicas.context("scale needs a replica count")?;
            scale(client, r, namespace, name, n).await?;
            format!("Scaled {what} to {n}")
        }
        ActionId::Restart => {
            rollout_restart(client, r, namespace, name).await?;
            format!("Restarting {what}")
        }
        ActionId::Cordon => {
            set_unschedulable(client, name, true).await?;
            format!("Cordoned {what}")
        }
        ActionId::Uncordon => {
            set_unschedulable(client, name, false).await?;
            format!("Uncordoned {what}")
        }
        ActionId::Trigger => {
            let job = trigger_cronjob(client, namespace, name).await?;
            format!("Created job {namespace}/{job} from {what}")
        }
        ActionId::Suspend => {
            set_suspended(client, namespace, name, true).await?;
            format!("Suspended {what}")
        }
        ActionId::Resume => {
            set_suspended(client, namespace, name, false).await?;
            format!("Resumed {what}")
        }
        ActionId::Delete => {
            resources::delete_resource(client, r, namespace, name).await?;
            format!("Deleted {what}")
        }
        ActionId::ForceDelete => {
            force_delete_pod(client, namespace, name).await?;
            format!("Force deleted {what}")
        }
        ActionId::Drain => bail!("drain reports progress: use drain_node"),
    })
}

// ---------------------------------------------------------------------------
// Drain.
// ---------------------------------------------------------------------------

/// Why a pod is left alone by a drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    DaemonSet,
    Mirror,
    /// Already being deleted.
    Terminating,
}

/// Decide whether a drain evicts `pod`. Pure.
pub fn drain_skip_reason(pod: &Pod) -> Option<SkipReason> {
    let meta = &pod.metadata;
    if meta
        .annotations
        .as_ref()
        .is_some_and(|a| a.contains_key(MIRROR_POD))
    {
        return Some(SkipReason::Mirror);
    }
    if meta
        .owner_references
        .iter()
        .flatten()
        .any(|o| o.kind == "DaemonSet" && o.controller.unwrap_or(false))
    {
        return Some(SkipReason::DaemonSet);
    }
    if meta.deletion_timestamp.is_some() {
        return Some(SkipReason::Terminating);
    }
    None
}

fn has_controller(pod: &Pod) -> bool {
    pod.metadata
        .owner_references
        .iter()
        .flatten()
        .any(|o| o.controller.unwrap_or(false))
}

/// Progress of a drain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DrainEvent {
    Cordoned {
        node: String,
    },
    /// What the drain is going to do.
    Planned {
        to_evict: u32,
        /// DaemonSet, mirror and already terminating pods left alone.
        skipped: u32,
        /// Of those to evict, pods without a controller: nothing recreates them.
        unmanaged: u32,
    },
    Evicted {
        namespace: String,
        name: String,
    },
    /// A PodDisruptionBudget refused the eviction; retrying.
    WaitingOnPdb {
        namespace: String,
        name: String,
        message: String,
    },
    Failed {
        namespace: String,
        name: String,
        error: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct DrainSummary {
    pub evicted: u32,
    pub failed: u32,
    pub skipped: u32,
    /// Evicted pods that had not finished terminating when the timeout hit.
    pub still_terminating: u32,
}

impl DrainSummary {
    pub fn complete(&self) -> bool {
        self.failed == 0 && self.still_terminating == 0
    }
}

/// Delay before retry number `attempt` (0-based) of a blocked eviction:
/// 1s, 2s, 4s, 8s, then 10s.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(4)).min(10))
}

/// Knobs of a drain.
#[derive(Debug, Clone)]
pub struct DrainOptions {
    /// Stop waiting (on PodDisruptionBudgets, on pods to terminate) after this.
    pub timeout: Duration,
    /// Only evict pods matching this label selector (`kubectl drain
    /// --pod-selector`). `None` drains every evictable pod.
    pub pod_selector: Option<String>,
}

impl DrainOptions {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            pod_selector: None,
        }
    }
}

/// Cordon `node`, then evict its pods through the Eviction API.
///
/// Evictions run concurrently. A 429 means a PodDisruptionBudget refuses for
/// now: the pod is retried with backoff until `timeout` is over. Progress is
/// reported through `on_event`. Returns once every evicted pod is gone, the
/// timeout has passed, or all that could be evicted failed.
pub async fn drain_node<F>(
    client: &Client,
    node: &str,
    opts: &DrainOptions,
    on_event: F,
) -> Result<DrainSummary>
where
    F: Fn(DrainEvent) + Send + Sync,
{
    let deadline = tokio::time::Instant::now() + opts.timeout;
    set_unschedulable(client, node, true).await?;
    on_event(DrainEvent::Cordoned {
        node: node.to_string(),
    });

    let pods_api: Api<Pod> = Api::all(client.clone());
    let mut lp = ListParams::default().fields(&format!("spec.nodeName={node}"));
    if let Some(sel) = &opts.pod_selector {
        lp = lp.labels(sel);
    }
    let pods = pods_api.list(&lp).await?.items;
    let (skipped, to_evict): (Vec<_>, Vec<_>) = pods
        .into_iter()
        .partition(|p| drain_skip_reason(p).is_some());
    on_event(DrainEvent::Planned {
        to_evict: to_evict.len() as u32,
        skipped: skipped.len() as u32,
        unmanaged: to_evict.iter().filter(|p| !has_controller(p)).count() as u32,
    });

    let on_event = &on_event;
    let results = futures::future::join_all(to_evict.iter().map(|pod| async move {
        let ns = pod.metadata.namespace.clone().unwrap_or_default();
        let name = pod.metadata.name.clone().unwrap_or_default();
        let api: Api<Pod> = Api::namespaced(client.clone(), &ns);
        let mut attempt = 0u32;
        let outcome = loop {
            match api.evict(&name, &EvictParams::default()).await {
                Ok(_) => break (true, pod.metadata.uid.clone()),
                Err(kube::Error::Api(r)) if r.code == 404 => break (true, None),
                Err(kube::Error::Api(r)) if r.code == 429 || r.code == 500 => {
                    let wait = backoff(attempt);
                    if tokio::time::Instant::now() + wait > deadline {
                        on_event(DrainEvent::Failed {
                            namespace: ns.clone(),
                            name: name.clone(),
                            error: format!(
                                "Timed out waiting for a PodDisruptionBudget to allow the eviction. {}",
                                r.message
                            ),
                        });
                        break (false, None);
                    }
                    if attempt == 0 {
                        on_event(DrainEvent::WaitingOnPdb {
                            namespace: ns.clone(),
                            name: name.clone(),
                            message: r.message.clone(),
                        });
                    }
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                }
                Err(e) => {
                    on_event(DrainEvent::Failed {
                        namespace: ns.clone(),
                        name: name.clone(),
                        error: describe_error(&e.into()),
                    });
                    break (false, None);
                }
            }
        };
        if outcome.0 {
            on_event(DrainEvent::Evicted {
                namespace: ns.clone(),
                name: name.clone(),
            });
        }
        outcome
    }))
    .await;

    let failed = results.iter().filter(|(ok, _)| !ok).count() as u32;
    let evicted = results.len() as u32 - failed;
    let mut waiting: HashSet<String> = results.into_iter().filter_map(|(_, uid)| uid).collect();

    // Wait for the evicted pods to actually leave the node.
    while !waiting.is_empty() && tokio::time::Instant::now() < deadline {
        let left: HashSet<String> = pods_api
            .list(&lp)
            .await?
            .items
            .into_iter()
            .filter_map(|p| p.metadata.uid)
            .collect();
        waiting.retain(|u| left.contains(u));
        if !waiting.is_empty() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(DrainSummary {
        evicted,
        failed,
        skipped: skipped.len() as u32,
        still_terminating: waiting.len() as u32,
    })
}

// ---------------------------------------------------------------------------
// CLI helpers.
// ---------------------------------------------------------------------------

/// Split `kind/name` (`deploy/web`, `cronjob/nightly`).
pub fn parse_target(s: &str) -> Result<(&str, &str)> {
    match s.split_once('/') {
        Some((k, n)) if !k.is_empty() && !n.is_empty() => Ok((k, n)),
        _ => bail!("expected <kind>/<name>, got '{s}'"),
    }
}

/// Resolve a `kind/name` argument against what the cluster serves.
pub async fn resolve_target(client: &Client, target: &str) -> Result<(ResourceKind, String)> {
    let (kind, name) = parse_target(target)?;
    let kinds = resources::discover(client).await?;
    let found = resources::resolve_kind(&kinds, kind)
        .with_context(|| format!("unknown resource type: {kind}"))?;
    Ok((found.clone(), name.to_string()))
}

/// `kubeloupe <action> <kind>/<name>` from the CLI.
pub async fn cli_action(
    client: &Client,
    target: &str,
    namespace: Option<&str>,
    action: ActionId,
    replicas: Option<i32>,
) -> Result<()> {
    let (kind, name) = resolve_target(client, target).await?;
    let ns = cli_namespace(&kind.resource, namespace);
    let msg = run_action(client, &kind.resource, &ns, &name, action, replicas)
        .await
        .map_err(|e| anyhow::anyhow!("{target}: {}", describe_error(&e)))?;
    println!("{msg}");
    Ok(())
}

/// Namespaced kinds default to `default` like kubectl.
pub fn cli_namespace(r: &ResourceRef, namespace: Option<&str>) -> String {
    if !r.namespaced {
        return String::new();
    }
    namespace.unwrap_or("default").to_string()
}

/// `kubeloupe drain <node>` with progress lines.
pub async fn cli_drain(client: &Client, node: &str, opts: &DrainOptions) -> Result<()> {
    let summary = drain_node(client, node, opts, |ev| println!("{}", render_event(&ev)))
        .await
        .map_err(|e| anyhow::anyhow!(describe_error(&e)))?;
    println!(
        "node/{node}: {} evicted, {} failed, {} skipped (DaemonSet, mirror, terminating)",
        summary.evicted, summary.failed, summary.skipped
    );
    if !summary.complete() {
        bail!(
            "drain of {node} did not complete ({} failed, {} still terminating)",
            summary.failed,
            summary.still_terminating
        );
    }
    Ok(())
}

/// One progress line.
pub fn render_event(ev: &DrainEvent) -> String {
    match ev {
        DrainEvent::Cordoned { node } => format!("node/{node} cordoned"),
        DrainEvent::Planned {
            to_evict,
            skipped,
            unmanaged,
        } => format!(
            "{to_evict} pods to evict, {skipped} skipped{}",
            if *unmanaged > 0 {
                format!(" ({unmanaged} have no controller and will not be recreated)")
            } else {
                String::new()
            }
        ),
        DrainEvent::Evicted { namespace, name } => format!("evicted pod/{namespace}/{name}"),
        DrainEvent::WaitingOnPdb {
            namespace, name, ..
        } => format!("pod/{namespace}/{name}: waiting on a PodDisruptionBudget, retrying"),
        DrainEvent::Failed {
            namespace,
            name,
            error,
        } => format!("pod/{namespace}/{name}: FAILED {error}"),
    }
}

/// `kubeloupe delete <kind>/<name> [--force]`.
pub async fn cli_delete(
    client: &Client,
    target: &str,
    namespace: Option<&str>,
    force: bool,
) -> Result<()> {
    let action = if force {
        let (kind, _) = parse_target(target)?;
        if !matches!(kind.to_ascii_lowercase().as_str(), "pod" | "pods" | "po") {
            bail!("--force only applies to pods");
        }
        ActionId::ForceDelete
    } else {
        ActionId::Delete
    };
    cli_action(client, target, namespace, action, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn rref(group: &str, kind: &str) -> ResourceRef {
        ResourceRef {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: kind.to_lowercase() + "s",
            namespaced: kind != "Node",
        }
    }

    fn ids(group: &str, kind: &str, verbs: &[&str]) -> Vec<ActionId> {
        let verbs: Vec<String> = verbs.iter().map(|v| v.to_string()).collect();
        actions_for(&rref(group, kind), &verbs)
            .iter()
            .map(|a| a.id)
            .collect()
    }

    #[test]
    fn registry_per_kind() {
        use ActionId::*;
        let d = ["delete"];
        assert_eq!(ids("apps", "Deployment", &d), [Scale, Restart, Delete]);
        assert_eq!(ids("apps", "StatefulSet", &d), [Scale, Restart, Delete]);
        assert_eq!(ids("apps", "ReplicaSet", &d), [Scale, Delete]);
        assert_eq!(ids("apps", "DaemonSet", &d), [Restart, Delete]);
        assert_eq!(ids("", "Node", &d), [Cordon, Uncordon, Drain, Delete]);
        assert_eq!(
            ids("batch", "CronJob", &d),
            [Trigger, Suspend, Resume, Delete]
        );
        assert_eq!(ids("", "Pod", &d), [Delete, ForceDelete]);
        assert_eq!(ids("", "ConfigMap", &d), [Delete]);
        // Same kind name in another group gets nothing special.
        assert_eq!(ids("example.com", "Deployment", &d), [Delete]);
        // No delete verb, no delete.
        assert!(ids("", "ConfigMap", &["get", "list"]).is_empty());
        assert_eq!(ids("apps", "Deployment", &[]), [Scale, Restart]);
    }

    #[test]
    fn registry_flags() {
        let verbs = vec!["delete".to_string()];
        let find = |g: &str, k: &str, id| {
            actions_for(&rref(g, k), &verbs)
                .into_iter()
                .find(|a| a.id == id)
                .unwrap()
        };
        assert!(find("", "Pod", ActionId::Delete).destructive);
        assert!(!find("", "Pod", ActionId::Delete).type_to_confirm);
        assert!(find("", "Pod", ActionId::ForceDelete).type_to_confirm);
        assert!(find("", "Node", ActionId::Drain).destructive);
        assert!(!find("apps", "Deployment", ActionId::Restart).destructive);
        assert_eq!(
            find("apps", "Deployment", ActionId::Scale).input,
            ActionInput::Replicas
        );
        assert!(scale_is_destructive(0));
        assert!(!scale_is_destructive(1));
    }

    #[test]
    fn restart_patch_targets_the_pod_template() {
        let p = restart_patch("2026-01-02T03:04:05Z");
        assert_eq!(
            p["spec"]["template"]["metadata"]["annotations"]["kubectl.kubernetes.io/restartedAt"],
            "2026-01-02T03:04:05Z"
        );
    }

    fn pod(json: serde_json::Value) -> Pod {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn drain_skips_daemonset_mirror_and_terminating_pods() {
        let plain = pod(json!({"metadata": {"name": "a"}}));
        assert_eq!(drain_skip_reason(&plain), None);

        let rs_owned = pod(json!({"metadata": {"name": "a", "ownerReferences": [
            {"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "rs", "uid": "u", "controller": true}]}}));
        assert_eq!(drain_skip_reason(&rs_owned), None);

        let ds = pod(json!({"metadata": {"name": "a", "ownerReferences": [
            {"apiVersion": "apps/v1", "kind": "DaemonSet", "name": "ds", "uid": "u", "controller": true}]}}));
        assert_eq!(drain_skip_reason(&ds), Some(SkipReason::DaemonSet));

        let mirror = pod(json!({"metadata": {"name": "a",
            "annotations": {"kubernetes.io/config.mirror": "abc"}}}));
        assert_eq!(drain_skip_reason(&mirror), Some(SkipReason::Mirror));

        let mut dying = plain.clone();
        dying.metadata.deletion_timestamp = Some(Time(chrono::Utc::now()));
        assert_eq!(drain_skip_reason(&dying), Some(SkipReason::Terminating));
        assert!(!has_controller(&plain));
        assert!(has_controller(&rs_owned));
    }

    #[test]
    fn backoff_grows_then_caps() {
        let secs: Vec<u64> = (0..7).map(|i| backoff(i).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 10, 10, 10]);
    }

    fn cronjob() -> CronJob {
        serde_json::from_value(json!({
            "apiVersion": "batch/v1", "kind": "CronJob",
            "metadata": {"name": "nightly", "namespace": "ops", "uid": "uid-1"},
            "spec": {
                "schedule": "0 2 * * *",
                "jobTemplate": {
                    "metadata": {"labels": {"app": "x"}, "annotations": {"a": "b"}},
                    "spec": {"template": {"spec": {
                        "restartPolicy": "Never",
                        "containers": [{"name": "c", "image": "busybox"}]}}}
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn job_from_cronjob_has_owner_annotation_and_spec() {
        let job = job_from_cronjob(&cronjob(), "abc12").unwrap();
        assert_eq!(job.metadata.name.as_deref(), Some("nightly-manual-abc12"));
        assert_eq!(job.metadata.namespace.as_deref(), Some("ops"));
        let ann = job.metadata.annotations.as_ref().unwrap();
        assert_eq!(ann.get(INSTANTIATE).map(String::as_str), Some("manual"));
        assert_eq!(ann.get("a").map(String::as_str), Some("b"));
        assert_eq!(job.metadata.labels.as_ref().unwrap()["app"], "x");
        let owner = &job.metadata.owner_references.as_ref().unwrap()[0];
        assert_eq!(
            (owner.kind.as_str(), owner.name.as_str(), owner.uid.as_str()),
            ("CronJob", "nightly", "uid-1")
        );
        assert_eq!(owner.api_version, "batch/v1");
        assert_eq!(owner.controller, Some(true));
        let spec = job.spec.unwrap();
        assert_eq!(
            spec.template.spec.unwrap().containers[0].image.as_deref(),
            Some("busybox")
        );
    }

    #[test]
    fn manual_job_name_fits_63_chars() {
        assert_eq!(manual_job_name("backup", "x1"), "backup-manual-x1");
        let long = "a".repeat(52);
        let n = manual_job_name(&long, "abcde");
        assert!(n.len() <= 63, "{n}");
        assert!(n.ends_with("-manual-abcde"));
        let dashed = format!("{}-", "b".repeat(54));
        let n = manual_job_name(&dashed, "abcde");
        assert!(!n.contains("--"), "{n}");
        assert_eq!(random_suffix().len(), 5);
    }

    #[test]
    fn error_messages_are_plain_words() {
        assert!(describe_status(403, "forbidden: x").starts_with("Not allowed"));
        assert!(describe_status(404, "pods \"a\" not found").starts_with("Already gone"));
        assert!(describe_status(
            429,
            "Cannot evict pod as it would violate the pod's disruption budget."
        )
        .starts_with("Blocked by a PodDisruptionBudget"));
        assert!(describe_status(429, "too many").contains("slow down"));
        assert!(describe_status(409, "").starts_with("Conflict"));
        assert!(describe_status(503, "down").contains("API server failed"));
        assert_eq!(describe_status(400, "bad name"), "bad name");
    }

    #[test]
    fn describe_error_reads_kube_api_errors_through_context() {
        let api = kube::Error::Api(kube::core::ErrorResponse {
            status: "Failure".into(),
            message: "deployments.apps \"x\" is forbidden".into(),
            reason: "Forbidden".into(),
            code: 403,
        });
        let err = anyhow::Error::new(api).context("scaling");
        assert!(describe_error(&err).starts_with("Not allowed"));
        assert_eq!(describe_error(&anyhow::anyhow!("plain")), "plain");
    }

    #[test]
    fn parse_target_and_namespace() {
        assert_eq!(parse_target("deploy/web").unwrap(), ("deploy", "web"));
        assert!(parse_target("web").is_err());
        assert!(parse_target("deploy/").is_err());
        assert_eq!(cli_namespace(&rref("apps", "Deployment"), None), "default");
        assert_eq!(cli_namespace(&rref("apps", "Deployment"), Some("x")), "x");
        assert_eq!(cli_namespace(&rref("", "Node"), Some("x")), "");
    }

    #[test]
    fn events_render_for_the_cli() {
        let ev = DrainEvent::WaitingOnPdb {
            namespace: "n".into(),
            name: "p".into(),
            message: String::new(),
        };
        assert!(render_event(&ev).contains("PodDisruptionBudget"));
        let planned = DrainEvent::Planned {
            to_evict: 3,
            skipped: 2,
            unmanaged: 1,
        };
        assert!(render_event(&planned).contains("will not be recreated"));
    }
}
