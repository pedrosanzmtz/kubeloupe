import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";
import type { Fake } from "./helpers";

const panel = (page: Page) => page.getByTestId("detail-panel");
const dock = (page: Page) => page.getByTestId("dock");
const tabs = (page: Page) => dock(page).getByRole("tab");
/** The pane of the tab with this id (`logs:ns/name`, `shell:ns/name`). */
const pane = (page: Page, tabId: string) => dock(page).locator(`[data-testid="dock-pane"][data-tab="${tabId}"]`);
const linesOf = (page: Page, tabId: string) => pane(page, tabId).getByTestId("log-line");

/** Open a pod's panel and click its Logs button; resolves to that tab's stream id. */
async function openPodLogs(page: Page, fake: Fake, name: string): Promise<number> {
  const before = (await fake.calls("cmdStreamPodLogs")).length;
  await row(page, name).click();
  await expect(page.locator("aside .title")).toHaveText(`demo/${name}`);
  await expect(panel(page).locator("h3", { hasText: "Containers" })).toBeVisible();
  await panel(page).getByRole("button", { name: "Logs", exact: true }).click();
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(before + 1);
  return (await fake.lastStreamId())!;
}

test("logs of two pods run side by side in two tabs, each with its own stream", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  const s1 = await openPodLogs(page, fake, "web-1");
  const s2 = await openPodLogs(page, fake, "web-2");

  await expect(tabs(page)).toHaveText(["Logs: web-1", "Logs: web-2"]);
  await expect(tabs(page).nth(1)).toHaveAttribute("aria-selected", "true");
  const calls = await fake.calls("cmdStreamPodLogs");
  expect(calls.map((c) => c.name)).toEqual(["web-1", "web-2"]);
  expect(s1).not.toBe(s2);
  expect(await fake.activeLogStreams()).toEqual([s1, s2]);

  // Each tab keeps its own buffer, also while hidden, across tab switches.
  await fake.emitLogLines(["one from web-1"], s1);
  await fake.emitLogLines(["one from web-2"], s2);
  await expect(linesOf(page, "logs:demo/web-2")).toHaveText(["one from web-2"]);
  await expect(pane(page, "logs:demo/web-1")).toBeHidden();

  await tabs(page).first().click();
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["one from web-1"]);
  await fake.emitLogLines(["two from web-2"], s2);
  await tabs(page).nth(1).click();
  await expect(linesOf(page, "logs:demo/web-2")).toHaveText(["one from web-2", "two from web-2"]);
  await tabs(page).first().click();
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["one from web-1"]);
  // Switching tabs restarts nothing.
  expect(await fake.calls("cmdStreamPodLogs")).toHaveLength(2);
  expect(fake.errors).toEqual([]);
});

test("closing a tab stops only its stream", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  const s1 = await openPodLogs(page, fake, "web-1");
  const s2 = await openPodLogs(page, fake, "web-2");

  await dock(page).getByRole("button", { name: "Close Logs: web-1" }).click();
  await expect(tabs(page)).toHaveText(["Logs: web-2"]);
  await expect.poll(() => fake.calls("cmdStopLogs")).toEqual([{ streamId: s1 }]);
  expect(await fake.activeLogStreams()).toEqual([s2]);

  await fake.emitLogLines(["web-2 still streaming"], s2);
  await expect(linesOf(page, "logs:demo/web-2")).toHaveText(["web-2 still streaming"]);

  // The last tab closes the dock.
  await dock(page).getByRole("button", { name: "Close Logs: web-2" }).click();
  await expect(dock(page)).toHaveCount(0);
  expect(await fake.activeLogStreams()).toEqual([]);
});

test("tabs survive closing the panel and changing the view; reopening focuses the tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  const s1 = await openPodLogs(page, fake, "web-1");
  await fake.emitLogLines(["kept"], s1);
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["kept"]);

  await panel(page).getByRole("button", { name: "Close details" }).click();
  await openKind(page, "Deployments");
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["kept"]);

  await openKind(page, "Pods");
  await row(page, "web-1").click();
  await expect(panel(page).locator("h3", { hasText: "Containers" })).toBeVisible();
  await panel(page).getByRole("button", { name: "Logs", exact: true }).click();
  await expect(tabs(page)).toHaveCount(1);
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["kept"]);
  expect(await fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  expect(await fake.calls("cmdStopLogs")).toHaveLength(0);
});

