import { expect, test } from "@playwright/test";
import { openApp, openKind } from "./helpers";

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
  const ns = page.getByRole("combobox", { name: "Namespace" });
  await expect(ns).toHaveValue("team-a");
  await ns.fill("team-b");
  await ns.press("Enter");
  await expect.poll(async () => (await fake.lastCall("cmdWatchStart")).namespace).toBe("team-b");
  await expect(page.locator("#known-namespaces option")).toHaveCount(2);
  expect(fake.errors).toEqual([]);
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
  await page.getByRole("combobox", { name: "Namespace" }).selectOption("demo");
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
