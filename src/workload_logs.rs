//! Workload-level logs: follow the logs of every pod behind a Deployment,
//! StatefulSet, DaemonSet, Job (or ReplicaSet), adding and removing per-pod
//! streams as pods come and go.
//!
//! The pods are found through the workload's label selector and watched with
//! the [`crate::watch`] layer. The decisions (which stream to start or stop
//! when a pod changes) live in the pure [`Tracker`]; [`stream_workload_logs`]
//! only executes them.

use crate::watch::{watch_pods_selected, Update};
use crate::{batch_lines, get_pod_logs, stream_pod_logs_follow, LogOptions};
use anyhow::{bail, Context, Result};
use futures::StreamExt;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::api::ListParams;
use kube::runtime::reflector::{ObjectRef, Store};
use kube::{Api, Client};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::str::FromStr;
use std::time::Duration;
use tokio::task::{AbortHandle, JoinSet};

/// Most concurrent per-container log streams of one workload stream.
pub const MAX_WORKLOAD_STREAMS: usize = 20;

const BATCH_LINES: usize = 500;
const BATCH_INTERVAL: Duration = Duration::from_millis(50);

/// Workload kinds whose pods can be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    Job,
    ReplicaSet,
}

impl FromStr for WorkloadKind {
    type Err = anyhow::Error;

    /// Accepts kubectl-style names and aliases, any case.
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "deployment" | "deployments" | "deploy" => Self::Deployment,
            "statefulset" | "statefulsets" | "sts" => Self::StatefulSet,
            "daemonset" | "daemonsets" | "ds" => Self::DaemonSet,
            "job" | "jobs" => Self::Job,
            "replicaset" | "replicasets" | "rs" => Self::ReplicaSet,
            other => bail!("unsupported workload kind '{other}' (use deploy, sts, ds, job or rs)"),
        })
    }
}

impl WorkloadKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::StatefulSet => "StatefulSet",
            Self::DaemonSet => "DaemonSet",
            Self::Job => "Job",
            Self::ReplicaSet => "ReplicaSet",
        }
    }
}

/// What `kubeloupe logs <ref>` refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    Pod(String),
    Workload(WorkloadKind, String),
}

/// Parse `name`, `pod/name` or `<kind>/name` (`deploy/web`, `sts/db`, ...).
pub fn parse_log_target(s: &str) -> Result<LogTarget> {
    let Some((kind, name)) = s.split_once('/') else {
        return Ok(LogTarget::Pod(s.to_string()));
    };
    if name.is_empty() {
        bail!("missing name in '{s}'");
    }
    if matches!(kind.to_ascii_lowercase().as_str(), "pod" | "pods" | "po") {
        return Ok(LogTarget::Pod(name.to_string()));
    }
    Ok(LogTarget::Workload(kind.parse()?, name.to_string()))
}

/// Render a label selector in the string form the API accepts.
pub fn selector_string(sel: &LabelSelector) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(labels) = &sel.match_labels {
        // BTreeMap: already sorted, so the output is stable.
        parts.extend(labels.iter().map(|(k, v)| format!("{k}={v}")));
    }
    for e in sel.match_expressions.iter().flatten() {
        let values = e.values.clone().unwrap_or_default().join(",");
        parts.push(match e.operator.as_str() {
            "In" => format!("{} in ({values})", e.key),
            "NotIn" => format!("{} notin ({values})", e.key),
            "Exists" => e.key.clone(),
            "DoesNotExist" => format!("!{}", e.key),
            _ => continue,
        });
    }
    parts.join(",")
}

