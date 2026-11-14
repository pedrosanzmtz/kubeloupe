<script lang="ts">
  import { onMount } from "svelte";
  import { keyboard } from "../stores/keyboard.svelte";

  export interface MenuItem {
    label: string;
    danger?: boolean;
    run: () => void;
  }

  interface Props {
    x: number;
    y: number;
    /** Shown above the items: which object(s) the menu is about. */
    title: string;
    items: MenuItem[];
    onclose: () => void;
  }
  let { x, y, title, items, onclose }: Props = $props();

  let el = $state<HTMLDivElement | null>(null);
  let left = $state(0);
  let top = $state(0);

  // Keep the menu inside the window.
  $effect(() => {
    const w = el?.offsetWidth ?? 0;
    const h = el?.offsetHeight ?? 0;
    left = Math.max(4, Math.min(x, window.innerWidth - w - 4));
    top = Math.max(4, Math.min(y, window.innerHeight - h - 4));
  });

  // While open, the menu has the keyboard: focus starts on the first item
  // and goes back where it was (the table) when the menu closes.
  onMount(() => {
    const before = document.activeElement as HTMLElement | null;
    keyboard.menus += 1;
    el?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    return () => {
      keyboard.menus -= 1;
      if (before?.isConnected) before.focus({ preventScroll: true });
    };
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      onclose();
      return;
    }
    const all = [...(el?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    if (all.length === 0) return;
    const at = all.indexOf(document.activeElement as HTMLElement);
    const go = ({ ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: all.length - 1 } as Record<string, number>)[
      e.key
    ];
    if (go === undefined) return;
    e.preventDefault();
    all[(go + all.length) % all.length].focus();
  }
</script>

<svelte:window {onkeydown} onresize={onclose} onblur={onclose} />
<!-- A click or right-click anywhere else dismisses the menu. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="shield" onclick={onclose} oncontextmenu={(e) => (e.preventDefault(), onclose())}></div>
<div class="menu" role="menu" data-testid="context-menu" bind:this={el} style:left="{left}px" style:top="{top}px">
  <div class="title" title={title}>{title}</div>
  {#each items as item (item.label)}
    <button
      type="button"
      role="menuitem"
      tabindex="-1"
      class:danger={item.danger}
      onclick={() => {
        onclose();
        item.run();
      }}>{item.label}</button
    >
  {/each}
</div>

<style>
  .shield {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .menu {
    position: fixed;
    z-index: 91;
    min-width: 170px;
    max-width: 280px;
    padding: 4px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
  }
  .title {
    padding: 4px 10px 6px;
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-bottom: 1px solid var(--border);
    margin-bottom: 4px;
  }
  button {
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text);
    padding: 6px 10px;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  button:hover,
  button:focus-visible {
    background: var(--bg-hover);
    outline: none;
  }
  button.danger {
    color: var(--error-text);
  }
</style>
