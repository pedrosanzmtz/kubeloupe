// Smoke suite: the real app (embedded frontend, real IPC, real watches, logs
// and exec) against the kind test cluster. The tests run in order in one app
// session; each one screenshots its end state (artifacts/smoke-NN-*.png).
//
// Run: see README.md (`npm --prefix desktop/e2e test`).

import assert from "node:assert/strict";
import { after, afterEach, before, describe, it } from "node:test";
import { By, Key } from "selenium-webdriver";
import { byTestId, launch, xq } from "./lib/app.mjs";
import { createFixture, deleteFixture, kubectl, podNames } from "./lib/cluster.mjs";

const TEST_TIMEOUT = 90_000;

/** @type {import("./lib/app.mjs").App} */
let app;
let fx;

describe("kubeloupe on WebKitGTK against kind", { timeout: 15 * 60_000 }, () => {
  before(async () => {
    fx = createFixture();
    app = await launch("smoke");
  });

  afterEach(async (t) => {
    await app?.shot(t.name);
  });

  after(async () => {
    await app?.close();
    deleteFixture(fx);
  });

  it("starts connected, with non-zero Overview cards", { timeout: TEST_TIMEOUT }, async () => {
    await app.waitFor(async () => (await app.statusText()) === "Connected", {
      timeout: 60_000,
      message: 'header status "Connected"',
    });
    const cards = await app.waitFor(
      async () => {
        const nums = await app.js(
          `return Array.from(document.querySelectorAll(".overview .card"), (c) => [
             c.querySelector(".label")?.textContent.trim(), c.querySelector(".num")?.textContent.trim()]);`,
        );
        return nums.length > 0 ? Object.fromEntries(nums) : null;
      },
      { message: "Overview cards" },
    );
    assert.ok(Number(cards["Namespaces"]) > 0, `Namespaces card: ${cards["Namespaces"]}`);
    assert.ok(Number(cards["Pods"]) > 0, `Pods card: ${cards["Pods"]}`);
    assert.match(cards["Nodes ready"] ?? "", /^[1-9]\d*\/[1-9]\d*$/);
  });

  it("lists the demo pods and goes Live", { timeout: TEST_TIMEOUT }, async () => {
    await app.nav("Pods");
    await app.waitFor(async () => (await app.statusText()) === "Live", { message: 'header status "Live"' });
    const expectations = [
      ["web-", (n, ns) => ns === "demo" && n.startsWith("web-")],
      ["logger-", (n, ns) => ns === "demo" && n.startsWith("logger-")],
      ["crashed", (n, ns) => ns === "demo" && n === "crashed"],
      ["hungry", (n, ns) => ns === "default" && n === "hungry"],
    ];
    for (const [query, match] of expectations) {
      await app.filter(query);
      await app.waitFor(async () => (await app.rows()).some(([n, ns]) => match(n, ns)), {
        message: `a pod row matching "${query}"`,
      });
    }
    await app.filter("");
  });

  it("removes a deleted pod and shows its replacement without a refresh", { timeout: TEST_TIMEOUT }, async () => {
    const [old] = podNames(fx.namespace, `app=${fx.deployment}`);
    assert.ok(old, "fixture pod exists");
    await app.nav("Pods");
    await app.filter(fx.deployment);
    await app.find(app.row(old, fx.namespace));
    kubectl(["-n", fx.namespace, "delete", "pod", old, "--wait=false", "--grace-period=0", "--force"]);
    const replacement = await app.waitFor(
      async () => {
        const names = (await app.rows()).filter(([, ns]) => ns === fx.namespace).map(([n]) => n);
        return !names.includes(old) && names.find((n) => n.startsWith(`${fx.deployment}-`));
      },
      { timeout: 60_000, message: `${old} to disappear and a replacement to appear` },
    );
    assert.notEqual(replacement, old);
  });

  it("shows log lines of demo/logger-*", { timeout: TEST_TIMEOUT }, async () => {
    const [pod] = podNames("demo", "app=logger");
    assert.ok(pod, "a demo logger pod exists");
    await app.openDetail("Pods", "demo", pod);
    await app.waitFor(
      async () => (await app.driver.findElements(By.css('[data-testid="detail-panel"] [data-testid="log-line"]'))).length > 0,
      { timeout: 20_000, message: "log lines in the pod panel" },
    );
  });

  it("runs a command in the pod terminal", { timeout: TEST_TIMEOUT }, async () => {
    // Our own pod: the shell session leaves the shared demo pods alone.
    const [pod] = podNames(fx.namespace, `app=${fx.deployment}`);
    await app.openDetail("Pods", fx.namespace, pod);
    await (await app.find(By.xpath("//*[@data-testid='detail-panel']//button[normalize-space()='Open terminal']"))).click();
    await app.find(byTestId("terminal"));
    const screen = () => app.js(`return document.querySelector('[data-testid="terminal"] .xterm-rows')?.textContent ?? ""`);
    // Wait for the shell prompt before typing.
    await app.waitFor(async () => /[#$]/.test(await screen()), { timeout: 30_000, message: "a shell prompt" });
    const input = await app.driver.findElement(By.css('[data-testid="terminal"] .xterm-helper-textarea'));
    await input.sendKeys("echo e2e-$((1+1))", Key.ENTER);
    await app.waitFor(async () => (await screen()).includes("e2e-2"), {
      timeout: 20_000,
      message: '"e2e-2" in the terminal',
    });
  });

  it("saves an edited ConfigMap from the YAML tab", { timeout: TEST_TIMEOUT }, async () => {
    await app.openDetail("Config Maps", fx.namespace, fx.configMap);
    await (
      await app.find(By.xpath("//*[@data-testid='detail-panel']//button[@role='tab'][normalize-space()='YAML']"))
    ).click();
    // The editor is CodeMirror; its root element exposes `kubeloupeEditor`
    // (getValue / setValue, which edits like typing) once it has loaded.
    const editor = await app.find(byTestId("yaml-editor"));
    const yaml = await app.waitFor(
      async () => {
        const v = await app.js("return arguments[0].kubeloupeEditor?.getValue() ?? null", editor);
        return v?.includes("greeting: before") ? v : null;
      },
      { message: "the ConfigMap's YAML in the editor" },
    );
    await app.js(
      "arguments[0].kubeloupeEditor.setValue(arguments[1])",
      editor,
      yaml.replace("greeting: before", "greeting: after"),
    );
    // Review runs a server dry run and shows the diff; Save is on that screen.
    const review = await app.find(byTestId("yaml-review"));
    await app.waitFor(() => review.isEnabled(), { message: "Review to be enabled by the edit" });
    await review.click();
    await (await app.find(byTestId("yaml-save"))).click();
    await app.waitFor(async () => /saved/i.test(await (await app.driver.findElement(byTestId("yaml-status"))).getText()), {
      message: "the save to finish",
    });
    await app.waitFor(
      () => kubectl(["-n", fx.namespace, "get", "configmap", fx.configMap, "-o", "jsonpath={.data.greeting}"]) === "after",
      { timeout: 10_000, message: "kubectl to return the new value" },
    );
  });

  it("follows workload logs of Deployment demo/web with pod prefixes", { timeout: TEST_TIMEOUT }, async () => {
    await app.openDetail("Deployments", "demo", "web");
    await (
      await app.find(By.xpath("//*[@data-testid='detail-panel']//button[@role='tab'][normalize-space()='Logs']"))
    ).click();
    const prefix = await app.waitFor(
      () =>
        app.js(
          `const line = document.querySelector('[data-testid="detail-panel"] [data-testid="log-line"] .pod');
           return line ? line.textContent.trim() : null;`,
        ),
      { timeout: 30_000, message: "a pod-prefixed log line" },
    );
    assert.match(prefix, /web-/);
  });

  it("scales a Deployment from the context menu", { timeout: TEST_TIMEOUT }, async () => {
    await app.nav("Deployments");
    await app.filter(fx.deployment);
    const row = await app.find(app.row(fx.deployment, fx.namespace));
    await app.driver.actions().contextClick(row).perform();
    await (await app.find(By.xpath(`//*[@data-testid='context-menu']//button[normalize-space()=${xq("Scale…")}]`))).click();
    const replicas = await app.find(byTestId("action-replicas"));
    await app.setValue(replicas, "2");
    await app.waitFor(async () => (await (await app.driver.findElement(byTestId("action-confirm"))).getText()) === "Scale to 2", {
      message: 'the "Scale to 2" button',
    });
    await (await app.driver.findElement(byTestId("action-confirm"))).click();
    await app.waitFor(
      () => kubectl(["-n", fx.namespace, "get", "deploy", fx.deployment, "-o", "jsonpath={.spec.replicas}"]) === "2",
      { timeout: 15_000, message: "spec.replicas to be 2" },
    );
    await app.waitFor(async () => (await app.driver.findElements(byTestId("action-dialog"))).length === 0, {
      message: "the dialog to close",
    });
  });
});
