import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { namespaceButton, openApp, toggleNamespaces } from "./helpers";

async function openHelm(page: Page) {
  await page.getByRole("navigation").getByRole("button", { name: "Releases", exact: true }).click();
  await expect(page.getByTestId("helm-releases")).toBeVisible();
}

function helmRows(page: Page): Locator {
  return page.getByTestId("helm-row");
}

function helmRow(page: Page, name: string): Locator {
  return helmRows(page).filter({ has: page.getByRole("gridcell", { name, exact: true }) });
}

function detail(page: Page): Locator {
  return page.getByTestId("helm-detail");
}

test("the Helm section lists releases from labels, then fills in their charts", async ({ page }) => {
  const fake = await openApp(page);
  const nav = page.getByRole("navigation");
  await expect(nav.getByRole("heading", { name: "Helm" })).toBeVisible();
  await openHelm(page);
  await expect(nav.getByRole("button", { name: "Releases" })).toHaveClass(/active/);

  await expect(helmRows(page)).toHaveCount(2);
  const web = helmRow(page, "web");
  await expect(web).toContainText("demo");
  await expect(web).toContainText("web-1.2.0");
  await expect(web).toContainText("2.4");
  await expect(web.getByRole("gridcell", { name: "deployed" })).toHaveClass(/phase-Running/);
  await expect(helmRow(page, "metrics").getByRole("gridcell", { name: "failed" })).toHaveClass(/phase-Failed/);
  await expect(page.locator("footer")).toContainText("2 of 2 releases");

  // One list (labels), then one batch of charts for the current revisions.
  expect((await fake.lastCall("cmdHelmList")).namespaces).toEqual([]);
  const charts = await fake.calls("cmdHelmCharts");
  expect(charts).toHaveLength(1);
  expect(charts[0].keys).toEqual([
    { namespace: "demo", name: "web", revision: 2, storage: "secret" },
    { namespace: "kube-system", name: "metrics", revision: 1, storage: "secret" },
  ]);

  // The filter matches charts too.
  await page.getByLabel("Filter").fill("metrics-server");
  await expect(helmRows(page)).toHaveCount(1);
  await page.getByLabel("Filter").fill("");

  // The namespace control applies, and forbidden namespaces become warnings.
  await expect(namespaceButton(page)).toBeVisible();
  await fake.setCluster((c) => (c.helm.forbidden = ["kube-system"]), null);
  await toggleNamespaces(page, "demo", "kube-system");
  await expect(page.getByTestId("helm-warning")).toContainText("Not allowed to list Secrets in namespace kube-system");
  await expect(helmRows(page)).toHaveCount(1);
  expect((await fake.lastCall("cmdHelmList")).namespaces).toEqual(["demo", "kube-system"]);
  expect(fake.errors).toEqual([]);
});

test("the detail shows overview, values, manifest with hidden Secrets, history with a diff, and resources", async ({
  page,
}) => {
  const fake = await openApp(page);
  await openHelm(page);
  await helmRow(page, "web").click();
  const d = detail(page);
  await expect(d).toContainText("demo/web");
  await expect(d.getByTestId("helm-status")).toHaveText("deployed");
  await expect(d).toContainText("web-1.2.0");
  await expect(d.getByTestId("helm-notes")).toHaveText("web says bye.");
  expect(await fake.lastCall("cmdHelmGet")).toEqual({ namespace: "demo", name: "web", revision: 0, reveal: false });

  // Values: user-supplied, or with the chart defaults.
  await d.getByRole("tab", { name: "Values" }).click();
  const values = d.getByRole("textbox", { name: "Values" });
  await expect(values).toContainText("greeting: bye");
  await expect(values).not.toContainText("replicas");
  await d.getByLabel("Include chart defaults").check();
  await expect(values).toContainText("replicas: 1");

  // Manifest: Secret values stay hidden until revealed.
  await d.getByRole("tab", { name: "Manifest" }).click();
  const manifest = d.getByRole("textbox", { name: "Manifest" });
  await expect(manifest).toContainText("<hidden: reveal to view>");
  await expect(manifest).not.toContainText("s3cret");
  await d.getByRole("button", { name: "Reveal" }).click();
  await expect(manifest).toContainText("s3cret");
  expect((await fake.lastCall("cmdHelmGet")).reveal).toBe(true);
  await d.getByRole("button", { name: "Hide" }).click();
  await expect(manifest).not.toContainText("s3cret");

  // History: both revisions, and a diff of the previous against the current.
  await d.getByRole("tab", { name: "History" }).click();
  await expect(d.getByTestId("helm-revision")).toHaveCount(2);
  const diff = d.getByRole("textbox", { name: "Revision 1 to 2" });
  await expect(diff).toContainText("greeting: bye");
  await d.getByRole("button", { name: "Manifest", exact: true }).click();
  await expect(d.getByTestId("helm-diff")).toContainText('greeting: "bye"');
  await d.getByLabel("Diff from revision").selectOption("2");
  await expect(d.getByTestId("helm-diff-same")).toBeVisible();

  // Resources link to their objects.
  await d.getByRole("tab", { name: "Resources" }).click();
  const resources = d.getByTestId("helm-resources");
  await expect(resources.getByRole("listitem")).toHaveCount(3);
  await resources.getByRole("button", { name: "web", exact: true }).click();
  await expect(page.getByRole("navigation").getByRole("button", { name: "Deployments", exact: true })).toHaveClass(
    /active/,
  );
  await expect(page.getByTestId("detail-panel")).toContainText("demo/web");
  expect(fake.errors).toEqual([]);
});

