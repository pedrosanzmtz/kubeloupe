<script lang="ts">
  import type { Theme } from "../bindings";
  import { DEFAULT_DETAIL_WIDTH, TAIL_OPTIONS } from "../settings";
  import type { LogDefaults } from "../settings";
  import { closeSettings } from "../stores/keyboard.svelte";
  import { applyAppearance, settings, updateSettings } from "../stores/settings.svelte";

  /** The Settings page (gear in the header, Mod+, or the palette). Every
   *  change applies and is saved at once. */
  const current = $derived(settings.current);

  const THEMES: { value: Theme; label: string }[] = [
    { value: "system", label: "System" },
    { value: "dark", label: "Dark" },
    { value: "light", label: "Light" },
  ];

  function setTheme(theme: Theme) {
    updateSettings({ theme });
    applyAppearance();
  }

  function setLogs(change: Partial<LogDefaults>) {
    updateSettings({ logs: { ...current.logs, ...change } });
  }

  function resetWidth() {
    updateSettings({ detail_width: DEFAULT_DETAIL_WIDTH });
    applyAppearance();
  }

  const tailLabel = (t: string) => (t === "all" ? "All lines" : `${t} lines`);

  function focus(node: HTMLElement) {
    node.focus();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && closeSettings()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Settings" data-testid="settings">
    <div class="head">
      <h2>Settings</h2>
      <button type="button" class="btn" use:focus onclick={closeSettings}>Close</button>
    </div>

    <fieldset>
      <legend>Theme</legend>
      <div class="choices" role="radiogroup" aria-label="Theme">
        {#each THEMES as t (t.value)}
          <label class="choice">
            <input
              type="radio"
              name="theme"
              value={t.value}
              checked={current.theme === t.value}
              onchange={() => setTheme(t.value)}
            />
            {t.label}
          </label>
        {/each}
      </div>
      <p class="muted hint">System follows your desktop's light or dark preference.</p>
    </fieldset>

    <fieldset>
      <legend>Log defaults</legend>
      <label class="row">
        <span>Tail</span>
        <select
          class="select"
          aria-label="Default tail lines"
          value={TAIL_OPTIONS.includes(current.logs.tail) ? current.logs.tail : "200"}
          onchange={(e) => setLogs({ tail: e.currentTarget.value })}
        >
          {#each TAIL_OPTIONS as t (t)}
            <option value={t}>{tailLabel(t)}</option>
          {/each}
        </select>
      </label>
      <label class="choice">
        <input
          type="checkbox"
          checked={current.logs.timestamps}
          onchange={(e) => setLogs({ timestamps: e.currentTarget.checked })}
        />
        Show timestamps
      </label>
      <label class="choice">
        <input type="checkbox" checked={current.logs.wrap} onchange={(e) => setLogs({ wrap: e.currentTarget.checked })} />
        Wrap long lines
      </label>
    </fieldset>

    <fieldset>
      <legend>Layout</legend>
      <div class="row">
        <span>Detail panel width: {current.detail_width}px</span>
        <button type="button" class="btn" disabled={current.detail_width === DEFAULT_DETAIL_WIDTH} onclick={resetWidth}
          >Reset</button
        >
      </div>
      <p class="muted hint">
        Drag the panel's left edge to resize it. The last context, view and namespaces, and each table's sort and
        columns, are remembered too.
      </p>
    </fieldset>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    z-index: 120;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .dialog {
    width: min(520px, calc(100vw - 32px));
    max-height: calc(100vh - 64px);
    overflow-y: auto;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 16px 20px;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  fieldset {
    border: none;
    padding: 0;
    margin: 16px 0 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  legend {
    padding: 0;
    margin-bottom: 8px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }
  .choices {
    display: flex;
    gap: 16px;
  }
  .choice,
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .row {
    justify-content: space-between;
  }
  .hint {
    margin: 0;
    font-size: 12px;
  }
</style>
