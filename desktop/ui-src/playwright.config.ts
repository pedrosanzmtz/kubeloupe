import { defineConfig, devices } from "@playwright/test";

// App tests: the real UI (test.html → src/test/main.ts) against the fake
// backend in src/test/fake-backend.ts, in Chromium. Run with `npm run test:e2e`.
//
// Not the Tauri port (1420), so the tests can run next to `cargo tauri dev`.
const PORT = 1430;
const CI = !!process.env.CI;

export default defineConfig({
  testDir: "src/test/e2e",
  fullyParallel: true,
  forbidOnly: CI,
  retries: 0,
  reporter: CI ? [["github"], ["list"], ["html", { open: "never" }]] : [["list"]],
  outputDir: "test-results",
  use: {
    baseURL: `http://localhost:${PORT}`,
    // Saved under test-results/ (uploaded by CI on failure).
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1400, height: 900 },
        // PW_CHANNEL=chrome uses an installed Google Chrome instead of
        // Playwright's own Chromium (when the browser download is blocked).
        channel: process.env.PW_CHANNEL || undefined,
      },
    },
  ],
  webServer: {
    command: `npx vite --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}/test.html`,
    // Never reuse: another worktree may be serving different code on the port.
    reuseExistingServer: false,
  },
});
