import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";

async function openYaml(page: Page, kindLabel: string, name: string) {
  const fake = await openApp(page);
  await openKind(page, kindLabel);
  await row(page, name).click();
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  return fake;
}

const DEPLOYMENT = { group: "apps", version: "v1", kind: "Deployment", plural: "deployments", namespaced: true };

/** The CodeMirror editor (its content element is the textbox), once loaded. */
async function editorOf(page: Page, kind: string): Promise<Locator> {
  await expect(page.locator("aside .code-editor")).toHaveAttribute("data-ready", "true");
  return page.getByRole("textbox", { name: `${kind} YAML`, exact: true });
}

/** Type at the end of the document, as raw text (no auto-indent). */
async function append(page: Page, editor: Locator, text: string) {
  await editor.click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.insertText(text);
}

function button(page: Page, name: string): Locator {
  return page.locator("aside").getByRole("button", { name, exact: true });
}

test("the YAML tab loads the object into the editor", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await expect(editor).toContainText("kind: Deployment");
  await expect(editor).toContainText('resourceVersion: "1"');
  // Highlighting, line numbers and folding are on.
  await expect(page.locator("aside .cm-lineNumbers")).toBeVisible();
  await expect(page.locator("aside .cm-foldGutter")).toBeVisible();
  await expect(editor).toBeEditable();
  expect(await fake.lastCall("cmdGetResourceYaml")).toEqual({
    resource: DEPLOYMENT,
    namespace: "demo",
    name: "web",
    reveal: false,
  });
  // Nothing changed yet: nothing to review.
  await expect(button(page, "Review")).toBeDisabled();
});

test("edit, review the diff, then save", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  await expect(page.locator("aside h3", { hasText: "YAML" }).locator(".dirty")).toBeVisible();

  await button(page, "Review").click();
  // Validated with a dry run first, at the version that was loaded.
  expect(await fake.lastCall("cmdApplyResourceYaml")).toMatchObject({ dryRun: true, resourceVersion: "1" });
  const diff = page.getByRole("textbox", { name: "Deployment YAML changes" });
  await expect(diff).toBeVisible();
  await expect(page.locator("aside .cm-changedLine")).toHaveText(["spec:", "  replicas: 5"]);
  await expect(page.locator("aside").getByText("Server-side validation passed.")).toBeVisible();

  // Side by side shows both versions.
  await button(page, "Side by side").click();
  await expect(page.locator("aside .cm-mergeView .cm-editor")).toHaveCount(2);
  await button(page, "Unified").click();

  await button(page, "Save").click();
  await expect(page.locator("aside").getByText("Saved.")).toBeVisible();
  const applies = await fake.calls("cmdApplyResourceYaml");
  expect(applies).toHaveLength(2);
  const saved = applies[1];
  expect(saved).toMatchObject({ resource: DEPLOYMENT, namespace: "demo", name: "web", resourceVersion: "1", dryRun: false });
  expect(saved.yaml).toMatch(/kind: Deployment\n[\s\S]*spec:\n {2}replicas: 5\n$/);
  // Reloaded from the server after the save: the new version, no unsaved edits.
  expect(await fake.calls("cmdGetResourceYaml")).toHaveLength(2);
  const reloaded = await editorOf(page, "Deployment");
  await expect(reloaded).toContainText('resourceVersion: "2"');
  await expect(reloaded).toContainText("replicas: 5");
  await expect(button(page, "Review")).toBeDisabled();
  await expect(page.getByRole("note")).toHaveCount(0);
});

test("review shows the fields a save would take over, and the save reports them", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await fake.setCluster(
    (cluster) => {
      cluster.nextTakeovers = [
        { manager: "kubectl-client-side-apply", fields: [".spec.replicas", '.spec.template.spec.containers[name="web"].image'] },
        { manager: "helm", fields: [".metadata.labels.app"] },
      ];
    },
    null,
  );
  await append(page, editor, "spec:\n  replicas: 5\n");

  await button(page, "Review").click();
  const note = page.getByRole("note");
  await expect(note).toContainText("Saving will take over fields another tool is managing.");
  await expect(note.getByRole("listitem")).toHaveText([
    'from kubectl-client-side-apply: .spec.replicas, .spec.template.spec.containers[name="web"].image',
    "from helm: .metadata.labels.app",
  ]);

  await button(page, "Save").click();
  await expect(note).toContainText("This save took over fields another tool was managing.");
  await expect(note.locator("strong")).toHaveText(["kubectl-client-side-apply", "helm"]);
});

test("a dry-run error is shown, points at the line, and nothing is saved", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: -1\n");
  await fake.failNext(
    "cmdApplyResourceYaml",
    'Deployment.apps "web" is invalid: spec.replicas: Invalid value: -1: must be greater than or equal to 0',
  );

  await button(page, "Review").click();
  const alert = page.locator("aside").getByRole("alert");
  await expect(alert).toContainText("The server rejected this YAML: Deployment.apps \"web\" is invalid: spec.replicas");
  await expect(alert).toContainText("(line 8)");
  await expect(page.locator("aside .cm-errorLine")).toHaveText("  replicas: -1");
  // Still editing, the edit kept; only the dry run was sent.
  await expect(editor).toContainText("replicas: -1");
  await expect(button(page, "Save")).toHaveCount(0);
  const applies = await fake.calls("cmdApplyResourceYaml");
  expect(applies.map((c) => c.dryRun)).toEqual([true]);
});

test("a rejected save shows the server's reason and keeps the edit", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "broken: [\n");
  await button(page, "Review").click();
  await fake.failNext("cmdApplyResourceYaml", "admission webhook denied the request");

  await button(page, "Save").click();
  await expect(page.locator("aside").getByRole("alert")).toContainText(
    "Could not save YAML: admission webhook denied the request",
  );
  await expect(await editorOf(page, "Deployment")).toContainText("broken: [");
});

