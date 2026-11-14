<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import type { EditorHandle } from "../../editor/setup";

  /**
   * A YAML code editor (CodeMirror 6). Generic: it knows nothing about
   * Kubernetes. `value` is the text; when it changes from outside (a
   * reload) the editor shows the new text, and every edit calls `onChange`.
   * CodeMirror is loaded on first mount, as its own chunk.
   */
  let {
    value,
    readonly = false,
    label,
    onChange = () => {},
    markLine = null,
    testid,
  }: {
    value: string;
    readonly?: boolean;
    /** Accessible name of the text area. */
    label: string;
    onChange?: (value: string) => void;
    /** A 1-based line to highlight (an error location), or null. */
    markLine?: number | null;
    /** `data-testid` of the root element. */
    testid?: string;
  } = $props();

  let host: HTMLDivElement;
  let editor = $state.raw<EditorHandle | null>(null);
  let loadError = $state("");
  let destroyed = false;

  /** The current text (the same as the last `onChange`). */
  export function getValue(): string {
    return editor ? editor.getValue() : value;
  }

  onMount(async () => {
    try {
      const { createEditor } = await import("../../editor/setup");
      if (destroyed) return;
      const handle = createEditor(host, { value, readonly, label, onChange: (v) => onChange(v) });
      editor = handle;
      // For the real-app E2E tests (WebDriver can't type reliably into
      // CodeMirror): `el.kubeloupeEditor.setValue(text)` on the root element
      // edits like typing does, `onChange` included.
      (host as HTMLDivElement & { kubeloupeEditor?: unknown }).kubeloupeEditor = {
        getValue: () => handle.getValue(),
        setValue: (v: string) => handle.setValue(v),
      };
    } catch (e) {
      loadError = String(e);
    }
  });

  onDestroy(() => {
    destroyed = true;
    editor?.destroy();
  });

  $effect(() => {
    editor?.setValue(value);
  });
  $effect(() => {
    editor?.setReadonly(readonly);
  });
  $effect(() => {
    editor?.markLine(markLine);
  });
</script>

<div class="code-editor" bind:this={host} data-ready={editor ? "true" : "false"} data-testid={testid}>
  {#if !editor}
    {#if loadError}
      <p class="error-text">Could not load the editor: {loadError}</p>
    {:else}
      <pre class="placeholder" aria-busy="true">{value}</pre>
    {/if}
  {/if}
</div>

<style>
  .code-editor {
    height: 100%;
    min-height: 0;
    background: #0d1013;
  }
  .placeholder {
    margin: 0;
    padding: 4px 8px 4px 40px;
    color: #d6dae0;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre;
    overflow: hidden;
  }
  .error-text {
    margin: 0;
    padding: 10px 16px;
    font-size: 12px;
  }
</style>
