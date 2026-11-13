# kubeloupe

A Lens alternative written in Rust — a Kubernetes IDE that stays fast while
streaming logs and watching cluster state.

> **Status:** early scaffold. The CLI core works against any cluster your
> kubeconfig can reach. The desktop UI is still to be decided.

## What it does today

- `kubeloupe pods` — list pods across all namespaces (name, namespace, phase, node)
- `kubeloupe pods -n <namespace>` — list pods in one namespace
- `kubeloupe namespaces` — list namespaces
- `kubeloupe nodes` — list nodes with readiness and kubelet version

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
- [ ] Desktop UI (Tauri) or terminal UI — to be decided

## License

MIT
