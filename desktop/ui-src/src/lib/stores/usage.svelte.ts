import { commands } from "../bindings";
import type { MetricsKind, UsageHistoryView } from "../bindings";

/** Same pace as the table's metrics poll (stores/live.svelte.ts). */
export const USAGE_INTERVAL_MS = 20_000;

/**
 * The usage history of one pod or node (`cmd_usage_history`), for the charts
 * in its detail panel. Polled while the panel is open; the backend samples
 * the object itself when no table did lately, so history keeps growing.
 * The panel calls `stop()` in `onDestroy`.
 */
export class UsageFeed {
  data = $state.raw<UsageHistoryView | null>(null);
  error = $state("");

  #what: MetricsKind;
  #namespace: string | null;
  #name: string;
  #timer: ReturnType<typeof setInterval> | null = null;
  #stopped = false;

  constructor(what: MetricsKind, namespace: string | null, name: string) {
    this.#what = what;
    this.#namespace = namespace;
    this.#name = name;
  }

  start() {
    void this.load();
    this.#timer = setInterval(() => void this.load(), USAGE_INTERVAL_MS);
  }

  async load() {
    try {
      const data = await commands.cmdUsageHistory(this.#what, this.#namespace, this.#name);
      if (this.#stopped) return;
      this.data = data;
      this.error = "";
    } catch (e) {
      if (!this.#stopped) this.error = String(e);
    }
  }

  stop() {
    this.#stopped = true;
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = null;
  }
}
