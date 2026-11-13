<script lang="ts">
  import { app } from "../stores/app.svelte";
</script>

<footer>
  <span>{app.count}</span>
  <span>{app.updated}</span>
</footer>

<style>
  footer {
    display: flex;
    gap: 16px;
    padding: 6px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-raised);
    color: var(--text-muted);
    font-size: 12px;
    flex-shrink: 0;
  }
</style>