test("a write by someone else is detected: diff against the latest, then save", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  await fake.setCluster((cluster) => {
    cluster.resourceVersions["apps/v1/deployments|demo/web"] = 7;
  }, null);

  await button(page, "Review").click();
  const alert = page.locator("aside").getByRole("alert");
  await expect(alert).toContainText("This object changed on the server since you loaded it.");
  await alert.getByRole("button", { name: "Diff against the latest" }).click();

  // Compared with the latest version; the edit is kept.
  await expect(page.getByRole("textbox", { name: "Deployment YAML changes" })).toBeVisible();
  await expect(page.locator("aside .cm-changedLine")).toContainText(['resourceVersion: "1"', "spec:", "  replicas: 5"]);
  await button(page, "Save").click();
  await expect(page.locator("aside").getByText("Saved.")).toBeVisible();
  expect(await fake.lastCall("cmdApplyResourceYaml")).toMatchObject({ resourceVersion: "7", dryRun: false });
});

test("a conflict can be resolved by reloading", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  await fake.setCluster((cluster) => {
    cluster.resourceVersions["apps/v1/deployments|demo/web"] = 3;
  }, null);
  await button(page, "Review").click();
  await button(page, "Reload (lose my edits)").click();
  await expect(editor).toContainText('resourceVersion: "3"');
  await expect(editor).not.toContainText("replicas: 5");
  await expect(page.locator("aside").getByRole("alert")).toHaveCount(0);
  await expect(button(page, "Review")).toBeDisabled();
});

test("closing the panel with unsaved edits asks first", async ({ page }) => {
  await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  const dialog = page.getByRole("alertdialog", { name: "Discard changes?" });

  await page.getByRole("button", { name: "Close details" }).click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Keep editing" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(editor).toContainText("replicas: 5");

  await page.getByRole("button", { name: "Close details" }).click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.locator("aside")).toHaveCount(0);

  // Discarded for good: re-opening shows the server's version.
  await row(page, "web").click();
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  await expect(await editorOf(page, "Deployment")).not.toContainText("replicas: 5");
});

test("switching view or object asks; switching tabs keeps the edit", async ({ page }) => {
  await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  const dialog = page.getByRole("alertdialog", { name: "Discard changes?" });

  // Another tab of the panel and back: the edit is still there.
  await page.locator("aside").getByRole("tab", { name: "Overview" }).click();
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  await expect(await editorOf(page, "Deployment")).toContainText("replicas: 5");

  // Another view: asks, Escape keeps editing.
  await page.getByRole("navigation").getByRole("button", { name: "Pods", exact: true }).click();
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.locator("aside .title")).toContainText("demo/web");

  // Another object: asks, and discarding opens it.
  await row(page, "api").click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.locator("aside .title")).toContainText("api");
});

test("switching context with unsaved edits asks first", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  const context = page.getByRole("combobox", { name: "Cluster context" });
  const dialog = page.getByRole("alertdialog", { name: "Discard changes?" });

  await context.selectOption("prod");
  await dialog.getByRole("button", { name: "Keep editing" }).click();
  await expect(context).toHaveValue("kind-dev");
  expect(await fake.calls("cmdSetContext")).toHaveLength(0);
  await expect(editor).toContainText("replicas: 5");

  await context.selectOption("prod");
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(page.locator("aside")).toHaveCount(0);
  expect(await fake.lastCall("cmdSetContext")).toEqual({ name: "prod" });
});

test("Reload with unsaved edits asks first", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = await editorOf(page, "Deployment");
  await append(page, editor, "spec:\n  replicas: 5\n");
  await button(page, "Reload").click();
  const dialog = page.getByRole("alertdialog", { name: "Discard changes?" });
  await dialog.getByRole("button", { name: "Keep editing" }).click();
  expect(await fake.calls("cmdGetResourceYaml")).toHaveLength(1);
  await expect(editor).toContainText("replicas: 5");

  await button(page, "Reload").click();
  await dialog.getByRole("button", { name: "Discard changes" }).click();
  await expect(editor).not.toContainText("replicas: 5");
  expect(await fake.calls("cmdGetResourceYaml")).toHaveLength(2);
});

test("a locked Secret is read-only until its values are revealed", async ({ page }) => {
  const fake = await openYaml(page, "Secrets", "db-creds");
  const aside = page.locator("aside");
  const editor = await editorOf(page, "Secret");
  await expect(editor).toContainText("password: <hidden>");
  await expect(editor).not.toBeEditable();
  await expect(editor).toHaveAttribute("contenteditable", "false");
  await expect(button(page, "Review")).toBeDisabled();
  await expect(aside.getByText("Values are hidden. Reveal them to edit this Secret.")).toBeVisible();

  await button(page, "Reveal values").click();
  await expect(editor).toContainText("password: hunter2");
  await expect(editor).toBeEditable();
  expect((await fake.lastCall("cmdGetResourceYaml")).reveal).toBe(true);

  await button(page, "Hide values").click();
  await expect(editor).toContainText("password: <hidden>");
  await expect(editor).not.toBeEditable();
  expect((await fake.lastCall("cmdGetResourceYaml")).reveal).toBe(false);
});

test("a failed load leaves nothing to review", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  await fake.failNext("cmdGetResourceYaml", 'deployments.apps "web" not found');
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  await expect(page.locator("aside").getByText('Could not load YAML: deployments.apps "web" not found')).toBeVisible();
  await expect(button(page, "Review")).toBeDisabled();
});
