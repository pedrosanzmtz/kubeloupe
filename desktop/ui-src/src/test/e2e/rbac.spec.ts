import { expect, test } from "@playwright/test";
import { namespaceButton, namespacePopover, openApp, openKind, toggleNamespaces } from "./helpers";

// A namespace-scoped user: a RoleBinding in team-a, nothing cluster-wide
// (scripts/test-cluster/limited-user.sh is the real-cluster version).
const limited = (cluster: import("../fixtures/cluster").FakeCluster) => {
  cluster.contexts = [{ name: "kind-kubeloupe-limited", current: true, namespace: "team-a" }];
  cluster.namespaces = null;
  cluster.defaultNamespace = "team-a";
  cluster.deniedClusterWide = cluster.kinds.filter((k) => k.resource.namespaced).map((k) => k.id);
  cluster.denied = cluster.kinds.filter((k) => !k.resource.namespaced).map((k) => k.id);
  cluster.overview = {
    namespaces: null,
    nodes: null,
    pods: {
      namespace: "team-a",
      total: 1,
      running: 1,
      pending: 0,
      succeeded: 0,
      failed: 0,
      waiting_errors: 0,
      by_namespace: [["team-a", 1]],
      problems: [],
      problems_total: 0,
    },
    capacity: null,
    events: { namespace: "team-a", items: [] },
    notes: [
      "Pods: showing namespace team-a only (not allowed across all namespaces)",
      "Nodes: not allowed to list nodes",
    ],
  };
};

test("a namespace-scoped user starts on its namespace with a usable sidebar", async ({ page }) => {
  const fake = await openApp(page, limited);
  const nav = page.getByRole("navigation");

  // Access is checked in team-a once the scope is known.
  await expect
    .poll(async () => (await fake.calls("cmdCheckAccess")).some((c) => c.namespace === "team-a"))
    .toBe(true);
  const pods = nav.getByRole("button", { name: "Pods", exact: true });
  await expect(pods).not.toHaveClass(/denied/);
  const nodes = nav.getByRole("button", { name: "Nodes", exact: true });
  await expect(nodes).toHaveClass(/denied/);
  await expect(nodes).toHaveAttribute("title", "You are not allowed to list Nodes");

  await openKind(page, "Pods");
  expect((await fake.lastCall("cmdWatchStart")).namespace).toBe("team-a");
  await expect(page.locator("header")).toContainText("Live");

  // Namespaces cannot be listed: the control takes free text.
  await expect(namespaceButton(page)).toHaveText("team-a");
  await namespaceButton(page).click();
  const search = page.getByRole("searchbox", { name: "Search namespaces" });
  await search.fill("team-b");
  await search.press("Enter");
  await expect.poll(async () => (await fake.lastCall("cmdWatchStart")).namespace).toBe("team-b");
  // Added to the selection, next to team-a, and offered from now on.
  await expect(namespaceButton(page)).toHaveText("2 namespaces");
  await expect(namespacePopover(page).getByRole("group").getByRole("checkbox")).toHaveCount(2);
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual(["team-a", "team-b"]);

  // Unticking team-a leaves team-b alone, and it is still offered.
  await namespacePopover(page).getByRole("checkbox", { name: "team-a", exact: true }).click();
  await expect(namespaceButton(page)).toHaveText("team-b");
  await expect(namespacePopover(page).getByRole("group").getByRole("checkbox")).toHaveCount(2);
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual(["team-b"]);
  expect(fake.errors).toEqual([]);
});

test("a namespace the user may not list shows on its own, the others stay live", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    cluster.deniedNamespaces = ["kube-system"];
  });
  await openKind(page, "Pods");
  await toggleNamespaces(page, "demo", "kube-system");
  await expect(page.getByRole("alert")).toHaveText(
    "You can't list Pods in namespace kube-system. (pods is forbidden)",
  );
  await expect(page.locator("header")).toContainText("Not allowed in kube-system");
  // demo's rows are there and its watch keeps running.
  await expect(page.getByText("4 of 4 items")).toBeVisible();
  expect((await fake.activeWatches()).map((w) => w.namespace)).toEqual(["demo", "kube-system"]);

  // Dropping the forbidden one makes the table live again.
  await page.getByRole("button", { name: "Remove kube-system" }).click();
  await expect(page.locator("header")).toContainText("Live");
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText("4 of 4 items")).toBeVisible();
});

test("a kind denied across all namespaces opens, says why, and does not retry", async ({ page }) => {
  const fake = await openApp(page, (cluster) => {
    cluster.deniedClusterWide = ["v1/pods"];
  });
  const pods = page.getByRole("navigation").getByRole("button", { name: "Pods", exact: true });
  await expect(pods).toHaveClass(/denied/);
  await expect(pods).toHaveAttribute("title", "Pods: not allowed across all namespaces — pick a namespace");
  await expect(pods).toHaveAttribute("aria-disabled", "false");

  await pods.click();
  await expect(page.getByRole("alert")).toContainText(
    "You can't list Pods across all namespaces. Pick a namespace you have access to.",
  );
  await expect(page.locator("header")).toContainText("Not allowed");
  // No retry loop: still the one watch start after the retry interval.
  await page.waitForTimeout(6000);
  expect(await fake.calls("cmdWatchStart")).toHaveLength(1);

  // Picking a namespace re-checks access there and watches it.
  await toggleNamespaces(page, "demo");
  await expect(pods).not.toHaveClass(/denied/);
  await expect(page.locator("header")).toContainText("Live");
  expect((await fake.lastCall("cmdCheckAccess")).namespace).toBe("demo");
});

test("the overview shows what is allowed, with notes", async ({ page }) => {
  await openApp(page, limited);
  await expect(page.getByText("Nodes: not allowed to list nodes")).toBeVisible();
  await expect(page.getByText("Pods in team-a")).toBeVisible();
  await expect(page.getByText("Nodes ready")).toHaveCount(0);
  await expect(page.getByText("1 pods", { exact: true })).toBeVisible();
  await expect(page.locator("header")).toContainText("Live");
});
