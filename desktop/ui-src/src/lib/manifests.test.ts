import { describe, expect, it } from "vitest";
import { resultLine, resultTitle, statusLabel, summarize } from "./manifests";
import type { ApplyResult } from "./bindings";

function result(over: Partial<ApplyResult>): ApplyResult {
  return {
    document: 1,
    line: 1,
    api_version: "v1",
    kind: "ConfigMap",
    name: "a",
    namespace: "demo",
    status: "created",
    message: "",
    takeovers: [],
    ...over,
  };
}

describe("manifest results", () => {
  it("labels statuses for a dry run and a real apply", () => {
    expect(statusLabel("created", true)).toBe("Would create");
    expect(statusLabel("created", false)).toBe("Created");
    expect(statusLabel("configured", false)).toBe("Updated");
    expect(statusLabel("skipped", false)).toBe("Not applied");
  });

  it("titles results", () => {
    expect(resultTitle(result({}))).toBe("ConfigMap demo/a");
    expect(resultTitle(result({ kind: "Namespace", namespace: "", name: "x" }))).toBe("Namespace x");
    expect(resultTitle(result({ kind: "", document: 3 }))).toBe("Document 3");
  });

  it("points at the field an error names, inside its own document", () => {
    const text = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: a\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: b\nspec:\n  replicas: -1\n";
    const r = result({ line: 6, status: "failed", message: "spec.replicas: Invalid value: -1: must be >= 0" });
    expect(resultLine(r, text)).toBe(11);
    // No field: the document's first line. A parser error: its own line.
    expect(resultLine(result({ line: 6, status: "failed", message: "forbidden" }), text)).toBe(6);
    expect(resultLine(result({ line: 9, status: "failed", message: "invalid YAML at line 9 column 1: x" }), text)).toBe(9);
  });

  it("summarizes", () => {
    expect(summarize([], true).ok).toBe(false);
    expect(summarize([result({}), result({})], true)).toEqual({
      text: "Validation passed for 2 objects. Nothing was created yet.",
      ok: true,
    });
    expect(summarize([result({ status: "failed" }), result({})], true).text).toBe(
      "Validation failed for 1 of 2 objects.",
    );
    expect(summarize([result({ status: "failed" }), result({ status: "skipped" })], false).text).toBe(
      "Nothing was applied: 1 of 2 objects failed validation.",
    );
    expect(summarize([result({ status: "failed" }), result({})], false).text).toBe("Applied 1 object; 1 failed.");
    expect(summarize([result({ status: "unchanged" })], false)).toEqual({ text: "Applied 1 object.", ok: true });
  });
});