/// The label selector (as a string) matching a workload's pods.
pub async fn workload_selector(
    client: &Client,
    kind: WorkloadKind,
    namespace: &str,
    name: &str,
) -> Result<String> {
    let c = client.clone();
    let sel: Option<LabelSelector> = match kind {
        WorkloadKind::Deployment => Api::<Deployment>::namespaced(c, namespace)
            .get(name)
            .await
            .map(|w| w.spec.map(|s| s.selector)),
        WorkloadKind::StatefulSet => Api::<StatefulSet>::namespaced(c, namespace)
            .get(name)
            .await
            .map(|w| w.spec.map(|s| s.selector)),
        WorkloadKind::DaemonSet => Api::<DaemonSet>::namespaced(c, namespace)
            .get(name)
            .await
            .map(|w| w.spec.map(|s| s.selector)),
        WorkloadKind::ReplicaSet => Api::<ReplicaSet>::namespaced(c, namespace)
            .get(name)
            .await
            .map(|w| w.spec.map(|s| s.selector)),
        WorkloadKind::Job => Api::<Job>::namespaced(c, namespace)
            .get(name)
            .await
            .map(|w| w.spec.and_then(|s| s.selector)),
    }
    .with_context(|| format!("{} {namespace}/{name}", kind.as_str()))?;
    let selector = sel.as_ref().map(selector_string).unwrap_or_default();
    // An empty selector would match every pod in the namespace.
    if selector.is_empty() {
        bail!("{} {namespace}/{name} has no label selector", kind.as_str());
    }
    Ok(selector)
}

// ---------------------------------------------------------------------------
// Stream bookkeeping (pure)
// ---------------------------------------------------------------------------

/// One log stream: a container of a pod.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub pod: String,
    pub container: String,
}

/// What the tracker needs to know about one container.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerView {
    pub name: String,
    pub restarts: i32,
    /// Running or terminated, so the API can serve its log.
    pub has_logs: bool,
}

/// What the tracker needs to know about a pod.
#[derive(Debug, Clone, PartialEq)]
pub struct PodView {
    pub name: String,
    pub uid: String,
    pub containers: Vec<ContainerView>,
}

impl PodView {
    /// Summarize `pod`. Without `wanted`, the regular containers are
    /// followed; with it, only that container (init containers included).
    pub fn from_pod(pod: &Pod, wanted: Option<&str>) -> Option<Self> {
        let name = pod.metadata.name.clone()?;
        let spec = pod.spec.as_ref()?;
        let status = pod.status.as_ref();
        let statuses: Vec<_> = status
            .into_iter()
            .flat_map(|s| {
                s.container_statuses
                    .iter()
                    .flatten()
                    .chain(s.init_container_statuses.iter().flatten())
            })
            .collect();
        let init = spec
            .init_containers
            .iter()
            .flatten()
            .filter(|c| wanted == Some(c.name.as_str()));
        let containers = spec
            .containers
            .iter()
            .chain(init)
            .filter(|c| wanted.is_none_or(|w| w == c.name))
            .map(|c| {
                let st = statuses.iter().find(|s| s.name == c.name);
                ContainerView {
                    name: c.name.clone(),
                    restarts: st.map_or(0, |s| s.restart_count),
                    has_logs: st
                        .and_then(|s| s.state.as_ref())
                        .is_some_and(|s| s.running.is_some() || s.terminated.is_some()),
                }
            })
            .collect();
        Some(Self {
            name,
            uid: pod.metadata.uid.clone().unwrap_or_default(),
            containers,
        })
    }
}

/// A step for the runtime to carry out.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Start following a container. `from_start` reads its whole log (pods
    /// that appeared after the stream began, so nothing is missed) instead
    /// of the user's tail/since.
    Start {
        key: StreamKey,
        uid: String,
        from_start: bool,
    },
    /// The pod is gone: stop its streams.
    Stop { key: StreamKey },
    /// The number of streams waiting for a free slot changed.
    Truncated { limit: usize, skipped: usize },
}

#[derive(Debug)]
struct Slot {
    uid: String,
    started_restarts: i32,
    latest_restarts: i32,
    running: bool,
}

#[derive(Debug)]
struct Queued {
    key: StreamKey,
    uid: String,
    restarts: i32,
    from_start: bool,
}

/// Decides which per-container streams should run, bounded by `limit`.
#[derive(Debug)]
pub struct Tracker {
    limit: usize,
    active: HashMap<StreamKey, Slot>,
    queued: Vec<Queued>,
    seen_snapshot: bool,
    reported_skipped: usize,
}

