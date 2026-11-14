// A small fuzzy matcher for the command palette: the query's characters must
// appear in order. Exact matches beat prefixes, prefixes beat substrings,
// and substrings beat scattered matches; within those, matches at word starts
// and runs of consecutive characters score higher.

const EXACT = 1000;
const SUBSTRING = 500;

function isBoundary(text: string, i: number): boolean {
  if (i === 0) return true;
  const prev = text[i - 1];
  if (/[\s\-_./:]/.test(prev)) return true;
  // camelCase: "StatefulSet" → S, S.
  return prev === prev.toLowerCase() && text[i] !== text[i].toLowerCase();
}

/** How well `query` matches `text`, or `null` if it does not. Higher is better. */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase();
  if (!q) return 0;
  const t = text.toLowerCase();
  if (t === q) return EXACT;
  const at = t.indexOf(q);
  if (at >= 0) {
    const start = at === 0 ? 200 : isBoundary(text, at) ? 100 : 0;
    return SUBSTRING + start - Math.min(at, 50) - Math.min(t.length - q.length, 50) / 10;
  }
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const c of q) {
    if (c === " ") continue;
    const j = t.indexOf(c, from);
    if (j < 0) return null;
    score += 1;
    if (j === prev + 1) score += 5;
    if (isBoundary(text, j)) score += 8;
    prev = j;
    from = j + 1;
  }
  return score - Math.min(t.length, 100) / 20;
}

/** The best score of `query` against any of `terms`. */
export function bestScore(query: string, terms: string[]): number | null {
  let best: number | null = null;
  for (const term of terms) {
    const s = fuzzyScore(query, term);
    if (s !== null && (best === null || s > best)) best = s;
  }
  return best;
}
