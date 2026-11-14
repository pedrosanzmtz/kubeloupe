// Starts the real desktop app under tauri-driver and wraps the WebDriver
// session in a few helpers. Used by smoke.test.mjs and bench.mjs.
//
// Environment (all optional):
//   KUBELOUPE_APP   path to the built app (default: <target>/debug/kubeloupe-desktop)
//   TAURI_DRIVER    path to tauri-driver (default: on PATH)
//   WEBKIT_DRIVER   path to WebKitWebDriver (default: tauri-driver's lookup on PATH)
//   E2E_CONTEXT     kube context to test against (default: kind-kubeloupe-test)
//   E2E_ARTIFACTS   where screenshots and logs go (default: desktop/e2e/artifacts)
//   E2E_DRIVER_PORT port for tauri-driver (default: 4444; the native driver gets +1)

import { spawn, execFileSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Builder, By } from "selenium-webdriver";

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_DIR = resolve(here, "..");
export const REPO = resolve(E2E_DIR, "../..");
export const CONTEXT = process.env.E2E_CONTEXT || "kind-kubeloupe-test";
export const ARTIFACTS = resolve(process.env.E2E_ARTIFACTS || join(E2E_DIR, "artifacts"));

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function appBinary() {
  if (process.env.KUBELOUPE_APP) return resolve(process.env.KUBELOUPE_APP);
  const target = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(REPO, "target");
  return join(target, "debug", process.platform === "win32" ? "kubeloupe-desktop.exe" : "kubeloupe-desktop");
}

/**
 * A kubeconfig holding only the test context, so the app picks it no matter
 * what the developer's own kubeconfig has as current.
 */
function testKubeconfig() {
  const yaml = execFileSync("kubectl", ["config", "view", "--minify", "--flatten", "--context", CONTEXT], {
    encoding: "utf8",
  });
  const dir = join(tmpdir(), `kubeloupe-e2e-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const path = join(dir, "kubeconfig");
  writeFileSync(path, yaml, { mode: 0o600 });
  return path;
}

function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolvePromise, reject) => {
    const attempt = () => {
      const sock = connect(port, "127.0.0.1");
      sock.once("connect", () => {
        sock.destroy();
        resolvePromise();
      });
      sock.once("error", () => {
        sock.destroy();
        if (Date.now() > deadline) reject(new Error(`nothing listens on port ${port}`));
        else setTimeout(attempt, 200);
      });
    };
    attempt();
  });
}

/**
 * Start tauri-driver and open a session, which launches the app. `name`
 * prefixes the log and screenshot files.
 */
export async function launch(name) {
  const app = appBinary();
  if (!existsSync(app)) {
    throw new Error(
      `${app} does not exist. Build it first:\n` +
        `  npm --prefix desktop/ui-src run build\n` +
        `  cargo build -p kubeloupe-desktop --features tauri/custom-protocol`,
    );
  }
  mkdirSync(ARTIFACTS, { recursive: true });
  const port = Number(process.env.E2E_DRIVER_PORT || 4444);
  const args = ["--port", String(port), "--native-port", String(port + 1)];
  if (process.env.WEBKIT_DRIVER) args.push("--native-driver", process.env.WEBKIT_DRIVER);

  // The app inherits this environment through tauri-driver and WebKitWebDriver.
  const env = { ...process.env, KUBECONFIG: testKubeconfig() };
  const log = createWriteStream(join(ARTIFACTS, `${name}-app.log`));
  const driverProc = spawn(process.env.TAURI_DRIVER || "tauri-driver", args, {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  driverProc.stdout.pipe(log);
  driverProc.stderr.pipe(log);
  const exited = new Promise((r) => driverProc.once("exit", r));
  driverProc.once("error", (e) => log.write(`tauri-driver: ${e}\n`));

  try {
    await waitForPort(port, 15_000);
    const driver = await new Builder()
      .usingServer(`http://127.0.0.1:${port}/`)
      .withCapabilities({ browserName: "wry", "tauri:options": { application: app } })
      .build();
    return new App(driver, name, async () => {
      await driver.quit().catch(() => {});
      driverProc.kill();
      await Promise.race([exited, sleep(5000)]);
      log.end();
    });
  } catch (e) {
    driverProc.kill();
    log.end();
    throw e;
  }
}