test("an older revision can be viewed", async ({ page }) => {
  const fake = await openApp(page);
  await openHelm(page);
  await helmRow(page, "web").click();
  const d = detail(page);
  await expect(d.getByTestId("helm-status")).toHaveText("deployed");
  await d.getByLabel("Revision", { exact: true }).selectOption("1");
  await expect(d.getByTestId("helm-status")).toHaveText("superseded");
  await expect(d.getByTestId("helm-notes")).toHaveText("web says hello.");
  expect((await fake.lastCall("cmdHelmGet")).revision).toBe(1);
  await expect(d.getByRole("button", { name: "Roll back to 1…" })).toBeEnabled();
});

test("rollback and uninstall are confirmed, then run through helm", async ({ page }) => {
  const fake = await openApp(page);
  await openHelm(page);
  await helmRow(page, "web").click();
  const d = detail(page);
  await d.getByRole("button", { name: "Rollback…" }).click();

  const dialog = page.getByTestId("helm-dialog");
  await expect(dialog).toContainText("Roll back web to revision 1?");
  await expect(dialog).toContainText("creates revision 3");
  // Escape cancels.
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(await fake.calls("cmdHelmRollback")).toHaveLength(0);

  // From the History tab.
  await d.getByRole("tab", { name: "History" }).click();
  await d.getByRole("button", { name: "Roll back to 1…" }).click();
  await dialog.getByTestId("helm-confirm").click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("status")).toContainText("Rolled back demo/web to revision 1");
  expect(await fake.lastCall("cmdHelmRollback")).toEqual({ namespace: "demo", name: "web", revision: 1 });
  await expect(helmRow(page, "web").getByRole("gridcell", { name: "3", exact: true })).toBeVisible();
  await expect(d.getByTestId("helm-revision")).toHaveCount(3);

  // Uninstall needs the name typed; a failure stays in the dialog.
  await d.getByRole("button", { name: "Uninstall…" }).click();
  await expect(dialog).toContainText("Uninstall web?");
  const confirm = dialog.getByTestId("helm-confirm");
  await expect(confirm).toBeDisabled();
  await dialog.getByRole("textbox").fill("web");
  await fake.failNext("cmdHelmUninstall", "Uninstall of web failed: uninstall: Release not loaded: web: release: not found");
  await confirm.click();
  await expect(dialog.getByRole("alert")).toContainText("release: not found");
  await confirm.click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("status")).toContainText("Uninstalled demo/web");
  await expect(helmRows(page)).toHaveCount(1);
  await expect(d).toHaveCount(0);
  expect(fake.errors).toEqual([]);
});

test("without helm, the actions explain how to install it", async ({ page }) => {
  const fake = await openApp(page, (c) => {
    c.helm.version = "";
  });
  await openHelm(page);
  await helmRow(page, "web").click();
  const d = detail(page);
  await expect(d.getByTestId("helm-missing")).toContainText("helm is not installed");
  await expect(d.getByRole("button", { name: "Rollback…" })).toBeDisabled();
  await expect(d.getByRole("button", { name: "Uninstall…" })).toBeDisabled();
  // Reading still works.
  await expect(d.getByTestId("helm-status")).toHaveText("deployed");
  expect(fake.errors).toEqual([]);
});

test("a failed list is shown, and Escape closes the detail", async ({ page }) => {
  const fake = await openApp(page);
  await fake.failNext("cmdHelmList", "Not allowed: your account lacks permission for this action.");
  await openHelm(page);
  await expect(page.getByTestId("helm-releases").getByRole("alert")).toContainText("Could not list Helm releases");
  await page.getByRole("button", { name: "Refresh" }).click();
  await expect(helmRows(page)).toHaveCount(2);
  await helmRow(page, "web").click();
  await expect(detail(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(detail(page)).toHaveCount(0);
});
