//! The live overview: watches of pods, nodes, namespaces and warning events,
//! folded into small per-object summaries and turned into an [`Overview`]
//! at most once per [`THROTTLE`], only when something changed.
//!
//! RBAC is handled per part, as in the one-shot overview it replaces: a
//! namespaced part (pods, events) refused across the cluster (403) is
//! watched in the fallback namespace instead; a part refused there too, or
//! a refused cluster-scoped part (nodes, namespaces), is left out with a note.

use super::summary::{self, NodeSummary, PodSummary};
use super::{EventInfo, Overview, RecentEvents};
use crate::watch::{watch_summaries, Update, WatchState};
use crate::{get_node_usage_total, is_forbidden};
use futures::stream::{self, BoxStream, SelectAll};
use futures::StreamExt;
use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod};
use kube::runtime::watcher;
use kube::{Api, Client};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// At most one summary per this interval.
pub const THROTTLE: Duration = Duration::from_secs(1);
/// Recompute this often (in throttle ticks) even without changes: whether a
/// pending pod is overdue depends on the clock.
const RECHECK_TICKS: u32 = 30;
/// How often node usage is polled (the metrics API cannot be watched).
const USAGE_INTERVAL: Duration = Duration::from_secs(30);
/// Warning events kept in memory; older ones are dropped.
const MAX_EVENTS_KEPT: usize = 500;
/// Warning events in a summary.
const EVENTS_SHOWN: usize = 20;

/// What [`watch_overview`] yields.
#[derive(Debug, Clone, PartialEq)]
pub enum OverviewUpdate {
    /// A new summary, sent when it differs from the previous one.
    Summary(Box<Overview>),
    /// `Connecting` until every part has its first list, `Live` after,
    /// `Reconnecting` while a watch is failing. Never `Forbidden`: a refused
    /// part is narrowed or left out, with a note in the summary.
    State(WatchState),
}

/// Watch the overview of the cluster. Parts the user may not see
/// cluster-wide fall back to `namespace` (`None` = the context's
/// namespace). The stream never ends on its own; drop it to stop.
pub fn watch_overview(
    client: &Client,
    namespace: Option<&str>,
) -> BoxStream<'static, OverviewUpdate> {
    let fallback = namespace.unwrap_or(client.default_namespace()).to_string();
    let live = Live::new(client.clone(), fallback);
    stream::unfold(live, |mut live| async move {
        let next = live.next().await;
        Some((next, live))
    })
    .boxed()
}

type Keyed<T> = (String, T);

enum Msg {
    Pods(Update<Keyed<PodSummary>>),
    Nodes(Update<Keyed<NodeSummary>>),
    Namespaces(Update<Keyed<()>>),
    Events(Update<Keyed<EventInfo>>),
    Usage(Option<(u64, u64)>),
}

/// Watch config for the overview: the initial list may come from the API
/// server's watch cache (`resourceVersion=0`) instead of etcd.
fn config() -> watcher::Config {
    watcher::Config::default().any_semantic()
}

fn api<K>(client: &Client, namespace: Option<&str>) -> Api<K>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope, DynamicType = ()>,
{
    match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    }
}

fn pods_stream(client: &Client, namespace: Option<&str>) -> BoxStream<'static, Msg> {
    watch_summaries(api::<Pod>(client, namespace), config(), |p: &Pod| {
        let s = summary::pod_summary(p);
        (format!("{}/{}", s.namespace, s.name), s)
    })
    .map(Msg::Pods)
    .boxed()
}

fn events_stream(client: &Client, namespace: Option<&str>) -> BoxStream<'static, Msg> {
    let config = config().fields("type=Warning");
    watch_summaries(api::<Event>(client, namespace), config, |e: &Event| {
        let info = summary::event_info(e);
        (info.key.clone(), info)
    })
    .map(Msg::Events)
    .boxed()
}

fn nodes_stream(client: &Client) -> BoxStream<'static, Msg> {
    watch_summaries(Api::<Node>::all(client.clone()), config(), |n: &Node| {
        (
            n.metadata.name.clone().unwrap_or_default(),
            summary::node_summary(n),
        )
    })
    .map(Msg::Nodes)
    .boxed()
}

