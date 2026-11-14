//! Usage history from metrics-server: a bounded ring buffer per pod and per
//! node, fed by whatever polls the metrics API (the tables, a detail panel).
//!
//! metrics-server only knows the current value, so history starts when
//! something first records it; `since` says when that was.

use crate::{NodeUsage, PodUsage};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};

/// How far back history goes.
pub const HISTORY_WINDOW_MS: u64 = 30 * 60 * 1000;
/// Samples closer together than this are dropped: several polls of the same
/// pod (one per selected namespace, a table and a panel) add one sample.
/// metrics-server's default resolution is 15 s, so nothing is lost.
pub const MIN_SPACING_MS: u64 = 10_000;
/// Hard cap per series, whatever the spacing: 30 min at 10 s.
pub const MAX_SAMPLES: usize = (HISTORY_WINDOW_MS / MIN_SPACING_MS) as usize;
/// Points per sparkline in a table cell.
pub const SPARK_POINTS: usize = 30;

/// One sample: epoch milliseconds, millicores and bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct UsageSample {
    pub t: u64,
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
}

/// A downsampled series for a table cell's sparkline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Spark {
    pub cpu: Vec<u64>,
    pub memory: Vec<u64>,
}

/// One container's samples (pods only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ContainerSeries {
    pub name: String,
    pub samples: Vec<UsageSample>,
}

/// The full history of one pod or node, oldest sample first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SeriesView {
    /// Epoch ms of the oldest sample still kept (0 = none yet).
    pub since: u64,
    pub samples: Vec<UsageSample>,
    pub containers: Vec<ContainerSeries>,
}

#[derive(Debug, Default)]
struct Ring(VecDeque<UsageSample>);

impl Ring {
    /// Add a sample unless the last one is too recent. Returns whether it
    /// was added.
    fn push(&mut self, s: UsageSample) -> bool {
        if let Some(last) = self.0.back() {
            if s.t < last.t + MIN_SPACING_MS {
                return false;
            }
        }
        self.0.push_back(s);
        while self.0.len() > MAX_SAMPLES {
            self.0.pop_front();
        }
        true
    }

    fn prune(&mut self, oldest: u64) {
        while self.0.front().is_some_and(|s| s.t < oldest) {
            self.0.pop_front();
        }
    }

    fn last_t(&self) -> Option<u64> {
        self.0.back().map(|s| s.t)
    }

    fn to_vec(&self) -> Vec<UsageSample> {
        self.0.iter().copied().collect()
    }
}

#[derive(Debug, Default)]
struct Series {
    total: Ring,
    containers: BTreeMap<String, Ring>,
}

impl Series {
    fn view(&self) -> SeriesView {
        SeriesView {
            since: self.total.0.front().map(|s| s.t).unwrap_or(0),
            samples: self.total.to_vec(),
            containers: self
                .containers
                .iter()
                .filter(|(_, r)| !r.0.is_empty())
                .map(|(name, r)| ContainerSeries {
                    name: name.clone(),
                    samples: r.to_vec(),
                })
                .collect(),
        }
    }

    fn spark(&self) -> Spark {
        let points = downsample(&self.total.to_vec(), SPARK_POINTS);
        Spark {
            cpu: points.iter().map(|s| s.cpu_millicores).collect(),
            memory: points.iter().map(|s| s.memory_bytes).collect(),
        }
    }
}

/// The history of one context. Pods are keyed `namespace/name` (like the
/// table rows), nodes by name.
#[derive(Debug, Default)]
pub struct UsageHistory {
    pods: HashMap<String, Series>,
    nodes: HashMap<String, Series>,
}

