// The bottom dock: log streams and terminals in tabs. Tabs outlive the
// detail panel and view changes; a context switch closes them all.

import { DOCK_DEFAULT_HEIGHT, closeTab as closeIn, openTab as openIn } from "../dock";
import type { DockTab, DockTabSpec } from "../dock";

export const dock = $state({
  /** A handful at most (the backend caps log streams at 10). */
  tabs: [] as DockTab[],
  active: null as string | null,
  /** False: only the tab strip shows (the tabs keep running). */
  expanded: true,
  /** The dock fills the window, the tables and panel are hidden. */
  maximized: false,
  height: DOCK_DEFAULT_HEIGHT,
});

/** Open a tab for `spec`, or focus it if it is already open. */
export function openDockTab(spec: DockTabSpec) {
  const next = openIn(dock.tabs, spec);
  if (next.tabs !== dock.tabs) dock.tabs = next.tabs;
  dock.active = next.active;
  dock.expanded = true;
}

export function activateDockTab(id: string) {
  dock.active = id;
  dock.expanded = true;
}

/** Close a tab; its component stops the stream or session it owns. */
export function closeDockTab(id: string) {
  const next = closeIn(dock.tabs, dock.active, id);
  dock.tabs = next.tabs;
  dock.active = next.active;
  if (dock.tabs.length === 0) dock.maximized = false;
}

export function closeAllDockTabs() {
  dock.tabs = [];
  dock.active = null;
  dock.maximized = false;
}

/** Ctrl+`: show or hide the dock's contents. */
export function toggleDock() {
  if (dock.tabs.length === 0) return;
  dock.expanded = !dock.expanded;
  if (!dock.expanded) dock.maximized = false;
}

export function toggleMaximized() {
  dock.maximized = !dock.maximized;
  if (dock.maximized) dock.expanded = true;
}
