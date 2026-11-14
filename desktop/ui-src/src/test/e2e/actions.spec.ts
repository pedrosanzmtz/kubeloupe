import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { column, openApp, openKind, row, rows } from "./helpers";

const menu = (page: Page) => page.getByRole("menu");
const dialog = (page: Page) => page.getByRole("alertdialog");

async function contextMenu(page: Page, name: string) {
  await row(page, name).click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  return menu(page);
}

test("the context menu offers the actions of each kind", async ({ page }) => {
  await openApp(page);
  const cases: [string, string, string, string[]][] = [
    ["Pods", "web-1", "demo/web-1", ["Logs", "Shell", "Port forward…", "Delete", "Force delete…"]],
    ["Deployments", "web", "demo/web", ["Logs", "Scale…", "Rollout restart", "Port forward…", "Delete"]],
    ["Nodes", "node-1", "node-1", ["Cordon", "Uncordon", "Drain…", "Delete"]],
    ["Secrets", "db-creds", "demo/db-creds", ["Delete"]],
  ];
  for (const [kind, name, title, items] of cases) {
    await openKind(page, kind);
    const m = await contextMenu(page, name);
    await expect(m.locator(".title")).toHaveText(title);
    await expect(m.getByRole("menuitem")).toHaveText(items);
    await page.keyboard.press("Escape");
    await expect(menu(page)).toHaveCount(0);
  }
});

test("the detail panel offers the same actions", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  await expect(page.getByRole("toolbar", { name: "Actions" }).getByRole("button")).toHaveText([
    "Scale…",
    "Rollout restart",
    "Port forward…",
    "Delete",
  ]);
});

test("a delete is confirmed in a dialog that names the object", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await (await contextMenu(page, "web")).getByRole("menuitem", { name: "Delete" }).click();

  await expect(dialog(page)).toHaveAccessibleName("Delete Deployment");
  await expect(dialog(page)).toContainText("This permanently deletes the Deployment:");
  await expect(dialog(page).getByRole("listitem")).toHaveText(["demo/web"]);
  expect(await fake.calls("cmdRunAction")).toEqual([]);

  // Cancel does nothing.
  await dialog(page).getByRole("button", { name: "Cancel" }).click();
  await expect(dialog(page)).toHaveCount(0);
  expect(await fake.calls("cmdRunAction")).toEqual([]);

  await (await contextMenu(page, "web")).getByRole("menuitem", { name: "Delete" }).click();
  await dialog(page).getByRole("button", { name: "Delete", exact: true }).click();
  await expect(dialog(page)).toHaveCount(0);
  expect(await fake.lastCall("cmdRunAction")).toMatchObject({
    namespace: "demo",
    name: "web",
    action: "delete",
    replicas: null,
  });
  await expect(page.getByRole("status")).toContainText("Deleted deployment demo/web");
  // The fake's watch reports the delete, like the real one.
  expect(await column(page, "Name")).toEqual(["api"]);
});

test("a failed action keeps the dialog open with the reason", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await fake.failNext("cmdRunAction", "deployments.apps \"web\" is forbidden");
  await (await contextMenu(page, "web")).getByRole("menuitem", { name: "Delete" }).click();
  await dialog(page).getByRole("button", { name: "Delete", exact: true }).click();
  await expect(dialog(page).getByRole("alert")).toHaveText('deployments.apps "web" is forbidden');
  await expect(rows(page)).toHaveCount(2);
});

test("scale starts at the current replicas and warns at 0", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  // api is "1/3": 3 desired.
  await (await contextMenu(page, "api")).getByRole("menuitem", { name: "Scale…" }).click();
  const replicas = dialog(page).getByRole("spinbutton", { name: "Replicas" });
  await expect(replicas).toHaveValue("3");
  await expect(dialog(page).getByRole("button", { name: "Scale to 3" })).toBeVisible();

  await replicas.fill("0");
  await expect(dialog(page)).toContainText("Scaling to 0 stops every pod of this workload");
  await dialog(page).getByRole("button", { name: "Scale to 0" }).click();
  await expect(dialog(page)).toHaveCount(0);
  expect(await fake.lastCall("cmdRunAction")).toMatchObject({ name: "api", action: "scale", replicas: 0 });
});