impl Tracker {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            active: HashMap::new(),
            queued: Vec::new(),
            seen_snapshot: false,
            reported_skipped: 0,
        }
    }

    fn running(&self) -> usize {
        self.active.values().filter(|s| s.running).count()
    }

    fn start_or_queue(&mut self, q: Queued, out: &mut Vec<Action>) {
        if self.running() < self.limit {
            self.active.insert(
                q.key.clone(),
                Slot {
                    uid: q.uid.clone(),
                    started_restarts: q.restarts,
                    latest_restarts: q.restarts,
                    running: true,
                },
            );
            out.push(Action::Start {
                key: q.key,
                uid: q.uid,
                from_start: q.from_start,
            });
        } else {
            self.queued.push(q);
        }
    }

    fn remove_pod(&mut self, pod: &str, out: &mut Vec<Action>) {
        let keys: Vec<StreamKey> = self
            .active
            .keys()
            .filter(|k| k.pod == pod)
            .cloned()
            .collect();
        for key in keys {
            self.active.remove(&key);
            out.push(Action::Stop { key });
        }
        self.queued.retain(|q| q.key.pod != pod);
    }

    fn known_uid(&self, pod: &str) -> Option<&str> {
        self.active
            .iter()
            .find(|(k, _)| k.pod == pod)
            .map(|(_, s)| s.uid.as_str())
            .or_else(|| {
                self.queued
                    .iter()
                    .find(|q| q.key.pod == pod)
                    .map(|q| q.uid.as_str())
            })
    }

    fn apply_pod(&mut self, pod: &PodView, from_start: bool, out: &mut Vec<Action>) {
        // Same name, new identity (a StatefulSet pod that was recreated).
        if self.known_uid(&pod.name).is_some_and(|u| u != pod.uid) {
            self.remove_pod(&pod.name, out);
        }
        for c in pod.containers.iter().filter(|c| c.has_logs) {
            let key = StreamKey {
                pod: pod.name.clone(),
                container: c.name.clone(),
            };
            let mut from_start = from_start;
            if let Some(slot) = self.active.get_mut(&key) {
                slot.latest_restarts = c.restarts;
                // The container restarted after its stream ended: follow
                // the new instance from its beginning.
                if slot.running || c.restarts <= slot.started_restarts {
                    continue;
                }
                self.active.remove(&key);
                from_start = true;
            } else if let Some(q) = self.queued.iter_mut().find(|q| q.key == key) {
                q.restarts = c.restarts;
                continue;
            }
            self.start_or_queue(
                Queued {
                    key,
                    uid: pod.uid.clone(),
                    restarts: c.restarts,
                    from_start,
                },
                out,
            );
        }
    }

    fn finish(&mut self, mut out: Vec<Action>) -> Vec<Action> {
        while self.running() < self.limit && !self.queued.is_empty() {
            let q = self.queued.remove(0);
            self.start_or_queue(q, &mut out);
        }
        if self.queued.len() != self.reported_skipped {
            self.reported_skipped = self.queued.len();
            out.push(Action::Truncated {
                limit: self.limit,
                skipped: self.queued.len(),
            });
        }
        out
    }

    /// The complete current pod set (first list, and every re-list). Pods
    /// of the first snapshot are followed with the user's tail/since; pods
    /// first seen later from their beginning.
    pub fn snapshot(&mut self, mut pods: Vec<PodView>) -> Vec<Action> {
        let initial = !self.seen_snapshot;
        self.seen_snapshot = true;
        pods.sort_by(|a, b| a.name.cmp(&b.name));
        let names: HashSet<&str> = pods.iter().map(|p| p.name.as_str()).collect();
        let stale: HashSet<String> = self
            .active
            .keys()
            .map(|k| &k.pod)
            .chain(self.queued.iter().map(|q| &q.key.pod))
            .filter(|p| !names.contains(p.as_str()))
            .cloned()
            .collect();
        let mut out = Vec::new();
        let mut stale: Vec<_> = stale.into_iter().collect();
        stale.sort();
        for pod in stale {
            self.remove_pod(&pod, &mut out);
        }
        for pod in &pods {
            self.apply_pod(pod, !initial, &mut out);
        }
        self.finish(out)
    }

    /// A pod was created or changed.
    pub fn applied(&mut self, pod: &PodView) -> Vec<Action> {
        let mut out = Vec::new();
        self.apply_pod(pod, true, &mut out);
        self.finish(out)
    }

    /// A pod was deleted.
    pub fn deleted(&mut self, pod: &str) -> Vec<Action> {
        let mut out = Vec::new();
        self.remove_pod(pod, &mut out);
        self.finish(out)
    }

    /// A stream ended by itself (the container exited, or it failed).
    /// Frees its slot; `uid` guards against a stale report.
    pub fn ended(&mut self, key: &StreamKey, uid: &str) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(slot) = self.active.get_mut(key).filter(|s| s.uid == uid) {
            slot.running = false;
            if slot.latest_restarts > slot.started_restarts {
                let restarts = slot.latest_restarts;
                self.active.remove(key);
                self.start_or_queue(
                    Queued {
                        key: key.clone(),
                        uid: uid.to_string(),
                        restarts,
                        from_start: true,
                    },
                    &mut out,
                );
            }
        }
        self.finish(out)
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// A log line and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct TaggedLine {
    pub pod: String,
    pub container: String,
    pub line: String,
}

