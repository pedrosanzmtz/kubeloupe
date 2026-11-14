import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { column, openApp, openKind, row } from "./helpers";

// Usage history (#47): sparklines in the metric cells, charts in the pod and
// node panels, from the fake series in fixtures/usage.ts.

const panel = (page: Page) => page.getByTestId("detail-panel");
const cell = (page: Page, name: string, value: string) =>
  row(page, name).getByRole("gridcell", { name: value, exact: true });

test("pod rows with history get a sparkline in the CPU and Memory cells", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  const cpu = cell(page, "web-1", "250m");
  await expect(cpu.getByTestId("sparkline")).toBeVisible();
  // 12 samples, one point each.
  expect((await cpu.locator("polyline").getAttribute("points"))?.split(" ")).toHaveLength(12);
  await expect(cell(page, "web-1", "64Mi").getByTestId("sparkline")).toBeVisible();
  await expect(cpu).toHaveAttribute("title", /^250m, trend since \d\d:\d\d$/);
  // web-2 has metrics but no history yet: no sparkline, the text alone.
  await expect(cell(page, "web-2", "1.5").getByTestId("sparkline")).toHaveCount(0);
  // The sparkline adds no text to the cell.
  expect(await column(page, "CPU")).toEqual(["—", "—", "250m", "1.5", "—"]);
});

test("node rows get sparklines too", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Nodes");
  await expect(cell(page, "node-1", "900m / 4").getByTestId("sparkline")).toBeVisible();
  await expect(cell(page, "node-1", "2Gi / 8Gi").getByTestId("sparkline")).toBeVisible();
});

test("the pod panel charts usage per container against requests and limits", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    // Give the pod a memory limit too.
    cluster.usage["pods|demo/web-1"].reference.memory_limit = 128 * 1024 * 1024;
  });
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  const charts = panel(page).getByTestId("usage-charts");
  await expect(charts.getByTestId("usage-since")).toHaveText(/^Collecting since \d\d:\d\d\./);
  const cpu = charts.getByTestId("usage-chart-cpu");
  const mem = charts.getByTestId("usage-chart-memory");
  // The pod's total and one line per container.
  for (const series of ["pod", "app", "sidecar"]) {
    await expect(cpu.locator(`polyline[data-series="${series}"]`)).toHaveCount(1);
  }
  await expect(cpu.locator("line[data-ref]")).toHaveCount(1);
  await expect(cpu.getByTestId("ref-request")).toHaveText("request 100m");
  await expect(mem.locator("line[data-ref]")).toHaveCount(2);
  await expect(mem.getByTestId("ref-request")).toHaveText("request 64Mi");
  await expect(mem.getByTestId("ref-limit")).toHaveText("limit 128Mi");
  // The last memory sample: app 95Mi + sidecar 8Mi.
  await expect(mem.locator("figcaption")).toContainText("now 103Mi");
  expect(await fake.calls("cmdUsageHistory")).toEqual([{ what: "pods", namespace: "demo", name: "web-1" }]);
});

test("a pod without history says so, and the panel stops polling when it closes", async ({ page }) => {
  await page.clock.install();
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-2").click();
  await expect(panel(page).getByTestId("usage-empty")).toBeVisible();
  await expect(panel(page).getByTestId("usage-chart-cpu")).toHaveCount(0);
  await page.clock.runFor(20_000);
  await expect.poll(async () => (await fake.calls("cmdUsageHistory")).length).toBe(2);
  await panel(page).getByRole("button", { name: "Close details" }).click();
  await expect(panel(page)).toHaveCount(0);
  await page.clock.runFor(60_000);
  expect((await fake.calls("cmdUsageHistory")).length).toBe(2);
});

test("the node panel charts usage against allocatable", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Nodes");
  await row(page, "node-1").click();
  const charts = panel(page).getByTestId("usage-charts");
  await expect(charts.getByTestId("usage-chart-cpu").getByTestId("ref-allocatable")).toHaveText("allocatable 3.8");
  await expect(charts.getByTestId("usage-chart-memory").getByTestId("ref-allocatable")).toHaveText("allocatable 7Gi");
  await expect(charts.locator('polyline[data-series="node"]')).toHaveCount(2);
  expect(await fake.calls("cmdUsageHistory")).toEqual([{ what: "nodes", namespace: null, name: "node-1" }]);
});
