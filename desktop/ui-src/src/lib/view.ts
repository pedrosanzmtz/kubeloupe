import type { Column, Ctx, Row } from "./columns";
import type { Entry } from "./stores/keyed.svelte";

export interface Sort {
  key: string;
  dir: "asc" | "desc";
}

const cmp = <V extends string | number>(a: V, b: V): number => (a < b ? -1 : a > b ? 1 : 0);

/**
 * Filter by the search text and sort. Without an explicit sort rows are in
 * key order (`namespace/name`), as in the old UI.
 */
export function visibleEntries(
  entries: Entry<Row>[],
  search: string,
  sort: Sort | null,
  columns: Column[],
  ctx: Ctx,
): Entry<Row>[] {
  const q = search.trim().toLowerCase();
  const filtered = q ? entries.filter((e) => e.hay.includes(q)) : entries.slice();
  const col = sort ? columns.find((c) => c.key === sort.key) : undefined;
  if (!col || !sort) {
    return filtered.sort((a, b) => cmp(a.key, b.key));
  }
  // Decorate once so the comparator does not recompute cells.
  const values = new Map<Entry<Row>, string | number>();
  for (const e of filtered) values.set(e, col.sort(e.row, ctx));
  const dir = sort.dir === "asc" ? 1 : -1;
  return filtered.sort((a, b) => {
    const c = cmp(values.get(a)!, values.get(b)!);
    return c !== 0 ? c * dir : cmp(a.key, b.key);
  });
}
