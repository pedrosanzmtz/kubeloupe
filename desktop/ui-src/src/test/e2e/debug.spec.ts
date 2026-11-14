import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";

const dock = (page: Page) => page.getByTestId("dock");
const tabs = (page: Page) => dock(page).getByRole("tab");
const pane = (page: Page, tabId: string) => dock(page).locator(`[data-testid="dock-pane"][data-tab="${tabId}"]`);
const debugDialog = (page: Page) => page.getByTestId("debug-dialog");

async function menuItem(page: Page, name: string, item: string) {
  await row(page, name).click({ button: "right" });
  await page.getByRole("menu").getByRole("menuitem", { name: item, exact: true }).click();
}

test("Debug… picks a container and an image, warns, and attaches in a dock tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await menuItem(page, "web-1", "Debug…");

  const d = debugDialog(page);
  await expect(d).toBeVisible();
  await expect(d.locator(".lead")).toHaveText("demo/web-1");
  await expect(d.getByText("cannot be removed")).toBeVisible();
  // Regular containers only (not the init container), the first one chosen.
  const target = d.getByLabel("Target container");
  await expect(target.locator("option")).toHaveText(["app", "sidecar"]);
  await expect(target).toHaveValue("app");
  await expect(d.getByLabel("Debug image")).toHaveValue("busybox:1.37");

  await target.selectOption("sidecar");
  await d.getByLabel("Debug image").fill("nicolaka/netshoot");
  await d.getByTestId("debug-confirm").click();
  await expect(d).toHaveCount(0);

  await expect(tabs(page)).toHaveText(["Debug: web-1"]);
  await expect.poll(() => fake.calls("cmdDebugStart")).toHaveLength(1);
  const call = await fake.lastCall("cmdDebugStart");
  expect(call).toMatchObject({ namespace: "demo", name: "web-1", target: "sidecar", image: "nicolaka/netshoot" });
  expect(String(call.session)).toMatch(/^ui-/);
  // Progress written before the command returned shows in the terminal.
  const term = pane(page, "debug:demo/web-1/sidecar").getByTestId("terminal");
  await expect(term).toContainText("Adding a debug container (nicolaka/netshoot)");
  await expect(term).toContainText("Attached to debugger-abcde (target sidecar)");
  expect(await fake.calls("cmdExecStart")).toHaveLength(0);

  // Opening the same debug again focuses the tab instead of adding a container.
  await menuItem(page, "web-1", "Debug…");
  await debugDialog(page).getByLabel("Target container").selectOption("sidecar");
  await debugDialog(page).getByTestId("debug-confirm").click();
  await expect(tabs(page)).toHaveCount(1);
  await page.waitForTimeout(100);
  expect(await fake.calls("cmdDebugStart")).toHaveLength(1);

  await dock(page).getByRole("button", { name: "Close Debug: web-1" }).click();
  await expect.poll(() => fake.calls("cmdExecClose")).toEqual([{ session: call.session }]);
  expect(fake.errors).toEqual([]);
});

test("a forbidden debug container shows the reason in the terminal", async ({ page }) => {
  await openApp(page, (cluster) => {
    cluster.debugError =
      "Not allowed: adding a debug container needs permission to patch pods/ephemeralcontainers. pods \"web-1\" is forbidden";
  });
  await openKind(page, "Pods");
  await menuItem(page, "web-1", "Debug…");
  await debugDialog(page).getByRole("button", { name: "Cancel" }).click();
  await expect(debugDialog(page)).toHaveCount(0);
  await expect(dock(page)).toHaveCount(0);

  await menuItem(page, "web-1", "Debug…");
  await debugDialog(page).getByTestId("debug-confirm").click();
  const term = pane(page, "debug:demo/web-1/app").getByTestId("terminal");
  await expect(term).toContainText("Adding a debug container (busybox:1.37)");
  await expect(term).toContainText("Could not open terminal: Not allowed");
});

test("a node's Shell opens a node shell in the dock and closes it with the tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Nodes");
  await menuItem(page, "node-1", "Shell");
  await expect(tabs(page)).toHaveText(["Node: node-1"]);
  await expect.poll(() => fake.calls("cmdNodeShellStart")).toHaveLength(1);
  const call = await fake.lastCall("cmdNodeShellStart");
  expect(call).toMatchObject({ node: "node-1", namespace: "kube-system" });
  const term = pane(page, "node:node-1").getByTestId("terminal");
  await expect(term).toContainText("Starting pod kube-system/node-shell-node-1-abcde");
  await expect(term).toContainText("root@node-1:/#");

  // From the detail panel's actions too; the open tab is focused.
  await row(page, "node-1").click();
  await page.getByRole("toolbar", { name: "Actions" }).getByRole("button", { name: "Shell" }).click();
  await expect(tabs(page)).toHaveCount(1);

  await dock(page).getByRole("button", { name: "Close Node: node-1" }).click();
  await expect.poll(() => fake.calls("cmdExecClose")).toEqual([{ session: call.session }]);
});

test("a node shell blocked by Pod Security says what to do", async ({ page }) => {
  await openApp(page, (cluster) => {
    cluster.nodeShellError =
      "Pod Security Admission does not allow a privileged pod in kube-system. Try a namespace whose Pod Security level allows privileged pods.";
  });
  await openKind(page, "Nodes");
  await menuItem(page, "node-2", "Shell");
  const term = pane(page, "node:node-2").getByTestId("terminal");
  await expect(term).toContainText("Could not open terminal: Pod Security Admission does not allow");
});

test("node shell pods left behind are offered for deletion at start-up", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    cluster.nodeShellLeftovers = [
      { namespace: "kube-system", name: "node-shell-node-1-xyz12", node: "node-1" },
      { namespace: "kube-system", name: "node-shell-node-2-xyz34", node: "node-2" },
    ];
  });
  const offer = page.getByTestId("node-shell-leftovers");
  await expect(offer).toContainText("2 node shell pods from earlier sessions are still running.");
  await expect(offer).toContainText("kube-system/node-shell-node-1-xyz12");
  await offer.getByRole("button", { name: "Delete" }).click();
  await expect(offer).toHaveCount(0);
  expect(await fake.calls("cmdNodeShellCleanup")).toHaveLength(1);
  await expect(page.getByRole("status").filter({ hasText: "Deleted 2 node shell pods" })).toBeVisible();
});

test("the palette offers Debug for a selected pod", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-2").click();
  await page.keyboard.press("Control+k");
  await page.keyboard.type(">debug");
  await page.keyboard.press("Enter");
  await expect(debugDialog(page)).toBeVisible();
  await expect(debugDialog(page).locator(".lead")).toHaveText("demo/web-2");
  expect(fake.errors).toEqual([]);
});
