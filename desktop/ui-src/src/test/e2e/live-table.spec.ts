import { expect, test } from "@playwright/test";
import { column, openApp, openKind, row, rows, toggleNamespaces } from "./helpers";

test("a watch snapshot renders rows", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");

  await expect(rows(page)).toHaveCount(5);
  expect(await column(page, "Name")).toEqual(["crashed", "db-0", "web-1", "web-2", "coredns-1"]);
  await expect(row(page, "crashed").getByRole("gridcell", { name: "CrashLoopBackOff" })).toHaveClass(
    /phase-CrashLoopBackOff/,
  );
  await expect(page.getByText("5 of 5 items")).toBeVisible();
  await expect(page.locator("header")).toContainText("Live");

  const start = await fake.lastCall("cmdWatchStart");
  expect(start).toEqual({
    resource: { group: "", version: "v1", kind: "Pod", plural: "pods", namespaced: true },
    namespace: null,
  });
  expect(fake.errors).toEqual([]);
});

test("the namespace selector restarts the watch for that namespace", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);

  await toggleNamespaces(page, "kube-system");
  await expect(rows(page)).toHaveCount(1);
  expect((await fake.lastCall("cmdWatchStart")).namespace).toBe("kube-system");
  expect(await fake.calls("cmdWatchStop")).toEqual([{ id: "watch-1" }]);
});

test("an incremental batch updates and deletes rows, keeping scroll and selection", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);

  // Enough rows to scroll: pod-000 … pod-199.
  const many = Array.from({ length: 200 }, (_, i) => fake.podRow("demo", `pod-${String(i).padStart(3, "0")}`));
  await fake.emitWatchBatch({ snapshot: true, applied: many });
  await expect(page.getByText("200 of 200 items")).toBeVisible();

  const scroller = page.locator(".table .scroller");
  await scroller.evaluate((el) => (el.scrollTop = 28 * 100));
  await expect(row(page, "pod-100")).toBeVisible();
  await row(page, "pod-102").getByRole("checkbox").check();
  await row(page, "pod-104").getByRole("checkbox").check();
  await expect(page.getByRole("toolbar", { name: "Selection" })).toContainText("2 selected");

  await fake.emitWatchBatch({
    applied: [fake.podRow("demo", "pod-102", "Terminating"), fake.podRow("demo", "pod-new")],
    deleted: ["demo/pod-101", "demo/pod-000"],
  });

  await expect(page.getByText("199 of 199 items")).toBeVisible();
  await expect(row(page, "pod-101")).toHaveCount(0);
  await expect(row(page, "pod-102").getByRole("gridcell", { name: "Terminating" })).toBeVisible();
  // Rows above the viewport were deleted, yet the scroll offset is unchanged…
  expect(await scroller.evaluate((el) => el.scrollTop)).toBe(28 * 100);
  // …and the checked rows are still checked.
  await expect(row(page, "pod-102").getByRole("checkbox")).toBeChecked();
  await expect(row(page, "pod-104").getByRole("checkbox")).toBeChecked();
  await expect(page.getByRole("toolbar", { name: "Selection" })).toContainText("2 selected");
});

test("deleting a checked row drops it from the selection", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").getByRole("checkbox").check();
  await row(page, "web-2").getByRole("checkbox").check();
  await fake.emitWatchBatch({ deleted: ["demo/web-1"] });
  await expect(page.getByRole("toolbar", { name: "Selection" })).toContainText("1 selected");
});

test("batches from a stale watch are ignored", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);
  const podsWatch = await fake.lastWatchId();

  await openKind(page, "Deployments");
  await expect(rows(page)).toHaveCount(2);
  expect(await fake.lastWatchId()).not.toBe(podsWatch);

  // The pods watch was stopped; a straggler from it must not leak into the
  // deployments table, as a snapshot or as an incremental batch.
  await fake.emitWatchBatch({ id: podsWatch!, snapshot: true, applied: [fake.podRow("demo", "ghost")] });
  await fake.emitWatchBatch({ id: podsWatch!, deleted: ["demo/web"] });
  await fake.emitWatchState({ id: podsWatch!, state: "reconnecting", error: "old watch" });
  // A batch for the current watch is applied, so the ones above were processed.
  await fake.emitWatchBatch({ deleted: ["demo/api"] });

  await expect(rows(page)).toHaveCount(1);
  expect(await column(page, "Name")).toEqual(["web"]);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("the reconnecting state shows the banner until the watch is live again", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(page.locator("header")).toContainText("Live");

  await fake.emitWatchState({ state: "reconnecting", error: "connection reset by peer" });
  await expect(page.getByRole("alert")).toHaveText("Watch interrupted, retrying: connection reset by peer");
  await expect(page.locator("header")).toContainText("Reconnecting…");
  // The rows stay while reconnecting.
  await expect(rows(page)).toHaveCount(5);

  await fake.emitWatchState({ state: "live" });
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.locator("header")).toContainText("Live");
});

test("a failed watch start says the cluster is unreachable", async ({ page }) => {
  const fake = await openApp(page);
  await fake.failNext("cmdWatchStart", "connection refused");
  await openKind(page, "Pods");
  await expect(page.getByRole("alert")).toHaveText("Could not reach the cluster: connection refused");
  await expect(page.locator("header")).toContainText("Cluster unreachable");
});
