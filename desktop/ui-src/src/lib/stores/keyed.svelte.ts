/** A row plus its precomputed lowercase search text. */
export interface Entry<T> {
  key: string;
  row: T;
  /** Every field, lowercased and joined, so filtering is one `includes`. */
  hay: string;
}

export interface BatchLike<T> {
  /** When true, `applied` is the full state and anything not in it is gone. */
  snapshot: boolean;
  applied: T[];
  /** Keys of removed rows. */
  deleted: string[];
}

function hayOf(row: object): string {
  let out = "";
  for (const v of Object.values(row)) out += String(v).toLowerCase() + "\u0000";
  return out;
}

/**
 * Keyed rows updated by watch batches.
 *
 * The rows live in a plain `Map` and are republished as one `$state.raw`
 * array per batch. Deep `$state` would wrap every row in a proxy, which is
 * wasteful at 5-10k rows; here a batch costs one array rebuild and every row
 * object is replaced, never mutated.
 */
export class KeyedStore<T extends object> {
  #map = new Map<string, Entry<T>>();
  #keyOf: (row: T) => string;

  /** All rows in insertion order; replaced (not mutated) on every change. */
  entries = $state.raw<Entry<T>[]>([]);

  constructor(keyOf: (row: T) => string) {
    this.#keyOf = keyOf;
  }

  get size(): number {
    return this.#map.size;
  }

  clear() {
    this.#map.clear();
    this.entries = [];
  }

  /** Apply one batch and publish the result once. */
  applyBatch(batch: BatchLike<T>) {
    if (batch.snapshot) {
      const keep = new Set<string>();
      for (const row of batch.applied) keep.add(this.#keyOf(row));
      for (const key of this.#map.keys()) if (!keep.has(key)) this.#map.delete(key);
    }
    for (const row of batch.applied) {
      const key = this.#keyOf(row);
      this.#map.set(key, { key, row, hay: hayOf(row) });
    }
    for (const key of batch.deleted) this.#map.delete(key);
    this.entries = [...this.#map.values()];
  }
}
