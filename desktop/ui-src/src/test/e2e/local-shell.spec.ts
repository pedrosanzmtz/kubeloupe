import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind } from "./helpers";

const dock = (page: Page) => page.getByTestId("dock");
const tabs = (page: Page) => dock(page).getByRole("tab");
const pane = (page: Page, tabId: string) => dock(page).locator(`[data-testid="dock-pane"][data-tab="${tabId}"]`);
const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });

async function runPalette(page: Page, text: string, option: string) {
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type(text);
  await expect(palette(page).getByRole("option").first()).toContainText(option);
  await page.keyboard.press("Enter");
  await expect(palette(page)).toHaveCount(0);
}

test("the palette opens a local terminal at the app's context, then closes it", async ({ page }) => {
  const fake = await openApp(page);
  await runPalette(page, "local term", "New local terminal");

  // All namespaces and no context namespace: kubectl's `default`.
  await expect(tabs(page)).toHaveText(["kind-dev · default"]);
  await expect(tabs(page).first()).toHaveAttribute("title", /does not change it/);
  await expect.poll(() => fake.calls("cmdLocalShellStart")).toHaveLength(1);
  expect(await fake.lastCall("cmdLocalShellStart")).toMatchObject({ namespace: "default" });
  const term = pane(page, "local:1").locator(".xterm");
  await expect(term).toContainText("kubectl uses context kind-dev, namespace default");
  await expect(term).toContainText("does not change this shell");

  await expect(page.locator(".xterm-helper-textarea")).toBeFocused();
  await page.keyboard.type("kubectl get pods");
  await expect
    .poll(async () => (await fake.calls("cmdLocalShellInput")).map((c) => c.data).join(""))
    .toBe("kubectl get pods");
  expect(await fake.calls("cmdExecInput")).toHaveLength(0);

  await dock(page).getByRole("button", { name: "Close kind-dev · default" }).click();
  await expect.poll(() => fake.calls("cmdLocalShellClose")).toEqual([{ session: "local-1" }]);
  expect(fake.errors).toEqual([]);
});

test("Ctrl+Shift+` opens a shell in the selected namespace; changing it later leaves open shells alone", async ({
  page,
}) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await runPalette(page, "ns demo", "demo");

  await page.keyboard.press("Control+Shift+Backquote");
  await expect(tabs(page)).toHaveText(["kind-dev · demo"]);
  expect(await fake.lastCall("cmdLocalShellStart")).toMatchObject({ namespace: "demo" });

  // Another namespace: the open shell keeps its label; a new one gets its own tab.
  // (The new terminal has the focus, and the palette key goes to the shell there.)
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await runPalette(page, "ns kube", "kube-system");
  await expect(tabs(page)).toHaveText(["kind-dev · demo"]);
  // Also from inside the terminal, which otherwise keeps every key.
  await pane(page, "local:1").locator(".xterm").click();
  await page.keyboard.press("Control+Shift+Backquote");
  await expect(tabs(page)).toHaveText(["kind-dev · demo", "kind-dev · kube-system"]);
  await expect(tabs(page).nth(1)).toHaveAttribute("aria-selected", "true");
  expect((await fake.calls("cmdLocalShellStart")).map((c) => c.namespace)).toEqual(["demo", "kube-system"]);

  // The dock's + button opens one too.
  await dock(page).getByRole("button", { name: "New local terminal" }).click();
  await expect(tabs(page)).toHaveCount(3);
  expect(fake.errors).toEqual([]);
});

test("a failed start shows the error in the terminal", async ({ page }) => {
  const fake = await openApp(page);
  await fake.failNext("cmdLocalShellStart", 'context "kind-dev" is not in the kubeconfig');
  await runPalette(page, "local term", "New local terminal");
  await expect(pane(page, "local:1").locator(".xterm")).toContainText(
    'Could not open terminal: context "kind-dev" is not in the kubeconfig',
  );
});
