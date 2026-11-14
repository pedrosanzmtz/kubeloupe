import { commands, events } from "../bindings";
import type { DetailWatchEvent, ResourceDetail } from "../bindings";
import type { Target } from "./detail.svelte";

/**
 * The detail of one object, kept up to date: a first `cmd_get_resource_detail`
 * (fast, and the place where "not found" or "forbidden" shows up), then a
 * `cmd_detail_watch` whose `detail-watch` events replace it. Both panels
 * create one on mount and `stop()` it on destroy.
 */
export class LiveDetail {
  info = $state.raw<ResourceDetail | null>(null);
  loadError = $state("");
  /** The object is gone; `info` is its last known state. */
  deleted = $state(false);
  /** The watch is failing and retrying; `info` may be stale. */
  watchError = $state("");
  /** The first load finished, successfully or not. */
  loaded = $state(false);

  #watchId: string | null = null;
  #stopped = false;
  /** A watch event arrived: it is newer than the first get. */
  #fromWatch = false;
  #buffer: DetailWatchEvent[] = [];
  #unlisten: (() => void) | null = null;

  #target: Target;

  constructor(target: Target) {
    this.#target = target;
  }

  async start() {
    const t = this.#target;
    const un = await events.detailWatch.listen((e) => this.#onEvent(e.payload));
    if (this.#stopped) return un();
    this.#unlisten = un;

    const [detail, watch] = await Promise.allSettled([
      commands.cmdGetResourceDetail(t.resource, t.namespace, t.name),
      commands.cmdDetailWatch(t.resource, t.namespace, t.name),
    ]);
    if (watch.status === "fulfilled") {
      if (this.#stopped) {
        commands.cmdWatchStop(watch.value).catch(() => {});
      } else {
        this.#watchId = watch.value;
      }
    } else {
      this.watchError = String(watch.reason);
    }
    if (this.#stopped) return;
    if (detail.status === "fulfilled") {
      if (!this.#fromWatch) this.info = detail.value;
    } else if (!this.#fromWatch) {
      this.loadError = String(detail.reason);
    }
    this.loaded = true;
    const buffered = this.#buffer.splice(0);
    if (this.#watchId !== null) buffered.forEach((ev) => this.#onEvent(ev));
  }

  #onEvent(ev: DetailWatchEvent) {
    if (this.#stopped) return;
    if (this.#watchId === null) {
      // The id is only known once cmd_detail_watch resolves; replay then.
      if (!this.loaded) this.#buffer.push(ev);
      return;
    }
    if (ev.id !== this.#watchId) return;
    this.#fromWatch = true;
    if (ev.state === "detail") {
      this.info = ev.detail;
      this.deleted = false;
      this.watchError = "";
      this.loadError = "";
    } else if (ev.state === "deleted") {
      this.deleted = true;
      this.watchError = "";
    } else {
      this.watchError = ev.error;
    }
  }

  stop() {
    this.#stopped = true;
    this.#buffer = [];
    this.#unlisten?.();
    this.#unlisten = null;
    if (this.#watchId) commands.cmdWatchStop(this.#watchId).catch(() => {});
    this.#watchId = null;
  }
}
