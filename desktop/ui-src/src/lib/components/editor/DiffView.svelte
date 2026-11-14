<script lang="ts">
  import { onDestroy } from "svelte";
  import type { DiffLayout } from "../../editor/setup";

  /** Read-only diff of two texts, unified or side by side (@codemirror/merge, loaded lazily). */
  let {
    original,
    modified,
    layout = "unified",
    label,
  }: {
    original: string;
    modified: string;
    layout?: DiffLayout;
    label: string;
  } = $props();

  let host: HTMLDivElement;
  let view: { destroy(): void } | null = null;
  let loadError = $state("");
  let ready = $state(false);
  /** Ignore a build that a newer one (or unmount) overtook. */
  let generation = 0;

  async function build(o: string, m: string, l: DiffLayout) {
    const gen = ++generation;
    try {
      const { createDiff } = await import("../../editor/setup");
      if (gen !== generation) return;
      view?.destroy();
      view = null;
      const v = await createDiff(host, o, m, l, label);
      if (gen !== generation) {
        v.destroy();
        return;
      }
      view = v;
      ready = true;
    } catch (e) {
      loadError = String(e);
    }
  }

  $effect(() => {
    void build(original, modified, layout);
  });

  onDestroy(() => {
    generation++;
    view?.destroy();
  });
</script>

<div class="diff" class:split={layout === "split"} bind:this={host} data-ready={ready ? "true" : "false"}>
  {#if loadError}
    <p class="error-text">Could not load the diff: {loadError}</p>
  {:else if !ready}
    <p class="muted">Loading diff…</p>
  {/if}
</div>

<style>
  .diff {
    height: 100%;
    min-height: 0;
    overflow: auto;
    background: #0d1013;
  }
  .diff > :global(.cm-editor),
  .diff > :global(.cm-mergeView) {
    min-height: 100%;
  }
  p {
    margin: 0;
    padding: 10px 16px;
    font-size: 12px;
  }
</style>
