// Showing the per-document results of "+ Create" (cmd_apply_manifests).

import type { ApplyResult, ApplyStatus } from "./bindings";
import { errorLine } from "./yamlEdit";

/** What happened to one object, in words ("Would create" in a dry run). */
export function statusLabel(status: ApplyStatus, dryRun: boolean): string {
  switch (status) {
    case "created":
      return dryRun ? "Would create" : "Created";
    case "configured":
      return dryRun ? "Would update" : "Updated";
    case "unchanged":
      return "Unchanged";
    case "failed":
      return "Failed";
    case "skipped":
      return "Not applied";
    case "pending":
      return "Checked on create";
  }
}

/** `Deployment demo/web`, or `Document 2` if it could not be read. */
export function resultTitle(r: ApplyResult): string {
  if (!r.kind) return `Document ${r.document}`;
  const name = r.name || "(no name)";
  return `${r.kind} ${r.namespace ? `${r.namespace}/${name}` : name}`;
}

/** The object exists (or will) and can be opened. */
export function isOk(status: ApplyStatus): boolean {
  return status === "created" || status === "configured" || status === "unchanged";
}

/**
 * The line to point at for a result: for an error that names a field
 * (`spec.replicas: Invalid value`), that field's line within the result's
 * document; otherwise the line the backend reported.
 */
export function resultLine(r: ApplyResult, text: string): number {
  if (r.status !== "failed" || !r.message || /\bat line \d+/.test(r.message)) return r.line;
  const lines = text.split("\n");
  const rest = lines.slice(r.line - 1);
  const end = rest.findIndex((l, i) => i > 0 && /^---(\s|$)/.test(l));
  const doc = (end < 0 ? rest : rest.slice(0, end)).join("\n");
  const at = errorLine(r.message, doc);
  return at === null ? r.line : r.line + at - 1;
}

export interface Summary {
  text: string;
  ok: boolean;
}

/** One sentence over all results. */
export function summarize(results: ApplyResult[], dryRun: boolean): Summary {
  if (results.length === 0) return { text: "There are no objects in this YAML.", ok: false };
  const count = (s: ApplyStatus) => results.filter((r) => r.status === s).length;
  const failed = count("failed");
  const n = results.length;
  const objects = (k: number) => `${k} object${k === 1 ? "" : "s"}`;
  if (dryRun) {
    if (failed) return { text: `Validation failed for ${failed} of ${objects(n)}.`, ok: false };
    return { text: `Validation passed for ${objects(n)}. Nothing was created yet.`, ok: true };
  }
  if (failed && count("skipped") === n - failed) {
    return { text: `Nothing was applied: ${failed} of ${objects(n)} failed validation.`, ok: false };
  }
  const done = results.filter((r) => isOk(r.status)).length;
  if (failed) return { text: `Applied ${objects(done)}; ${failed} failed.`, ok: false };
  return { text: `Applied ${objects(done)}.`, ok: true };
}
