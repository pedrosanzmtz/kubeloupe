//! Live updates of one object's detail: a watcher on that object (by
//! `metadata.name`) plus one on the events about it, coalesced so the panel
//! gets at most one new detail per interval, and a `deleted` state when the
//! object goes away.

use super::detail::{detail_of, summarize_events};
use super::ops::dyn_api;
use super::{ResourceDetail, ResourceRef};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Event as CoreEvent;
use kube::api::{Api, DynamicObject};
use kube::runtime::reflector::{self, Store};
use kube::runtime::watcher::{self, Event};
use kube::runtime::WatchStreamExt;
use kube::Client;
use serde::Serialize;
use std::time::Duration;

/// Default time between detail updates.
pub const DETAIL_FLUSH: Duration = Duration::from_millis(300);

/// What a detail watch reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum DetailState {
    /// The object as it is now.
    Detail { detail: Box<ResourceDetail> },
    /// The object no longer exists (it may come back with the same name).
    Deleted,
    /// The watch failed and is retrying; the last detail is still shown.
    Error { error: String },
}

/// Tracks the watched object across watcher restarts and decides what to
/// send: nothing if nothing visible changed since the last update.
#[derive(Debug, Default)]
pub(crate) struct DetailTracker {
    current: Option<DynamicObject>,
    /// Objects listed since the last `Init`, until `InitDone`.
    init: Option<Option<DynamicObject>>,
    /// The object is known: an initial list finished or it was seen since.
    known: bool,
    dirty: bool,
    last: Option<DetailState>,
}

impl DetailTracker {
    pub(crate) fn object(&mut self, event: Event<DynamicObject>) {
        match event {
            Event::Init => self.init = Some(None),
            Event::InitApply(o) => self.init = Some(Some(o)),
            Event::InitDone => {
                self.current = self.init.take().flatten();
                self.known = true;
                self.dirty = true;
            }
            Event::Apply(o) => {
                self.current = Some(o);
                self.known = true;
                self.dirty = true;
            }
            Event::Delete(_) => {
                self.current = None;
                self.dirty = true;
            }
        }
    }

    /// Something about the object's events changed.
    pub(crate) fn events_changed(&mut self) {
        if self.current.is_some() {
            self.dirty = true;
        }
    }

    /// The update to send, if any. `events` are the object's events.
    pub(crate) fn flush(
        &mut self,
        r: &ResourceRef,
        events: impl FnOnce(&str) -> Vec<CoreEvent>,
    ) -> Option<DetailState> {
        if !self.dirty || !self.known {
            return None;
        }
        self.dirty = false;
        let next = match &self.current {
            None => DetailState::Deleted,
            Some(obj) => {
                let uid = obj.metadata.uid.as_deref().unwrap_or_default();
                match detail_of(r, obj, summarize_events(events(uid))) {
                    Ok(detail) => DetailState::Detail {
                        detail: Box::new(detail),
                    },
                    Err(e) => DetailState::Error {
                        error: e.to_string(),
                    },
                }
            }
        };
        if self.last.as_ref() == Some(&next) {
            return None;
        }
        self.last = Some(next.clone());
        Some(next)
    }
}

/// Events in `store` about the object with `uid`.
fn events_of(store: &Store<CoreEvent>, uid: &str) -> Vec<CoreEvent> {
    store
        .state()
        .into_iter()
        .filter(|e| e.involved_object.uid.as_deref() == Some(uid))
        .map(|e| (*e).clone())
        .collect()
}

