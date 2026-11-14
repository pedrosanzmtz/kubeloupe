import { expect, test } from "@playwright/test";
import { openApp, openKind, rows } from "./helpers";

test("the sidebar is built from discovery", async ({ page }) => {
  await openApp(page);
  const nav = page.getByRole("navigation");

  await expect(nav.getByRole("heading")).toHaveText([
    "Workloads",
    "Network",
    "Config",
    "Storage",
    "Cluster",
    "Helm",
    "Custom Resources",
    "Other",
  ]);
  for (const label of ["Pods", "Deployments", "CronJobs", "Services", "Secrets", "Persistent Volume Claims", "Nodes"]) {
    await expect(nav.getByRole("button", { name: label, exact: true })).toBeVisible();
  }

  // CRDs are grouped by API group, collapsed until opened.
  const group = nav.locator("details", { has: page.locator("summary", { hasText: "example.com" }) });
  await expect(group.getByRole("button", { name: "Widgets" })).toBeHidden();
  await group.locator("summary").click();
  await group.getByRole("button", { name: "Widgets" }).click();
  await expect(rows(page)).toHaveCount(1);
  await expect(group.getByRole("button", { name: "Widgets" })).toHaveClass(/active/);
});

test("denied kinds are greyed out and do not open", async ({ page }) => {
  const fake = await openApp(page);
  const nav = page.getByRole("navigation");
  const rbac = nav.locator("details", { has: page.locator("summary", { hasText: "rbac.authorization.k8s.io" }) });
  await rbac.locator("summary").click();

  const denied = rbac.getByRole("button", { name: "Clusterroles", exact: true });
  await expect(denied).toBeDisabled();
  await expect(denied).toHaveClass(/denied/);
  await expect(denied).toHaveAttribute("title", "You are not allowed to list Clusterroles");
  await expect(rbac.getByRole("button", { name: "Roles", exact: true })).toHaveAttribute("aria-disabled", "false");

  // aria-disabled: Playwright would wait for it to become enabled.
  await denied.click({ force: true });
  // Still on the overview: nothing was watched.
  await expect(page.getByRole("searchbox", { name: "Filter" })).toHaveCount(0);
  expect(await fake.calls("cmdWatchStart")).toEqual([]);

  // Every discovered kind was checked in one call.
  const [check] = await fake.calls("cmdCheckAccess");
  expect((check.kinds as unknown[]).length).toBe(await page.evaluate(() => window.__fake.cluster.kinds.length));
});

test("a failed discovery says so and leaves only the overview", async ({ page }) => {
  await openApp(page, (_cluster, fake) => fake.failNext("cmdDiscover", "403 forbidden"), { waitForKinds: false });
  await expect(page.getByRole("alert")).toHaveText("Could not discover the cluster's resource types: 403 forbidden");
  await expect(page.getByRole("navigation").getByRole("button")).toHaveText(["Overview"]);
});

test("the overview shows the cluster summary", async ({ page }) => {
  await openApp(page);
  await expect(page.getByText("2/2")).toBeVisible();
  await expect(page.getByText("5 pods, 2 nodes")).toBeVisible();
  await expect(page.locator("header")).toContainText("Live");
});

test("Refresh rediscovers and restarts the current watch", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(rows(page)).toHaveCount(5);
  await page.getByRole("button", { name: "Refresh" }).click();
  await expect.poll(async () => (await fake.calls("cmdDiscover")).length).toBe(2);
  await expect.poll(async () => (await fake.calls("cmdWatchStart")).length).toBe(2);
  await expect(rows(page)).toHaveCount(5);
});
