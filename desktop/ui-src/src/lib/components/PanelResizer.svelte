<script lang="ts">
  import { clampWidth, MIN_DETAIL_WIDTH } from "../settings";
  import { detailWidth, settings, updateSettings } from "../stores/settings.svelte";

  /** The drag handle on the detail panel's left edge. The panel is at the
   *  window's right edge, so its width is the distance from the pointer to
   *  there. The width is set live through `--detail-width` and saved when
   *  the drag ends. */
  let width = $state(detailWidth());
  let dragging = $state(false);

  $effect(() => {
    void settings.current.detail_width;
    if (!dragging) width = detailWidth();
  });

  function show(px: number) {
    width = clampWidth(px, window.innerWidth);
    document.documentElement.style.setProperty("--detail-width", `${width}px`);
  }

  function save() {
    if (width !== settings.current.detail_width) updateSettings({ detail_width: width });
  }

  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragging = true;
  }

  function onpointermove(e: PointerEvent) {
    if (dragging) show(window.innerWidth - e.clientX);
  }

  function onpointerup() {
    if (!dragging) return;
    dragging = false;
    save();
  }

  function onkeydown(e: KeyboardEvent) {
    const step = e.shiftKey ? 64 : 16;
    if (e.key === "ArrowLeft") show(width + step);
    else if (e.key === "ArrowRight") show(width - step);
    else return;
    e.preventDefault();
    save();
  }
</script>

<!-- A focusable separator is interactive (WAI-ARIA window splitter). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="resizer"
  class:dragging
  role="separator"
  aria-orientation="vertical"
  aria-label="Resize the detail panel"
  aria-valuenow={width}
  aria-valuemin={MIN_DETAIL_WIDTH}
  tabindex="0"
  data-testid="panel-resizer"
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  {onkeydown}
></div>

<style>
  .resizer {
    width: 5px;
    margin-left: -5px;
    flex-shrink: 0;
    cursor: col-resize;
    position: relative;
    z-index: 5;
    touch-action: none;
  }
  .resizer:hover,
  .resizer.dragging,
  .resizer:focus-visible {
    background: var(--accent);
    outline: none;
  }
</style>
