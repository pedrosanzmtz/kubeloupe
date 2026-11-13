# kubeloupe

A Lens alternative written in Rust — a Kubernetes IDE that stays fast while
streaming logs and watching cluster state.

> **Status:** the desktop app (Tauri) is in: Overview, Pods, Namespaces and
> Nodes views with CPU/memory usage, pod details with live log streaming, a
> terminal (pod exec), port-forwarding and YAML editing, and multi-cluster
> switching from the kubeconfig contexts.

## What it does today

- Desktop app — Overview (counts, pods by namespace, recent events),
  Pods, Namespaces and Nodes views, with namespace filter, search and
  30s auto-refresh; click a pod for its details and live logs, an
  interactive terminal (pod exec via xterm.js), port-forwarding to
  localhost, and a YAML editor with server-side apply; Pods and Nodes
  show CPU/memory usage when the metrics API is available; switch
  clusters from the context selector in the header (see below)
- `kubeloupe pods` — list pods across all namespaces (name, namespace, phase, node)
- `kubeloupe pods -n <namespace>` — list pods in one namespace
- `kubeloupe namespaces` — list namespaces
- `kubeloupe nodes` — list nodes with readiness and kubelet version
- `kubeloupe logs <pod> [-n <namespace>] [--tail N]` — print a pod's logs
- `kubeloupe top [pods|nodes]` — CPU/memory usage (needs metrics-server)
- `kubeloupe yaml <pod> [-n <namespace>]` — print a pod as YAML
- `kubeloupe exec <pod> [-n <namespace>] [-c <container>] -- <command>` —
  run a command in a pod and print its output
- `kubeloupe port-forward <pod> <port> [-n <namespace>]` — forward a pod
  port to localhost until Ctrl-C

## Desktop app

The desktop app is a Tauri app in `desktop/`. It talks to the cluster
through the same Rust core as the CLI.

```sh
cargo run -p kubeloupe-desktop
```

On Linux, Tauri needs the WebKitGTK system libraries first
(`libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`,
`librsvg2-dev`). On macOS and Windows they are not needed.

## Prerequisites

- Rust (stable) — `cargo` on your `PATH`
- A working kubeconfig (`~/.kube/config`) pointing at a cluster, or
  in-cluster config when running inside Kubernetes

## Run it

```sh
cargo run -- pods
cargo run -- pods -n kube-system
cargo run -- namespaces
cargo run -- nodes
cargo run -- logs <pod> -n <namespace>
cargo run -- top pods
cargo run -- yaml <pod> -n <namespace>
cargo run -- exec <pod> -n <namespace> -- ls /
cargo run -- port-forward <pod> 80 -n <namespace>
```

## Test cluster

`bash scripts/test-cluster/setup.sh` creates two local test clusters with
[kwok](https://kwok.sigs.k8s.io/) (Kubernetes WithOut Kubelet): real API
servers (kube-apiserver + etcd, Kubernetes v1.36) with simulated nodes and
pods. No Docker, no VM, no cgroup support needed — that is why kwok, and not
kind/k3s/minikube, is the test route here.

It creates:

- `kubeloupe-test` — 2 nodes and the demo workload
  (`scripts/test-cluster/demo.yaml`): nginx and logger deployments, a
  Pending pod (`hungry`, impossible resource request) and a Failed pod
  (`crashed`)
- `kubeloupe-staging` — 1 node, empty, so the context switcher has a second
  cluster to switch to

It merges both contexts into `~/.kube/config` and leaves `kubeloupe-test`
current, so `cargo run -p kubeloupe-desktop` opens against the test cluster.

One honest limitation: kwok simulates pods, there is no kubelet and no
metrics-server, so **pod logs, pod exec, and port-forwarding fail** on the
test clusters (the app shows the real error, e.g. "no exec found for
container"), and the **CPU/memory columns show "—"**. Those need a cluster
with real containers (kind, k3s, or a real cluster) and, for usage,
metrics-server installed. Everything else — lists, details, events,
overview, context switching, YAML editing — works against the real API
server. The usage columns are verified against a mock API server that
serves the metrics.k8s.io API (see `kubeloupe top` against it).

Stop the clusters with `kwokctl delete cluster --name kubeloupe-test` (and
`kubeloupe-staging`); re-run the script to recreate them. If the machine
restarts, the cluster processes stop but their state persists under
`~/.kwok` — bring them back with `kwokctl start cluster --name
kubeloupe-test` (and `kubeloupe-staging`); the kubeconfig stays valid.

## Roadmap

- [x] Cluster overview: counts, pods by namespace, recent events
- [x] Cluster resource usage (needs the metrics API)
- [x] Live log streaming
- [x] Pod exec (terminal into a container)
- [x] Port-forwarding and resource YAML editing
- [x] Multi-cluster support with fast switching
- [x] Desktop UI (Tauri)

## License

MIT
