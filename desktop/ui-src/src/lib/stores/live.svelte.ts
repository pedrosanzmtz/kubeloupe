import { commands, events } from "../bindings";
import type { WatchBatch, WatchStateChanged } from "../bindings";
import type { ResourceKind } from "../bindings";
import { EMPTY_METRICS, isNode, isPod } from "../columns";
import type { Metrics, Row } from "../columns";
import { setStatus, showError, stamp } from "./app.svelte";
import { KeyedStore } from "./keyed.svelte";

const METRICS_INTERVAL_MS = 20_000;
const AGE_INTERVAL_MS = 15_000;
const WATCH_RETRY_MS = 5_000;

type WatchEvent = { kind: "batch"; payload: WatchBatch } | { kind: "state"; payload: WatchStateChanged };

/**
 * The live table behind every resource view.
 *
 * The backend watches the cluster and pushes coalesced `watch-batch` events
 * ({snapshot, applied, deleted}); they land in a keyed store. Metrics cannot
 * be watched, so they are polled and merged at render time, and ages are
 * recomputed from `created` whenever `now` ticks.
 */
export class LiveTable {
  store = new KeyedStore<Row>((row) => row.key);
  metrics = $state.raw<Metrics>(EMPTY_METRICS);
  now = $state.raw(Date.now());

  #watchId: string | null = null;
  /** Bumped on every start/stop to invalidate in-flight starts. */
  #token = 0;
  #starting = false;
  /** Events that arrived before `cmd_watch_start` resolved. */
  #buffer: WatchEvent[] = [];
  #retryTimer: ReturnType<typeof setTimeout> | null = null;
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
    if (this.#watchId === null) {
      // The id is only known once cmd_watch_start resolves; replay then.
      if (this.#starting) this.#buffer.push(ev);
      return;
    }
    if (ev.payload.id !== this.#watchId) return; // a stopped watch
    if (ev.kind === "batch") {
      this.store.applyBatch(ev.payload);
      stamp();
    } else {
      this.#setWatchState(ev.payload);
    }
  }

  #setWatchState(ws: WatchStateChanged) {
    if (ws.state === "live") {
      setStatus(true, "Live");
      showError("");
    } else if (ws.state === "reconnecting") {
      setStatus(false, "Reconnecting…");
      showError("Watch interrupted, retrying: " + ws.error);
    } else {
      setStatus(null, "Connecting…");
    }
  }

  async start(kind: ResourceKind, namespace: string | null) {
    this.stop();
    const token = ++this.#token;
    this.#starting = true;
    this.store.clear();
    this.#setWatchState({ id: "", state: "connecting" });
    let id: string;
    try {
      id = await commands.cmdWatchStart(
        kind.resource,
        kind.resource.namespaced ? namespace || null : null,
      );
    } catch (e) {
      if (token !== this.#token) return;
      this.#starting = false;
      setStatus(false, "Cluster unreachable");
      showError("Could not reach the cluster: " + e);
      this.#retryTimer = setTimeout(() => this.start(kind, namespace), WATCH_RETRY_MS);
      return;
    }
    if (token !== this.#token) {
      commands.cmdWatchStop(id).catch(() => {});
      return;
    }
    this.#watchId = id;
    this.#starting = false;
    for (const ev of this.#buffer.splice(0)) this.#onEvent(ev);
    if (isPod(kind) || isNode(kind)) {
      // Only pods and nodes have metric columns.
      this.loadMetrics();
      this.#metricsTimer = setInterval(() => this.loadMetrics(), METRICS_INTERVAL_MS);
    }
    this.#ageTimer = setInterval(() => (this.now = Date.now()), AGE_INTERVAL_MS);
  }

  stop() {
    this.#token++;
    this.#starting = false;
    this.#buffer = [];
    if (this.#retryTimer) clearTimeout(this.#retryTimer);
    if (this.#metricsTimer) clearInterval(this.#metricsTimer);
    if (this.#ageTimer) clearInterval(this.#ageTimer);
    this.#retryTimer = this.#metricsTimer = this.#ageTimer = null;
    const id = this.#watchId;
    this.#watchId = null;
    if (id) commands.cmdWatchStop(id).catch(() => {});
  }

  /** Metrics can't be watched: poll them and merge into the live rows. */
  async loadMetrics() {
    const token = this.#token;
    let next: Metrics = EMPTY_METRICS;
    try {
      const snap = await commands.cmdGetMetrics();
      if (token !== this.#token) return;
      next = {
        pods: new Map(snap.pods.map((p) => [`${p.namespace}/${p.name}`, p])),
        nodes: new Map(snap.nodes.map((n) => [n.name, n])),
      };
    } catch {
      // Metrics are best effort; the tables show "—" without them.
      if (token !== this.#token) return;
    }
    this.metrics = next;
    this.now = Date.now();
  }
}

export const live = new LiveTable();
