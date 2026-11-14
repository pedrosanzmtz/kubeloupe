import type { RelatedGroup, RelatedItem } from "./bindings";

/** The kind summary as one line: `3/3 ready · RollingUpdate 25%/25% · revision 4`. */
export function summaryLine(parts: string[]): string {
  return parts.filter((p) => p.trim() !== "").join(" · ");
}

/** Class of a related-table cell: statuses are coloured like the tables
 *  (`phase-Running`), and a `Ready` count that is short (`1/3`) is amber. */
export function cellClass(column: string, value: string): string {
  if (column === "Status") return `phase-${value}`;
  if (column === "Ready") {
    const m = /^(\d+)\/(\d+)$/.exec(value);
    if (m && m[1] !== m[2]) return "short";
  }
  return "";
}

/** Whether the group's rows stand for objects that can be opened. */
export function isObjectGroup(g: RelatedGroup): boolean {
  return g.kind !== "" && g.api_version !== "";
}

/** A stable key for a row of `g`. */
export function itemKey(g: RelatedGroup, item: RelatedItem, index: number): string {
  return isObjectGroup(g) ? `${item.namespace}/${item.name}/${index}` : `${index}`;
}
