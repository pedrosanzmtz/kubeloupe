// The command palette's entries, built from the app's state each time it
// opens: commands, actions for the selected objects (#23's registry), kinds,
// namespaces, contexts and the objects of the current table.

import { replicasFromRow, targetLabel } from "../actions";
import type { PaletteItem } from "../palette";
import { pushRecent } from "../palette";
import { PORT_FORWARDS_VIEW } from "../forwards";
import { app, canOpenDenied, kinds, refresh, setView, switchContext, switchNamespace } from "./app.svelte";
import { requestAction } from "./actions.svelte";
import { openCreate } from "./create.svelte";
import { openDetail } from "./detail.svelte";
import { activeTable, openHelp } from "./keyboard.svelte";
import { whenDiscarded } from "./unsaved.svelte";

/** Ids of recently run entries, newest first (this session). */
let recent: string[] = [];

export function recentIds(): string[] {
  return recent;
}

export function rememberPick(id: string) {
  recent = pushRecent(recent, id);
}

export function paletteItems(): PaletteItem[] {
  const items: PaletteItem[] = [
    { id: "cmd:create", group: "Command", label: "Create resource…", terms: ["new", "apply", "yaml"], run: openCreate },
    { id: "cmd:overview", group: "Command", label: "Go to Overview", run: () => setView("overview") },
    {
      id: "cmd:port-forwards",
      group: "Command",
      label: "Go to Port Forwards",
      terms: ["forward", "tunnel"],
      run: () => setView(PORT_FORWARDS_VIEW),
    },
    { id: "cmd:refresh", group: "Command", label: "Refresh", terms: ["reload"], run: refresh },
    { id: "cmd:help", group: "Command", label: "Keyboard shortcuts", terms: ["help", "keys"], run: openHelp },
  ];

  const table = activeTable();
  if (table) {
    const kind = table.kind;
    const { targets, row } = table.actionTargets();
    if (targets.length > 1) {
      const del = kind.actions.find((a) => a.id === "delete");
      if (del) {
        items.push({
          id: "action:delete",
          group: "Action",
          label: `Delete ${targets.length} selected…`,
          detail: kind.label,
          run: () => requestAction(kind, targets, del),
        });
      }
    } else if (targets.length === 1) {
      const keys = kind.columns.map((c) => c.key);
      for (const a of kind.actions) {
        items.push({
          id: `action:${a.id}`,
          group: "Action",
          label: a.label,
          detail: targetLabel(targets[0]),
          run: () => requestAction(kind, targets, a, replicasFromRow(row ?? undefined, keys)),
        });
      }
    }
  }

  for (const k of kinds.all) {
    if (kinds.denied.has(k.id) && !canOpenDenied(k)) continue;
    items.push({
      id: `kind:${k.id}`,
      group: "Kind",
      label: k.label,
      detail: k.resource.group || undefined,
      terms: [k.resource.plural, k.resource.kind, ...k.short_names],
      run: () => setView(k.id),
    });
  }

  // Picking a namespace shows only that one (the toolbar control adds more).
  const namespaces = app.namespacesForbidden ? app.knownNamespaces : app.namespaces.map((n) => n.name);
  items.push({
    id: "ns:",
    group: "Namespace",
    label: "All namespaces",
    detail: app.selectedNamespaces.length === 0 ? "current" : undefined,
    run: () => switchNamespace(""),
  });
  for (const ns of namespaces) {
    items.push({
      id: `ns:${ns}`,
      group: "Namespace",
      label: ns,
      detail: app.selectedNamespaces.includes(ns) ? "current" : undefined,
      run: () => switchNamespace(ns),
    });
  }

  for (const c of app.contexts) {
    items.push({
      id: `ctx:${c.name}`,
      group: "Context",
      label: c.name,
      detail: c.current ? "current" : undefined,
      run: () => whenDiscarded(() => void switchContext(c.name)),
    });
  }

  if (table) {
    const kind = table.kind;
    for (const r of table.rows()) {
      items.push({
        id: `obj:${kind.id}:${r.key}`,
        group: "Object",
        label: `${kind.resource.plural}/${r.name}`,
        detail: r.namespace || undefined,
        terms: [r.name],
        run: () => openDetail({ resource: kind.resource, namespace: r.namespace, name: r.name }),
      });
    }
  }
  return items;
}
