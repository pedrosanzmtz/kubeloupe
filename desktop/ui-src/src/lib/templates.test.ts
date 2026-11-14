import { describe, expect, it } from "vitest";
import { hasTemplate, templateFor } from "./templates";
import { lineOfPath } from "./yamlEdit";
import type { ResourceRef } from "./bindings";

const ref = (group: string, kind: string, namespaced = true, version = "v1"): ResourceRef => ({
  group,
  version,
  kind,
  plural: kind.toLowerCase() + "s",
  namespaced,
});

describe("templateFor", () => {
  it("has a template for the common kinds", () => {
    const kinds: [string, string][] = [
      ["apps", "Deployment"],
      ["", "Service"],
      ["", "ConfigMap"],
      ["", "Secret"],
      ["batch", "Job"],
      ["batch", "CronJob"],
      ["networking.k8s.io", "Ingress"],
      ["", "PersistentVolumeClaim"],
      ["", "Namespace"],
    ];
    for (const [group, kind] of kinds) {
      const r = ref(group, kind, kind !== "Namespace");
      expect(hasTemplate(r), kind).toBe(true);
      const yaml = templateFor(r, "team-a");
      expect(yaml).toContain(`kind: ${kind}\n`);
      expect(yaml.startsWith(`apiVersion: ${group ? group + "/" : ""}v1\n`), kind).toBe(true);
      expect(lineOfPath(yaml, ["metadata", "name"]), kind).not.toBeNull();
      if (kind !== "Namespace") expect(yaml).toContain("  namespace: team-a\n");
    }
  });

  it("uses stringData for Secrets, so values are plain text", () => {
    const yaml = templateFor(ref("", "Secret"), "x");
    expect(lineOfPath(yaml, ["stringData", "password"])).not.toBeNull();
    expect(yaml).not.toMatch(/^data:/m);
  });

  it("falls back to a skeleton, CRDs included", () => {
    const widget = ref("example.com", "Widget", true, "v1alpha1");
    expect(hasTemplate(widget)).toBe(false);
    expect(templateFor(widget, "")).toBe(
      "apiVersion: example.com/v1alpha1\nkind: Widget\nmetadata:\n  name: my-widget\n  namespace: default\nspec: {}\n",
    );
    expect(templateFor(ref("example.com", "Cluster", false), "team-a")).toBe(
      "apiVersion: example.com/v1\nkind: Cluster\nmetadata:\n  name: my-cluster\nspec: {}\n",
    );
  });

  it("starts with a ConfigMap when no kind is selected", () => {
    expect(templateFor(null, "")).toContain("kind: ConfigMap\n");
  });
});
