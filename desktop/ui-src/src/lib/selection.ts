/**
 * Keys from `anchor` to `to` (inclusive) in `ordered`, in either direction.
 * Falls back to just `to` when the anchor is not in the list any more.
 */
export function rangeKeys(ordered: string[], anchor: string | null, to: string): string[] {
  const a = anchor === null ? -1 : ordered.indexOf(anchor);
  const b = ordered.indexOf(to);
  if (b < 0) return [];
  if (a < 0) return [to];
  return ordered.slice(Math.min(a, b), Math.max(a, b) + 1);
}
