import { commands } from "../bindings";
import type { Related, ResourceDetail } from "../bindings";
import type { Target } from "./detail.svelte";

/** How long after a change of the object its related objects are reloaded;
 *  changes within this window share one reload. */
export const RELATED_DELAY_MS = 500;

/**
 * The summary and related objects of one object (`cmd_get_related`), for the
 * detail panels. Loaded on `refresh()`, and again (debounced) when `seen()`
 * gets a detail that differs from the last one: the panels pass every detail
 * of their live detail watch, so the related data follows the object.
 */
export class LiveRelated {
  data = $state.raw<Related | null>(null);
  error = $state("");
  loading = $state(false);

  #target: Target;
  #stopped = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #inFlight = false;
  /** A refresh was asked for while one was running. */
  #again = false;
  /** The last detail passed to `seen()`, as JSON. */
  #lastDetail = "";

  constructor(target: Target) {
    this.#target = target;
  }

  async refresh() {
    if (this.#stopped) return;
    if (this.#inFlight) {
      this.#again = true;
      return;
    }
    this.#inFlight = true;
    this.loading = true;
    const t = this.#target;
    try {
      const data = await commands.cmdGetRelated(t.resource, t.namespace, t.name);
      if (!this.#stopped) {
        this.data = data;
        this.error = "";
      }
    } catch (e) {
      if (!this.#stopped) this.error = String(e);
    } finally {
      this.#inFlight = false;
      this.loading = false;
    }
    if (this.#again) {
      this.#again = false;
      void this.refresh();
    }
  }

  /** A detail of the object. The first one only records it (the panel loads
   *  on mount); a different one later means the object changed. */
  seen(detail: ResourceDetail) {
    const json = JSON.stringify(detail);
    const first = this.#lastDetail === "";
    if (json === this.#lastDetail) return;
    this.#lastDetail = json;
    if (!first) this.changed();
  }

  /** The object changed: reload soon. */
  changed() {
    if (this.#stopped || this.#timer !== undefined) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      void this.refresh();
    }, RELATED_DELAY_MS);
  }

  stop() {
    this.#stopped = true;
    clearTimeout(this.#timer);
    this.#timer = undefined;
  }
}
