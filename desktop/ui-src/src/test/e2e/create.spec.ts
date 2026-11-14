import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";

function dialog(page: Page): Locator {
  return page.getByRole("dialog", { name: "Create resources" });
}

function button(page: Page, name: string): Locator {
  return dialog(page).getByRole("button", { name, exact: true });
}

/** Open "+ Create" and wait for the editor. */
async function openCreate(page: Page): Promise<Locator> {
  await page.getByRole("button", { name: "+ Create" }).click();
  await expect(dialog(page).locator(".code-editor")).toHaveAttribute("data-ready", "true");
  return dialog(page).getByRole("textbox", { name: "New resources YAML" });
}

/** Replace the editor's text, as raw text (no auto-indent). */
async function setText(page: Page, editor: Locator, text: string) {
  await editor.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.insertText(text);
}

const results = (page: Page) => dialog(page).getByTestId("create-result");

test("+ Create starts from a template for the kind, validates, creates, and opens the result", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Config Maps");
  await page.getByRole("combobox", { name: "Namespace" }).selectOption("demo");
  const editor = await openCreate(page);
  await expect(editor).toContainText("kind: ConfigMap");
  await expect(editor).toContainText("namespace: demo");
  await expect(dialog(page)).toContainText("Objects without a namespace go to demo.");

  await button(page, "Validate").click();
  expect(await fake.lastCall("cmdApplyManifests")).toMatchObject({ namespace: "demo", dryRun: true, skipInvalid: false });
  await expect(results(page)).toHaveText([/Would create\s*ConfigMap demo\/my-config\s*line 1/]);
  await expect(dialog(page).getByTestId("create-summary")).toHaveText(
    "Validation passed for 1 object. Nothing was created yet.",
  );
  // A dry run stored nothing.
  await expect(row(page, "my-config")).toHaveCount(0);

  await button(page, "Create").click();
  expect(await fake.lastCall("cmdApplyManifests")).toMatchObject({ dryRun: false });
  await expect(results(page)).toHaveText([/^Created\s*ConfigMap demo\/my-config/]);
  await expect(dialog(page).getByTestId("create-summary")).toHaveText("Applied 1 object.");
  // The table's watch shows it.
  await expect(row(page, "my-config")).toHaveCount(1);

  // Clicking a created object opens it.
  await results(page).first().click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.locator("aside")).toContainText("my-config");
  expect(fake.errors).toEqual([]);
});

test("other kinds get a template or the generic skeleton", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Secrets");
  let editor = await openCreate(page);
  await expect(editor).toContainText("stringData:");
  await expect(editor).toContainText("namespace: default");
  await button(page, "Close").click();

  // A CRD (its API group is collapsed in the sidebar).
  await page.getByRole("navigation").locator("summary", { hasText: "example.com" }).click();
  await openKind(page, "Widgets");
  editor = await openCreate(page);
  await expect(editor).toContainText("apiVersion: example.com/v1");
  await expect(editor).toContainText("kind: Widget");
  await expect(editor).toContainText("spec: {}");
});

test("a failing document blocks the batch; the valid ones can be applied alone", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Config Maps");
  await fake.setCluster(
    (cluster) => {
      cluster.applyErrors = { bad: "The cluster rejected the request as invalid. data.x: Invalid value" };
    },
    null,
  );
  const editor = await openCreate(page);
  await setText(
    page,
    editor,
    "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: good\n  namespace: demo\n---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: bad\n  namespace: demo\ndata:\n  x: y\n",
  );

  await button(page, "Validate").click();
  await expect(results(page)).toHaveText([
    /Would create\s*ConfigMap demo\/good\s*line 1/,
    /Failed\s*ConfigMap demo\/bad\s*line 7\s*The cluster rejected the request as invalid\. data\.x: Invalid value/,
  ]);
  await expect(dialog(page).getByTestId("create-summary")).toHaveText("Validation failed for 1 of 2 objects.");
  // The editor points at the field the error names.
  await expect(dialog(page).locator(".cm-errorLine")).toHaveText("  x: y");

  await button(page, "Create").click();
  await expect(results(page).first()).toContainText("Not applied");
  await expect(dialog(page).getByTestId("create-summary")).toHaveText(
    "Nothing was applied: 1 of 2 objects failed validation.",
  );
  await expect(row(page, "good")).toHaveCount(0);

  await button(page, "Create the valid ones only").click();
  expect(await fake.lastCall("cmdApplyManifests")).toMatchObject({ dryRun: false, skipInvalid: true });
  await expect(results(page)).toHaveText([/^Created\s*ConfigMap demo\/good/, /^Failed/]);
  await expect(row(page, "good")).toHaveCount(1);
});

test("Open file… loads a YAML file into the editor", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Config Maps");
  const editor = await openCreate(page);
  // What choosing a file does: the hidden input gets it and fires "change".
  await dialog(page)
    .getByTestId("create-file")
    .evaluate((input: HTMLInputElement) => {
      const dt = new DataTransfer();
      dt.items.add(new File(["apiVersion: v1\nkind: Namespace\nmetadata:\n  name: from-file\n"], "app.yaml"));
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
  await expect(editor).toContainText("name: from-file");
  await expect(dialog(page).locator("h2 .dirty")).toBeVisible();
});

test("dropping a file opens it", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Config Maps");
  const editor = await openCreate(page);
  const transfer = await page.evaluateHandle(() => {
    const dt = new DataTransfer();
    dt.items.add(new File(["apiVersion: v1\nkind: Namespace\nmetadata:\n  name: dropped\n"], "drop.yaml"));
    return dt;
  });
  await dialog(page).dispatchEvent("dragover", { dataTransfer: transfer });
  await dialog(page).dispatchEvent("drop", { dataTransfer: transfer });
  await expect(editor).toContainText("name: dropped");
});

test("closing with edits asks first", async ({ page }) => {
  await openApp(page);
  await openKind(page, "Config Maps");
  const editor = await openCreate(page);
  await setText(page, editor, "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: mine\n");

  await button(page, "Cancel").click();
  const ask = page.getByRole("alertdialog", { name: "Discard changes?" });
  await ask.getByRole("button", { name: "Keep editing" }).click();
  await expect(editor).toContainText("name: mine");

  await page.keyboard.press("Escape");
  await ask.getByRole("button", { name: "Discard changes" }).click();
  await expect(dialog(page)).toHaveCount(0);
});

test("an error from the backend is shown", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Config Maps");
  await openCreate(page);
  await fake.failNext("cmdApplyManifests", "Not signed in: the cluster rejected your credentials.");
  await button(page, "Validate").click();
  await expect(dialog(page).getByRole("alert")).toHaveText("Not signed in: the cluster rejected your credentials.");
});
