/** Lines kept per stream; older ones fall off the front. */
export const LOG_BUFFER_MAX = 50_000;

/**
 * Bounded line buffer, deliberately outside reactive state: it is trimmed in
 * chunks (10% over the cap) so appending stays amortised O(1) per line.
 */
export class LogBuffer {
  lines: string[] = [];

  push(batch: string[]) {
    const lines = this.lines;
    for (const l of batch) lines.push(l);
    if (lines.length > LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10) {
      lines.splice(0, lines.length - LOG_BUFFER_MAX);
    }
  }

  clear() {
    this.lines = [];
  }
}

/** Case-insensitive substring match; an empty filter matches everything. */
export function logMatches(text: string, filterLower: string): boolean {
  return !filterLower || text.toLowerCase().includes(filterLower);
}

export interface Segment {
  text: string;
  hit: boolean;
}

/** Split `text` into plain and highlighted (case-insensitive match) segments. */
export function splitHighlights(text: string, filter: string): Segment[] {
  if (!filter) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  const needle = filter.toLowerCase();
  const out: Segment[] = [];
  let pos = 0;
  for (;;) {
    const i = lower.indexOf(needle, pos);
    if (i < 0) break;
    if (i > pos) out.push({ text: text.slice(pos, i), hit: false });
    out.push({ text: text.slice(i, i + needle.length), hit: true });
    pos = i + needle.length;
  }
  if (pos < text.length) out.push({ text: text.slice(pos), hit: false });
  return out;
}
