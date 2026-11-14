//! Live cluster state via `kube::runtime` watchers and reflectors.
//!
//! [`watch`] turns a watcher (with `.default_backoff()`, so it re-lists after
//! a 410 Gone and reconnects after a network drop) into a stream of
//! [`Update`]s of display-ready summaries. [`run_batched`] coalesces those
//! into at most one [`Batch`] per interval, which is what a UI wants instead
//! of one event per object.

use crate::resources::{make_row, ResourceKind, ResourceRow};
use crate::{namespace_info, node_info, pod_info, NamespaceInfo, NodeInfo, PodInfo};
use futures::stream::{self, BoxStream};
use futures::{Stream, StreamExt};
use k8s_openapi::api::core::v1::{Namespace, Node, Pod};
use kube::api::DynamicObject;
use kube::runtime::reflector::{self, Store};
use kube::runtime::watcher::{self, Event};
use kube::runtime::WatchStreamExt;
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

/// Default time between batches.
pub const DEFAULT_FLUSH: Duration = Duration::from_millis(150);

/// Stable identity of a row, used to diff updates (`namespace/name` for
/// namespaced objects, `name` otherwise).
pub trait Keyed {
    fn key(&self) -> String;
    /// Whether `other` would render identically. `age` is ignored: it is
    /// derived from the creation time, which UIs recompute themselves.
    fn same(&self, other: &Self) -> bool;
}

impl Keyed for PodInfo {
    fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
    fn same(&self, o: &Self) -> bool {
        (&self.name, &self.namespace, &self.phase, &self.node)
            == (&o.name, &o.namespace, &o.phase, &o.node)
    }
}

impl Keyed for NamespaceInfo {
    fn key(&self) -> String {
        self.name.clone()
    }
    fn same(&self, o: &Self) -> bool {
        (&self.name, &self.status) == (&o.name, &o.status)
    }
}

impl Keyed for NodeInfo {
    fn key(&self) -> String {
        self.name.clone()
    }
    fn same(&self, o: &Self) -> bool {
        (&self.name, &self.ready, &self.version) == (&o.name, &o.ready, &o.version)
    }
}

impl Keyed for ResourceRow {
    fn key(&self) -> String {
        self.key.clone()
    }
    /// Cells hold everything the backend computes; the age column's cell is
    /// empty (the UI derives it from `created`), so age ticks never differ.
    fn same(&self, o: &Self) -> bool {
        (&self.name, &self.namespace, &self.cells) == (&o.name, &o.namespace, &o.cells)
    }
}

/// Connection state of a watch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum WatchState {
    /// Waiting for the first full list.
    Connecting,
    /// Receiving updates.
    Live,
    /// The watch failed and is backing off before retrying (it re-lists
    /// by itself when it has to).
    Reconnecting { error: String },
    /// The server refused the list or watch (403). Retrying would not help,
    /// so the watch stops; `error` is the server's message.
    Forbidden { error: String },
}

/// One step of a watch, with objects already summarized.
#[derive(Debug, Clone, PartialEq)]
pub enum Update<T> {
    /// The complete current set. Sent once after the initial list and again
    /// after every re-list; replaces whatever the consumer had.
    Snapshot(Vec<T>),
    Applied(T),
    Deleted(T),
    State(WatchState),
}

/// A coalesced set of changes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Batch<T> {
    /// When true, `applied` is the full state and anything not in it is gone.
    pub snapshot: bool,
    pub applied: Vec<T>,
    /// Keys of removed objects.
    pub deleted: Vec<String>,
}

/// Output of [`run_batched`].
#[derive(Debug, Clone, PartialEq)]
pub enum Output<T> {
    Batch(Batch<T>),
    State(WatchState),
}

/// Coalesces updates between flushes: the last change per key wins, a
/// delete cancels a pending apply (and vice versa), and objects whose
/// summary did not change since they were last sent are dropped, so
/// resourceVersion-only churn never reaches the UI.
#[derive(Debug)]
pub struct Batcher<T> {
    known: HashMap<String, T>,
    applied: HashMap<String, T>,
    deleted: HashSet<String>,
}

