# kubeloupe

A Lens alternative written in Rust — a Kubernetes IDE that stays fast while
streaming logs and watching cluster state.

> **Status:** early scaffold. The CLI core works against any cluster your
> kubeconfig can reach. The desktop app (Tauri) is in: Pods, Namespaces and
> Nodes views with search and auto-refresh.

## What it does today

- Desktop app — Pods, Namespaces and Nodes views, with namespace filter,
  search and 30s auto-refresh (see below)
- `kubeloupe pods` — list pods across all namespaces (name, namespace, phase, node)
- `kubeloupe pods -n <namespace>` — list pods in one namespace
- `kubeloupe namespaces` — list namespaces
- `kubeloupe nodes` — list nodes with readiness and kubelet version

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
```

## Roadmap

- [ ] Cluster overview: workloads, events, resource usage
- [ ] Live log streaming and pod exec
- [ ] Port-forwarding and resource YAML editing
- [ ] Multi-cluster support with fast switching
- [x] Desktop UI (Tauri)

## License

MIT
