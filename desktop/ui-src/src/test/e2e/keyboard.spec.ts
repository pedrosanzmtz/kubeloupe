import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row, rows } from "./helpers";

const palette = (page: Page) => page.getByRole("dialog", { name: "Command palette" });
const paletteInput = (page: Page) => palette(page).getByRole("combobox");
const options = (page: Page) => palette(page).getByRole("option");
const grid = (page: Page) => page.getByRole("grid");
const panel = (page: Page) => page.getByTestId("detail-panel");
const menu = (page: Page) => page.getByRole("menu");
const dialog = (page: Page) => page.getByRole("alertdialog");
const help = (page: Page) => page.getByRole("dialog", { name: "Keyboard shortcuts" });

/** The row the grid's aria-activedescendant points at. */
async function focusedRowName(page: Page): Promise<string> {
  const id = await grid(page).getAttribute("aria-activedescendant");
  expect(id, "a focused row").toBeTruthy();
  return (await page.locator(`#${id} [role="gridcell"]`).nth(1).textContent())?.trim() ?? "";
}

test("the palette opens, and `deploy` goes to Deployments", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeVisible();
  await expect(paletteInput(page)).toBeFocused();

  await page.keyboard.type("deploy");
  await expect(options(page).first()).toContainText("Deployments");
  await page.keyboard.press("Enter");
  await expect(palette(page)).toHaveCount(0);
  await expect(grid(page)).toHaveAccessibleName("Deployments");
  await expect(rows(page)).toHaveCount(2);

  // Ctrl/Cmd+P too; the last pick comes first, marked recent. Esc closes.
  await page.keyboard.press("ControlOrMeta+p");
  await expect(palette(page).locator(".group").first()).toHaveText("Recent");
  await expect(options(page).first()).toContainText("Deployments");
  await page.keyboard.press("Escape");
  await expect(palette(page)).toHaveCount(0);
  await expect(grid(page)).toHaveAccessibleName("Deployments");
});

test("palette prefixes: namespaces, actions, objects and Create resource…", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");

  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("ns kube");
  await expect(options(page)).toHaveText(["kube-system"]);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("combobox", { name: "Namespace" })).toHaveValue("kube-system");
  await expect(rows(page)).toHaveCount(1);

  // An object of the current table opens its detail panel.
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("coredns");
  await expect(options(page).first()).toContainText("pods/coredns-1");
  await page.keyboard.press("Enter");
  await expect(panel(page).locator(".title")).toHaveText("kube-system/coredns-1");

  // `>` lists actions for the object (here: the one in the detail panel).
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type(">");
  await expect(options(page).filter({ hasText: "Force delete…" })).toContainText("kube-system/coredns-1");
  await page.keyboard.type("create");
  await expect(options(page).first()).toContainText("Create resource…");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: /Create/ })).toBeVisible();
});

test("arrow keys and j/k move the focused row, Enter opens it", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await expect(grid(page)).toHaveAttribute("aria-rowcount", "6");

  // Rows are in key order: crashed, db-0, web-1, web-2, coredns-1.
  await page.keyboard.press("ArrowDown");
  await expect(grid(page)).toBeFocused();
  expect(await focusedRowName(page)).toBe("crashed");
  await page.keyboard.press("j");
  await page.keyboard.press("ArrowDown");
  expect(await focusedRowName(page)).toBe("web-1");
  await page.keyboard.press("k");
  expect(await focusedRowName(page)).toBe("db-0");
  await page.keyboard.press("j");
  await expect(row(page, "web-1")).toHaveAttribute("aria-rowindex", "4");

  // A watch batch that removes the row above keeps the focus on web-1.
  await fake.emitWatchBatch({ deleted: ["demo/db-0"] });
  await expect(rows(page)).toHaveCount(4);
  expect(await focusedRowName(page)).toBe("web-1");

  await page.keyboard.press("Enter");
  await expect(panel(page).locator(".title")).toHaveText("demo/web-1");

  // Space toggles the checkbox selection of the focused row.
  await page.keyboard.press(" ");
  await expect(row(page, "web-1").getByRole("checkbox")).toBeChecked();
  await expect(row(page, "web-1")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press(" ");
  await expect(row(page, "web-1").getByRole("checkbox")).not.toBeChecked();

  // End and Home jump to the last and first rows.
  await page.keyboard.press("End");
  expect(await focusedRowName(page)).toBe("coredns-1");
  await page.keyboard.press("Home");
  expect(await focusedRowName(page)).toBe("crashed");
});

