import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import WorkloadLogsHost from "./lib/components/WorkloadLogsHost.svelte";
import { openWorkloadLogs } from "./lib/stores/workloadLogs.svelte";

const app = mount(App, { target: document.getElementById("app")! });

// The workload logs drawer lives on its own root so it works regardless of
// the app layout. Detail panels open it with `openWorkloadLogs(...)`.
const drawer = document.createElement("div");
document.body.appendChild(drawer);
mount(WorkloadLogsHost, { target: drawer });

// Temporary test entry (until the detail panels of #3 call
// `openWorkloadLogs`): `#workload-logs=Deployment/demo/web` in the URL.
const m = /^#workload-logs=([^/]+)\/([^/]+)\/(.+)$/.exec(window.location.hash);
if (m) openWorkloadLogs({ kind: m[1], namespace: m[2], name: m[3] });

export default app;