/** XPath string literal (handles both quote kinds). */
export function xq(s) {
  if (!s.includes("'")) return `'${s}'`;
  if (!s.includes('"')) return `"${s}"`;
  return `concat('${s.split("'").join(`', "'", '`)}')`;
}

export const byTestId = (id) => By.css(`[data-testid="${id}"]`);

export class App {
  #shots = 0;

  constructor(driver, name, close) {
    this.driver = driver;
    this.name = name;
    this.close = close;
  }

  /**
   * Poll `fn` until it returns something truthy and return that. Errors
   * (stale elements, not-found) count as "not yet".
   */
  async waitFor(fn, { timeout = 20_000, interval = 250, message = "condition" } = {}) {
    const deadline = Date.now() + timeout;
    let lastErr;
    for (;;) {
      try {
        const v = await fn();
        if (v) return v;
        lastErr = undefined;
      } catch (e) {
        lastErr = e;
      }
      if (Date.now() > deadline) {
        throw new Error(`timed out after ${timeout} ms waiting for ${message}${lastErr ? `: ${lastErr.message}` : ""}`);
      }
      await sleep(interval);
    }
  }

  /** Wait for a visible element. */
  async find(locator, opts = {}) {
    return this.waitFor(
      async () => {
        const el = await this.driver.findElement(locator);
        return (await el.isDisplayed()) ? el : null;
      },
      { message: String(locator), ...opts },
    );
  }

  js(script, ...args) {
    return this.driver.executeScript(script, ...args);
  }

  /** Screenshot plus the page's visible text, numbered in the order taken. */
  async shot(label) {
    const n = String(++this.#shots).padStart(2, "0");
    const base = join(ARTIFACTS, `${this.name}-${n}-${label.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`);
    try {
      writeFileSync(`${base}.png`, await this.driver.takeScreenshot(), "base64");
      writeFileSync(`${base}.txt`, await this.js("return document.body.innerText"));
    } catch (e) {
      writeFileSync(`${base}.txt`, `screenshot failed: ${e}\n`);
    }
  }

  // --- App-specific helpers -------------------------------------------------

  async statusText() {
    return (await this.driver.findElement(byTestId("connection-status"))).getText();
  }

  /** Click a kind (or Overview) in the sidebar. */
  async nav(label) {
    await (await this.find(By.xpath(`//nav//button[normalize-space()=${xq(label)}]`))).click();
  }

  /** Set an input's value the way typing would (fires `input`). */
  async setValue(el, value) {
    await this.js(
      `const el = arguments[0];
       const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement : HTMLInputElement;
       Object.getOwnPropertyDescriptor(proto.prototype, "value").set.call(el, arguments[1]);
       el.dispatchEvent(new Event("input", { bubbles: true }));
       el.dispatchEvent(new Event("change", { bubbles: true }));`,
      el,
      value,
    );
  }

  /** Type into the table's filter box. */
  async filter(text) {
    await this.setValue(await this.find(By.css('input[aria-label="Filter"]')), text);
  }

  /** [name, namespace] of every rendered table row. */
  rows() {
    return this.js(
      `return Array.from(document.querySelectorAll('[data-testid="resource-row"]'), (r) => {
         const c = r.querySelectorAll('[role="cell"]');
         return [c[1]?.textContent.trim() ?? "", c[2]?.textContent.trim() ?? ""];
       });`,
    );
  }

  /** The table row whose Name cell is `name` (and Namespace cell `namespace`). */
  row(name, namespace) {
    const cell = (v) => `[*[@role='cell'][normalize-space()=${xq(v)}]]`;
    return By.xpath(`//*[@data-testid='resource-row']${cell(name)}${namespace ? cell(namespace) : ""}`);
  }

  /** Open a kind, filter to `name` and click its row; waits for the detail panel. */
  async openDetail(kindLabel, namespace, name) {
    await this.nav(kindLabel);
    await this.filter(name);
    await (await this.find(this.row(name, namespace))).click();
    await this.waitFor(
      async () =>
        (await (await this.driver.findElement(By.css('[data-testid="detail-panel"] .title'))).getText()).includes(
          `${namespace}/${name}`,
        ),
      { message: `detail panel of ${namespace}/${name}` },
    );
  }
}
