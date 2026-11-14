import { expect, test } from "@playwright/test";
import { column, namespaceButton, namespacePopover, openApp, openKind, rows, toggleNamespaces } from "./helpers";

const POD = { group: "", version: "v1", kind: "Pod", plural: "pods", namespaced: true };

test("two namespaces: one watch each, feeding one table", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);

  await toggleNamespaces(page, "demo", "kube-system");
  // All namespaces -> demo -> demo + kube-system: the demo watch is kept.
  expect((await fake.calls("cmdWatchStart")).slice(1)).toEqual([
    { resource: POD, namespace: "demo" },
    { resource: POD, namespace: "kube-system" },
  ]);
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual(["demo", "kube-system"]);
  await expect(rows(page)).toHaveCount(5);
  await expect(page.locator("header")).toContainText("Live");
  await expect(namespaceButton(page)).toHaveText("2 namespaces");
  await expect(page.getByRole("list", { name: "Selected namespaces" }).getByRole("listitem")).toHaveText([
    /demo/,
    /kube-system/,
  ]);
  // Metrics are polled for the shown namespaces only.
  await expect
    .poll(async () => (await fake.calls("cmdGetMetrics")).slice(-2).map((c) => c.namespace))
    .toEqual(["demo", "kube-system"]);
  expect(fake.errors).toEqual([]);
});

test("a snapshot from one namespace's watch does not remove the other's rows", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await toggleNamespaces(page, "demo", "kube-system");
  await expect(rows(page)).toHaveCount(5);
  const [demo, kubeSystem] = await fake.activeWatches();

  // demo relists with a single pod: kube-system's coredns-1 stays.
  await fake.emitWatchBatch({ id: demo.id, snapshot: true, applied: [fake.podRow("demo", "web-9")] });
  await expect.poll(() => column(page, "Name")).toEqual(["coredns-1", "web-9"]);

  // And the other way round.
  await fake.emitWatchBatch({ id: kubeSystem.id, snapshot: true, applied: [] });
  await expect.poll(() => column(page, "Name")).toEqual(["web-9"]);
});

test("deselecting a namespace stops its watch and removes its rows", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await toggleNamespaces(page, "demo", "kube-system");
  await expect(rows(page)).toHaveCount(5);
  const [demo, kubeSystem] = await fake.activeWatches();

  await page.getByRole("button", { name: "Remove demo" }).click();
  await expect.poll(() => column(page, "Name")).toEqual(["coredns-1"]);
  expect((await fake.calls("cmdWatchStop")).at(-1)).toEqual({ id: demo.id });
  expect(await fake.activeWatches()).toEqual([kubeSystem]);
  await expect(namespaceButton(page)).toHaveText("kube-system");

  // Back to All namespaces: one watch without a namespace.
  await namespaceButton(page).click();
  await namespacePopover(page).getByRole("checkbox", { name: "All namespaces" }).check();
  await expect(rows(page)).toHaveCount(5);
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual([null]);
  await expect(namespaceButton(page)).toHaveText("All namespaces");
});

test("a reconnecting namespace is named in the header", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await toggleNamespaces(page, "demo", "kube-system");
  await expect(page.locator("header")).toContainText("Live");
  const [, kubeSystem] = await fake.activeWatches();

  await fake.emitWatchState({ id: kubeSystem.id, state: "reconnecting", error: "connection reset" });
  await expect(page.locator("header")).toContainText("Reconnecting… (kube-system)");
  await expect(page.getByRole("alert")).toHaveText("Watch of kube-system interrupted, retrying: connection reset");
  await expect(rows(page)).toHaveCount(5);

  await fake.emitWatchState({ id: kubeSystem.id, state: "live" });
  await expect(page.locator("header")).toContainText("Live");
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("the control is usable from the keyboard and searches", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);

  await namespaceButton(page).focus();
  await page.keyboard.press("Enter");
  const search = page.getByRole("searchbox", { name: "Search namespaces" });
  await expect(search).toBeFocused();
  await search.fill("kube");
  await expect(namespacePopover(page).getByRole("group").getByRole("checkbox")).toHaveCount(1);
  await search.press("Enter");
  await expect(namespaceButton(page)).toHaveText("kube-system");
  await expect(rows(page)).toHaveCount(1);

  // The arrows walk the list; Space ticks.
  await search.press("ArrowDown"); // All namespaces
  await page.keyboard.press("ArrowDown"); // default
  await page.keyboard.press("ArrowDown"); // demo
  await page.keyboard.press("Space");
  await expect(namespaceButton(page)).toHaveText("2 namespaces");
  await expect(rows(page)).toHaveCount(5);

  await page.keyboard.press("Escape");
  await expect(namespacePopover(page)).toHaveCount(0);
  await expect(namespaceButton(page)).toBeFocused();
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual(["kube-system", "demo"]);
});

test("the selection is capped at 10 namespaces, suggesting All", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    cluster.namespaces = Array.from({ length: 12 }, (_, i) => ({
      name: `ns-${String(i).padStart(2, "0")}`,
      status: "Active",
      created: 0,
      age: "1d",
    }));
  });
  await openKind(page, "Pods");
  await toggleNamespaces(page, ...Array.from({ length: 10 }, (_, i) => `ns-${String(i).padStart(2, "0")}`));
  await expect(namespaceButton(page)).toHaveText("10 namespaces");
  expect(await fake.activeWatches()).toHaveLength(10);

  await namespaceButton(page).click();
  await expect(namespacePopover(page)).toContainText("At most 10 namespaces. Choose All namespaces to see more.");
  await expect(namespacePopover(page).getByRole("checkbox", { name: "ns-10" })).toBeDisabled();
  await expect(namespacePopover(page).getByRole("checkbox", { name: "ns-00" })).toBeEnabled();
});
