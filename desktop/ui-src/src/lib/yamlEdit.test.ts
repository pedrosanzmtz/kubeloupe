import { describe, expect, it } from "vitest";
import { errorLine, lineOfPath, parsePath, pathsIn, resourceVersionOf } from "./yamlEdit";

const DEPLOY = `apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: demo
  resourceVersion: '4711'
  labels:
    app: web
spec:
  replicas: 2
  template:
    spec:
      containers:
      - name: web
        image: nginx
        env:
        - name: A
          value: "1"
      - name: sidecar
        image: busybox
status:
  replicas: 2
`;

describe("lineOfPath", () => {
  it("finds nested keys", () => {
    expect(lineOfPath(DEPLOY, ["metadata", "name"])).toBe(4);
    expect(lineOfPath(DEPLOY, ["spec", "replicas"])).toBe(10);
    expect(lineOfPath(DEPLOY, ["status", "replicas"])).toBe(22);
  });

  it("finds list items in the unindented style, and their keys", () => {
    const c = ["spec", "template", "spec", "containers"];
    expect(lineOfPath(DEPLOY, [...c, 0])).toBe(14);
    expect(lineOfPath(DEPLOY, [...c, 0, "name"])).toBe(14);
    expect(lineOfPath(DEPLOY, [...c, 0, "image"])).toBe(15);
    expect(lineOfPath(DEPLOY, [...c, 1, "image"])).toBe(20);
    expect(lineOfPath(DEPLOY, [...c, 0, "env", 0, "value"])).toBe(18);
  });

  it("finds indented list items", () => {
    const y = "spec:\n  ports:\n    - port: 80\n    - port: 443\n      name: https\n";
    expect(lineOfPath(y, ["spec", "ports", 1, "name"])).toBe(5);
  });

  it("falls back to the deepest part that exists", () => {
    expect(lineOfPath(DEPLOY, ["spec", "template", "spec", "nope"])).toBe(12);
    expect(lineOfPath(DEPLOY, ["spec", "template", "spec", "containers", 7])).toBe(13);
    expect(lineOfPath(DEPLOY, ["nope"])).toBeNull();
  });

  it("does not match a key of a sibling or a deeper level", () => {
    // `replicas` under status, not under spec, when asked for status.
    expect(lineOfPath(DEPLOY, ["metadata", "replicas"])).toBe(3);
    expect(lineOfPath(DEPLOY, ["image"])).toBeNull();
  });
});

describe("resourceVersionOf", () => {
  it("reads quoted and plain values", () => {
    expect(resourceVersionOf(DEPLOY)).toBe("4711");
    expect(resourceVersionOf('metadata:\n  resourceVersion: "12"\n')).toBe("12");
    expect(resourceVersionOf("metadata:\n  resourceVersion: 12\n")).toBe("12");
  });
  it("is empty without one", () => {
    expect(resourceVersionOf("metadata:\n  name: x\n")).toBe("");
    expect(resourceVersionOf("")).toBe("");
  });
});

describe("error locations", () => {
  it("parses paths", () => {
    expect(parsePath(".spec.containers[0].image")).toEqual(["spec", "containers", 0, "image"]);
  });

  it("finds the paths an API server error names", () => {
    expect(
      pathsIn('Deployment.apps "web" is invalid: spec.replicas: Invalid value: -1: must be greater than or equal to 0'),
    ).toEqual([["spec", "replicas"]]);
    expect(pathsIn(".spec.template.spec.containers[1].image: field not declared in schema")).toEqual([
      ["spec", "template", "spec", "containers", 1, "image"],
    ]);
    expect(pathsIn('strict decoding error: unknown field "spec.foo"')).toEqual([["spec", "foo"]]);
    // As a kind cluster reports a field a ConfigMap does not have.
    expect(
      pathsIn(
        "failed to create typed patch object (default/kl-probe; /v1, Kind=ConfigMap): .spec: field not declared in schema",
      ),
    ).toEqual([["spec"]]);
    expect(pathsIn("Invalid value: something")).toEqual([]);
    expect(pathsIn("admission webhook denied the request")).toEqual([]);
  });

  it("points at the line", () => {
    expect(errorLine("spec.replicas: Invalid value: -1", DEPLOY)).toBe(10);
    expect(errorLine("invalid YAML: did not find expected node content at line 3 column 9", DEPLOY)).toBe(3);
    expect(errorLine('unknown field "spec.template.spec.containers[1].imagePullPolicy"', DEPLOY)).toBe(19);
    expect(errorLine("admission webhook denied the request", DEPLOY)).toBeNull();
  });
});