/// The key of a pod, as the table rows use it.
pub fn pod_key(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

impl UsageHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (the context changed).
    pub fn clear(&mut self) {
        self.pods.clear();
        self.nodes.clear();
    }

    /// Number of pods and nodes with history.
    pub fn len(&self) -> usize {
        self.pods.len() + self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Record pod usage taken at `now` (epoch ms).
    pub fn record_pods(&mut self, now: u64, pods: &[PodUsage]) {
        for p in pods {
            let series = self.pods.entry(pod_key(&p.namespace, &p.name)).or_default();
            if !series.total.push(UsageSample {
                t: now,
                cpu_millicores: p.cpu_millicores,
                memory_bytes: p.memory_bytes,
            }) {
                continue;
            }
            for c in &p.containers {
                series
                    .containers
                    .entry(c.name.clone())
                    .or_default()
                    .push(UsageSample {
                        t: now,
                        cpu_millicores: c.cpu_millicores,
                        memory_bytes: c.memory_bytes,
                    });
            }
        }
        self.prune(now);
    }

    /// Record node usage taken at `now` (epoch ms).
    pub fn record_nodes(&mut self, now: u64, nodes: &[NodeUsage]) {
        for n in nodes {
            self.nodes
                .entry(n.name.clone())
                .or_default()
                .total
                .push(UsageSample {
                    t: now,
                    cpu_millicores: n.cpu_millicores,
                    memory_bytes: n.memory_bytes,
                });
        }
        self.prune(now);
    }

    /// Drop samples older than the window, and pods and nodes left with none
    /// (deleted, or no longer shown), so memory stays bounded.
    fn prune(&mut self, now: u64) {
        let oldest = now.saturating_sub(HISTORY_WINDOW_MS);
        for map in [&mut self.pods, &mut self.nodes] {
            map.retain(|_, s| {
                s.total.prune(oldest);
                for r in s.containers.values_mut() {
                    r.prune(oldest);
                }
                s.containers.retain(|_, r| !r.0.is_empty());
                !s.total.0.is_empty()
            });
        }
    }

    /// Epoch ms of the newest sample of a pod (`namespace/name`) or node.
    pub fn last_sample(&self, pod: bool, key: &str) -> Option<u64> {
        let map = if pod { &self.pods } else { &self.nodes };
        map.get(key).and_then(|s| s.total.last_t())
    }

    /// The full history of a pod (`namespace/name`) or node.
    pub fn view(&self, pod: bool, key: &str) -> SeriesView {
        let map = if pod { &self.pods } else { &self.nodes };
        map.get(key).map(Series::view).unwrap_or_default()
    }

    /// Sparklines for the given pods, keyed `namespace/name`.
    pub fn pod_sparks<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a PodUsage>,
    ) -> HashMap<String, Spark> {
        keys.into_iter()
            .filter_map(|p| {
                let key = pod_key(&p.namespace, &p.name);
                let spark = self.pods.get(&key)?.spark();
                Some((key, spark))
            })
            .collect()
    }

    /// Sparklines for the given nodes, keyed by name.
    pub fn node_sparks<'a>(
        &self,
        nodes: impl IntoIterator<Item = &'a NodeUsage>,
    ) -> HashMap<String, Spark> {
        nodes
            .into_iter()
            .filter_map(|n| Some((n.name.clone(), self.nodes.get(&n.name)?.spark())))
            .collect()
    }

    /// Epoch ms of the oldest sample of any of the given keys (0 = none).
    pub fn since<'a>(&self, pod: bool, keys: impl IntoIterator<Item = &'a str>) -> u64 {
        let map = if pod { &self.pods } else { &self.nodes };
        keys.into_iter()
            .filter_map(|k| map.get(k)?.total.0.front().map(|s| s.t))
            .min()
            .unwrap_or(0)
    }
}

