# Contributing to kubeloupe

Guide for people and coding agents. `AGENTS.md` is a symlink to this file.
User-facing docs are in `README.md`; the plan is the roadmap issue #24.

## Layout

- `src/`: the core library, one module per concern (`client`, `pods`, `logs`,
  `workload_logs`, `exec`, `portforward`, `metrics`, `overview`, `watch`,
  `actions`, `cli`, `get`, `describe`), plus `src/resources/` (discovery,
  columns, table, ops, detail). `lib.rs` re-exports them. The CLI is
  `src/main.rs`, with its arguments in `src/args.rs`; `tests/cli.rs` runs the
  binary.
- `desktop/src/`: the Tauri app. `main.rs` builds and registers everything,
  `state.rs` holds `AppState`, `commands/` has one module per concern.
- `desktop/ui-src/`: the Svelte 5 + Vite + TypeScript UI.
  `src/lib/bindings.ts` is generated; `src/test/` is the fake backend and the
  Playwright tests.
- `tests/cluster.rs`: integration tests against the kind cluster.
- `scripts/test-cluster/`: `setup.sh` (kind cluster, metrics-server),
  `demo.yaml` (the demo workload) and `limited-user.sh` (a namespace-scoped
  user, opt-in).

## Rules

- Kubernetes logic goes in `src/` so the CLI can use it too. Tauri commands
  stay thin wrappers that return `Result<T, String>`.
- Keep error messages plain and specific: say what was not allowed, what is
  already gone, what blocked it.
- Keep files small and one concern per module; parallel branches collide in
  big files.

## Checks (what CI runs)

Run all of these before opening a PR:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p kubeloupe-desktop export_typescript_bindings   # then commit bindings.ts if it changed

cd desktop/ui-src
npm ci
npm run build
npm run check        # svelte-check, 0 errors and 0 warnings
npm test             # Vitest unit tests
npx playwright test  # app tests in Chromium against the fake backend
```

On Linux the Rust checks need the WebKitGTK libraries listed in the README.
Playwright needs a browser once: `npx playwright install chromium` (add
`--with-deps` to install its system libraries too).

`integration.yml` runs the cluster tests (below) on every PR as well.

## IPC bindings

- tauri-specta generates `desktop/ui-src/src/lib/bindings.ts` from the
  commands and events. Never edit it by hand. CI fails if it is stale.
- On a merge or rebase conflict in `bindings.ts`, take either side and
  regenerate it with `cargo test -p kubeloupe-desktop export_typescript_bindings`.
- Avoid `Option` fields in command and event output types where you can.
  Otherwise specta splits the type into separate Serialize and Deserialize
  variants in TypeScript.
- Types in `src/` that cross the IPC boundary derive
  `#[cfg_attr(feature = "specta", derive(specta::Type))]`, so the core and
  the CLI build without specta.
- A new command also needs a handler in `HANDLERS` in
  `desktop/ui-src/src/test/fake-backend.ts`; `npm run check` fails until it
  has one. The header comment there explains how to add commands and events.

## Svelte

- Keep large collections in `$state.raw` or a plain `Map`, never deep `$state`.
- Tables use `VirtualList` (`lib/components/VirtualList.svelte`) and
  `KeyedStore` (`lib/stores/keyed.svelte.ts`).
- Components that start a stream (logs, exec, port-forward, watch) stop it
  in `onDestroy`.
- Put pure logic in `lib/*.ts` next to a `*.test.ts`. Tests of modules that use
  runes are named `*.svelte.test.ts`.

## Frontend tests

In `desktop/ui-src`:

- `npm test`: Vitest, for `src/**/*.test.ts`.
- `npm run test:e2e` (same as `npx playwright test`): the real UI
  (`test.html`) in Chromium, with every Tauri `invoke` served by the fake
  backend in `src/test/fake-backend.ts` from the fixtures in
  `src/test/fixtures/`. Tests live in `src/test/e2e/`. Its Vite server uses
  port 1430, so it runs next to `cargo tauri dev` (1420), but only one
  worktree at a time can run it ("1430 is already used" means another one is).
- `PW_CHANNEL=chrome npx playwright test` uses an installed Google Chrome if
  the Playwright browser download is blocked.
- `test.html` must never end up in `dist/`; CI checks this.

## Cluster tests

- kind only. `bash scripts/test-cluster/setup.sh` creates the cluster
  (context `kind-kubeloupe-test`).
- `cargo test -- --ignored --test-threads=4` runs `tests/cluster.rs`. Run one
  test with `cargo test --test cluster -- --ignored <name>`. Set
  `KUBELOUPE_TEST_CONTEXT` to use another context.
- New cluster tests are `#[ignore]`d and create what they need in a temporary
  namespace (`in_temp_ns`), which they delete afterwards.
- `tests/helm_cluster.rs` needs `helm` (on `PATH`, or the binary in
  `KUBELOUPE_HELM`); it skips without it, except in CI. It installs
  `scripts/test-cluster/charts/demo-chart` in a temporary namespace.
- The demo objects are shared: don't change `demo/web`, `demo/logger`,
  `demo/crashed`, `demo/multi` or `default/hungry`.
- Node-level tests (cordon, drain) affect the whole cluster. They restore the
  node afterwards and only evict pods they created (`--pod-selector`).
- RBAC: `bash scripts/test-cluster/limited-user.sh` creates a ServiceAccount
  with `edit` in `team-a` only and writes a separate kubeconfig,
  `~/.kube/kubeloupe-limited.yaml` (it never touches `~/.kube/config`). The
  `limited_*` tests run as that user when
  `KUBELOUPE_LIMITED_KUBECONFIG=~/.kube/kubeloupe-limited.yaml` is set, and
  skip otherwise. Try the app as that user with
  `KUBECONFIG=~/.kube/kubeloupe-limited.yaml cargo tauri dev`. The token
  lasts 24h; re-run the script to refresh it.

## Parallel agents

- Work in your own git worktree (`.claude/` is git-ignored for this). Don't
  take on two issues that change the same component or command module at the
  same time; each issue lists the files it touches.
- Give each worktree its own cargo target dir (the default `target/` inside
  the worktree). A shared `CARGO_TARGET_DIR` makes parallel builds overwrite
  each other's artifacts.
- All agents share one kind cluster. Keep tests in their own namespaces, and
  don't run two node-level tests at once.

## Branches and PRs

- Start from an up-to-date `main`. Branch name: `issue-<number>-<short-name>`.
- PR body sections: **What**, **Design**, **Verified** (the exact commands you
  ran and their results), **Not verified** / manual testing checklist, then
  `Closes #<number>`.
- Say plainly what you could not verify, for example clicking through the GUI
  or behaviour in WebKitGTK (Playwright runs Chromium, not the real webview).
