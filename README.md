# kubeloupe

[![CI](https://github.com/pedrosanzmtz/kubeloupe/actions/workflows/ci.yml/badge.svg)](https://github.com/pedrosanzmtz/kubeloupe/actions/workflows/ci.yml)

A Lens alternative written in Rust: a Kubernetes IDE that stays fast while
streaming logs and watching cluster state. It comes as a desktop app (Tauri +
Svelte) and a CLI, both built on the same Rust core.

> **Status:** early. The desktop app browses every resource type the cluster
> serves (CRDs included) with live watches, shows logs for pods and whole
> workloads, opens terminals and port-forwards, edits YAML with server-side
> apply, and runs the common actions (scale, restart, cordon, drain, trigger,
> suspend, delete). There is no tagged release yet. What comes next is tracked
> in the [roadmap (#24)](https://github.com/pedrosanzmtz/kubeloupe/issues/24).

## Desktop app

- **Every resource type**, built-in or custom, found through API discovery
  and grouped in a Lens-style sidebar (workloads, network, config, storage,
  cluster, custom resources). Kinds you aren't allowed to list are
  greyed out.
- **Live tables**: lists are kept up to date by Kubernetes watches, not by
  polling, and stay smooth with thousands of rows. Filter by namespace and
  by text. Pods and Nodes show CPU/memory usage when the metrics API is
  available.
- **Overview**: pod counts (crash-looping pods count as errors, not as
  running), problem pods, CPU/memory/pod capacity against requests and usage,
  and the newest warning events, each linking to its object. Kept live by
  watches, not by re-listing the cluster.
- **Details** for any object. For pods: live logs (pick the container, or the
  previous instance of a crashed one), an interactive terminal (pod exec), and
  port-forwarding to localhost.
- **Workload logs**: follow every pod of a Deployment, StatefulSet, DaemonSet
  or Job in one stream, including pods that a rollout replaces.
- **YAML editor** with server-side apply. After saving, it tells you which
  fields it took over from other field managers (for example from
  `kubectl` or a controller). Secret values stay hidden until you reveal them.
- **Create from YAML** ("+ Create"): one or many documents, from a template
  for the current kind, a file you open or drop. Validated with a server dry
  run first; the results are listed per object.
- **Actions** from the row menu, the detail panel or on several selected rows:
  scale, rollout restart, cordon/uncordon, drain (with progress, respecting
  PodDisruptionBudgets), trigger/suspend/resume a CronJob, delete and force
  delete. Destructive ones ask for confirmation and list the exact objects.
- **Multi-cluster**: switch between your kubeconfig contexts from the header.

## CLI

The CLI uses the current kubeconfig context. Run `kubeloupe --help` or
`kubeloupe <command> --help` for every option.

| Command | What it does |
|---|---|
| `kubeloupe get <kind> [-n NS]` | List any resource type, built-in or custom (`deploy`, `svc`, `widgets.example.com`), with the same columns as the app |
| `kubeloupe watch [<kind>] [-n NS]` | Print live changes to a resource type (default `pods`) until Ctrl-C |
| `kubeloupe pods [-n NS]` | List pods (all namespaces by default) |
| `kubeloupe namespaces` | List namespaces |
| `kubeloupe nodes` | List nodes with readiness and kubelet version |
| `kubeloupe logs <pod> [-n NS] [-c CONTAINER] [-p] [-f] [--tail N]` | A pod's logs: `-c` picks a container, `-p` the previous (crashed) instance, `-f` follows |
| `kubeloupe logs deploy/<name>` (also `sts/`, `ds/`, `job/`) | The logs of every pod of a workload, as `[pod] line` |
| `kubeloupe top [pods\|nodes]` | CPU/memory usage (needs metrics-server) |
| `kubeloupe yaml <pod> [-n NS]` | Print a pod as YAML |
| `kubeloupe exec <pod> [-n NS] [-c CONTAINER] -- <command>` | Run a command in a pod and print its output |
| `kubeloupe port-forward <pod> <port> [-n NS]` | Forward a pod port to a free local port until Ctrl-C |

Actions report errors in plain words (not allowed, already gone, blocked by a
PodDisruptionBudget). Namespaced actions default to the `default` namespace.

| Command | What it does |
|---|---|
| `kubeloupe scale <deploy\|sts\|rs>/<name> --replicas N [-n NS]` | Set the replica count |
| `kubeloupe restart <deploy\|sts\|ds>/<name> [-n NS]` | Rollout restart |
| `kubeloupe cordon <node>` / `kubeloupe uncordon <node>` | Mark a node unschedulable / schedulable |
| `kubeloupe drain <node> [--timeout SECS] [--pod-selector SEL]` | Cordon, then evict the node's pods through the Eviction API with progress output. Skips DaemonSet and mirror pods and retries evictions a PodDisruptionBudget refuses |
| `kubeloupe trigger cronjob/<name> [-n NS]` | Create a Job from a CronJob now |
| `kubeloupe suspend cronjob/<name> [-n NS]` / `resume cronjob/<name>` | Suspend or resume a CronJob |
| `kubeloupe delete <kind>/<name> [-n NS] [--force]` | Delete an object of any kind. `--force` (pods only) uses grace period 0 |
| `kubeloupe apply -f <file\|-> [-n NS] [--dry-run]` | Create or update the objects of a (multi-document) YAML file, like `kubectl apply`. Validated with a dry run first; nothing is applied if any object fails |

## Install

Releases will be published on the
[Releases page](https://github.com/pedrosanzmtz/kubeloupe/releases) with these
files:

- Linux: `.AppImage` (`chmod +x` it and run) or `.deb` (`sudo apt install ./kubeloupe_*.deb`)
- macOS: `.dmg` (universal, Apple silicon and Intel)
- Windows: `.msi` or the NSIS `-setup.exe`
- CLI only: `kubeloupe-cli-<version>-<target>.tar.gz` (`.zip` on Windows)

The builds are not signed yet, so the OS will warn you the first time:

- macOS: drag the app to Applications, then right-click it and choose Open
  (then Open again). If macOS says the app is damaged, run
  `xattr -dr com.apple.quarantine /Applications/kubeloupe.app`.
- Windows: SmartScreen shows "Windows protected your PC"; click
  More info, then Run anyway.
- Linux needs no workaround; the AppImage needs FUSE 2 (`libfuse2`) on
  recent Ubuntu.

## Build and run from source

You need:

- Rust (stable)
- A kubeconfig (`~/.kube/config`, or `KUBECONFIG`) pointing at a cluster, or
  in-cluster config when running inside Kubernetes
- For the desktop app: Node (the version in `desktop/ui-src/.nvmrc`), the
  Tauri CLI, and on Linux the WebKitGTK libraries
  (`sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`).
  macOS and Windows need nothing extra.

The CLI:

```sh
cargo run -- --help
cargo run -- get deploy -n kube-system
cargo run -- logs deploy/<name> -n <namespace> -f
```

The desktop app:

```sh
cargo install tauri-cli --version "^2" --locked   # once
npm --prefix desktop/ui-src ci                    # once, and after lockfile changes
cd desktop && cargo tauri dev                     # starts Vite on :1420 and the app
```

A debug build loads the UI from the Vite dev server, so
`cargo run -p kubeloupe-desktop` on its own shows a connection error unless
`npm --prefix desktop/ui-src run dev` is running. `cargo tauri build` makes a
release build with the UI bundled.

## Test cluster

`bash scripts/test-cluster/setup.sh` creates a local test cluster with
[kind](https://kind.sigs.k8s.io/) (Kubernetes IN Docker): real nodes running
real containers, so logs, exec, port-forwarding and the CPU/memory columns
all work end to end. It needs Docker, usable by your user without `sudo`.
The script installs `kind` and `kubectl` into `~/.local/bin` if they are
missing.

It creates `kubeloupe-test` (a control-plane node and a worker) with:

- metrics-server (patched with `--kubelet-insecure-tls`, which kind's
  self-signed kubelet certs need), so the usage columns and `kubeloupe top`
  work; the numbers appear about a minute after it starts
- the demo workload (`scripts/test-cluster/demo.yaml`) in namespace `demo`:
  the `web` (nginx) and `logger` deployments (the logger writes a line every
  2 s, handy for live logs), a two-container pod (`multi`), a crash-looping
  pod (`crashed`), and in `default` a pod that stays Pending (`hungry`)

Pass `--staging` to also create `kubeloupe-staging`, a single empty node, so
the context switcher has a second cluster to switch to. Each kind cluster
costs roughly 500 MB to 1 GB of RAM, which is why it is opt-in.

kind adds the contexts (`kind-kubeloupe-test`, `kind-kubeloupe-staging`) to
`~/.kube/config` and the script leaves `kind-kubeloupe-test` current.

Delete the clusters with `kind delete cluster --name kubeloupe-test` (and
`kubeloupe-staging`); re-run the script to recreate them.

## Contributing

See [CLAUDE.md](CLAUDE.md) (also available as `AGENTS.md`) for the code
layout, the checks CI runs, how to regenerate the IPC bindings, and how to run
the unit, frontend and cluster tests.

## License

MIT
