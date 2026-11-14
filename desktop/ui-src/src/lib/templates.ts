// Starting YAML for "+ Create": a template for the common kinds, a generic
// skeleton (apiVersion, kind, metadata.name, spec) for everything else.

import type { ResourceRef } from "./bindings";

type Body = (ns: string) => string;

const meta = (name: string, ns: string) => `metadata:\n  name: ${name}\n  namespace: ${ns}\n`;

const TEMPLATES: Record<string, Body> = {
  "apps/Deployment": (ns) =>
    `apiVersion: apps/v1
kind: Deployment
${meta("my-app", ns)}spec:
  replicas: 1
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
        - name: app
          image: nginx:1.27
          ports:
            - containerPort: 80
`,
  "/Service": (ns) =>
    `apiVersion: v1
kind: Service
${meta("my-service", ns)}spec:
  selector:
    app: my-app
  ports:
    - port: 80
      targetPort: 80
`,
  "/ConfigMap": (ns) =>
    `apiVersion: v1
kind: ConfigMap
${meta("my-config", ns)}data:
  key: value
`,
  "/Secret": (ns) =>
    `apiVersion: v1
kind: Secret
${meta("my-secret", ns)}type: Opaque
# Plain text here; the cluster stores it base64-encoded in data.
stringData:
  password: change-me
`,
  "batch/Job": (ns) =>
    `apiVersion: batch/v1
kind: Job
${meta("my-job", ns)}spec:
  backoffLimit: 2
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: job
          image: busybox:1.36
          command: ["sh", "-c", "echo hello"]
`,
  "batch/CronJob": (ns) =>
    `apiVersion: batch/v1
kind: CronJob
${meta("my-cronjob", ns)}spec:
  schedule: "*/5 * * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers:
            - name: job
              image: busybox:1.36
              command: ["sh", "-c", "date"]
`,
  "networking.k8s.io/Ingress": (ns) =>
    `apiVersion: networking.k8s.io/v1
kind: Ingress
${meta("my-ingress", ns)}spec:
  rules:
    - host: example.local
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: my-service
                port:
                  number: 80
`,
  "/PersistentVolumeClaim": (ns) =>
    `apiVersion: v1
kind: PersistentVolumeClaim
${meta("my-claim", ns)}spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
`,
  "/Namespace": () =>
    `apiVersion: v1
kind: Namespace
metadata:
  name: my-namespace
`,
};

/** Whether `resource` has its own template (otherwise: the skeleton). */
export function hasTemplate(resource: ResourceRef): boolean {
  return `${resource.group}/${resource.kind}` in TEMPLATES;
}

/**
 * The YAML "+ Create" starts with for `resource` (null: no kind selected,
 * so a ConfigMap), in `namespace` ("" = all namespaces, so "default").
 */
export function templateFor(resource: ResourceRef | null, namespace: string): string {
  const ns = namespace || "default";
  if (!resource) return TEMPLATES["/ConfigMap"](ns);
  const t = TEMPLATES[`${resource.group}/${resource.kind}`];
  if (t) return t(ns);
  const apiVersion = resource.group ? `${resource.group}/${resource.version}` : resource.version;
  const name = `my-${resource.kind.toLowerCase()}`;
  return (
    `apiVersion: ${apiVersion}\nkind: ${resource.kind}\n` +
    (resource.namespaced ? meta(name, ns) : `metadata:\n  name: ${name}\n`) +
    "spec: {}\n"
  );
}
