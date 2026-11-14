import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";

const panel = (page: Page) => page.locator("aside");
const dialog = (page: Page) => page.getByTestId("forward-dialog");
const forwardRows = (page: Page) => page.getByTestId("forward-row");

async function openForwardsView(page: Page) {
  await openKind(page, "Port Forwards");
  await expect(page.getByTestId("port-forwards")).toBeVisible();
}

test("a pod forward outlives its panel and is listed, opened and stopped from Port Forwards", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  await panel(page).getByLabel("Pod port").fill("8080");
  await panel(page).getByLabel("Local port").fill("18080");
  await panel(page).getByRole("button", { name: "Forward", exact: true }).click();
  await expect(panel(page).getByText("Forwarding http://127.0.0.1:18080 → web-1:8080")).toBeVisible();
  expect(await fake.lastCall("cmdPortforwardStart")).toEqual({
    target: { kind: "pod", namespace: "demo", name: "web-1" },
    remotePort: "8080",
    localPort: 18080,
  });

  // Closing the panel leaves the forward running.
  await page.getByRole("button", { name: "Close details" }).click();
  await expect(panel(page)).toHaveCount(0);
  expect(await fake.calls("cmdPortforwardStop")).toEqual([]);

  await openForwardsView(page);
  await expect(page.getByText("Switching context stops all of them.")).toBeVisible();
  await expect(forwardRows(page)).toHaveCount(1);
  const r = forwardRows(page).first();
  await expect(r).toContainText("pod/demo/web-1");
  await expect(r).toContainText("18080 → 8080");
  await expect(r).toContainText("http://127.0.0.1:18080");
  await expect(r).toContainText("Listening");
  await expect(page.getByRole("contentinfo")).toContainText("1 forward");

  await r.getByRole("button", { name: "Open http://127.0.0.1:18080 in the browser" }).click();
  await expect.poll(() => page.evaluate(() => window.__fake.opened())).toEqual(["http://127.0.0.1:18080"]);

  // A failing forward says why.
  await fake.setCluster((c) => (c.forwards[0].status.last_error = "pod web-1: Already gone: the object no longer exists."), null);
  await expect(r).toContainText("pod web-1: Already gone: the object no longer exists.");

  await r.getByRole("button", { name: "Stop http://127.0.0.1:18080" }).click();
  await expect(forwardRows(page)).toHaveCount(0);
  expect((await fake.lastCall("cmdPortforwardStop")).id).toMatch(/^pf-/);
  await expect(page.getByText("No port-forwards.")).toBeVisible();
  expect(fake.errors).toEqual([]);
});

test("the context menu starts a Service forward on a chosen port", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Services");
  await row(page, "api").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Port forward…" }).click();

  const d = dialog(page);
  await expect(d).toContainText("svc/demo/api");
  await expect(d).toContainText("Switching context stops all forwards.");
  // Only the TCP ports are offered, the first one picked.
  const select = d.getByLabel("Service port");
  await expect(select.locator("option")).toHaveText(["80/http → web", "9090/metrics → 9090"]);
  await expect(select).toHaveValue("80");

  await d.getByLabel("Local port").fill("abc");
  await d.getByTestId("forward-confirm").click();
  await expect(d.getByRole("alert")).toHaveText("The local port must be a number between 0 and 65535, or empty.");

  await d.getByLabel("Local port").fill("9000");
  await d.getByTestId("forward-confirm").click();
  await expect(d).toHaveCount(0);
  expect(await fake.lastCall("cmdPortforwardStart")).toEqual({
    target: { kind: "service", namespace: "demo", name: "api" },
    remotePort: "80",
    localPort: 9000,
  });
  await expect(page.getByText("Forwarding http://127.0.0.1:9000 → svc/demo/api:80")).toBeVisible();

  // The same local port again is refused, in plain words, inside the dialog.
  await row(page, "api").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Port forward…" }).click();
  await d.getByLabel("Service port").selectOption("9090");
  await d.getByLabel("Local port").fill("9000");
  await d.getByTestId("forward-confirm").click();
  await expect(d.getByRole("alert")).toHaveText(
    "local port 9000 is already in use: pick another one, or leave it empty for a free one",
  );
  await d.getByRole("button", { name: "Cancel" }).click();

  await openForwardsView(page);
  await expect(forwardRows(page)).toHaveCount(1);
  await expect(forwardRows(page).first()).toContainText("svc/demo/api");
  await expect(forwardRows(page).first()).toContainText("api-1");
});

test("the Service panel forwards each TCP port", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Services");
  await row(page, "api").click();
  const ports = panel(page).getByTestId("service-ports");
  await expect(ports).toContainText("80/http → web");
  await expect(ports).toContainText("53/dns → 53");
  // UDP cannot be forwarded.
  await expect(ports.getByRole("button", { name: /^Forward port/ })).toHaveCount(2);

  await ports.getByRole("button", { name: "Forward port 9090" }).click();
  expect(await fake.lastCall("cmdPortforwardStart")).toEqual({
    target: { kind: "service", namespace: "demo", name: "api" },
    remotePort: "9090",
    localPort: null,
  });
  // The button becomes the forward's URL, which opens it.
  await ports.getByRole("button", { name: "Open http://127.0.0.1:9090" }).click();
  await expect.poll(() => page.evaluate(() => window.__fake.opened())).toEqual(["http://127.0.0.1:9090"]);

  // The sidebar counts it.
  await expect(page.getByRole("navigation").getByRole("button", { name: "Port Forwards" })).toContainText("1");
});

test("a deployment forward asks for a port name or number", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Port forward…" }).click();
  const d = dialog(page);
  await expect(d).toContainText("deploy/demo/web");
  await d.getByLabel("Remote port").fill("HTTP");
  await d.getByLabel("Remote port").press("Enter");
  await expect(d.getByRole("alert")).toHaveText('"HTTP" is neither a port number nor a port name.');
  await d.getByLabel("Remote port").fill("http");
  await d.getByLabel("Remote port").press("Enter");
  await expect(d).toHaveCount(0);
  expect(await fake.lastCall("cmdPortforwardStart")).toEqual({
    target: { kind: "deployment", namespace: "demo", name: "web" },
    remotePort: "http",
    localPort: null,
  });
});
