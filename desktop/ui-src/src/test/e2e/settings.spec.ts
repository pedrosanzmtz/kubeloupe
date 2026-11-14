import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { namespaceButton, namespacePopover, openApp, openKind, row, toggleNamespaces } from "./helpers";

// What the app remembers across restarts (#39). The fake backend keeps the
// settings in sessionStorage, so a page reload stands in for a restart.

/** Reload the page (restart the app) and wait for discovery. */
async function restart(page: Page) {
  await page.reload();
  await expect(page.getByRole("navigation").getByRole("button", { name: "Pods", exact: true })).toBeVisible();
}

const header = (page: Page, label: string) => page.getByRole("columnheader", { name: label, exact: false });
const activeKind = (page: Page) => page.getByRole("navigation").locator("button.active");

test("the view and the table sort survive a restart", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await header(page, "Restarts").click();
  await header(page, "Restarts").click();
  await expect(header(page, "Restarts")).toHaveAttribute("aria-sort", "descending");

  await restart(page);
  await expect(activeKind(page)).toHaveText("Pods");
  await expect(header(page, "Restarts")).toHaveAttribute("aria-sort", "descending");
  // Other kinds keep their own (no) sort.
  await openKind(page, "Deployments");
  await expect(page.locator('[role="columnheader"][aria-sort="descending"]')).toHaveCount(0);
  expect(fake.errors).toEqual([]);
});

test("hidden columns are chosen per kind and remembered", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await page.getByRole("button", { name: "Columns" }).click();
  const chooser = page.getByRole("dialog", { name: "Columns" });
  await expect(chooser.getByRole("checkbox", { name: "Name", exact: true })).toBeDisabled();
  await chooser.getByRole("checkbox", { name: "Node", exact: true }).uncheck();
  await page.keyboard.press("Escape");
  await expect(chooser).toHaveCount(0);
  await expect(header(page, "Node")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Columns (1 hidden)" })).toBeVisible();

  await restart(page);
  await expect(activeKind(page)).toHaveText("Pods");
  await expect(header(page, "Node")).toHaveCount(0);
  await expect(header(page, "Status")).toBeVisible();

  await page.getByRole("button", { name: "Columns (1 hidden)" }).click();
  await chooser.getByRole("button", { name: "Show all" }).click();
  await expect(header(page, "Node")).toBeVisible();
});

test("the context and its namespaces survive a restart", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await toggleNamespaces(page, "demo");
  await expect(namespaceButton(page)).toHaveText("demo");

  await page.getByRole("combobox", { name: "Cluster context" }).selectOption("prod");
  await expect.poll(async () => (await fake.calls("cmdSetContext")).length).toBe(1);
  // prod has nothing remembered yet: the overview and all namespaces.
  await expect(activeKind(page)).toHaveText("Overview");

  await restart(page);
  // Restarted on prod. The fake's kubeconfig still says kind-dev is current.
  expect((await fake.lastCall("cmdSetContext")).name).toBe("prod");
  await expect(page.getByRole("combobox", { name: "Cluster context" })).toHaveValue("prod");
  await expect(activeKind(page)).toHaveText("Overview");

  // Back on kind-dev, its view and namespace come back.
  await page.getByRole("combobox", { name: "Cluster context" }).selectOption("kind-dev");
  await expect(activeKind(page)).toHaveText("Pods");
  await expect(namespaceButton(page)).toHaveText("demo");
  expect(fake.errors).toEqual([]);
});

test("what no longer exists falls back safely", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    cluster.settings.last_context = "deleted-cluster";
    cluster.settings.contexts["kind-dev"] = {
      view: "example.org/v1/vanished",
      namespaces: ["gone"],
      typed_namespaces: [],
    };
  });
  // No switch to a context that is gone, the overview instead of a kind
  // that is not served, and all namespaces instead of a deleted one.
  expect(await fake.calls("cmdSetContext")).toEqual([]);
  await expect(activeKind(page)).toHaveText("Overview");
  await openKind(page, "Pods");
  await expect(namespaceButton(page)).toHaveText("All namespaces");
  expect(fake.errors).toEqual([]);
});

test("the app starts with the defaults when the settings cannot be read", async ({ page }) => {
  const fake = await openApp(page, (_cluster, f) => {
    f.failNext("cmdGetSettings", "settings store unavailable");
  });
  await expect(activeKind(page)).toHaveText("Overview");
  await openKind(page, "Pods");
  await expect(row(page, "web-1")).toBeVisible();
  expect(fake.errors).toEqual([]);
});