test("Esc closes the palette, then the menu, then the dialog, then the panel", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(panel(page)).toBeVisible();

  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(palette(page)).toHaveCount(0);
  await expect(panel(page)).toBeVisible();

  await page.keyboard.press("Shift+F10");
  await expect(menu(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu(page)).toHaveCount(0);
  await expect(panel(page)).toBeVisible();

  await page.keyboard.press("Delete");
  await expect(dialog(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
  await expect(panel(page)).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);
});

test("the row menu opens from the keyboard and its items are reachable with arrows", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Shift+F10");
  await expect(menu(page).locator(".title")).toHaveText("demo/crashed");
  await expect(menu(page).getByRole("menuitem", { name: "Delete", exact: true })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu(page).getByRole("menuitem", { name: "Force delete…" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(dialog(page)).toHaveAccessibleName("Force delete Pod");
});

test("Delete opens the confirm dialog naming the focused or selected rows", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("j");
  await page.keyboard.press("Delete");
  await expect(dialog(page)).toHaveAccessibleName("Delete Pod");
  await expect(dialog(page).getByRole("listitem")).toHaveText(["demo/db-0"]);
  await page.keyboard.press("Escape");
  expect(await fake.calls("cmdRunAction")).toEqual([]);

  // With rows checked, Delete is about them.
  await page.keyboard.press(" ");
  await page.keyboard.press("j");
  await page.keyboard.press(" ");
  await page.keyboard.press("Delete");
  await expect(dialog(page)).toHaveAccessibleName("Delete 2 Pods");
  await expect(dialog(page).getByRole("listitem")).toHaveText(["demo/db-0", "demo/web-1"]);
});

test("/ focuses the filter and ? lists the shortcuts", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Pods");
  await page.keyboard.press("/");
  await expect(page.getByRole("searchbox", { name: "Filter" })).toBeFocused();
  // Typing in the filter is just typing.
  await page.keyboard.type("web/?j");
  await expect(page.getByRole("searchbox", { name: "Filter" })).toHaveValue("web/?j");
  await expect(help(page)).toHaveCount(0);

  await page.getByRole("searchbox", { name: "Filter" }).fill("");
  await grid(page).focus();
  await page.keyboard.press("?");
  await expect(help(page)).toBeVisible();
  for (const text of ["Command palette", "Focus the table filter", "Open the row's menu", "Switch detail panel tab"]) {
    await expect(help(page)).toContainText(text);
  }
  await expect(help(page).locator("kbd").first()).toHaveText("Ctrl+K");
  await page.keyboard.press("Escape");
  await expect(help(page)).toHaveCount(0);
});

test("Ctrl/Cmd+1…9 switch the detail panel tabs", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  const tab = (name: string) => panel(page).getByRole("tab", { name });
  await expect(tab("Overview")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ControlOrMeta+2");
  await expect(tab("Logs")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ControlOrMeta+3");
  await expect(tab("YAML")).toHaveAttribute("aria-selected", "true");
  // There is no fourth tab.
  await page.keyboard.press("ControlOrMeta+4");
  await expect(tab("YAML")).toHaveAttribute("aria-selected", "true");
});

test("keys are ignored inside the YAML editor", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  await panel(page).getByRole("tab", { name: "YAML" }).click();
  await expect(panel(page).locator(".code-editor")).toHaveAttribute("data-ready", "true");
  const editor = page.getByRole("textbox", { name: "Deployment YAML", exact: true });
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.type("j?/");
  await page.keyboard.press("Escape");
  await page.keyboard.press("ControlOrMeta+k");
  await expect(editor).toContainText("j?/");
  await expect(help(page)).toHaveCount(0);
  await expect(palette(page)).toHaveCount(0);
  await expect(panel(page)).toBeVisible();
});

test("keys are ignored inside the terminal", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, "web-1").click();
  await panel(page).getByRole("button", { name: "Open terminal" }).click();
  const term = panel(page).locator(".xterm");
  await expect(term).toBeVisible();
  await term.click();
  await expect(page.locator(".xterm-helper-textarea")).toBeFocused();
  for (const key of ["?", "Escape", "Delete", "ControlOrMeta+k", "j"]) await page.keyboard.press(key);

  await expect(help(page)).toHaveCount(0);
  await expect(palette(page)).toHaveCount(0);
  await expect(dialog(page)).toHaveCount(0);
  await expect(panel(page)).toBeVisible();
  // The keys went to the shell instead.
  await expect.poll(async () => (await fake.calls("cmdExecInput")).map((c) => c.data).join("")).toContain("?");
});
