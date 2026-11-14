import { defineConfig, mergeConfig } from "vitest/config";
import viteConfig from "./vite.config.ts";

// Unit tests for the pure UI logic (`src/**/*.test.ts`). Modules that use
// runes are compiled by the Svelte plugin from vite.config.ts; their tests are
// named `*.svelte.test.ts` so the plugin compiles the test file too. The
// `browser` condition makes `svelte` resolve to its client runtime, where
// `$state` is reactive (the server build is not).
//
// Playwright tests (`src/test/e2e/*.spec.ts`) are run by `npm run test:e2e`.
export default mergeConfig(
  viteConfig,
  defineConfig({
    resolve: { conditions: ["browser"] },
    test: {
      include: ["src/**/*.test.ts"],
      environment: "./vitest.client-env.mjs",
    },
  }),
);
