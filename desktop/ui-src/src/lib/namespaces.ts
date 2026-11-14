// The namespace selection: an empty list means all namespaces.

/** One watch per selected namespace; above this, All namespaces is cheaper. */
export const MAX_NAMESPACES = 10;

/** Trimmed, without blanks or duplicates, in the order given, capped. */
export function normalizeNamespaces(list: readonly string[]): string[] {
  const out: string[] = [];
  for (const raw of list) {
    const ns = raw.trim();
    if (ns && !out.includes(ns)) out.push(ns);
  }
  return out.slice(0, MAX_NAMESPACES);
}

/** Add `ns` to the selection, or remove it; adding past the cap does nothing. */
export function toggleNamespace(selected: readonly string[], ns: string): string[] {
  if (selected.includes(ns)) return selected.filter((s) => s !== ns);
  if (selected.length >= MAX_NAMESPACES) return [...selected];
  return [...selected, ns];
}

export function sameNamespaces(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((ns, i) => ns === b[i]);
}

/** The text of the namespace control's button. */
export function namespacesLabel(selected: readonly string[]): string {
  if (selected.length === 0) return "All namespaces";
  if (selected.length === 1) return selected[0];
  return `${selected.length} namespaces`;
}

/** The options matching `query` (case-insensitive substring). */
export function filterNamespaces(options: readonly string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  return q ? options.filter((ns) => ns.toLowerCase().includes(q)) : [...options];
}

/** `"a"`, `"a and b"`, `"a, b and c"`. */
export function joinNames(names: readonly string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The state of one watch behind the live table. */
export interface WatchStatus {
  /** The namespace it watches, or `null` for all namespaces / cluster scope. */
  namespace: string | null;
  state: "connecting" | "live" | "reconnecting" | "forbidden" | "failed";
  error: string;
  /** For `failed`: the header status (`clusterError(...).status`). */
  status?: string;
}

export interface CombinedStatus {
  ok: boolean | null;
  text: string;
  /** The banner message; `null` leaves the banner as it is. */
  error: string | null;
}

/**
 * One header status and banner for all the watches of a table: Live only
 * when every watch is live, and which namespaces are not.
 */
export function combineWatches(watches: readonly WatchStatus[], label: string, namespaced: boolean): CombinedStatus {
  if (watches.length === 1) return single(watches[0], label, namespaced);
  const of = (state: WatchStatus["state"]) => watches.filter((w) => w.state === state);
  const names = (ws: WatchStatus[]) => joinNames(ws.map((w) => w.namespace ?? ""));
  const messages = watches.flatMap((w) => {
    const ns = w.namespace ?? "";
    if (w.state === "reconnecting") return [`Watch of ${ns} interrupted, retrying: ${w.error}`];
    if (w.state === "forbidden") return [`You can't list ${label} in namespace ${ns}. (${w.error})`];
    if (w.state === "failed") return [`${ns}: ${w.error}`];
    return [];
  });
  const error = messages.length ? messages.join("\n") : of("connecting").length ? null : "";
  const failed = of("failed");
  if (failed.length) return { ok: false, text: `${failed[0].status ?? "Failed"} (${names(failed)})`, error };
  const reconnecting = of("reconnecting");
  if (reconnecting.length) return { ok: false, text: `Reconnecting… (${names(reconnecting)})`, error };
  const forbidden = of("forbidden");
  if (forbidden.length === watches.length) return { ok: false, text: "Not allowed", error };
  if (forbidden.length) return { ok: false, text: `Not allowed in ${names(forbidden)}`, error };
  if (of("connecting").length) return { ok: null, text: "Connecting…", error };
  return { ok: true, text: "Live", error };
}

function single(w: WatchStatus, label: string, namespaced: boolean): CombinedStatus {
  switch (w.state) {
    case "live":
      return { ok: true, text: "Live", error: "" };
    case "reconnecting":
      return { ok: false, text: "Reconnecting…", error: "Watch interrupted, retrying: " + w.error };
    case "forbidden": {
      // The backend stopped the watch: retrying would get the same 403.
      const where = !namespaced
        ? ""
        : w.namespace
          ? ` in namespace ${w.namespace}`
          : " across all namespaces. Pick a namespace you have access to";
      return { ok: false, text: "Not allowed", error: `You can't list ${label}${where}. (${w.error})` };
    }
    case "failed":
      return { ok: false, text: w.status ?? "Failed", error: w.error };
    default:
      return { ok: null, text: "Connecting…", error: null };
  }
}