fn namespaces_stream(client: &Client) -> BoxStream<'static, Msg> {
    watch_summaries(
        Api::<Namespace>::all(client.clone()),
        config(),
        |n: &Namespace| (n.metadata.name.clone().unwrap_or_default(), ()),
    )
    .map(Msg::Namespaces)
    .boxed()
}

/// Node usage now and then every [`USAGE_INTERVAL`]; ends on a 403.
fn usage_stream(client: &Client) -> BoxStream<'static, Msg> {
    stream::unfold((client.clone(), true), |(client, first)| async move {
        if !first {
            tokio::time::sleep(USAGE_INTERVAL).await;
        }
        match get_node_usage_total(&client).await {
            Ok(usage) => Some((Msg::Usage(usage), (client, false))),
            Err(e) if is_forbidden(&e) => None,
            // Best effort: no usage until the next poll works.
            Err(_) => Some((Msg::Usage(None), (client, false))),
        }
    })
    .boxed()
}

/// One watched part of the overview.
struct Part<T> {
    what: &'static str,
    namespaced: bool,
    items: HashMap<String, T>,
    /// The namespace it was narrowed to after a 403 across the cluster.
    scope: Option<String>,
    /// The first full list arrived.
    synced: bool,
    /// Refused everywhere it was tried: left out.
    denied: bool,
    /// The watch is failing and retrying.
    error: Option<String>,
}

impl<T> Part<T> {
    fn new(what: &'static str, namespaced: bool) -> Self {
        Self {
            what,
            namespaced,
            items: HashMap::new(),
            scope: None,
            synced: false,
            denied: false,
            error: None,
        }
    }

    /// Fold in an update. Returns whether the items changed, or `Err` if
    /// the watch was refused (and has ended).
    fn apply(&mut self, update: Update<Keyed<T>>) -> Result<bool, ()> {
        match update {
            Update::Snapshot(items) => {
                self.items = items.into_iter().collect();
                self.synced = true;
                self.error = None;
            }
            Update::Applied((key, item)) => {
                self.items.insert(key, item);
            }
            Update::Deleted((key, _)) => {
                self.items.remove(&key);
            }
            Update::State(WatchState::Forbidden { .. }) => return Err(()),
            Update::State(WatchState::Reconnecting { error }) => {
                self.error = Some(error);
                return Ok(false);
            }
            Update::State(WatchState::Live) => {
                self.error = None;
                return Ok(false);
            }
            Update::State(WatchState::Connecting) => return Ok(false),
        }
        Ok(true)
    }

    /// After a 403: narrow to `fallback` (returns true: start a watch
    /// there) or, if that was already tried or the part is cluster-scoped,
    /// leave the part out.
    fn refused(&mut self, fallback: &str) -> bool {
        self.items.clear();
        self.synced = false;
        self.error = None;
        if self.namespaced && self.scope.is_none() {
            self.scope = Some(fallback.to_string());
            true
        } else {
            self.denied = true;
            false
        }
    }

    fn ready(&self) -> bool {
        self.synced || self.denied
    }

    fn visible(&self) -> bool {
        !self.denied
    }

    /// Why the part is narrowed or missing, if it is.
    fn note(&self, fallback: &str) -> Option<String> {
        let what = self.what;
        match (self.denied, self.namespaced, &self.scope) {
            (true, true, _) => Some(format!(
                "{what}: not allowed across all namespaces or in namespace {fallback}"
            )),
            (true, false, _) => Some(format!(
                "{what}: not allowed to list {}",
                what.to_lowercase()
            )),
            (false, _, Some(ns)) => Some(format!(
                "{what}: showing namespace {ns} only (not allowed across all namespaces)"
            )),
            (false, _, None) => None,
        }
    }
}

/// Drop the oldest events beyond [`MAX_EVENTS_KEPT`].
fn trim_events(events: &mut HashMap<String, EventInfo>) {
    if events.len() <= MAX_EVENTS_KEPT {
        return;
    }
    let mut by_age: Vec<(i64, String)> = events.iter().map(|(k, e)| (e.time, k.clone())).collect();
    by_age.sort_unstable();
    let extra = events.len() - MAX_EVENTS_KEPT;
    for (_, key) in by_age.into_iter().take(extra) {
        events.remove(&key);
    }
}

