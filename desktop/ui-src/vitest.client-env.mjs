// A Vitest environment that runs in Node (no DOM) but transforms modules the
// way the browser build does. With the default `node` environment the Svelte
// plugin compiles `.svelte.ts` modules for the server, where `$effect` never
// runs; this keeps the tests on the client runtime the app actually uses.
export default {
  name: "node-client",
  viteEnvironment: "client",
  setup() {
    return { teardown() {} };
  },
};
