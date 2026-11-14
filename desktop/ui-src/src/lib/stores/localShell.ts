// "New local terminal": a dock tab with a shell on this machine, pointed at
// the app's context and namespace as they are now.

import { localShellNamespace } from "../dock";
import { app } from "./app.svelte";
import { openDockTab } from "./dock.svelte";

let seq = 0;

export function openLocalShell() {
  const ctx = app.contexts.find((c) => c.current);
  openDockTab({
    type: "localShell",
    seq: ++seq,
    context: ctx?.name ?? "",
    namespace: localShellNamespace(app.selectedNamespaces, ctx?.namespace),
  });
}