test("the row menu opens logs and a shell; a terminal keeps its session while hidden", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").click({ button: "right" });
  await page.getByRole("menu").getByRole("menuitem", { name: "Shell" }).click();
  await expect(tabs(page)).toHaveText(["Shell: web-1"]);
  // From the menu the tab looks the pod up and execs into its first regular container.
  await expect.poll(() => fake.calls("cmdExecStart")).toHaveLength(1);
  expect(await fake.lastCall("cmdExecStart")).toMatchObject({ namespace: "demo", name: "web-1", container: "app" });
  await expect(pane(page, "shell:demo/web-1").getByTestId("terminal")).toBeVisible();

  await row(page, "web-2").click({ button: "right" });
  await page.getByRole("menu").getByRole("menuitem", { name: "Logs" }).click();
  await expect(tabs(page)).toHaveText(["Shell: web-1", "Logs: web-2"]);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  // The terminal is hidden, not closed.
  await expect(pane(page, "shell:demo/web-1")).toBeHidden();
  expect(await fake.calls("cmdExecClose")).toHaveLength(0);

  await dock(page).getByRole("button", { name: "Close Shell: web-1" }).click();
  await expect.poll(() => fake.calls("cmdExecClose")).toEqual([{ session: "exec-1" }]);
  expect(await fake.calls("cmdStopLogs")).toHaveLength(0);
});

test("a workload's Logs button opens its merged logs in the dock", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  await panel(page).getByRole("button", { name: "Logs", exact: true }).click();
  await expect(tabs(page)).toHaveText(["Logs: deployment/web"]);
  await expect.poll(() => fake.calls("cmdStreamWorkloadLogs")).toHaveLength(1);
  expect(await fake.lastCall("cmdStreamWorkloadLogs")).toMatchObject({ kind: "Deployment", namespace: "demo", name: "web" });
  // The panel has no inline log viewer any more.
  await expect(panel(page).getByTestId("log-output")).toHaveCount(0);
});

test("the backend's stream cap is reported in the tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await fake.failNext("cmdStreamPodLogs", "Too many log streams: 10 are already open. Close a log tab to open another.");
  await openPodLogs(page, fake, "web-1");
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText([
    "Could not start log stream: Too many log streams: 10 are already open. Close a log tab to open another.",
  ]);
});

test("Ctrl+` collapses and expands the dock; double-click maximizes a tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  const s1 = await openPodLogs(page, fake, "web-1");
  await fake.emitLogLines(["hello"], s1);
  const output = pane(page, "logs:demo/web-1").getByTestId("log-output");
  await expect(output).toBeVisible();

  await page.keyboard.press("Control+Backquote");
  await expect(output).toBeHidden();
  await expect(tabs(page)).toHaveCount(1);
  await page.keyboard.press("Control+Backquote");
  await expect(linesOf(page, "logs:demo/web-1")).toHaveText(["hello"]);

  await tabs(page).first().dblclick();
  await expect(page.getByTestId("resource-table")).toBeHidden();
  await dock(page).getByRole("button", { name: "Restore dock" }).click();
  await expect(page.getByTestId("resource-table")).toBeVisible();
  expect(await fake.calls("cmdStreamPodLogs")).toHaveLength(1);
});

test("dragging the top edge resizes the dock", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await openPodLogs(page, fake, "web-1");
  const before = (await dock(page).boundingBox())!.height;
  const handle = (await page.getByTestId("dock-resizer").boundingBox())!;
  await page.mouse.move(handle.x + 50, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle.x + 50, handle.y - 100, { steps: 5 });
  await page.mouse.up();
  // About 100px taller (the handle straddles the edge, so give or take a few).
  await expect.poll(async () => Math.abs((await dock(page).boundingBox())!.height - (before + 100))).toBeLessThan(8);
});

test("a context switch closes every tab", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await openPodLogs(page, fake, "web-1");
  await expect(tabs(page)).toHaveCount(1);
  await page.getByLabel("Cluster context").selectOption("prod");
  await expect(dock(page)).toHaveCount(0);
  expect(await fake.activeLogStreams()).toEqual([]);
});
