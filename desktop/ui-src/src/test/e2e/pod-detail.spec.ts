import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { openApp, openKind, row } from "./helpers";
import type { Fake } from "./helpers";

async function openPod(page: Page, name = "web-1"): Promise<Fake> {
  const fake = await openApp(page);
  await openKind(page, "Pods");
  await row(page, name).click();
  await expect(page.locator("aside .title")).toHaveText(`demo/${name}`);
  return fake;
}

const panel = (page: Page) => page.getByTestId("detail-panel");
const logLines = (page: Page) => panel(page).getByTestId("log-line");

test("the pod panel shows details and starts the log stream with the right args", async ({ page }) => {
  const fake = await openPod(page);
  await expect(panel(page).locator(".card .image").filter({ hasText: "nginx:1.27" })).toBeVisible();
  await expect(row(page, "web-1")).toHaveClass(/selected/);

  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  const call = await fake.lastCall("cmdStreamPodLogs");
  // The first non-init container, the last 200 lines, no time limit.
  expect(call).toEqual({
    namespace: "demo",
    name: "web-1",
    container: "app",
    previous: false,
    tailLines: 200,
    sinceSeconds: null,
    timestamps: false,
    streamId: await fake.lastStreamId(),
  });
  expect(await fake.calls("cmdGetResourceDetail")).toEqual([
    { resource: expect.objectContaining({ kind: "Pod", group: "" }), namespace: "demo", name: "web-1" },
  ]);

  await fake.emitLogLines(["hello", "world"]);
  await expect(logLines(page)).toHaveText(["hello", "world"]);
  await expect(panel(page).getByText("2 lines")).toBeVisible();
});

test("changing an option restarts the stream; lines of the old stream are dropped", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  const first = (await fake.lastStreamId())!;
  await fake.emitLogLines(["from the first stream"]);
  await expect(logLines(page)).toHaveText(["from the first stream"]);

  await panel(page).getByTitle("Tail lines").selectOption("1000");
  await panel(page).getByTitle("Since").selectOption("3600");
  await panel(page).getByTitle("Container").selectOption("init-db");
  await panel(page).getByLabel("Timestamps").check();
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(5);
  const last = await fake.lastCall("cmdStreamPodLogs");
  expect(last).toMatchObject({ container: "init-db", tailLines: 1000, sinceSeconds: 3600, timestamps: true });
  const current = (await fake.lastStreamId())!;
  expect(current).toBeGreaterThan(first);

  // The restart cleared the view; a late batch from the first stream stays out.
  await fake.emitLogLines(["straggler"], first);
  await fake.emitLogLines(["fresh"], current);
  await expect(logLines(page)).toHaveText(["fresh"]);
});

test("the log filter shows matching lines and highlights the match", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await fake.emitLogLines(["GET /health 200", "ERROR db timeout", "GET /api 500 error"]);
  await expect(logLines(page)).toHaveCount(3);

  await panel(page).getByPlaceholder("Filter logs…").fill("error");
  await expect(logLines(page)).toHaveText(["ERROR db timeout", "GET /api 500 error"]);
  // Case-insensitive, highlighted in the original case.
  await expect(logLines(page).locator("mark")).toHaveText(["ERROR", "error"]);

  // New lines go through the same filter.
  await fake.emitLogLines(["all good", "another error"]);
  await expect(logLines(page)).toHaveCount(3);

  await panel(page).getByPlaceholder("Filter logs…").fill("");
  await expect(logLines(page)).toHaveCount(5);
  await expect(logLines(page).locator("mark")).toHaveCount(0);
});

test("pause holds new lines back until resume", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await fake.emitLogLines(["one"]);
  await expect(logLines(page)).toHaveText(["one"]);

  await panel(page).getByRole("button", { name: "Pause" }).click();
  await fake.emitLogLines(["two", "three"]);
  await expect(panel(page).getByText("paused · 3 lines")).toBeVisible();
  await expect(logLines(page)).toHaveText(["one"]);

  await panel(page).getByRole("button", { name: "Resume" }).click();
  await expect(logLines(page)).toHaveText(["one", "two", "three"]);
  await expect(panel(page).getByText("3 lines", { exact: true })).toBeVisible();
  // Pausing does not touch the stream itself.
  expect(await fake.calls("cmdStreamPodLogs")).toHaveLength(1);
});

test("stream end and errors are shown as system lines", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await fake.emitLogLines(["last words"]);
  // Log lines are flushed on the next animation frame while system lines are
  // appended at once, so wait for the line before ending the stream.
  await expect(logLines(page)).toHaveText(["last words"]);
  await page.evaluate(() => window.__fake.emitLogEnd("container not found"));
  await expect(logLines(page)).toHaveText(["last words", "container not found", "[stream ended]"]);
});

test("download saves the buffered lines through cmd_save_logs", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await fake.emitLogLines(["alpha", "beta"]);
  await expect(logLines(page)).toHaveCount(2);

  await panel(page).getByRole("button", { name: "Download" }).click();
  await expect.poll(() => fake.calls("cmdSaveLogs")).toHaveLength(1);
  const call = await fake.lastCall("cmdSaveLogs");
  expect(call.filename).toMatch(/^web-1_app_\d{4}-\d\d-\d\dT[\d-]+Z\.log$/);
  expect(call.content).toBe("alpha\nbeta\n");
  await expect(panel(page).getByText(`saved to /home/tester/Downloads/${call.filename}`)).toBeVisible();
});

test("a failed download is reported in the log view", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await fake.failNext("cmdSaveLogs", "No Downloads folder");
  await panel(page).getByRole("button", { name: "Download" }).click();
  await expect(logLines(page)).toHaveText(["Could not save logs: No Downloads folder"]);
});

test("closing the panel stops the log stream", async ({ page }) => {
  const fake = await openPod(page);
  await expect.poll(() => fake.calls("cmdStreamPodLogs")).toHaveLength(1);
  await panel(page).getByRole("button", { name: "Close details" }).click();
  await expect(panel(page)).toHaveCount(0);
  await expect.poll(() => fake.calls("cmdStopPodLogs")).toHaveLength(1);
});
