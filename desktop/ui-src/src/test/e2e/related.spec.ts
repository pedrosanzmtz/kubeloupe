import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import type { ResourceDetail } from "../../lib/bindings";
import { detail } from "../fixtures/cluster";
import { DISCOVERED } from "../fixtures/discover";
import { openApp, openKind, row } from "./helpers";
import type { Fake } from "./helpers";

const panel = (page: Page) => page.locator("aside");
const group = (page: Page, title: string) => panel(page).getByRole("region", { name: title, exact: true });

async function openObject(page: Page, kindLabel: string, name: string, title: string): Promise<Fake> {
  const fake = await openApp(page);
  await openKind(page, kindLabel);
  await row(page, name).click();
  await expect(panel(page).locator(".title")).toHaveText(title);
  await expect.poll(() => fake.activeDetailWatches()).toHaveLength(1);
  return fake;
}

test("a Deployment shows its summary, ReplicaSets and pods, and rows open them", async ({ page }) => {
  const fake = await openObject(page, "Deployments", "web", "Deployment demo/web");
  const p = panel(page);
  await expect(p.getByTestId("kind-summary")).toHaveText("2/2 ready · RollingUpdate 25%/25% · revision 4");
  await expect(p.getByTestId("kind-facts")).toContainText("Rollout");
  await expect(p.getByTestId("kind-facts")).toContainText("complete");

  const call = (await fake.calls("cmdGetRelated"))[0];
  expect(call).toEqual({
    resource: expect.objectContaining({ group: "apps", kind: "Deployment" }),
    namespace: "demo",
    name: "web",
  });

  await expect(group(page, "ReplicaSets").locator("tbody tr")).toHaveCount(1);
  const pods = group(page, "Pods");
  await expect(pods.locator("h3")).toHaveText("Pods (2)");
  await expect(pods.locator("thead th")).toHaveText(["Name", "Ready", "Status", "Restarts", "Age"]);
  await expect(pods.locator("td.phase-CrashLoopBackOff")).toHaveText("CrashLoopBackOff");
  await expect(pods.locator("td.short")).toHaveText("0/1");

  // A row opens its object; the pod gets the pod panel.
  await pods.locator("tbody tr").filter({ hasText: "web-1" }).click();
  await expect(p.locator(".title")).toHaveText("demo/web-1");

  // The pod panel lists what the pod uses, and those open too.
  await expect(group(page, "ConfigMaps")).toContainText("volume config, envFrom (app)");
  await expect(group(page, "Secrets")).toContainText("db-creds");
  await group(page, "ConfigMaps").getByRole("button", { name: "settings" }).click();
  await expect(p.locator(".title")).toHaveText("ConfigMap demo/settings");
  expect(fake.errors).toEqual([]);
});

test("the link in a row opens the ReplicaSet", async ({ page }) => {
  await openObject(page, "Deployments", "web", "Deployment demo/web");
  await group(page, "ReplicaSets").getByRole("button", { name: "web-5d8f" }).click();
  await expect(panel(page).locator(".title")).toHaveText("ReplicaSet demo/web-5d8f");
});

test("related objects reload when the detail watch reports a change, and on Refresh", async ({ page }) => {
  const fake = await openObject(page, "Deployments", "web", "Deployment demo/web");
  await expect(group(page, "Pods").locator("tbody tr")).toHaveCount(2);
  await expect.poll(async () => (await fake.calls("cmdGetRelated")).length).toBe(1);

  // A third pod comes up and the Deployment's status changes.
  await fake.setCluster((c) => {
    const r = c.related["apps/v1/deployments|demo/web"];
    r.summary[0] = "3/3 ready";
    const pods = r.groups.find((g) => g.title === "Pods")!;
    pods.items.push({ namespace: "demo", name: "web-3", cells: ["web-3", "1/1", "Running", "0", "1s"] });
  }, null);
  const changed = relabelled();
  await fake.emitDetail({ state: "detail", detail: changed });

  await expect(group(page, "Pods").locator("tbody tr")).toHaveCount(3);
  await expect(panel(page).getByTestId("kind-summary")).toHaveText(/^3\/3 ready/);
  expect((await fake.calls("cmdGetRelated")).length).toBe(2);

  // The same detail again is not a change.
  await fake.emitDetail({ state: "detail", detail: changed });
  await page.waitForTimeout(800);
  expect((await fake.calls("cmdGetRelated")).length).toBe(2);

  await panel(page).getByRole("button", { name: "Refresh related objects" }).click();
  await expect.poll(async () => (await fake.calls("cmdGetRelated")).length).toBe(3);
});

/** A detail of demo/web with a label the first one does not have, as the
 *  detail watch would send it after an edit. */
function relabelled(): ResourceDetail {
  const kind = DISCOVERED.find((k) => k.id === "apps/v1/deployments")!;
  return detail(kind, "demo", "web", { labels: { rolled: "yes" } });
}

test("plain rows, empty groups and a group that could not load", async ({ page }) => {
  await openObject(page, "Nodes", "node-1", "Node node-1");
  await expect(panel(page).getByTestId("kind-summary")).toHaveText("Ready · v1.37.0");
  const alloc = group(page, "Allocated resources");
  await expect(alloc.locator("tbody tr")).toHaveText([/cpu\s*750m \(37%\)\s*1 \(50%\)\s*2/]);
  // Plain rows are not links.
  await expect(alloc.getByRole("button")).toHaveCount(0);
  await expect(group(page, "Taints")).toContainText("None.");

  await openKind(page, "Config Maps");
  await row(page, "settings").click();
  await expect(panel(page).locator(".title")).toHaveText("ConfigMap demo/settings");
  await expect(group(page, "Used by pods")).toContainText("pods is forbidden");
});

test("nothing related shows no summary, and a failed load says so", async ({ page }) => {
  // The fake has no related data for demo/api, like a kind without relations.
  const fake = await openObject(page, "Deployments", "api", "Deployment demo/api");
  await expect.poll(async () => (await fake.calls("cmdGetRelated")).length).toBe(1);
  await expect(panel(page).getByTestId("kind-summary")).toHaveCount(0);
  await expect(panel(page).getByTestId("related-group")).toHaveCount(0);

  await page.evaluate(() => window.__fake.failNext("cmdGetRelated", "replicasets.apps is forbidden"));
  await row(page, "web").click();
  await expect(panel(page)).toContainText("Could not load related objects: replicasets.apps is forbidden");
});