/// Why a per-container stream stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// The pod was deleted (or left the selector).
    Deleted,
    /// The container exited and its log ended.
    Ended,
    Failed(String),
}

/// What [`stream_workload_logs`] reports.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkloadEvent {
    /// A batch of merged lines (at most 500, or 50 ms of lines).
    Lines(Vec<TaggedLine>),
    Started {
        pod: String,
        container: String,
    },
    Stopped {
        pod: String,
        container: String,
        reason: StopReason,
    },
    /// More containers matched than `limit`; `skipped` of them wait for a
    /// free slot (0 once all are being followed).
    Truncated {
        limit: usize,
        skipped: usize,
    },
}

struct End {
    key: StreamKey,
    uid: String,
    error: Option<String>,
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn pod_view(
    store: &Store<Pod>,
    namespace: &str,
    name: &str,
    wanted: Option<&str>,
) -> Option<PodView> {
    let pod = store.get(&ObjectRef::new(name).within(namespace))?;
    PodView::from_pod(&pod, wanted)
}

/// Follow the logs of every pod behind a workload until the future is
/// dropped. Streams are added when pods appear (and their containers run)
/// and aborted when pods are deleted, so a rollout restart is followed from
/// the old pods to the new ones. Dropping the future stops everything.
///
/// `opts.tail_lines` / `since_seconds` apply to the pods present at the
/// start; pods that appear later are read from their beginning. At most
/// [`MAX_WORKLOAD_STREAMS`] containers are followed at once; the rest are
/// reported through [`WorkloadEvent::Truncated`] and started as slots free up.
pub async fn stream_workload_logs<F>(
    client: &Client,
    kind: WorkloadKind,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
    on_event: F,
) -> Result<()>
where
    F: FnMut(WorkloadEvent) + Send,
{
    stream_workload_logs_limited(
        client,
        kind,
        namespace,
        name,
        opts,
        MAX_WORKLOAD_STREAMS,
        on_event,
    )
    .await
}

/// [`stream_workload_logs`] with an explicit stream limit.
pub async fn stream_workload_logs_limited<F>(
    client: &Client,
    kind: WorkloadKind,
    namespace: &str,
    name: &str,
    opts: &LogOptions,
    limit: usize,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(WorkloadEvent) + Send,
{
    anyhow::ensure!(
        !opts.previous,
        "previous logs are not available for workload streams"
    );
    let selector = workload_selector(client, kind, namespace, name).await?;
    let (store, mut updates) = watch_pods_selected(client, namespace, &selector);

    let (line_tx, line_rx) = tokio::sync::mpsc::unbounded_channel::<TaggedLine>();
    let (batch_tx, mut batch_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<TaggedLine>>();
    let _batcher = AbortOnDrop(tokio::spawn(batch_lines(
        line_rx,
        BATCH_LINES,
        BATCH_INTERVAL,
        move |b| {
            let _ = batch_tx.send(b);
        },
    )));

    // Dropping the set aborts every per-pod task.
    let mut streams: JoinSet<End> = JoinSet::new();
    let mut handles: HashMap<StreamKey, (String, AbortHandle)> = HashMap::new();
    let mut tracker = Tracker::new(limit);
    let wanted = opts.container.as_deref();

    loop {
        let actions = tokio::select! {
            update = updates.next() => match update {
                None => break,
                Some(Update::Snapshot(_)) => {
                    let pods = store
                        .state()
                        .iter()
                        .filter_map(|p| PodView::from_pod(p, wanted))
                        .collect();
                    tracker.snapshot(pods)
                }
                Some(Update::Applied(info)) => {
                    match pod_view(&store, namespace, &info.name, wanted) {
                        Some(view) => tracker.applied(&view),
                        None => Vec::new(),
                    }
                }
                Some(Update::Deleted(info)) => tracker.deleted(&info.name),
                Some(Update::State(_)) => Vec::new(),
            },
            Some(lines) = batch_rx.recv() => {
                on_event(WorkloadEvent::Lines(lines));
                Vec::new()
            }
            Some(done) = streams.join_next(), if !streams.is_empty() => {
                // Aborted tasks report a cancellation; nothing to do for them.
                match done {
                    Ok(end) if handles.get(&end.key).is_some_and(|(u, _)| *u == end.uid) => {
                        handles.remove(&end.key);
                        on_event(WorkloadEvent::Stopped {
                            pod: end.key.pod.clone(),
                            container: end.key.container.clone(),
                            reason: match &end.error {
                                Some(e) => StopReason::Failed(e.clone()),
                                None => StopReason::Ended,
                            },
                        });
                        tracker.ended(&end.key, &end.uid)
                    }
                    _ => Vec::new(),
                }
            }
        };
        for action in actions {
            match action {
                Action::Start {
                    key,
                    uid,
                    from_start,
                } => {
                    let mut o = opts.clone();
                    o.container = Some(key.container.clone());
                    if from_start {
                        o.tail_lines = None;
                        o.since_seconds = None;
                    }
                    let (client, ns, tx) = (client.clone(), namespace.to_string(), line_tx.clone());
                    let task_key = key.clone();
                    let task_uid = uid.clone();
                    let handle = streams.spawn(async move {
                        let (pod, container) = (task_key.pod.clone(), task_key.container.clone());
                        let result = stream_pod_logs_follow(&client, &ns, &pod, &o, true, |line| {
                            let _ = tx.send(TaggedLine {
                                pod: pod.clone(),
                                container: container.clone(),
                                line,
                            });
                        })
                        .await;
                        End {
                            key: task_key,
                            uid: task_uid,
                            error: result.err().map(|e| e.to_string()),
                        }
                    });
                    on_event(WorkloadEvent::Started {
                        pod: key.pod.clone(),
                        container: key.container.clone(),
                    });
                    handles.insert(key, (uid, handle));
                }
                Action::Stop { key } => {
                    if let Some((_, handle)) = handles.remove(&key) {
                        handle.abort();
                        on_event(WorkloadEvent::Stopped {
                            pod: key.pod,
                            container: key.container,
                            reason: StopReason::Deleted,
                        });
                    }
                }
                Action::Truncated { limit, skipped } => {
                    on_event(WorkloadEvent::Truncated { limit, skipped });
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI output
// ---------------------------------------------------------------------------

/// 256-colour codes that read well on dark and light terminals.
const PALETTE: [u8; 12] = [39, 208, 41, 170, 220, 75, 203, 114, 141, 215, 81, 177];

/// Index into a palette for `pod`; stable across runs (FNV-1a).
pub fn pod_color_index(pod: &str, palette_len: usize) -> usize {
    let mut h: u32 = 0x811c9dc5;
    for b in pod.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h as usize % palette_len
}

/// `[pod] ` or `[pod/container] `, coloured per pod when `color` is set.
pub fn format_prefix(pod: &str, container: &str, with_container: bool, color: bool) -> String {
    let label = if with_container {
        format!("[{pod}/{container}]")
    } else {
        format!("[{pod}]")
    };
    if color {
        format!(
            "\x1b[38;5;{}m{label}\x1b[0m ",
            PALETTE[pod_color_index(pod, PALETTE.len())]
        )
    } else {
        format!("{label} ")
    }
}

/// Print a workload's logs (CLI output): the last `opts.tail_lines` of each
/// pod, one pod after another; with `follow`, keep streaming (and following
/// pods through restarts) until interrupted.
pub async fn print_workload_logs(
    client: &Client,
    namespace: Option<&str>,
    kind: WorkloadKind,
    name: &str,
    opts: &LogOptions,
    follow: bool,
) -> Result<()> {
    let namespace = namespace.unwrap_or("default");
    let color = std::io::stdout().is_terminal();
    if follow {
        anyhow::ensure!(
            !opts.previous,
            "--follow cannot be combined with --previous"
        );
        // Show the container name once a pod has more than one stream.
        let mut per_pod: HashMap<String, usize> = HashMap::new();
        return stream_workload_logs(client, kind, namespace, name, opts, |ev| match ev {
            WorkloadEvent::Lines(lines) => {
                for l in lines {
                    let multi = per_pod.get(&l.pod).is_some_and(|n| *n > 1);
                    println!(
                        "{}{}",
                        format_prefix(&l.pod, &l.container, multi, color),
                        l.line
                    );
                }
            }
            WorkloadEvent::Started { pod, .. } => *per_pod.entry(pod).or_default() += 1,
            WorkloadEvent::Stopped {
                pod,
                container,
                reason,
            } => {
                let why = match reason {
                    StopReason::Deleted => "pod deleted".to_string(),
                    StopReason::Ended => "log ended".to_string(),
                    StopReason::Failed(e) => format!("error: {e}"),
                };
                eprintln!("kubeloupe: [{pod}/{container}] {why}");
            }
            WorkloadEvent::Truncated { limit, skipped } => {
                if skipped > 0 {
                    eprintln!("kubeloupe: following {limit} containers, {skipped} more waiting");
                }
            }
        })
        .await;
    }

    let selector = workload_selector(client, kind, namespace, name).await?;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let mut list = pods
        .list(&ListParams::default().labels(&selector))
        .await?
        .items;
    list.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    let views: Vec<PodView> = list
        .iter()
        .filter_map(|p| PodView::from_pod(p, opts.container.as_deref()))
        .collect();
    if views.is_empty() {
        eprintln!("kubeloupe: no pods match {}/{name}", kind.as_str());
    }
    let mut remaining = MAX_WORKLOAD_STREAMS;
    for pod in &views {
        let with_container = pod.containers.len() > 1;
        for c in pod.containers.iter().filter(|c| c.has_logs) {
            if remaining == 0 {
                eprintln!("kubeloupe: showing the first {MAX_WORKLOAD_STREAMS} containers only");
                return Ok(());
            }
            remaining -= 1;
            let mut o = opts.clone();
            o.container = Some(c.name.clone());
            match get_pod_logs(client, namespace, &pod.name, &o).await {
                Ok(text) => {
                    let prefix = format_prefix(&pod.name, &c.name, with_container, color);
                    for line in text.lines() {
                        println!("{prefix}{line}");
                    }
                }
                Err(e) => eprintln!("kubeloupe: [{}/{}] {e}", pod.name, c.name),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement;

    fn view(name: &str, uid: &str, containers: &[(&str, i32, bool)]) -> PodView {
        PodView {
            name: name.into(),
            uid: uid.into(),
            containers: containers
                .iter()
                .map(|(n, r, l)| ContainerView {
                    name: (*n).into(),
                    restarts: *r,
                    has_logs: *l,
                })
                .collect(),
        }
    }

    fn key(pod: &str, c: &str) -> StreamKey {
        StreamKey {
            pod: pod.into(),
            container: c.into(),
        }
    }

    fn start(pod: &str, c: &str, uid: &str, from_start: bool) -> Action {
        Action::Start {
            key: key(pod, c),
            uid: uid.into(),
            from_start,
        }
    }

    #[test]
    fn selector_strings() {
        let sel = LabelSelector {
            match_labels: Some([("b".into(), "2".into()), ("a".into(), "1".into())].into()),
            match_expressions: Some(vec![
                LabelSelectorRequirement {
                    key: "tier".into(),
                    operator: "In".into(),
                    values: Some(vec!["x".into(), "y".into()]),
                },
                LabelSelectorRequirement {
                    key: "legacy".into(),
                    operator: "DoesNotExist".into(),
                    values: None,
                },
                LabelSelectorRequirement {
                    key: "env".into(),
                    operator: "Exists".into(),
                    values: None,
                },
                LabelSelectorRequirement {
                    key: "z".into(),
                    operator: "NotIn".into(),
                    values: Some(vec!["q".into()]),
                },
            ]),
        };
        assert_eq!(
            selector_string(&sel),
            "a=1,b=2,tier in (x,y),!legacy,env,z notin (q)"
        );
        assert_eq!(selector_string(&LabelSelector::default()), "");
    }

    #[test]
    fn parses_targets() {
        assert_eq!(
            parse_log_target("web-1").unwrap(),
            LogTarget::Pod("web-1".into())
        );
        assert_eq!(
            parse_log_target("pod/web-1").unwrap(),
            LogTarget::Pod("web-1".into())
        );
        assert_eq!(
            parse_log_target("deploy/web").unwrap(),
            LogTarget::Workload(WorkloadKind::Deployment, "web".into())
        );
        assert_eq!(
            parse_log_target("STS/db").unwrap(),
            LogTarget::Workload(WorkloadKind::StatefulSet, "db".into())
        );
        assert_eq!(
            parse_log_target("ds/agent").unwrap(),
            LogTarget::Workload(WorkloadKind::DaemonSet, "agent".into())
        );
        assert_eq!(
            parse_log_target("job/migrate").unwrap(),
            LogTarget::Workload(WorkloadKind::Job, "migrate".into())
        );
        assert!(parse_log_target("cronjob/x").is_err());
        assert!(parse_log_target("deploy/").is_err());
    }

    #[test]
    fn colors_are_stable_and_in_range() {
        let a = pod_color_index("web-7d9c-abcde", PALETTE.len());
        assert_eq!(a, pod_color_index("web-7d9c-abcde", PALETTE.len()));
        assert!(a < PALETTE.len());
        assert_eq!(format_prefix("p", "c", false, false), "[p] ");
        assert_eq!(format_prefix("p", "c", true, false), "[p/c] ");
        assert!(format_prefix("p", "c", false, true).starts_with("\x1b[38;5;"));
    }

    #[test]
    fn first_snapshot_uses_user_tail_later_pods_start_from_the_beginning() {
        let mut t = Tracker::new(20);
        assert_eq!(
            t.snapshot(vec![
                view("b", "u2", &[("app", 0, true)]),
                view("a", "u1", &[("app", 0, true)])
            ]),
            vec![
                start("a", "app", "u1", false),
                start("b", "app", "u2", false)
            ]
        );
        // A new pod: nothing until its container has logs, then from start.
        assert!(t.applied(&view("c", "u3", &[("app", 0, false)])).is_empty());
        assert_eq!(
            t.applied(&view("c", "u3", &[("app", 0, true)])),
            vec![start("c", "app", "u3", true)]
        );
        // Further updates of a followed pod change nothing.
        assert!(t.applied(&view("c", "u3", &[("app", 0, true)])).is_empty());
    }

    #[test]
    fn deleting_a_pod_stops_its_streams_and_rollout_follows_new_pods() {
        let mut t = Tracker::new(20);
        t.snapshot(vec![view(
            "old",
            "u1",
            &[("app", 0, true), ("side", 0, true)],
        )]);
        let mut stops = t.deleted("old");
        stops.sort_by_key(|a| format!("{a:?}"));
        assert_eq!(
            stops,
            vec![
                Action::Stop {
                    key: key("old", "app")
                },
                Action::Stop {
                    key: key("old", "side")
                }
            ]
        );
        assert_eq!(
            t.applied(&view("new", "u9", &[("app", 0, true)])),
            vec![start("new", "app", "u9", true)]
        );
        assert!(t.deleted("unknown").is_empty());
    }

    #[test]
    fn recreated_pod_with_same_name_restarts_its_stream() {
        let mut t = Tracker::new(20);
        t.snapshot(vec![view("db-0", "u1", &[("db", 0, true)])]);
        assert_eq!(
            t.applied(&view("db-0", "u2", &[("db", 0, true)])),
            vec![
                Action::Stop {
                    key: key("db-0", "db")
                },
                start("db-0", "db", "u2", true)
            ]
        );
    }

    #[test]
    fn relist_drops_pods_that_vanished() {
        let mut t = Tracker::new(20);
        t.snapshot(vec![
            view("a", "u1", &[("app", 0, true)]),
            view("b", "u2", &[("app", 0, true)]),
        ]);
        assert_eq!(
            t.snapshot(vec![
                view("b", "u2", &[("app", 0, true)]),
                view("n", "u3", &[("app", 0, true)])
            ]),
            vec![
                Action::Stop {
                    key: key("a", "app")
                },
                start("n", "app", "u3", true)
            ]
        );
    }

    #[test]
    fn limit_queues_extra_streams_and_promotes_them() {
        let mut t = Tracker::new(2);
        let pods: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|n| view(n, n, &[("app", 0, true)]))
            .collect();
        assert_eq!(
            t.snapshot(pods),
            vec![
                start("a", "app", "a", false),
                start("b", "app", "b", false),
                Action::Truncated {
                    limit: 2,
                    skipped: 2
                }
            ]
        );
        // Deleting a pod frees a slot for the next queued one.
        assert_eq!(
            t.deleted("a"),
            vec![
                Action::Stop {
                    key: key("a", "app")
                },
                start("c", "app", "c", false),
                Action::Truncated {
                    limit: 2,
                    skipped: 1
                }
            ]
        );
        // A stream that ends by itself frees its slot too.
        assert_eq!(
            t.ended(&key("b", "app"), "b"),
            vec![
                start("d", "app", "d", false),
                Action::Truncated {
                    limit: 2,
                    skipped: 0
                }
            ]
        );
    }

    #[test]
    fn restarted_container_is_followed_again_once_its_stream_ended() {
        let mut t = Tracker::new(20);
        t.snapshot(vec![view("p", "u", &[("app", 0, true)])]);
        // Restart observed while the old stream is still open: wait for it.
        assert!(t.applied(&view("p", "u", &[("app", 1, true)])).is_empty());
        assert_eq!(
            t.ended(&key("p", "app"), "u"),
            vec![start("p", "app", "u", true)]
        );
        // Ended with no restart: stays ended, and a stale report is ignored.
        let mut t = Tracker::new(20);
        t.snapshot(vec![view("p", "u", &[("app", 0, true)])]);
        assert!(t.ended(&key("p", "app"), "other").is_empty());
        assert!(t.ended(&key("p", "app"), "u").is_empty());
        assert!(t.applied(&view("p", "u", &[("app", 0, true)])).is_empty());
        assert_eq!(
            t.applied(&view("p", "u", &[("app", 1, true)])),
            vec![start("p", "app", "u", true)]
        );
    }

    #[test]
    fn pod_view_picks_containers() {
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "p", "uid": "u"},
            "spec": {
                "initContainers": [{"name": "init"}],
                "containers": [{"name": "app"}, {"name": "side"}]
            },
            "status": {
                "initContainerStatuses": [{"name": "init", "ready": true, "restartCount": 0, "image": "i", "imageID": "", "state": {"terminated": {"exitCode": 0}}}],
                "containerStatuses": [
                    {"name": "app", "ready": true, "restartCount": 2, "image": "i", "imageID": "", "state": {"running": {}}},
                    {"name": "side", "ready": false, "restartCount": 0, "image": "i", "imageID": "", "state": {"waiting": {}}}
                ]
            }
        }))
        .unwrap();
        let all = PodView::from_pod(&pod, None).unwrap();
        assert_eq!(
            all.containers,
            vec![
                ContainerView {
                    name: "app".into(),
                    restarts: 2,
                    has_logs: true
                },
                ContainerView {
                    name: "side".into(),
                    restarts: 0,
                    has_logs: false
                },
            ]
        );
        let init = PodView::from_pod(&pod, Some("init")).unwrap();
        assert_eq!(init.containers.len(), 1);
        assert!(init.containers[0].has_logs);
        assert!(PodView::from_pod(&pod, Some("nope"))
            .unwrap()
            .containers
            .is_empty());
    }
}
