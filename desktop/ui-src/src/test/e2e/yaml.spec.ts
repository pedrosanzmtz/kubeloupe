import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";

async function openYaml(page: Page, kindLabel: string, name: string) {
  const fake = await openApp(page);
  await openKind(page, kindLabel);
  await row(page, name).click();
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  return fake;
}

const DEPLOYMENT = { group: "apps", version: "v1", kind: "Deployment", plural: "deployments", namespaced: true };

test("the YAML tab loads the object", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = page.getByRole("textbox", { name: "Deployment YAML" });
  await expect(editor).toHaveValue(/kind: Deployment\n/);
  await expect(editor).toHaveValue(/  name: web\n  namespace: demo\n/);
  expect(await fake.lastCall("cmdGetResourceYaml")).toEqual({
    resource: DEPLOYMENT,
    namespace: "demo",
    name: "web",
    reveal: false,
  });
});

test("save applies the edited YAML and reloads it", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = page.getByRole("textbox", { name: "Deployment YAML" });
  await expect(editor).toHaveValue(/kind: Deployment/);
  const edited = (await editor.inputValue()) + "spec:\n  replicas: 5\n";
  await editor.fill(edited);

  await page.locator("aside").getByRole("button", { name: "Save" }).click();
  await expect(page.locator("aside").getByText("Saved.")).toBeVisible();
  expect(await fake.lastCall("cmdApplyResourceYaml")).toEqual({
    resource: DEPLOYMENT,
    namespace: "demo",
    name: "web",
    yaml: edited,
  });
  // Reloaded from the server after the save.
  expect(await fake.calls("cmdGetResourceYaml")).toHaveLength(2);
  await expect(editor).toHaveValue(edited);
  await expect(page.getByRole("note")).toHaveCount(0);
});

test("a save that takes over fields says which and from whom", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  await expect(page.getByRole("textbox", { name: "Deployment YAML" })).toHaveValue(/kind: Deployment/);
  await fake.setCluster(
    (cluster) => {
      cluster.nextTakeovers = [
        { manager: "kubectl-client-side-apply", fields: [".spec.replicas", ".spec.template.spec.containers[name=\"web\"].image"] },
        { manager: "helm", fields: [".metadata.labels.app"] },
      ];
    },
    null,
  );

  await page.locator("aside").getByRole("button", { name: "Save" }).click();
  const note = page.getByRole("note");
  await expect(note).toContainText("This save took over fields another tool was managing.");
  await expect(note.getByRole("listitem")).toHaveText([
    'from kubectl-client-side-apply: .spec.replicas, .spec.template.spec.containers[name="web"].image',
    "from helm: .metadata.labels.app",
  ]);
  await expect(note.locator("strong")).toHaveText(["kubectl-client-side-apply", "helm"]);

  // The next clean save clears the note.
  await page.locator("aside").getByRole("button", { name: "Save" }).click();
  await expect(note).toHaveCount(0);
});

test("a rejected save shows the server's reason and keeps the edit", async ({ page }) => {
  const fake = await openYaml(page, "Deployments", "web");
  const editor = page.getByRole("textbox", { name: "Deployment YAML" });
  await expect(editor).toHaveValue(/kind: Deployment/);
  await editor.fill("kind: Deployment\nbroken: [\n");
  await fake.failNext("cmdApplyResourceYaml", "admission webhook denied the request");

  await page.locator("aside").getByRole("button", { name: "Save" }).click();
  await expect(page.locator("aside").getByText("Could not save YAML: admission webhook denied the request")).toBeVisible();
  await expect(editor).toHaveValue("kind: Deployment\nbroken: [\n");
});

test("Secret values stay hidden until revealed", async ({ page }) => {
  const fake = await openYaml(page, "Secrets", "db-creds");
  const aside = page.locator("aside");
  const editor = page.getByRole("textbox", { name: "Secret YAML" });
  await expect(editor).toHaveValue(/password: <hidden>/);
  await expect(editor).not.toBeEditable();
  await expect(aside.getByRole("button", { name: "Save" })).toBeDisabled();
  await expect(aside.getByText("Values are hidden. Reveal them to edit this Secret.")).toBeVisible();

  await aside.getByRole("button", { name: "Reveal values" }).click();
  await expect(editor).toHaveValue(/password: hunter2/);
  await expect(editor).toBeEditable();
  await expect(aside.getByRole("button", { name: "Save" })).toBeEnabled();
  expect((await fake.lastCall("cmdGetResourceYaml")).reveal).toBe(true);

  await aside.getByRole("button", { name: "Hide values" }).click();
  await expect(editor).toHaveValue(/password: <hidden>/);
  await expect(editor).not.toBeEditable();
  expect((await fake.lastCall("cmdGetResourceYaml")).reveal).toBe(false);
});

test("a failed load leaves nothing to save", async ({ page }) => {
  const fake = await openApp(page);
  await openKind(page, "Deployments");
  await row(page, "web").click();
  await fake.failNext("cmdGetResourceYaml", "deployments.apps \"web\" not found");
  await page.locator("aside").getByRole("tab", { name: "YAML" }).click();
  await expect(page.locator("aside").getByText('Could not load YAML: deployments.apps "web" not found')).toBeVisible();
  await expect(page.locator("aside").getByRole("button", { name: "Save" })).toBeDisabled();
});
