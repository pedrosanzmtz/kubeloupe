import { commands, events } from "../bindings";
import type { WatchBatch, WatchStateChanged } from "../bindings";
import type { MetricsKind, MetricsSnapshot, ResourceKind } from "../bindings";
import { EMPTY_METRICS, isNode, isPod } from "../columns";
import type { Metrics, Row } from "../columns";
import { setStatus, showError, stamp } from "./app.svelte";
import { clusterError } from "../clusterError";
import { combineWatches } from "../namespaces";
import type { WatchStatus } from "../namespaces";
import { KeyedStore } from "./keyed.svelte";

const METRICS_INTERVAL_MS = 20_000;
const AGE_INTERVAL_MS = 15_000;
const WATCH_RETRY_MS = 5_000;

type WatchEvent = { kind: "batch"; payload: WatchBatch } | { kind: "state"; payload: WatchStateChanged };

/** One backend watch: a namespace, or `""` for all namespaces / cluster scope. */
interface Watch {
  key: string;
  /** Known once `cmd_watch_start` resolves. */
  id: string | null;
  starting: boolean;
  status: WatchStatus;
  retry: ReturnType<typeof setTimeout> | null;
}

/**
 * The live table behind every resource view.
 *
 * The backend watches the cluster and pushes coalesced `watch-batch` events
 * ({snapshot, applied, deleted}); they land in a keyed store. With several
 * namespaces selected there is one watch per namespace, all feeding the same
 * store: each watch is a source there, so its snapshot only replaces its own
 * rows. Metrics cannot be watched, so they are polled and merged at render
 * time, and ages are recomputed from `created` whenever `now` ticks.
 */
export class LiveTable {
  store = new KeyedStore<Row>((row) => row.key);
  metrics = $state.raw<Metrics>(EMPTY_METRICS);
  now = $state.raw(Date.now());

  #kind: ResourceKind | null = null;
  /** The running watches by key, in selection order. */
  #watches = new Map<string, Watch>();
  #byId = new Map<string, Watch>();
  /** Events that arrived before their `cmd_watch_start` resolved. */
  #pending: WatchEvent[] = [];
  /** Bumped when the watched set changes, to drop stale metrics. */
  #token = 0;
  #metricsTimer: ReturnType<typeof setInterval> | null = null;
  #ageTimer: ReturnType<typeof setInterval> | null = null;