impl<T: Keyed + Clone> Default for Batcher<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Keyed + Clone> Batcher<T> {
    pub fn new() -> Self {
        Self {
            known: HashMap::new(),
            applied: HashMap::new(),
            deleted: HashSet::new(),
        }
    }

    pub fn apply(&mut self, item: T) {
        let key = item.key();
        self.deleted.remove(&key);
        self.applied.insert(key, item);
    }

    pub fn delete(&mut self, item: &T) {
        let key = item.key();
        self.applied.remove(&key);
        self.deleted.insert(key);
    }

    /// Replace everything with `items`; pending changes are discarded.
    pub fn snapshot(&mut self, items: Vec<T>) -> Batch<T> {
        self.applied.clear();
        self.deleted.clear();
        self.known = items.iter().map(|i| (i.key(), i.clone())).collect();
        Batch {
            snapshot: true,
            applied: items,
            deleted: Vec::new(),
        }
    }

    /// Take the pending changes, or `None` if nothing visible changed.
    pub fn flush(&mut self) -> Option<Batch<T>> {
        let mut applied = Vec::new();
        for (key, item) in self.applied.drain() {
            if !self.known.get(&key).is_some_and(|k| k.same(&item)) {
                self.known.insert(key, item.clone());
                applied.push(item);
            }
        }
        let mut deleted = Vec::new();
        for key in self.deleted.drain() {
            if self.known.remove(&key).is_some() {
                deleted.push(key);
            }
        }
        if applied.is_empty() && deleted.is_empty() {
            return None;
        }
        applied.sort_by_key(|i| i.key());
        deleted.sort();
        Some(Batch {
            snapshot: false,
            applied,
            deleted,
        })
    }
}

/// Drive `updates` to completion, calling `sink` with state changes right
/// away, snapshots immediately, and other changes batched every `interval`.
pub async fn run_batched<T, S, F>(mut updates: S, interval: Duration, mut sink: F)
where
    T: Keyed + Clone,
    S: Stream<Item = Update<T>> + Unpin,
    F: FnMut(Output<T>),
{
    let mut batcher = Batcher::new();
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            update = updates.next() => match update {
                None => break,
                Some(Update::Snapshot(items)) => sink(Output::Batch(batcher.snapshot(items))),
                Some(Update::Applied(item)) => batcher.apply(item),
                Some(Update::Deleted(item)) => batcher.delete(&item),
                Some(Update::State(state)) => sink(Output::State(state)),
            },
            _ = tick.tick() => {
                if let Some(batch) = batcher.flush() {
                    sink(Output::Batch(batch));
                }
            }
        }
    }
    if let Some(batch) = batcher.flush() {
        sink(Output::Batch(batch));
    }
}

/// Whether a watcher error is the server refusing access (HTTP 403).
fn is_forbidden(e: &watcher::Error) -> bool {
    match e {
        watcher::Error::InitialListFailed(kube::Error::Api(r))
        | watcher::Error::WatchStartFailed(kube::Error::Api(r))
        | watcher::Error::WatchFailed(kube::Error::Api(r))
        | watcher::Error::WatchError(r) => r.code == 403,
        _ => false,
    }
}

/// Tracks the watcher's lifecycle and turns raw events into [`Update`]s.
struct Tracker<T> {
    init: Vec<T>,
    live: bool,
    failing: bool,
    /// A 403 ended the watch.
    done: bool,
}

