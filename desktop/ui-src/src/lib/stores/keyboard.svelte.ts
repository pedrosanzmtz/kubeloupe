// The global keyboard handler: turns a keydown into a command from the
// keymap (`lib/keys.ts`) and runs it on whatever is on screen. Components
// with keyboard behaviour (the resource table, the detail tabs) register a
// handle here while mounted.

import type { ResourceKind } from "../bindings";
import type { ActionTarget } from "../actions";
import type { Row } from "../columns";
import { bindingFor, inEditorOrTerminal, isActivatable, isMacPlatform, isTextEntry } from "../keys";
import { actionUi } from "./actions.svelte";
import { createUi } from "./create.svelte";
import { closeDetail, detail } from "./detail.svelte";
import { toggleDock } from "./dock.svelte";
import { unsaved } from "./unsaved.svelte";

export const keyboard = $state({
  palette: false,
  help: false,
  settings: false,
  /** Context menus open (they handle their own keys meanwhile). */
  menus: 0,
});

export type RowMove = "next" | "prev" | "first" | "last";

/** What the keyboard can do with the resource table on screen. */
export interface TableHandle {
  kind: ResourceKind;
  move(to: RowMove): void;
  openFocused(): void;
  toggleFocused(): void;
  menuForFocused(): void;
  deleteRows(): void;
  /** Every row of the table (not only the filtered ones), for the palette. */
  rows(): Row[];
  /** The checked rows, else the focused row, else the one in the detail panel. */
  actionTargets(): { targets: ActionTarget[]; row: Row | null };
  /** Give the table the keyboard focus (when a dialog or the palette closes). */
  focus(): void;
}

export interface TabsHandle {
  count(): number;
  select(index: number): void;
}

let table: TableHandle | null = null;
let tabs: TabsHandle | null = null;

/** Register the table on screen; returns the unregister function. */
export function registerTable(handle: TableHandle): () => void {
  table = handle;
  return () => {
    if (table === handle) table = null;
  };
}

export function activeTable(): TableHandle | null {
  return table;
}

/** Register the detail panel's tabs (Ctrl/Cmd+1…9); returns the unregister function. */
export function registerTabs(handle: TabsHandle): () => void {
  tabs = handle;
  return () => {
    if (tabs === handle) tabs = null;
  };
}

export function openPalette() {
  keyboard.help = false;
  keyboard.palette = true;
}

export function closePalette() {
  keyboard.palette = false;
}

export function openHelp() {
  keyboard.palette = false;
  keyboard.help = true;
}

export function closeHelp() {
  keyboard.help = false;
}

export function openSettings() {
  keyboard.palette = false;
  keyboard.help = false;
  keyboard.settings = true;
}

export function closeSettings() {
  keyboard.settings = false;
}

/** A menu or a dialog has the keyboard (each handles its own Esc). */
function modalOpen(): boolean {
  return keyboard.menus > 0 || actionUi.request !== null || createUi.open || unsaved.asking;
}

const mac = isMacPlatform();

/**
 * The window's keydown handler (capture phase, so it sees the state before
 * a menu or dialog reacts to the same key).
 */
export function handleKey(e: KeyboardEvent) {
  if (e.defaultPrevented || e.isComposing) return;
  const hit = bindingFor(e, mac);
  if (!hit) return;
  const target = e.target instanceof Element ? e.target : document.activeElement;
  if (inEditorOrTerminal(target) && !(hit.binding.inTerminal && target?.closest(".xterm"))) return;
  if (isTextEntry(target) && !hit.binding.inInputs) return;
  if (modalOpen()) return;

  const command = hit.binding.command;
  // The palette and the help overlay sit on top: they take only their own keys.
  if (keyboard.palette) {
    if (command === "palette") done(e, closePalette);
    return;
  }
  if (keyboard.help) {
    if (command === "escape" || command === "help") done(e, closeHelp);
    return;
  }
  if (keyboard.settings) {
    if (command === "escape" || command === "settings") done(e, closeSettings);
    return;
  }

  switch (command) {
    case "palette":
      return done(e, openPalette);
    case "help":
      return done(e, openHelp);
    case "settings":
      return done(e, openSettings);
    case "escape":
      if (detail.target) done(e, closeDetail);
      return;
    case "focusFilter": {
      const filter = document.getElementById("table-filter");
      if (filter) done(e, () => filter.focus());
      return;
    }
    case "rowDown":
    case "rowUp":
    case "rowFirst":
    case "rowLast": {
      const to = ({ rowDown: "next", rowUp: "prev", rowFirst: "first", rowLast: "last" } as const)[command];
      if (table) done(e, () => table?.move(to));
      return;
    }
    case "openRow":
    case "toggleRow":
      // Enter and Space keep their meaning on buttons, checkboxes and links.
      if (!table || isActivatable(target)) return;
      return done(e, () => (command === "openRow" ? table?.openFocused() : table?.toggleFocused()));
    case "rowMenu":
      if (table) done(e, () => table?.menuForFocused());
      return;
    case "deleteRows":
      if (table) done(e, () => table?.deleteRows());
      return;
    case "toggleDock":
      return done(e, toggleDock);
    case "detailTab":
      if (tabs && hit.index < tabs.count()) done(e, () => tabs?.select(hit.index));
      return;
  }
}

function done(e: KeyboardEvent, fn: () => void) {
  e.preventDefault();
  e.stopPropagation();
  fn();
}
