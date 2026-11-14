<script lang="ts">
  import { app } from "../stores/app.svelte";
</script>

{#if app.error}
  <div class="banner" role="alert">{app.error}</div>
{/if}

<style>
  .banner {
    margin: 12px 16px 0;
    padding: 10px 12px;
    background: rgba(229, 72, 77, 0.15);
    border: 1px solid var(--red);
    border-radius: 6px;
    color: var(--error-text);
  }
</style>