impl<T> Tracker<T> {
    fn step<K>(
        &mut self,
        event: Result<Event<K>, watcher::Error>,
        convert: &impl Fn(&K) -> T,
    ) -> Vec<Update<T>> {
        let mut out = Vec::new();
        match event {
            Err(e) if is_forbidden(&e) => {
                self.done = true;
                self.live = false;
                out.push(Update::State(WatchState::Forbidden {
                    error: e.to_string(),
                }));
            }
            Err(e) => {
                // Only report the first failure of a streak.
                if !self.failing {
                    self.failing = true;
                    self.live = false;
                    out.push(Update::State(WatchState::Reconnecting {
                        error: e.to_string(),
                    }));
                }
            }
            Ok(event) => {
                self.failing = false;
                match event {
                    Event::Init => self.init.clear(),
                    Event::InitApply(k) => self.init.push(convert(&k)),
                    Event::InitDone => {
                        out.push(Update::Snapshot(std::mem::take(&mut self.init)));
                        self.live = true;
                        out.push(Update::State(WatchState::Live));
                    }
                    Event::Apply(k) => {
                        out.push(Update::Applied(convert(&k)));
                        self.mark_live(&mut out);
                    }
                    Event::Delete(k) => {
                        out.push(Update::Deleted(convert(&k)));
                        self.mark_live(&mut out);
                    }
                }
            }
        }
        out
    }

    /// The watch resumed after an error without needing a re-list.
    fn mark_live(&mut self, out: &mut Vec<Update<T>>) {
        if !self.live {
            self.live = true;
            out.push(Update::State(WatchState::Live));
        }
    }
}