  /** Subscribe to the backend events; returns a cleanup function. */
  async listen(): Promise<() => void> {
    const unlisteners = await Promise.all([
      events.watchBatch.listen((e) => this.#onEvent({ kind: "batch", payload: e.payload })),
      events.watchState.listen((e) => this.#onEvent({ kind: "state", payload: e.payload })),
    ]);
    return () => unlisteners.forEach((un) => un());
  }

  #onEvent(ev: WatchEvent) {
    const w = this.#byId.get(ev.payload.id);
    if (w) this.#handle(w, ev);
    // The id is only known once cmd_watch_start resolves; replay then.
    // Anything else is from a stopped watch.
    else if (this.#anyStarting()) this.#pending.push(ev);
  }

  #handle(w: Watch, ev: WatchEvent) {
    if (ev.kind === "batch") {
      this.store.applyBatch(ev.payload, w.key);
      stamp();
    } else {
      const s = ev.payload;
      w.status = { namespace: w.status.namespace, state: s.state, error: "error" in s ? s.error : "" };
      this.#publishState();
    }
  }

  #anyStarting(): boolean {
    for (const w of this.#watches.values()) if (w.starting) return true;
    return false;
  }

  #publishState() {
    const kind = this.#kind;
    if (!kind || this.#watches.size === 0) return;
    const c = combineWatches(
      [...this.#watches.values()].map((w) => w.status),
      kind.label,
      kind.resource.namespaced,
    );
    setStatus(c.ok, c.text);
    if (c.error !== null) showError(c.error);
  }

  /** The watch keys for a selection: one per namespace, or `""` for all. */
  #keysFor(namespaces: readonly string[]): string[] {
    if (!this.#kind?.resource.namespaced || namespaces.length === 0) return [""];
    return [...namespaces];
  }

  /** Watch `kind` in `namespaces` (empty: all namespaces), from scratch. */
  start(kind: ResourceKind, namespaces: readonly string[]) {
    this.stop();
    this.#kind = kind;
    this.store.clear();
    for (const key of this.#keysFor(namespaces)) void this.#startWatch(key);
    this.#publishState();
    this.#startMetrics();
    this.#ageTimer = setInterval(() => (this.now = Date.now()), AGE_INTERVAL_MS);
  }

  /**
   * Change the namespaces of the running table: start watches for the added
   * ones, stop the removed ones and drop their rows. The others keep going.
   */
  setNamespaces(namespaces: readonly string[]) {
    if (!this.#kind) return;
    const want = this.#keysFor(namespaces);
    const before = [...this.#watches.keys()];
    if (before.length === want.length && before.every((k, i) => k === want[i])) return;
    for (const [key, w] of [...this.#watches]) {
      if (!want.includes(key)) {
        this.#stopWatch(w);
        this.store.removeSource(key);
      }
    }
    // Keep the map in selection order, so messages list namespaces that way.
    const kept = new Map(this.#watches);
    this.#watches.clear();
    for (const key of want) {
      const w = kept.get(key);
      if (w) this.#watches.set(key, w);
      else void this.#startWatch(key);
    }
    this.#publishState();
    this.#startMetrics();
  }

  async #startWatch(key: string) {
    const kind = this.#kind;
    if (!kind) return;
    const namespace = key || null;
    const w: Watch = {
      key,
      id: null,
      starting: true,
      status: { namespace, state: "connecting", error: "" },
      retry: null,
    };
    this.#watches.set(key, w);
    let id: string;
    try {
      id = await commands.cmdWatchStart(kind.resource, namespace);
    } catch (e) {
      if (this.#watches.get(key) !== w) return;
      w.starting = false;
      if (!this.#anyStarting()) this.#pending = [];
      const err = clusterError(e);
      w.status = { namespace, state: "failed", error: err.message, status: err.status };
      this.#publishState();
      w.retry = setTimeout(() => {
        if (this.#watches.get(key) !== w) return;
        this.#watches.delete(key);
        void this.#startWatch(key);
      }, WATCH_RETRY_MS);
      return;
    }
    if (this.#watches.get(key) !== w) {
      // Stopped (or the table restarted) while starting.
      commands.cmdWatchStop(id).catch(() => {});
      if (!this.#anyStarting()) this.#pending = [];
      return;
    }
    w.id = id;
    w.starting = false;
    this.#byId.set(id, w);
    const mine = this.#pending.filter((ev) => ev.payload.id === id);
    this.#pending = this.#anyStarting() ? this.#pending.filter((ev) => ev.payload.id !== id) : [];
    for (const ev of mine) this.#handle(w, ev);
  }

  #stopWatch(w: Watch) {
    if (w.retry) clearTimeout(w.retry);
    w.retry = null;
    if (this.#watches.get(w.key) === w) this.#watches.delete(w.key);
    if (w.id) {
      this.#byId.delete(w.id);
      commands.cmdWatchStop(w.id).catch(() => {});
    }
  }

  stop() {
    this.#token++;
    for (const w of [...this.#watches.values()]) this.#stopWatch(w);
    this.#watches.clear();
    this.#byId.clear();
    this.#pending = [];
    if (this.#metricsTimer) clearInterval(this.#metricsTimer);
    if (this.#ageTimer) clearInterval(this.#ageTimer);
    this.#metricsTimer = this.#ageTimer = null;
    this.#kind = null;
  }

  /** Only pods and nodes have metric columns; poll just the one on screen. */
  #startMetrics() {
    this.#token++;
    if (this.#metricsTimer) clearInterval(this.#metricsTimer);
    this.#metricsTimer = null;
    const kind = this.#kind;
    const what: MetricsKind | null = !kind ? null : isPod(kind) ? "pods" : isNode(kind) ? "nodes" : null;
    if (!what) return;
    this.loadMetrics(what);
    this.#metricsTimer = setInterval(() => this.loadMetrics(what), METRICS_INTERVAL_MS);
  }

  /** Metrics can't be watched: poll them and merge into the live rows. */
  async loadMetrics(what: MetricsKind) {
    const token = this.#token;
    // Pod metrics only for the namespaces the table shows, one call each.
    const scopes = what === "pods" ? [...this.#watches.keys()].map((k) => k || null) : [null];
    const results = await Promise.allSettled(scopes.map((ns) => commands.cmdGetMetrics(ns, what)));
    if (token !== this.#token) return;
    // Metrics are best effort; the tables show "—" without them.
    const snaps = results
      .filter((r): r is PromiseFulfilledResult<MetricsSnapshot> => r.status === "fulfilled")
      .map((r) => r.value);
    this.metrics = {
      pods: new Map(snaps.flatMap((s) => s.pods.map((p) => [`${p.namespace}/${p.name}`, p] as const))),
      nodes: new Map(snaps.flatMap((s) => s.nodes.map((n) => [n.name, n] as const))),
    };
    this.now = Date.now();
  }
}

export const live = new LiveTable();