/// Reduce `samples` to at most `points` by averaging equal-sized buckets
/// (the last bucket takes the remainder). Each point keeps its bucket's last
/// timestamp. Shorter series are returned as they are.
pub fn downsample(samples: &[UsageSample], points: usize) -> Vec<UsageSample> {
    if points == 0 {
        return Vec::new();
    }
    if samples.len() <= points {
        return samples.to_vec();
    }
    (0..points)
        .map(|i| {
            let start = i * samples.len() / points;
            let end = (i + 1) * samples.len() / points;
            let bucket = &samples[start..end];
            let n = bucket.len() as u64;
            UsageSample {
                t: bucket[bucket.len() - 1].t,
                cpu_millicores: bucket.iter().map(|s| s.cpu_millicores).sum::<u64>() / n,
                memory_bytes: bucket.iter().map(|s| s.memory_bytes).sum::<u64>() / n,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContainerUsage;

    fn pod(name: &str, cpu: u64, mem: u64) -> PodUsage {
        PodUsage {
            namespace: "demo".into(),
            name: name.into(),
            cpu_millicores: cpu,
            memory_bytes: mem,
            containers: vec![
                ContainerUsage {
                    name: "app".into(),
                    cpu_millicores: cpu - 1,
                    memory_bytes: mem - 1,
                },
                ContainerUsage {
                    name: "sidecar".into(),
                    cpu_millicores: 1,
                    memory_bytes: 1,
                },
            ],
        }
    }

    fn sample(t: u64, cpu: u64, mem: u64) -> UsageSample {
        UsageSample {
            t,
            cpu_millicores: cpu,
            memory_bytes: mem,
        }
    }

    #[test]
    fn records_pods_with_their_containers() {
        let mut h = UsageHistory::new();
        h.record_pods(1_000_000, &[pod("web", 100, 1000)]);
        h.record_pods(1_020_000, &[pod("web", 200, 2000)]);
        let v = h.view(true, "demo/web");
        assert_eq!(v.since, 1_000_000);
        assert_eq!(
            v.samples,
            vec![sample(1_000_000, 100, 1000), sample(1_020_000, 200, 2000)]
        );
        assert_eq!(v.containers.len(), 2);
        assert_eq!(v.containers[0].name, "app");
        assert_eq!(v.containers[0].samples[1], sample(1_020_000, 199, 1999));
        assert_eq!(h.view(true, "demo/other"), SeriesView::default());
    }

    #[test]
    fn samples_too_close_together_are_dropped() {
        let mut h = UsageHistory::new();
        h.record_pods(1_000_000, &[pod("web", 100, 1000)]);
        // A second namespace poll, or the panel, a few seconds later.
        h.record_pods(1_003_000, &[pod("web", 999, 9999)]);
        let v = h.view(true, "demo/web");
        assert_eq!(v.samples.len(), 1);
        assert_eq!(v.containers[0].samples.len(), 1);
        assert_eq!(h.last_sample(true, "demo/web"), Some(1_000_000));
    }

    #[test]
    fn old_samples_and_gone_pods_are_pruned() {
        let mut h = UsageHistory::new();
        let t0 = 10_000_000;
        h.record_pods(t0, &[pod("gone", 5, 5), pod("web", 5, 5)]);
        let mut t = t0;
        for _ in 0..400 {
            t += 15_000;
            h.record_pods(t, &[pod("web", 10, 10)]);
        }
        // "gone" was not seen for longer than the window.
        assert_eq!(h.len(), 1);
        let v = h.view(true, "demo/web");
        assert!(v.samples.len() <= MAX_SAMPLES);
        assert!(v.since >= t - HISTORY_WINDOW_MS);
        assert_eq!(v.samples.last().unwrap().t, t);
    }

    #[test]
    fn ring_never_exceeds_its_cap() {
        let mut r = Ring::default();
        for i in 0..(MAX_SAMPLES as u64 * 2) {
            r.push(sample(i * MIN_SPACING_MS, i, i));
        }
        assert_eq!(r.0.len(), MAX_SAMPLES);
        assert_eq!(r.0.front().unwrap().cpu_millicores, MAX_SAMPLES as u64);
    }

    #[test]
    fn nodes_and_sparks() {
        let mut h = UsageHistory::new();
        let node = NodeUsage {
            name: "n1".into(),
            cpu_millicores: 500,
            cpu_capacity_millicores: 4000,
            memory_bytes: 10,
            memory_capacity_bytes: 100,
        };
        h.record_nodes(1_000_000, std::slice::from_ref(&node));
        let sparks = h.node_sparks([&node]);
        assert_eq!(
            sparks["n1"],
            Spark {
                cpu: vec![500],
                memory: vec![10]
            }
        );
        assert_eq!(h.since(false, ["n1", "n2"]), 1_000_000);
        assert_eq!(h.since(true, ["demo/web"]), 0);
        let p = pod("web", 100, 1000);
        h.record_pods(1_000_000, std::slice::from_ref(&p));
        assert_eq!(h.pod_sparks([&p])["demo/web"].cpu, vec![100]);
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn downsampling_averages_buckets() {
        let s: Vec<UsageSample> = (0..10).map(|i| sample(i, i * 10, i)).collect();
        // Short enough: unchanged.
        assert_eq!(downsample(&s, 10), s);
        assert_eq!(downsample(&s, 0), vec![]);
        let d = downsample(&s, 5);
        assert_eq!(d.len(), 5);
        assert_eq!(d[0], sample(1, 5, 0));
        assert_eq!(d[4], sample(9, 85, 8));
        // Uneven: 10 into 3 buckets of 3, 3 and 4.
        let d = downsample(&s, 3);
        assert_eq!(
            d.iter().map(|s| s.cpu_millicores).collect::<Vec<_>>(),
            vec![10, 40, 75]
        );
        assert_eq!(d[2].t, 9);
    }
}