/// Watch `api`, keeping a reflector [`Store`] of the full objects and
/// yielding summaries (via `convert`) as [`Update`]s. Failures are retried
/// with backoff, except a 403: the stream then ends after a
/// [`WatchState::Forbidden`]. Otherwise it never ends on its own; drop it to
/// stop.
///
/// `dyntype` is the kind's dynamic type (`()` for typed resources, an
/// `ApiResource` for `DynamicObject`), so one watcher serves both the typed
/// pod/node/namespace watches and any discovered kind.
pub fn watch_with<K, T>(
    api: Api<K>,
    dyntype: K::DynamicType,
    convert: impl Fn(&K) -> T + Send + 'static,
) -> (Store<K>, BoxStream<'static, Update<T>>)
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
    K::DynamicType: Eq + Hash + Clone + Send + Sync + Unpin,
    T: Send + 'static,
{
    watch_with_config(api, dyntype, watcher::Config::default(), convert)
}

/// [`watch_with`] with a custom watcher config (label/field selectors).
pub fn watch_with_config<K, T>(
    api: Api<K>,
    dyntype: K::DynamicType,
    config: watcher::Config,
    convert: impl Fn(&K) -> T + Send + 'static,
) -> (Store<K>, BoxStream<'static, Update<T>>)
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
    K::DynamicType: Eq + Hash + Clone + Send + Sync + Unpin,
    T: Send + 'static,
{
    let writer = reflector::store::Writer::new(dyntype);
    let store = writer.as_reader();
    let events =
        reflector::reflector(writer, watcher::watcher(api, config).default_backoff()).boxed();
    (store, track(events, convert))
}

/// Like [`watch_with_config`], but without a reflector store: only the
/// summaries exist, so memory stays proportional to what `convert` keeps.
/// For consumers that fold the updates into their own state (the overview).
pub fn watch_summaries<K, T>(
    api: Api<K>,
    config: watcher::Config,
    convert: impl Fn(&K) -> T + Send + 'static,
) -> BoxStream<'static, Update<T>>
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + 'static,
    T: Send + 'static,
{
    track(
        watcher::watcher(api, config).default_backoff().boxed(),
        convert,
    )
}

/// Turn raw watcher events into [`Update`]s: [`WatchState::Connecting`]
/// first, and the end right after a 403.
fn track<K, T>(
    events: BoxStream<'static, Result<Event<K>, watcher::Error>>,
    convert: impl Fn(&K) -> T + Send + 'static,
) -> BoxStream<'static, Update<T>>
where
    K: Send + 'static,
    T: Send + 'static,
{
    let tracker = Tracker {
        init: Vec::new(),
        live: false,
        failing: false,
        done: false,
    };
    // End right after a 403 instead of polling the watcher again, which
    // would only sleep in its backoff and retry.
    let updates = stream::unfold(
        (events, tracker, convert),
        |(mut events, mut t, convert)| async move {
            if t.done {
                return None;
            }
            let event = events.next().await?;
            let out = t.step(event, &convert);
            Some((out, (events, t, convert)))
        },
    )
    .flat_map(stream::iter);
    let first = stream::once(futures::future::ready(Update::State(
        WatchState::Connecting,
    )));
    first.chain(updates).boxed()
}

/// [`watch_with`] for a typed resource.
pub fn watch<K, T>(api: Api<K>, convert: fn(&K) -> T) -> (Store<K>, BoxStream<'static, Update<T>>)
where
    K: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
    T: Send + 'static,
{
    watch_with(api, (), convert)
}

/// Watch any discovered kind, optionally in one namespace, yielding table
/// rows built from the kind's column definitions.
pub fn watch_resource(
    client: &Client,
    kind: &ResourceKind,
    namespace: Option<&str>,
) -> (
    Store<DynamicObject>,
    BoxStream<'static, Update<ResourceRow>>,
) {
    let api = crate::resources::dyn_api(client, &kind.resource, namespace);
    let columns = Arc::new(kind.columns.clone());
    watch_with(api, kind.resource.api_resource(), move |o| {
        make_row(&columns, o)
    })
}

pub fn watch_pods(
    client: &Client,
    namespace: Option<&str>,
) -> (Store<Pod>, BoxStream<'static, Update<PodInfo>>) {
    let api: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    watch(api, pod_info)
}

/// Watch the pods in `namespace` matching a label `selector`.
pub fn watch_pods_selected(
    client: &Client,
    namespace: &str,
    selector: &str,
) -> (Store<Pod>, BoxStream<'static, Update<PodInfo>>) {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    watch_with_config(
        api,
        (),
        watcher::Config::default().labels(selector),
        pod_info,
    )
}

pub fn watch_nodes(client: &Client) -> (Store<Node>, BoxStream<'static, Update<NodeInfo>>) {
    watch(Api::all(client.clone()), node_info)
}

pub fn watch_namespaces(
    client: &Client,
) -> (Store<Namespace>, BoxStream<'static, Update<NamespaceInfo>>) {
    watch(Api::all(client.clone()), namespace_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(ns: &str, name: &str, phase: &str) -> PodInfo {
        PodInfo {
            created: 0,
            name: name.into(),
            namespace: ns.into(),
            phase: phase.into(),
            node: String::new(),
            age: String::new(),
        }
    }

    #[test]
    fn last_apply_per_key_wins() {
        let mut b = Batcher::new();
        b.apply(pod("a", "x", "Pending"));
        b.apply(pod("a", "x", "Running"));
        let batch = b.flush().unwrap();
        assert_eq!(batch.applied, vec![pod("a", "x", "Running")]);
        assert!(b.flush().is_none());
    }

    #[test]
    fn unchanged_summaries_are_dropped() {
        let mut b = Batcher::new();
        b.snapshot(vec![pod("a", "x", "Running")]);
        b.apply(pod("a", "x", "Running"));
        assert!(b.flush().is_none());
        b.apply(pod("a", "x", "Failed"));
        assert_eq!(b.flush().unwrap().applied.len(), 1);
    }

    #[test]
    fn age_only_changes_are_not_sent() {
        let mut b = Batcher::new();
        b.snapshot(vec![pod("a", "x", "Running")]);
        let mut older = pod("a", "x", "Running");
        older.age = "5m".into();
        b.apply(older);
        assert!(b.flush().is_none());
    }

    #[test]
    fn create_then_delete_within_a_batch_cancels_out() {
        let mut b = Batcher::new();
        let p = pod("a", "x", "Pending");
        b.apply(p.clone());
        b.delete(&p);
        assert!(b.flush().is_none());
    }

    #[test]
    fn delete_of_known_then_recreate_is_not_a_change() {
        let mut b = Batcher::new();
        let p = pod("a", "x", "Running");
        b.snapshot(vec![p.clone()]);
        b.delete(&p);
        b.apply(p);
        assert!(b.flush().is_none());
    }

    #[test]
    fn delete_of_known_is_reported_once() {
        let mut b = Batcher::new();
        let p = pod("a", "x", "Running");
        b.snapshot(vec![p.clone()]);
        b.delete(&p);
        let batch = b.flush().unwrap();
        assert_eq!(batch.deleted, vec!["a/x".to_string()]);
        b.delete(&p);
        assert!(b.flush().is_none());
    }

    #[test]
    fn snapshot_discards_pending_changes() {
        let mut b = Batcher::new();
        b.apply(pod("a", "old", "Running"));
        let snap = b.snapshot(vec![pod("a", "new", "Running")]);
        assert!(snap.snapshot);
        assert!(b.flush().is_none());
    }

    #[tokio::test]
    async fn run_batched_emits_snapshot_then_one_coalesced_batch() {
        let updates = stream::iter(vec![
            Update::State(WatchState::Connecting),
            Update::Snapshot(vec![pod("a", "x", "Running")]),
            Update::State(WatchState::Live),
            Update::Applied(pod("a", "y", "Pending")),
            Update::Applied(pod("a", "y", "Running")),
            Update::Deleted(pod("a", "x", "Running")),
        ]);
        let mut out = Vec::new();
        run_batched(updates, Duration::from_secs(3600), |o| out.push(o)).await;
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], Output::State(WatchState::Connecting));
        assert!(matches!(&out[1], Output::Batch(b) if b.snapshot && b.applied.len() == 1));
        assert_eq!(out[2], Output::State(WatchState::Live));
        match &out[3] {
            Output::Batch(b) => {
                assert!(!b.snapshot);
                assert_eq!(b.applied, vec![pod("a", "y", "Running")]);
                assert_eq!(b.deleted, vec!["a/x".to_string()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tracker_snapshots_on_init_done_and_reports_failures_once() {
        let mut t = Tracker::<String> {
            init: vec![],
            live: false,
            failing: false,
            done: false,
        };
        let conv = |s: &String| s.clone();
        assert!(t.step(Ok(Event::Init), &conv).is_empty());
        assert!(t
            .step(Ok(Event::InitApply("a".to_string())), &conv)
            .is_empty());
        let out = t.step(Ok(Event::InitDone), &conv);
        assert_eq!(
            out,
            vec![
                Update::Snapshot(vec!["a".to_string()]),
                Update::State(WatchState::Live)
            ]
        );
        let err = || Err(watcher::Error::NoResourceVersion);
        assert_eq!(t.step(err(), &conv).len(), 1);
        assert!(t.step(err(), &conv).is_empty());
        let out = t.step(Ok(Event::Apply("b".to_string())), &conv);
        assert_eq!(out.last(), Some(&Update::State(WatchState::Live)));
        assert!(!t.done);
    }

    #[test]
    fn tracker_stops_on_forbidden() {
        let mut t = Tracker::<String> {
            init: vec![],
            live: false,
            failing: false,
            done: false,
        };
        let conv = |s: &String| s.clone();
        let forbidden =
            watcher::Error::InitialListFailed(kube::Error::Api(kube::core::ErrorResponse {
                status: "Failure".into(),
                message: "pods is forbidden".into(),
                reason: "Forbidden".into(),
                code: 403,
            }));
        let out = t.step(Err(forbidden), &conv);
        assert!(
            matches!(out.as_slice(), [Update::State(WatchState::Forbidden { error })] if error.contains("forbidden")),
            "{out:?}"
        );
        assert!(t.done);
    }
}
