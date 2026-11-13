<script lang="ts">
  import { untrack } from "svelte";
  import type { Snippet } from "svelte";
  import { get } from "svelte/store";
  import { createVirtualizer } from "@tanstack/svelte-virtual";

  interface Props {
    count: number;
    /** Row height in px (the estimate when `dynamic`). */
    rowHeight?: number;
    /**
     * Stable key of row `i`. Pass a new function whenever the data behind it
     * changes so the virtualizer re-reads the keys.
     */
    getKey: (index: number) => string | number;
    row: Snippet<[number]>;
    /** Measure rows instead of assuming `rowHeight` (wrapped log lines). */
    dynamic?: boolean;
    overscan?: number;
    /** Allow horizontal scrolling (unwrapped log lines). */
    scrollX?: boolean;
    scrollEl?: HTMLDivElement | null;
    onscroll?: (e: Event) => void;
  }

  let {
    count,
    rowHeight = 28,
    getKey,
    row,
    dynamic = false,
    overscan = 10,
    scrollX = false,
    scrollEl = $bindable(null),
    onscroll,
  }: Props = $props();

  const virtualizer = createVirtualizer<HTMLDivElement, HTMLElement>({
    count: 0,
    getScrollElement: () => scrollEl,
    estimateSize: () => rowHeight,
  });

  // Push the changed count / keys into the virtualizer. `get()` + `untrack`
  // keep this effect from re-triggering itself through the store.
  $effect(() => {
    const c = count;
    const keyFn = getKey;
    const over = overscan;
    void scrollEl;
    untrack(() => get(virtualizer).setOptions({ count: c, getItemKey: keyFn, overscan: over }));
  });

  function measure(node: HTMLElement) {
    if (dynamic) get(virtualizer).measureElement(node);
  }

  export function scrollToBottom() {
    if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
  }

  export function scrollToTop() {
    if (scrollEl) scrollEl.scrollTop = 0;
  }
</script>

<div class="scroller" class:scroll-x={scrollX} bind:this={scrollEl} {onscroll}>
  <div class="sizer" style:height="{$virtualizer.getTotalSize()}px">
    {#each $virtualizer.getVirtualItems() as vi (vi.key)}
      <div
        class="vrow"
        data-index={vi.index}
        style:transform="translateY({vi.start}px)"
        style:height={dynamic ? undefined : `${vi.size}px`}
        use:measure
      >
        {@render row(vi.index)}
      </div>
    {/each}
  </div>
</div>

<style>
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-gutter: stable;
    position: relative;
  }
  .scroller.scroll-x {
    overflow-x: auto;
  }
  .sizer {
    position: relative;
    width: 100%;
  }
  .vrow {
    position: absolute;
    top: 0;
    left: 0;
    width: 100%;
  }
</style>