test("a harmless action runs without a dialog", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await (await contextMenu(page, "web")).getByRole("menuitem", { name: "Rollout restart" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("status")).toContainText("restart deployment demo/web: done");
  expect(await fake.lastCall("cmdRunAction")).toMatchObject({ action: "restart", replicas: null });
});

test("a failed action without a dialog stays in a notice", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Nodes");
  await fake.failNext("cmdRunAction", "nodes \"node-1\" is forbidden");
  await (await contextMenu(page, "node-1")).getByRole("menuitem", { name: "Cordon", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(/Cordon node-1: nodes "node-1" is forbidden/);
});

test("bulk delete names every object and lists partial failures", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await fake.setCluster(
    (cluster) => {
      cluster.bulkErrors = {
        "demo/db-0": "pods \"db-0\" is forbidden: User cannot delete",
        "demo/crashed": "the object has been modified",
      };
    },
    null,
  );
  for (const name of ["crashed", "db-0", "web-1"]) await row(page, name).getByRole("checkbox").check();

  // The context menu on a selected row acts on the whole selection.
  await (await contextMenu(page, "db-0")).getByRole("menuitem", { name: "Delete 3 selected…" }).click();
  await expect(dialog(page)).toHaveAccessibleName("Delete 3 Pods");
  await expect(dialog(page)).toContainText("This permanently deletes 3 Pods:");
  await expect(dialog(page).getByRole("listitem")).toHaveText(["demo/crashed", "demo/db-0", "demo/web-1"]);

  await dialog(page).getByRole("button", { name: "Delete 3" }).click();
  await expect(dialog(page)).toContainText("1 of 3 deleted. 2 could not be deleted:");
  await expect(dialog(page).getByRole("listitem")).toHaveText([
    "demo/crashed the object has been modified",
    'demo/db-0 pods "db-0" is forbidden: User cannot delete',
  ]);
  expect(await fake.lastCall("cmdBulkDelete")).toMatchObject({
    items: [
      { namespace: "demo", name: "crashed" },
      { namespace: "demo", name: "db-0" },
      { namespace: "demo", name: "web-1" },
    ],
  });

  await dialog(page).getByRole("button", { name: "Close" }).click();
  await expect(row(page, "web-1")).toHaveCount(0);
  await expect(rows(page)).toHaveCount(4);
});

test("bulk delete without failures closes with a notice", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await page.getByRole("searchbox", { name: "Filter" }).fill("web");
  await page.getByRole("checkbox", { name: "Select all visible rows" }).check();
  await page.getByRole("toolbar", { name: "Selection" }).getByRole("button", { name: "Delete 2…" }).click();
  await dialog(page).getByRole("button", { name: "Delete 2" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByRole("status")).toContainText("Deleted 2 pods");
  await expect(rows(page)).toHaveCount(0);
});

test("force delete needs the typed name", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await (await contextMenu(page, "web-1")).getByRole("menuitem", { name: "Force delete…" }).click();

  await expect(dialog(page)).toHaveAccessibleName("Force delete Pod");
  await expect(dialog(page)).toContainText("stuck terminating");
  const confirm = dialog(page).getByRole("button", { name: "Force delete" });
  const typed = dialog(page).getByRole("textbox", { name: /to confirm/ });
  await expect(typed).toBeFocused();
  await expect(confirm).toBeDisabled();

  await typed.fill("web-");
  await expect(confirm).toBeDisabled();
  await typed.press("Enter");
  expect(await fake.calls("cmdRunAction")).toEqual([]);

  await typed.fill("web-1");
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(dialog(page)).toHaveCount(0);
  expect(await fake.lastCall("cmdRunAction")).toMatchObject({ name: "web-1", action: "forceDelete" });
  await expect(row(page, "web-1")).toHaveCount(0);
});

test("drain shows its progress in the dialog", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Nodes");
  await (await contextMenu(page, "node-2")).getByRole("menuitem", { name: "Drain…" }).click();
  await expect(dialog(page)).toContainText("PodDisruptionBudget");
  await dialog(page).getByRole("button", { name: "Drain" }).click();
  await expect(dialog(page)).toContainText("Node node-2 cordoned");
  await expect(dialog(page)).toContainText("0 evicted, 0 failed, 0 skipped.");
  expect(await fake.lastCall("cmdDrainNode")).toEqual({ node: "node-2", timeoutSecs: 300 });
  await dialog(page).getByRole("button", { name: "Close" }).click();
  await expect(dialog(page)).toHaveCount(0);
});