test("namespaces typed by a namespace-scoped user are remembered", async ({ page }) => {
  await openApp(page, (cluster) => {
    cluster.contexts = [{ name: "limited", current: true, namespace: "team-a" }];
    cluster.namespaces = null;
    cluster.defaultNamespace = "team-a";
  });
  await openKind(page, "Pods");
  await namespaceButton(page).click();
  const search = page.getByRole("searchbox", { name: "Search namespaces" });
  await search.fill("team-b");
  await search.press("Enter");
  await expect(namespaceButton(page)).toHaveText("2 namespaces");
  await page.keyboard.press("Escape");

  await restart(page);
  await expect(namespaceButton(page)).toHaveText("2 namespaces");
  await namespaceButton(page).click();
  await namespacePopover(page).getByRole("checkbox", { name: "team-b", exact: true }).click();
  await expect(namespaceButton(page)).toHaveText("team-a");
  // Unticked, but still offered.
  await expect(namespacePopover(page).getByRole("checkbox", { name: "team-b", exact: true })).toBeVisible();
});

test("the Settings page sets the theme and the log defaults", async ({ page }) => {
  const fake = await openApp(page);
  const html = page.locator("html");
  await expect(html).not.toHaveAttribute("data-theme");

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByRole("radio", { name: "Light" }).check();
  await expect(html).toHaveAttribute("data-theme", "light");
  const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(bg).toBe("rgb(245, 246, 248)");
  await dialog.getByRole("checkbox", { name: "Show timestamps" }).check();
  await dialog.getByLabel("Default tail lines").selectOption("1000");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  await restart(page);
  await expect(html).toHaveAttribute("data-theme", "light");
  // New log viewers start from the defaults.
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  await page.getByTestId("detail-panel").getByRole("button", { name: "Logs", exact: true }).click();
  const logs = page.getByTestId("dock");
  await expect(logs.getByRole("checkbox", { name: "Timestamps" })).toBeChecked();
  await expect(logs.getByTitle("Tail lines")).toHaveValue("1000");
  await expect.poll(async () => (await fake.lastCall("cmdStreamPodLogs")).tailLines).toBe(1000);

  // Dark, from the keyboard shortcut this time.
  await page.keyboard.press("Control+,");
  await dialog.getByRole("radio", { name: "Dark" }).check();
  await expect(html).toHaveAttribute("data-theme", "dark");
  await dialog.getByRole("button", { name: "Close" }).click();
  expect(fake.errors).toEqual([]);
});

test("Settings is in the command palette", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Control+k");
  await page.keyboard.type("settings");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
});

test("the detail panel width is resizable and remembered", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  const panel = page.getByTestId("detail-panel");
  await expect(panel).toBeVisible();
  const before = (await panel.boundingBox())!.width;
  expect(Math.round(before)).toBe(420);

  const handle = page.getByTestId("panel-resizer");
  const box = (await handle.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x - 150, box.y + 100, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await panel.boundingBox())!.width)).toBeGreaterThan(540);
  const after = Math.round((await panel.boundingBox())!.width);

  await restart(page);
  await row(page, "web-1").click();
  await expect.poll(async () => Math.round((await panel.boundingBox())!.width)).toBe(after);

  // The keyboard works too.
  await page.getByTestId("panel-resizer").focus();
  await page.keyboard.press("ArrowRight");
  await expect.poll(async () => Math.round((await panel.boundingBox())!.width)).toBe(after - 16);
});

test("the dock height is remembered", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  await page.getByTestId("detail-panel").getByRole("button", { name: "Logs", exact: true }).click();
  const dock = page.getByTestId("dock");
  const before = Math.round((await dock.boundingBox())!.height);
  const handle = (await page.getByTestId("dock-resizer").boundingBox())!;
  await page.mouse.move(handle.x + 200, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle.x + 200, handle.y - 100, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => Math.round((await dock.boundingBox())!.height)).toBeGreaterThan(before + 80);
  const after = Math.round((await dock.boundingBox())!.height);

  await restart(page);
  await row(page, "web-1").click();
  await page.getByTestId("detail-panel").getByRole("button", { name: "Logs", exact: true }).click();
  await expect.poll(async () => Math.round((await dock.boundingBox())!.height)).toBe(after);
});