struct Live {
    client: Client,
    fallback: String,
    streams: SelectAll<BoxStream<'static, Msg>>,
    pods: Part<PodSummary>,
    events: Part<EventInfo>,
    namespaces: Part<()>,
    nodes: Part<NodeSummary>,
    usage: Option<(u64, u64)>,
    /// Something changed since the last summary.
    dirty: bool,
    ticks: u32,
    tick: Option<tokio::time::Interval>,
    last: Option<Overview>,
    state: Option<WatchState>,
    out: VecDeque<OverviewUpdate>,
}

impl Live {
    fn new(client: Client, fallback: String) -> Self {
        let streams = stream::select_all([
            pods_stream(&client, None),
            events_stream(&client, None),
            namespaces_stream(&client),
            nodes_stream(&client),
            usage_stream(&client),
        ]);
        Self {
            client,
            fallback,
            streams,
            pods: Part::new("Pods", true),
            events: Part::new("Events", true),
            namespaces: Part::new("Namespaces", false),
            nodes: Part::new("Nodes", false),
            usage: None,
            dirty: true,
            ticks: 0,
            tick: None,
            last: None,
            state: None,
            out: VecDeque::new(),
        }
    }

    async fn next(&mut self) -> OverviewUpdate {
        loop {
            if let Some(update) = self.out.pop_front() {
                return update;
            }
            // Created here, not in `new`: it needs the runtime.
            let tick = self.tick.get_or_insert_with(|| {
                let mut t = tokio::time::interval(THROTTLE);
                t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                t
            });
            tokio::select! {
                Some(msg) = self.streams.next(), if !self.streams.is_empty() => {
                    self.handle(msg);
                    // The first summary goes out as soon as every part has
                    // its list, not on the next tick; later ones are throttled.
                    if self.last.is_none() && self.ready() {
                        self.flush();
                    }
                }
                _ = tick.tick() => self.flush(),
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        let fallback = self.fallback.clone();
        let changed = match msg {
            Msg::Pods(u) => match self.pods.apply(u) {
                Ok(changed) => changed,
                Err(()) => {
                    if self.pods.refused(&fallback) {
                        self.streams
                            .push(pods_stream(&self.client, Some(&fallback)));
                    }
                    true
                }
            },
            Msg::Events(u) => match self.events.apply(u) {
                Ok(changed) => {
                    trim_events(&mut self.events.items);
                    changed
                }
                Err(()) => {
                    if self.events.refused(&fallback) {
                        self.streams
                            .push(events_stream(&self.client, Some(&fallback)));
                    }
                    true
                }
            },
            Msg::Namespaces(u) => self.namespaces.apply(u).unwrap_or_else(|()| {
                self.namespaces.refused(&fallback);
                true
            }),
            Msg::Nodes(u) => self.nodes.apply(u).unwrap_or_else(|()| {
                self.nodes.refused(&fallback);
                true
            }),
            Msg::Usage(usage) => {
                let changed = usage != self.usage;
                self.usage = usage;
                changed
            }
        };
        self.dirty |= changed;
    }

    fn ready(&self) -> bool {
        self.pods.ready() && self.events.ready() && self.namespaces.ready() && self.nodes.ready()
    }

    fn flush(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(RECHECK_TICKS) {
            self.dirty = true;
        }
        let ready = self.ready();
        if ready && self.dirty {
            self.dirty = false;
            let overview = self.summarize(chrono::Utc::now().timestamp());
            if self.last.as_ref() != Some(&overview) {
                self.last = Some(overview.clone());
                self.out
                    .push_back(OverviewUpdate::Summary(Box::new(overview)));
            }
        }
        let error = [
            &self.pods.error,
            &self.events.error,
            &self.namespaces.error,
            &self.nodes.error,
        ]
        .into_iter()
        .find_map(|e| e.clone());
        let state = match error {
            Some(error) => WatchState::Reconnecting { error },
            None if ready => WatchState::Live,
            None => WatchState::Connecting,
        };
        if self.state.as_ref() != Some(&state) {
            self.state = Some(state.clone());
            self.out.push_back(OverviewUpdate::State(state));
        }
    }

    fn summarize(&self, now: i64) -> Overview {
        let pods = self
            .pods
            .visible()
            .then(|| summary::pod_counts(self.pods.scope.clone(), self.pods.items.values(), now));
        let nodes = self
            .nodes
            .visible()
            .then(|| summary::node_counts(self.nodes.items.values()));
        let whole_cluster = self.pods.visible() && self.pods.scope.is_none();
        let capacity = (whole_cluster && self.nodes.visible()).then(|| {
            summary::capacity(
                self.pods.items.values(),
                self.nodes.items.values(),
                self.usage,
            )
        });
        let events = self.events.visible().then(|| RecentEvents {
            namespace: self.events.scope.clone(),
            items: summary::newest_events(self.events.items.values(), EVENTS_SHOWN),
        });
        let notes = [
            self.pods.note(&self.fallback),
            self.events.note(&self.fallback),
            self.namespaces.note(&self.fallback),
            self.nodes.note(&self.fallback),
        ]
        .into_iter()
        .flatten()
        .collect();
        Overview {
            namespaces: self
                .namespaces
                .visible()
                .then_some(self.namespaces.items.len()),
            nodes,
            pods,
            capacity,
            events,
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(name: &str) -> Keyed<PodSummary> {
        let s = PodSummary {
            namespace: "demo".into(),
            name: name.into(),
            status: "Running".into(),
            state: summary::PodState::Running,
            ready: true,
            terminating: false,
            restarts: 0,
            created: 1,
            scheduled: true,
            cpu_request_millicores: 0,
            memory_request_bytes: 0,
        };
        (format!("demo/{name}"), s)
    }

    #[test]
    fn part_folds_updates_and_reports_refusal() {
        let mut p = Part::new("Pods", true);
        assert!(!p.ready());
        assert_eq!(
            p.apply(Update::Snapshot(vec![pod("a"), pod("b")])),
            Ok(true)
        );
        assert!(p.ready());
        assert_eq!(p.apply(Update::Deleted(pod("a"))), Ok(true));
        assert_eq!(p.apply(Update::Applied(pod("c"))), Ok(true));
        let mut keys: Vec<_> = p.items.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["demo/b", "demo/c"]);
        let refused = Update::State(WatchState::Forbidden { error: "no".into() });
        assert_eq!(p.apply(refused), Err(()));
    }

    #[test]
    fn refused_namespaced_parts_fall_back_once_then_drop_out() {
        let mut p: Part<()> = Part::new("Pods", true);
        assert!(p.refused("team-a"));
        assert_eq!(p.scope.as_deref(), Some("team-a"));
        assert!(!p.ready());
        assert_eq!(
            p.note("team-a").unwrap(),
            "Pods: showing namespace team-a only (not allowed across all namespaces)"
        );
        assert!(!p.refused("team-a"));
        assert!(p.denied && p.ready());
        assert_eq!(
            p.note("team-a").unwrap(),
            "Pods: not allowed across all namespaces or in namespace team-a"
        );

        let mut nodes: Part<()> = Part::new("Nodes", false);
        assert!(!nodes.refused("team-a"));
        assert_eq!(nodes.note("x").unwrap(), "Nodes: not allowed to list nodes");
    }

    #[test]
    fn only_the_newest_events_are_kept() {
        let mut events = HashMap::new();
        for i in 0..(MAX_EVENTS_KEPT as i64 + 10) {
            let key = format!("demo/e{i}");
            let e = EventInfo {
                key: key.clone(),
                kind: "Warning".into(),
                reason: String::new(),
                message: String::new(),
                object: String::new(),
                object_api_version: String::new(),
                object_kind: String::new(),
                object_namespace: String::new(),
                object_name: String::new(),
                time: i,
                count: 1,
            };
            events.insert(key, e);
        }
        trim_events(&mut events);
        assert_eq!(events.len(), MAX_EVENTS_KEPT);
        assert!(!events.contains_key("demo/e9"));
        assert!(events.contains_key("demo/e10"));
    }
}
