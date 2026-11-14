// Command palette entries and how a query picks them. Building the entries
// from the app's state lives in `stores/palette.svelte.ts`; this module is
// pure so the ranking is unit tested.

import { bestScore } from "./fuzzy";

export type PaletteGroup = "Recent" | "Command" | "Action" | "Kind" | "Namespace" | "Context" | "Object";

export interface PaletteItem {
  /** Stable across openings (recent items are remembered by it). */
  id: string;
  group: Exclude<PaletteGroup, "Recent">;
  label: string;
  /** Shown dimmed after the label. */
  detail?: string;
  /** What the query is matched against; the label is always included. */
  terms?: string[];
  run: () => void;
}

export type Scope = "all" | "actions" | "namespaces" | "contexts";

/** `>` limits the palette to actions, `ns ` to namespaces, `ctx ` to contexts. */
export function parseQuery(raw: string): { scope: Scope; text: string } {
  const q = raw.trimStart();
  if (q.startsWith(">")) return { scope: "actions", text: q.slice(1).trim() };
  const m = /^(ns|ctx)(\s+|$)(.*)$/i.exec(q);
  // "ns" alone is also the Namespaces kind's alias: only a space makes it a prefix.
  if (m && m[2]) return { scope: m[1].toLowerCase() === "ns" ? "namespaces" : "contexts", text: m[3].trim() };
  return { scope: "all", text: q.trim() };
}

const IN_SCOPE: Record<Scope, PaletteItem["group"][] | null> = {
  all: null,
  actions: ["Action", "Command"],
  namespaces: ["Namespace"],
  contexts: ["Context"],
};

/** Objects only show up once something is typed: a table can be huge. */
const NEEDS_QUERY: PaletteItem["group"][] = ["Object"];

export const MAX_RESULTS = 50;
const RECENT_BONUS = 40;

export interface Ranked {
  item: PaletteItem;
  /** "Recent" for remembered items when nothing is typed. */
  group: PaletteGroup;
}

/** The items to show for `raw`, best first. `recent` is newest first. */
export function rankItems(items: PaletteItem[], raw: string, recent: string[]): Ranked[] {
  const { scope, text } = parseQuery(raw);
  const groups = IN_SCOPE[scope];
  const pool = items.filter((it) => !groups || groups.includes(it.group));
  const recency = (id: string) => {
    const i = recent.indexOf(id);
    return i < 0 ? 0 : RECENT_BONUS - i;
  };

  if (!text) {
    const byId = new Map(pool.map((it) => [it.id, it]));
    const recents = recent.flatMap((id) => byId.get(id) ?? []);
    const seen = new Set(recents.map((it) => it.id));
    const rest = pool.filter((it) => !seen.has(it.id) && (scope !== "all" || !NEEDS_QUERY.includes(it.group)));
    return [
      ...recents.map((item) => ({ item, group: "Recent" as const })),
      ...rest.map((item) => ({ item, group: item.group })),
    ].slice(0, MAX_RESULTS);
  }

  const scored: { item: PaletteItem; score: number; order: number }[] = [];
  pool.forEach((item, order) => {
    const s = bestScore(text, [item.label, ...(item.terms ?? [])]);
    if (s !== null) scored.push({ item, score: s + recency(item.id), order });
  });
  scored.sort((a, b) => b.score - a.score || a.order - b.order);
  return scored.slice(0, MAX_RESULTS).map(({ item }) => ({ item, group: item.group }));
}

/** Remember `id` as the most recent pick (keeps the 10 newest). */
export function pushRecent(recent: string[], id: string): string[] {
  return [id, ...recent.filter((r) => r !== id)].slice(0, 10);
}
