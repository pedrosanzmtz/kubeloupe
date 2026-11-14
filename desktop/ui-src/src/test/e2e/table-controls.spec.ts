import { expect, test } from "@playwright/test";
import { column, openApp, openKind, row, rows } from "./helpers";

test("the filter narrows the rows and the footer count", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);

  const filter = page.getByRole("searchbox", { name: "Filter" });
  await filter.fill("WEB");
  expect(await column(page, "Name")).toEqual(["web-1", "web-2"]);
  await expect(page.getByText("2 of 5 items")).toBeVisible();

  // Any field matches, not just the name.
  await filter.fill("node-2");
  expect(await column(page, "Name")).toEqual(["crashed"]);

  await filter.fill("nothing-matches");
  await expect(rows(page)).toHaveCount(0);
  await expect(page.getByText("No items found.")).toBeVisible();

  await filter.fill("");
  await expect(rows(page)).toHaveCount(5);
});

test("a header click sorts ascending, then descending, then back to key order", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  const name = page.getByRole("columnheader", { name: /^Name\s*[▲▼]?$/ });
  const keyOrder = ["crashed", "db-0", "web-1", "web-2", "coredns-1"];
  expect(await column(page, "Name")).toEqual(keyOrder);

  await name.click();
  await expect(name).toHaveAttribute("aria-sort", "ascending");
  expect(await column(page, "Name")).toEqual(["coredns-1", "crashed", "db-0", "web-1", "web-2"]);

  await name.click();
  await expect(name).toHaveAttribute("aria-sort", "descending");
  expect(await column(page, "Name")).toEqual(["web-2", "web-1", "db-0", "crashed", "coredns-1"]);

  await name.click();
  await expect(name).toHaveAttribute("aria-sort", "none");
  expect(await column(page, "Name")).toEqual(keyOrder);
});

test("sorting by a metric column sorts by usage, missing metrics last", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await expect(row(page, "web-1").getByRole("gridcell", { name: "250m" })).toBeVisible();
  await page.getByRole("columnheader", { name: "CPU" }).click();
  await page.getByRole("columnheader", { name: "CPU" }).click();
  expect((await column(page, "Name")).slice(0, 2)).toEqual(["web-2", "web-1"]);
});

test("pod metrics are merged into the CPU and Memory columns", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(row(page, "web-1").getByRole("gridcell", { name: "250m" })).toBeVisible();
  await expect(row(page, "web-1").getByRole("gridcell", { name: "64Mi" })).toBeVisible();
  await expect(row(page, "web-2").getByRole("gridcell", { name: "1.5", exact: true })).toBeVisible();
  await expect(row(page, "web-2").getByRole("gridcell", { name: "1.5Gi" })).toBeVisible();
  // No metrics for this pod.
  expect(await column(page, "CPU")).toEqual(["—", "—", "250m", "1.5", "—"]);
  expect((await fake.calls("cmdGetMetrics")).length).toBeGreaterThan(0);
});

test("node metrics show usage over capacity", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Nodes");
  expect(await column(page, "CPU")).toEqual(["900m / 4", "—"]);
  expect(await column(page, "Memory")).toEqual(["2Gi / 8Gi", "—"]);
});

test("kinds without metric columns do not poll metrics", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await expect(rows(page)).toHaveCount(2);
  expect(await fake.calls("cmdGetMetrics")).toEqual([]);
});

test("select all selects the visible rows only; shift-click selects a range", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  const selection = page.getByRole("toolbar", { name: "Selection" });

  await page.getByRole("searchbox", { name: "Filter" }).fill("web");
  await page.getByRole("checkbox", { name: "Select all visible rows" }).check();
  await expect(selection).toContainText("2 selected");
  await expect(selection.getByRole("button", { name: "Delete 2…" })).toBeVisible();

  // Rows that are filtered away leave the selection.
  await page.getByRole("searchbox", { name: "Filter" }).fill("web-2");
  await expect(selection).toContainText("1 selected");
  await selection.getByRole("button", { name: "Clear" }).click();
  await expect(selection).toHaveCount(0);

  await page.getByRole("searchbox", { name: "Filter" }).fill("");
  await row(page, "crashed").getByRole("checkbox").click();
  await row(page, "web-2").getByRole("checkbox").click({ modifiers: ["Shift"] });
  await expect(selection).toContainText("4 selected");
  await expect(row(page, "coredns-1").getByRole("checkbox")).not.toBeChecked();
});
