<script lang="ts">
  import { tick, untrack } from "svelte";
  import { app, setNamespaces } from "../stores/app.svelte";
  import { keyboard } from "../stores/keyboard.svelte";
  import { filterNamespaces, MAX_NAMESPACES, namespacesLabel, toggleNamespace } from "../namespaces";

  // The namespace control: a button that opens a searchable checkbox list,
  // and the picked namespaces as removable chips. When namespaces cannot be
  // listed it offers the known ones and takes typed names.

  let open = $state(false);
  let query = $state("");
  let root: HTMLDivElement | undefined = $state();
  let trigger: HTMLButtonElement | undefined = $state();
  let search: HTMLInputElement | undefined = $state();

  const selected = $derived(app.selectedNamespaces);
  const full = $derived(selected.length >= MAX_NAMESPACES);
  const options = $derived.by(() => {
    if (!app.namespacesForbidden) return app.namespaces.map((n) => n.name);
    const known = [...app.knownNamespaces];
    for (const ns of selected) if (!known.includes(ns)) known.push(ns);
    return known.sort();
  });
  const shown = $derived(filterNamespaces(options, query));
  const typed = $derived(query.trim());
  /** A typed namespace that is not offered yet (free text, when not listable). */
  const addable = $derived(app.namespacesForbidden && typed !== "" && !options.includes(typed));

  // While open it handles its own keys (Esc, arrows), like a context menu.
  $effect(() => {
    if (!open) return;
    untrack(() => (keyboard.menus += 1));
    return () => {
      untrack(() => (keyboard.menus -= 1));
    };
  });

  async function show() {
    open = true;
    await tick();
    search?.focus();
  }

  function hide(focusTrigger = false) {
    open = false;
    query = "";
    if (focusTrigger) trigger?.focus();
  }

  function toggle(ns: string) {
    setNamespaces(toggleNamespace(selected, ns));
  }

  function addTyped() {
    if (!full && !selected.includes(typed)) setNamespaces([...selected, typed]);
    query = "";
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key !== "Enter") return;
    e.preventDefault();
    if (addable) addTyped();
    else if (options.includes(typed)) toggle(typed);
    else if (shown.length === 1) toggle(shown[0]);
    else return;
    query = "";
  }

  /** Escape closes; the arrows move between the search box and the options. */
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      hide(true);
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...(root?.querySelectorAll<HTMLElement>(".popover input:not(:disabled), .popover button") ?? [])];
    const i = items.indexOf(document.activeElement as HTMLElement);
    const next = items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
    if (next) {
      e.preventDefault();
      next.focus();
    }
  }

  function onOutside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) hide();
  }

  function onFocusOut(e: FocusEvent) {
    if (open && root && !root.contains(e.relatedTarget as Node | null) && e.relatedTarget) hide();
  }
</script>

<svelte:window onpointerdown={onOutside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="picker" bind:this={root} onkeydown={onKey} onfocusout={onFocusOut}>
  <button
    type="button"
    class="select trigger"
    aria-label="Namespace"
    aria-haspopup="dialog"
    aria-expanded={open}
    title={app.namespacesForbidden ? "You may not list namespaces: type the ones you have access to" : undefined}
    bind:this={trigger}
    onclick={() => (open ? hide() : show())}>{namespacesLabel(selected)}</button
  >
  {#if open}
    <div class="popover" role="dialog" aria-label="Namespaces">
      <input
        class="input"
        type="search"
        placeholder={app.namespacesForbidden ? "Type a namespace…" : "Search…"}
        aria-label="Search namespaces"
        bind:this={search}
        bind:value={query}
        onkeydown={onSearchKey}
      />
      <label class="option all">
        <input
          type="checkbox"
          checked={selected.length === 0}
          onchange={(e) => {
            setNamespaces([]);
            e.currentTarget.checked = true;
          }}
        />
        All namespaces
      </label>
      <div class="options" role="group" aria-label="Namespace list">
        {#each shown as ns (ns)}
          {@const on = selected.includes(ns)}
          <label class="option" class:off={!on && full}>
            <input type="checkbox" checked={on} disabled={!on && full} onchange={() => toggle(ns)} />
            {ns}
          </label>
        {:else}
          {#if !addable}<p class="muted empty">No matching namespace.</p>{/if}
        {/each}
      </div>
      {#if addable}
        <button type="button" class="btn add" disabled={full} onclick={addTyped}>Add “{typed}”</button>
      {/if}
      {#if full}
        <p class="muted note">At most {MAX_NAMESPACES} namespaces. Choose All namespaces to see more.</p>
      {/if}
    </div>
  {/if}
  {#if selected.length > 1}
    <ul class="chips" aria-label="Selected namespaces">
      {#each selected as ns (ns)}
        <li class="chip">
          {ns}<button type="button" aria-label="Remove {ns}" title="Stop showing {ns}" onclick={() => toggle(ns)}
            >×</button
          >
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex-wrap: wrap;
  }
  .trigger {
    min-width: 160px;
    text-align: left;
  }
  .popover {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 20;
    width: 260px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
  }
  .options {
    max-height: 260px;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .option {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
  }
  .option:hover {
    background: var(--bg-hover);
  }
  .option.all {
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    padding-bottom: 8px;
  }
  .option.off {
    color: var(--text-muted);
    cursor: default;
  }
  .empty,
  .note {
    margin: 4px 6px;
    font-size: 12px;
  }
  .add {
    align-self: flex-start;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 8px;
    border: 1px solid var(--border);
    border-radius: 12px;
    font-size: 12px;
  }
  .chip button {
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0 4px;
    font-size: 13px;
  }
  .chip button:hover {
    color: var(--text);
  }
</style>
