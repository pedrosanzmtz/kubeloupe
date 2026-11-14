import { mount } from "svelte";
import "../app.css";
import App from "../App.svelte";
import { installFakeBackend } from "./fake-backend";
import { defaultCluster } from "./fixtures/cluster";

// Test-only entry, served by Vite at /test.html for the Playwright tests (like
// bench.html, it is not part of the production build, which only bundles
// index.html): the real app on top of the fake backend.
const cluster = defaultCluster();
const fake = installFakeBackend(cluster);
window.__fakeInit?.(cluster, fake);
mount(App, { target: document.getElementById("app")! });
