// kubectl against the test context, and the temporary objects the smoke
// suite changes. The shared demo objects (demo/web, demo/logger, ...) are
// only read.

import { execFileSync } from "node:child_process";
import { CONTEXT } from "./app.mjs";

export function kubectl(args, input) {
  return execFileSync("kubectl", ["--context", CONTEXT, ...args], {
    encoding: "utf8",
    input,
    stdio: ["pipe", "pipe", "pipe"],
  }).trim();
}

/** Names of the pods matching a label selector. */
export function podNames(namespace, selector) {
  const out = kubectl(["-n", namespace, "get", "pods", "-l", selector, "-o", "jsonpath={.items[*].metadata.name}"]);
  return out ? out.split(/\s+/) : [];
}

/**
 * A temporary namespace with a ConfigMap and a small logging Deployment, all
 * with unique names (the filter box matches across namespaces, and other
 * runs may share the cluster).
 */
export function createFixture() {
  const suffix = `${Date.now().toString(36)}${process.pid.toString(36)}`;
  const fx = {
    namespace: `e2e-${suffix}`,
    configMap: `e2e-config-${suffix}`,
    deployment: `e2e-app-${suffix}`,
  };
  kubectl(
    ["apply", "-f", "-"],
    `
apiVersion: v1
kind: Namespace
metadata:
  name: ${fx.namespace}
  labels:
    kubeloupe-e2e: "true"
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${fx.configMap}
  namespace: ${fx.namespace}
data:
  greeting: before
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ${fx.deployment}
  namespace: ${fx.namespace}
spec:
  replicas: 1
  selector:
    matchLabels:
      app: ${fx.deployment}
  template:
    metadata:
      labels:
        app: ${fx.deployment}
    spec:
      terminationGracePeriodSeconds: 0
      containers:
        - name: app
          image: busybox:1.36
          imagePullPolicy: IfNotPresent
          command: ["sh", "-c", "i=0; while true; do echo e2e line $i; i=$((i+1)); sleep 1; done"]
`,
  );
  kubectl(["-n", fx.namespace, "rollout", "status", `deploy/${fx.deployment}`, "--timeout=180s"]);
  return fx;
}

export function deleteFixture(fx) {
  if (!fx) return;
  try {
    kubectl(["delete", "namespace", fx.namespace, "--wait=false"]);
  } catch (e) {
    console.error(`could not delete namespace ${fx.namespace}: ${e.message}`);
  }
}
