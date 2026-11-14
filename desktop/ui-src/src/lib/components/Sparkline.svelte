<script lang="ts">
  import { sparkPoints } from "../usage";

  /** A tiny trend line for a table cell (#47). Decorative: the cell's text
   *  says the current value. Nothing is drawn for fewer than two points. */
  let { values }: { values: readonly number[] } = $props();
  const W = 32;
  const H = 12;
  const points = $derived(values.length > 1 ? sparkPoints(values, W, H) : "");
</script>

{#if points}
  <svg class="spark" data-testid="sparkline" width={W} height={H} viewBox="-1 -1 {W + 2} {H + 2}" aria-hidden="true">
    <polyline {points} />
  </svg>
{/if}

<style>
  .spark {
    vertical-align: middle;
    margin-left: 6px;
    overflow: visible;
  }
  polyline {
    fill: none;
    stroke: var(--accent);
    stroke-width: 1.2;
    stroke-linejoin: round;
    vector-effect: non-scaling-stroke;
  }
</style>
