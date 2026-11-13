<script lang="ts">
  import { onMount } from "svelte";
  import { commands } from "../bindings";
  import type { PodRef } from "../stores/detail.svelte";

  let { pod }: { pod: PodRef } = $props();

  let text = $state("");
  let status = $state("");

  async function load() {
    text = "Loading…";
    status = "";
    try {
      text = await commands.cmdGetPodYaml(pod.namespace, pod.name);
    } catch (e) {
      text = "";
      status = "Could not load YAML: " + e;
    }
  }

  async function save() {
    status = "Saving…";
    try {
      await commands.cmdApplyPodYaml(pod.namespace, pod.name, text);
      status = "Saved.";
      await load();
      status = "Saved.";
    } catch (e) {
      status = "Could not save YAML: " + e;
    }
  }

  onMount(load);
</script>

<div class="section-header">
  <h3>YAML</h3>
  <button type="button" class="btn" onclick={load}>Reload</button>
  <button type="button" class="btn primary" onclick={save}>Save</button>
</div>
<textarea spellcheck="false" aria-label="Pod YAML" bind:value={text}></textarea>
<p class="detail-status muted">{status}</p>

<style>
  textarea {
    width: 100%;
    min-height: 220px;
    margin: 0;
    padding: 10px 16px;
    background: #0d1013;
    color: #d6dae0;
    border: none;
    border-top: 1px solid var(--border);
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
  }
</style>
