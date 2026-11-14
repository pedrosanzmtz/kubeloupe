# Real-app E2E tests (WebKitGTK)

These tests run the built desktop app, with real IPC, watches, logs and exec,
against the kind test cluster. [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/)
proxies WebDriver to `WebKitWebDriver`, so they only run on Linux. The UI tests
with a fake backend are in `desktop/ui-src/src/test/`. Both suites use the same
`data-testid` attributes.

- `smoke.test.mjs` is the smoke suite (`node:test` + `selenium-webdriver`). It
  covers the Overview, the Pods list, a live delete and replace, pod logs, the
  terminal, saving YAML, workload logs and scaling from the context menu. It
  reads the shared demo objects but never changes them. The objects it changes
  live in a temporary `e2e-*` namespace, which it deletes when it finishes.
- `bench.mjs` is the 10k-row table benchmark (`ui-src/bench.html`) running in
  the app's webview. It only reports numbers and never fails on slow ones.

Every test saves a screenshot and the page text to `artifacts/` as
`smoke-NN-<test>.png` and `.txt`, whether it passes or fails. The app's log
goes to `artifacts/smoke-app.log`. CI uploads the whole folder as the
`e2e-screenshots` artifact on every run:

```sh
gh run download <run-id> -n e2e-screenshots
```

## Running locally

You need the kind cluster (`bash scripts/test-cluster/setup.sh`), plus
`WebKitWebDriver` (Ubuntu: `sudo apt install webkit2gtk-driver`), `tauri-driver`
(`cargo install tauri-driver --locked`) and, if you don't want windows opening
on your desktop, `xvfb-run` (`sudo apt install xvfb`).

```sh
# Build the app with the frontend embedded. This is what
# `cargo tauri build --debug --no-bundle` does.
npm --prefix desktop/ui-src ci
npm --prefix desktop/ui-src run build
cargo build -p kubeloupe-desktop --features tauri/custom-protocol

npm --prefix desktop/e2e ci
xvfb-run -a -s "-screen 0 1600x1000x24" npm --prefix desktop/e2e test
xvfb-run -a -s "-screen 0 1600x1000x24" npm --prefix desktop/e2e run bench   # serves ui-src with Vite on :1420
```

The app gets a kubeconfig that holds only `kind-kubeloupe-test`, so the
current context in your own kubeconfig doesn't matter. These environment
variables are optional:

| variable | default |
| --- | --- |
| `KUBELOUPE_APP` | `$CARGO_TARGET_DIR` (or `target`)`/debug/kubeloupe-desktop` |
| `TAURI_DRIVER` / `WEBKIT_DRIVER` | `tauri-driver` / `WebKitWebDriver` on `PATH` |
| `E2E_CONTEXT` | `kind-kubeloupe-test` |
| `E2E_ARTIFACTS` | `desktop/e2e/artifacts` |
| `E2E_DRIVER_PORT` | `4444` (the native driver uses the next port) |
| `BENCH_SECONDS` / `BENCH_PORT` | `30` / `1420` |

If you run several copies at once, give each its own `E2E_DRIVER_PORT` and
`BENCH_PORT`.

A plain `cargo build` without `tauri/custom-protocol` loads `devUrl`
(`http://localhost:1420`) instead of the embedded frontend. The smoke suite
doesn't start Vite, so a passing run shows that the embedded assets were the
ones loaded.