/// Watch one object and call `sink` with its detail, at most once per
/// `interval`, until the future is dropped. Watch failures are retried with
/// backoff and reported as [`DetailState::Error`]; events are best effort.
pub async fn run_detail_watch<F>(
    client: Client,
    r: ResourceRef,
    namespace: String,
    name: String,
    interval: Duration,
    mut sink: F,
) where
    F: FnMut(DetailState),
{
    let api = dyn_api(&client, &r, Some(&namespace));
    let by_name = watcher::Config::default().fields(&format!("metadata.name={name}"));
    let mut objects = watcher::watcher(api, by_name).default_backoff().boxed();

    // Events of cluster-scoped objects are filed in some namespace.
    let events_api: Api<CoreEvent> = if namespace.is_empty() || !r.namespaced {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    let about = watcher::Config::default().fields(&format!(
        "involvedObject.name={name},involvedObject.kind={}",
        r.kind
    ));
    let writer = reflector::store::Writer::<CoreEvent>::default();
    let store = writer.as_reader();
    let mut events = reflector::reflector(
        writer,
        watcher::watcher(events_api, about).default_backoff(),
    )
    .boxed();

    let mut tracker = DetailTracker::default();
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = objects.next() => match ev {
                None => break,
                Some(Ok(ev)) => tracker.object(ev),
                Some(Err(e)) => {
                    let error = DetailState::Error { error: e.to_string() };
                    if tracker.last.as_ref() != Some(&error) {
                        tracker.last = Some(error.clone());
                        // Send the detail again once the watch recovers.
                        tracker.dirty = true;
                        sink(error);
                    }
                }
            },
            ev = events.next() => match ev {
                None => break,
                Some(Ok(_)) => tracker.events_changed(),
                // Without access to events the panel shows none.
                Some(Err(_)) => {}
            },
            _ = tick.tick() => {
                if let Some(state) = tracker.flush(&r, |uid| events_of(&store, uid)) {
                    sink(state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod_ref() -> ResourceRef {
        crate::pod_ref()
    }

    fn cm_ref() -> ResourceRef {
        ResourceRef {
            group: String::new(),
            version: "v1".into(),
            kind: "ConfigMap".into(),
            plural: "configmaps".into(),
            namespaced: true,
        }
    }

    fn obj(kind: &str, uid: &str, labels: serde_json::Value) -> DynamicObject {
        serde_json::from_value(json!({
            "apiVersion": "v1", "kind": kind,
            "metadata": { "name": "x", "namespace": "ns", "uid": uid, "labels": labels },
            "spec": { "containers": [{ "name": "c", "image": "busybox" }] }
        }))
        .unwrap()
    }

    fn event(uid: &str, reason: &str) -> CoreEvent {
        serde_json::from_value(json!({
            "metadata": { "name": format!("x.{reason}"), "namespace": "ns" },
            "involvedObject": { "uid": uid, "name": "x", "kind": "Pod" },
            "reason": reason, "type": "Warning", "message": "m", "count": 2
        }))
        .unwrap()
    }

    fn labels(s: &DetailState) -> Vec<String> {
        match s {
            DetailState::Detail { detail } => detail.labels.keys().cloned().collect(),
            other => panic!("expected a detail, got {other:?}"),
        }
    }

    #[test]
    fn sends_after_the_initial_list_then_changes_only() {
        let r = cm_ref();
        let mut t = DetailTracker::default();
        assert_eq!(t.flush(&r, |_| vec![]), None);

        t.object(Event::Init);
        t.object(Event::InitApply(obj(
            "ConfigMap",
            "u1",
            json!({ "a": "1" }),
        )));
        // Nothing until the list is complete.
        assert_eq!(t.flush(&r, |_| vec![]), None);
        t.object(Event::InitDone);
        assert_eq!(labels(&t.flush(&r, |_| vec![]).unwrap()), ["a"]);
        assert_eq!(t.flush(&r, |_| vec![]), None);

        // A resourceVersion-only change is not sent again.
        t.object(Event::Apply(obj("ConfigMap", "u1", json!({ "a": "1" }))));
        assert_eq!(t.flush(&r, |_| vec![]), None);

        t.object(Event::Apply(obj(
            "ConfigMap",
            "u1",
            json!({ "a": "1", "b": "2" }),
        )));
        assert_eq!(labels(&t.flush(&r, |_| vec![]).unwrap()), ["a", "b"]);

        t.object(Event::Delete(obj("ConfigMap", "u1", json!({}))));
        assert_eq!(t.flush(&r, |_| vec![]), Some(DetailState::Deleted));

        // Recreated under the same name.
        t.object(Event::Apply(obj("ConfigMap", "u2", json!({ "c": "3" }))));
        let s = t.flush(&r, |_| vec![]).unwrap();
        assert_eq!(labels(&s), ["c"]);
        assert!(matches!(s, DetailState::Detail { detail } if detail.uid == "u2"));
    }

    #[test]
    fn missing_object_and_relist_without_it_are_deleted() {
        let r = cm_ref();
        let mut t = DetailTracker::default();
        t.object(Event::Init);
        t.object(Event::InitDone);
        assert_eq!(t.flush(&r, |_| vec![]), Some(DetailState::Deleted));

        let mut t = DetailTracker::default();
        t.object(Event::Apply(obj("ConfigMap", "u1", json!({}))));
        assert!(t.flush(&r, |_| vec![]).is_some());
        // The watcher restarted and the object was gone by then.
        t.object(Event::Init);
        t.object(Event::InitDone);
        assert_eq!(t.flush(&r, |_| vec![]), Some(DetailState::Deleted));
    }

    #[test]
    fn events_of_the_current_uid_are_included() {
        let r = pod_ref();
        let mut t = DetailTracker::default();
        t.object(Event::Apply(obj("Pod", "u1", json!({}))));
        let s = t.flush(&r, |uid| vec![event(uid, "BackOff")]).unwrap();
        let DetailState::Detail { detail } = s else {
            panic!("{s:?}")
        };
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.events[0].reason, "BackOff");
        assert_eq!(detail.events[0].count, 2);
        // Pods get their section.
        assert_eq!(detail.pod.containers[0].name, "c");

        // A new event marks it dirty.
        t.events_changed();
        let s = t
            .flush(&r, |uid| {
                vec![event(uid, "BackOff"), event(uid, "Unhealthy")]
            })
            .unwrap();
        let DetailState::Detail { detail } = s else {
            panic!("{s:?}")
        };
        assert_eq!(detail.events.len(), 2);
    }
}
