const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const state = {
  view: "overview",
  namespace: "",
  search: "",
  namespaces: [],
  timer: null,
  selectedPod: null,
  detailContainers: [],
  logFollow: true,
  metrics: { pods: {}, nodes: {} },
  execSession: null,
  term: null,
  fitAddon: null,
  pfSession: null,
};

const COLUMNS = {
  pods: [
    ["name", "Name"],
    ["namespace", "Namespace"],
    ["phase", "Phase"],
    ["node", "Node"],
    ["age", "Age"],
    ["cpu", "CPU"],
    ["memory", "Memory"],
  ],
  namespaces: [
    ["name", "Name"],
    ["status", "Status"],
    ["age", "Age"],
  ],
  nodes: [
    ["name", "Name"],
    ["ready", "Ready"],
    ["version", "Version"],
    ["age", "Age"],
    ["cpu", "CPU"],
    ["memory", "Memory"],
  ],
};

const els = {
  dot: document.getElementById("status-dot"),
  statusText: document.getElementById("status-text"),
  refreshBtn: document.getElementById("refresh-btn"),
  contextSelect: document.getElementById("context-select"),
  namespaceSelect: document.getElementById("namespace-select"),
  searchInput: document.getElementById("search-input"),
  autoRefresh: document.getElementById("auto-refresh"),
  error: document.getElementById("error"),
  toolbar: document.getElementById("toolbar"),
  tableWrap: document.getElementById("table-wrap"),
  overview: document.getElementById("overview"),
  head: document.getElementById("table-head"),
  body: document.getElementById("table-body"),
  empty: document.getElementById("empty"),
  count: document.getElementById("count"),
  updated: document.getElementById("updated"),
  detail: document.getElementById("detail"),
  detailTitle: document.getElementById("detail-title"),
  detailBody: document.getElementById("detail-body"),
  detailClose: document.getElementById("detail-close"),
  logOutput: document.getElementById("log-output"),
  logFollow: document.getElementById("log-follow"),
  logClear: document.getElementById("log-clear"),
  logPause: document.getElementById("log-pause"),
  logDownload: document.getElementById("log-download"),
  logContainer: document.getElementById("log-container"),
  logPrevious: document.getElementById("log-previous"),
  logTail: document.getElementById("log-tail"),
  logSince: document.getElementById("log-since"),
  logTimestamps: document.getElementById("log-timestamps"),
  logWrap: document.getElementById("log-wrap"),
  logFilter: document.getElementById("log-filter"),
  logStatus: document.getElementById("log-status"),
  terminal: document.getElementById("terminal"),
  termOpen: document.getElementById("term-open"),
  termClose: document.getElementById("term-close"),
  pfPort: document.getElementById("pf-port"),
  pfStart: document.getElementById("pf-start"),
  pfStop: document.getElementById("pf-stop"),
  pfStatus: document.getElementById("pf-status"),
  yamlReload: document.getElementById("yaml-reload"),
  yamlSave: document.getElementById("yaml-save"),
  yamlEditor: document.getElementById("yaml-editor"),
  yamlStatus: document.getElementById("yaml-status"),
};

function setStatus(ok, message) {
  els.dot.className = "dot " + (ok === null ? "unknown" : ok ? "ok" : "error");
  els.statusText.textContent = message;
}

function showError(message) {
  if (!message) {
    els.error.classList.add("hidden");
    els.error.textContent = "";
    return;
  }
  els.error.textContent = message;
  els.error.classList.remove("hidden");
}

function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

async function loadContexts() {
  try {
    const contexts = await invoke("cmd_list_contexts");
    els.contextSelect.innerHTML = "";
    for (const ctx of contexts) {
      const opt = document.createElement("option");
      opt.value = ctx.name;
      opt.textContent = ctx.name;
      if (ctx.current) opt.selected = true;
      els.contextSelect.appendChild(opt);
    }
    els.contextSelect.style.visibility = contexts.length > 1 ? "visible" : "hidden";
  } catch (e) {
    // No kubeconfig contexts (e.g. in-cluster): leave the selector hidden.
    els.contextSelect.style.visibility = "hidden";
  }
}

async function loadNamespaces() {
  try {
    state.namespaces = await invoke("cmd_list_namespaces");
    const current = els.namespaceSelect.value;
    els.namespaceSelect.innerHTML = '<option value="">All namespaces</option>';
    for (const ns of state.namespaces) {
      const opt = document.createElement("option");
      opt.value = ns.name;
      opt.textContent = ns.name;
      els.namespaceSelect.appendChild(opt);
    }
    els.namespaceSelect.value = current;
  } catch (e) {
    // Namespaces failing usually means the cluster is unreachable;
    // the main view load will surface the error.
  }
}

function filtered(rows) {
  const q = state.search.trim().toLowerCase();
  if (!q) return rows;
  return rows.filter((row) =>
    Object.values(row).some((v) => String(v).toLowerCase().includes(q))
  );
}

function formatCpu(millicores) {
  if (millicores >= 1000) {
    return (millicores / 1000).toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  }
  return millicores + "m";
}

function formatMemory(bytes) {
  const units = ["B", "Ki", "Mi", "Gi", "Ti", "Pi"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  if (unit === 0) return bytes + "B";
  return value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "") + units[unit];
}

function metricValue(row, key) {
  let m = null;
  if (state.view === "pods") {
    m = state.metrics.pods[`${row.namespace}/${row.name}`];
    if (!m) return "—";
    return key === "cpu" ? formatCpu(m.cpu_millicores) : formatMemory(m.memory_bytes);
  }
  if (state.view === "nodes") {
    m = state.metrics.nodes[row.name];
    if (!m) return "—";
    return key === "cpu"
      ? formatCpu(m.cpu_millicores) +
          (m.cpu_capacity_millicores ? " / " + formatCpu(m.cpu_capacity_millicores) : "")
      : formatMemory(m.memory_bytes) +
          (m.memory_capacity_bytes ? " / " + formatMemory(m.memory_capacity_bytes) : "");
  }
  return "";
}

function render(rows) {
  const cols = COLUMNS[state.view];
  els.head.innerHTML =
    "<tr>" + cols.map(([, label]) => `<th>${label}</th>`).join("") + "</tr>";

  const visible = filtered(rows);
  els.body.innerHTML = visible
    .map((row) => {
      const cells = cols
        .map(([key]) => {
          const value =
            key === "cpu" || key === "memory" ? metricValue(row, key) : row[key] ?? "";
          const cls = key === "phase" ? ` class="phase-${value}"` : "";
          return `<td${cls}>${escapeHtml(String(value))}</td>`;
        })
        .join("");
      const isSelected =
        state.view === "pods" &&
        state.selectedPod &&
        state.selectedPod.namespace === row.namespace &&
        state.selectedPod.name === row.name;
      const attrs =
        state.view === "pods"
          ? ` class="clickable${isSelected ? " selected" : ""}" data-namespace="${escapeHtml(row.namespace)}" data-name="${escapeHtml(row.name)}"`
          : "";
      return `<tr${attrs}>${cells}</tr>`;
    })
    .join("");

  els.empty.classList.toggle("hidden", visible.length > 0);
  els.count.textContent = `${visible.length} of ${rows.length} items`;
  els.updated.textContent = "Updated " + new Date().toLocaleTimeString();
}

function renderOverview(ov) {
  const card = (num, label) =>
    `<div class="card"><div class="num">${num}</div><div class="label">${label}</div></div>`;
  const events = ov.events
    .map(
      (e) => `<div class="event event-${escapeHtml(e.kind)}">
        <span class="event-age">${escapeHtml(e.age)}</span>
        <span class="event-reason">${escapeHtml(e.reason)}</span>
        <span class="event-object">${escapeHtml(e.object)}</span>
        <span class="event-message">${escapeHtml(e.message)}${e.count > 1 ? ` (x${e.count})` : ""}</span>
      </div>`
    )
    .join("");
  const byNs = ov.pods_by_namespace
    .map(([ns, n]) => `<tr><td>${escapeHtml(ns)}</td><td>${n}</td></tr>`)
    .join("");
  els.overview.innerHTML = `
    <div class="cards">
      ${card(ov.namespaces, "Namespaces")}
      ${card(`${ov.nodes_ready}/${ov.nodes}`, "Nodes ready")}
      ${card(ov.pods_total, "Pods")}
      ${card(ov.pods_running, "Running")}
      ${card(ov.pods_pending, "Pending")}
      ${card(ov.pods_failed, "Failed")}
    </div>
    <div class="overview-sections">
      <section>
        <h3>Pods by namespace</h3>
        <table><thead><tr><th>Namespace</th><th>Pods</th></tr></thead><tbody>${byNs}</tbody></table>
      </section>
      <section>
        <h3>Recent events</h3>
        ${events || '<p class="muted">No events.</p>'}
      </section>
    </div>`;
  els.count.textContent = `${ov.pods_total} pods, ${ov.nodes} nodes`;
  els.updated.textContent = "Updated " + new Date().toLocaleTimeString();
}

async function loadMetrics() {
  try {
    const snap = await invoke("cmd_get_metrics");
    state.metrics = { pods: {}, nodes: {} };
    for (const p of snap.pods) state.metrics.pods[`${p.namespace}/${p.name}`] = p;
    for (const n of snap.nodes) state.metrics.nodes[n.name] = n;
  } catch (e) {
    // Metrics are best effort; the tables show "—" without them.
    state.metrics = { pods: {}, nodes: {} };
  }
}

async function loadView() {
  showError("");
  try {
    let rows;
    if (state.view === "pods") {
      rows = await invoke("cmd_list_pods", {
        namespace: state.namespace || null,
      });
    } else if (state.view === "namespaces") {
      rows = await invoke("cmd_list_namespaces");
    } else {
      rows = await invoke("cmd_list_nodes");
    }
    setStatus(true, "Connected");
    await loadMetrics();
    render(rows);
  } catch (e) {
    setStatus(false, "Cluster unreachable");
    showError("Could not reach the cluster: " + e);
    render([]);
  }
}

async function loadOverview() {
  showError("");
  try {
    const ov = await invoke("cmd_get_overview");
    setStatus(true, "Connected");
    renderOverview(ov);
  } catch (e) {
    setStatus(false, "Cluster unreachable");
    showError("Could not reach the cluster: " + e);
    els.overview.innerHTML = "";
  }
}

function scheduleAutoRefresh() {
  if (state.timer) clearInterval(state.timer);
  state.timer = null;
  if (els.autoRefresh.checked) {
    state.timer = setInterval(() => {
      if (state.view === "overview") loadOverview();
      else loadView();
    }, 30_000);
  }
}

function setView(view) {
  state.view = view;
  closeDetail();
  document.querySelectorAll(".nav-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.view === view);
  });
  const isOverview = view === "overview";
  els.toolbar.classList.toggle("hidden", isOverview);
  els.tableWrap.classList.toggle("hidden", isOverview);
  els.overview.classList.toggle("hidden", !isOverview);
  els.namespaceSelect.style.visibility = view === "pods" ? "visible" : "hidden";
  if (isOverview) loadOverview();
  else loadView();
}

// ---- Logs -----------------------------------------------------------------
// Lines live in a bounded buffer (LOG_BUFFER_MAX, trimmed in chunks so it is
// amortised O(1) per line). The DOM only ever shows the newest LOG_DOM_MAX
// matching lines, and incoming batches are appended once per animation frame
// instead of re-rendering the buffer.
const LOG_BUFFER_MAX = 50000;
const LOG_DOM_MAX = 5000;

const logs = {
  streamId: 0,
  buf: [],
  pending: [],
  paused: false,
  filter: "",
  raf: 0,
  filterTimer: 0,
};

function logLineNode(text) {
  const div = document.createElement("div");
  const f = logs.filter;
  if (!f) {
    div.textContent = text;
    return div;
  }
  const lower = text.toLowerCase();
  const needle = f.toLowerCase();
  let pos = 0;
  for (;;) {
    const i = lower.indexOf(needle, pos);
    if (i < 0) break;
    if (i > pos) div.appendChild(document.createTextNode(text.slice(pos, i)));
    const mark = document.createElement("mark");
    mark.textContent = text.slice(i, i + needle.length);
    div.appendChild(mark);
    pos = i + needle.length;
  }
  if (pos < text.length) div.appendChild(document.createTextNode(text.slice(pos)));
  return div;
}

function logMatches(text) {
  return !logs.filter || text.toLowerCase().includes(logs.filter.toLowerCase());
}

function updateLogStatus() {
  const n = logs.buf.length;
  els.logStatus.textContent =
    (logs.paused ? "paused · " : "") + n.toLocaleString() + (n >= LOG_BUFFER_MAX ? "+" : "") + " lines";
}

function isAtBottom() {
  return els.logOutput.scrollHeight - els.logOutput.scrollTop - els.logOutput.clientHeight < 40;
}

// Append system messages (errors, "stream ended"); these are not part of the buffer.
function appendLog(text, cls) {
  const atBottom = isAtBottom();
  const div = document.createElement("div");
  if (cls) div.className = cls;
  div.textContent = text;
  els.logOutput.appendChild(div);
  if (state.logFollow && atBottom) els.logOutput.scrollTop = els.logOutput.scrollHeight;
}

function flushLogs() {
  logs.raf = 0;
  if (logs.paused || logs.pending.length === 0) return;
  const lines = logs.pending;
  logs.pending = [];
  const atBottom = isAtBottom();
  const frag = document.createDocumentFragment();
  for (const line of lines) {
    if (logMatches(line)) frag.appendChild(logLineNode(line));
  }
  els.logOutput.appendChild(frag);
  let extra = els.logOutput.childElementCount - LOG_DOM_MAX;
  while (extra-- > 0) els.logOutput.removeChild(els.logOutput.firstChild);
  if (state.logFollow && atBottom) els.logOutput.scrollTop = els.logOutput.scrollHeight;
  updateLogStatus();
}

function onLogBatch(lines) {
  const buf = logs.buf;
  for (const l of lines) buf.push(l);
  if (buf.length > LOG_BUFFER_MAX + LOG_BUFFER_MAX / 10) {
    buf.splice(0, buf.length - LOG_BUFFER_MAX);
  }
  if (logs.paused) {
    updateLogStatus();
    return;
  }
  for (const l of lines) logs.pending.push(l);
  if (!logs.raf) logs.raf = requestAnimationFrame(flushLogs);
}

// Rebuild the DOM from the buffer (filter change, resume).
function renderLogs() {
  logs.pending = [];
  els.logOutput.textContent = "";
  const frag = document.createDocumentFragment();
  const picked = [];
  for (let i = logs.buf.length - 1; i >= 0 && picked.length < LOG_DOM_MAX; i--) {
    if (logMatches(logs.buf[i])) picked.push(logs.buf[i]);
  }
  for (let i = picked.length - 1; i >= 0; i--) frag.appendChild(logLineNode(picked[i]));
  els.logOutput.appendChild(frag);
  if (state.logFollow) els.logOutput.scrollTop = els.logOutput.scrollHeight;
  updateLogStatus();
}

function resetLogs() {
  logs.buf = [];
  logs.pending = [];
  els.logOutput.textContent = "";
  updateLogStatus();
}

function setLogPaused(paused) {
  logs.paused = paused;
  els.logPause.textContent = paused ? "Resume" : "Pause";
  els.logPause.classList.toggle("active", paused);
  if (!paused) renderLogs();
  else updateLogStatus();
}

function populateLogContainers(list) {
  els.logContainer.innerHTML = "";
  for (const c of list) {
    const opt = document.createElement("option");
    opt.value = c.name;
    opt.textContent = c.init ? `${c.name} (init)` : c.name;
    els.logContainer.appendChild(opt);
  }
  const firstMain = list.find((c) => !c.init);
  if (firstMain) els.logContainer.value = firstMain.name;
}

async function startLogs() {
  const pod = state.selectedPod;
  if (!pod) return;
  resetLogs();
  setLogPaused(false);
  const since = els.logSince.value;
  const tail = els.logTail.value;
  logs.streamId += 1;
  try {
    await invoke("cmd_stream_pod_logs", {
      namespace: pod.namespace,
      name: pod.name,
      container: els.logContainer.value || null,
      previous: els.logPrevious.checked,
      tailLines: tail === "all" ? null : Number(tail),
      sinceSeconds: since === "all" ? null : Number(since),
      timestamps: els.logTimestamps.checked,
      streamId: logs.streamId,
    });
  } catch (e) {
    appendLog("Could not start log stream: " + e, "error-text");
  }
}

async function downloadLogs() {
  const pod = state.selectedPod;
  if (!pod) return;
  const container = els.logContainer.value || "pod";
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const filename = `${pod.name}_${container}${els.logPrevious.checked ? "_previous" : ""}_${stamp}.log`;
  try {
    // Written by the backend: Blob/anchor downloads are not reliable in
    // Tauri's webviews, and a save dialog would need an extra plugin.
    const path = await invoke("cmd_save_logs", { filename, content: logs.buf.join("\n") + "\n" });
    els.logStatus.textContent = "saved to " + path;
  } catch (e) {
    appendLog("Could not save logs: " + e, "error-text");
  }
}

async function openDetail(namespace, name) {
  state.selectedPod = { namespace, name };
  els.detailTitle.textContent = `${namespace}/${name}`;
  els.detailBody.innerHTML = "<p class='muted'>Loading…</p>";
  resetLogs();
  els.logPrevious.checked = false;
  els.logContainer.innerHTML = "";
  closeTerminal();
  stopPortForward();
  els.yamlEditor.value = "";
  els.yamlStatus.textContent = "";
  els.detail.classList.remove("hidden");
  try {
    const d = await invoke("cmd_get_pod_details", { namespace, name });
    state.detailContainers = d.containers || [];
    populateLogContainers(d.log_containers || []);
    const labels = Object.entries(d.labels || {})
      .map(([k, v]) => `<span class="label-chip">${escapeHtml(k)}=${escapeHtml(v)}</span>`)
      .join(" ");
    const containers = d.containers
      .map(
        (c) => `<tr>
          <td>${escapeHtml(c.name)}</td>
          <td>${escapeHtml(c.image)}</td>
          <td>${escapeHtml(c.state)}</td>
          <td>${c.restarts}</td>
        </tr>`
      )
      .join("");
    els.detailBody.innerHTML = `
      <dl class="meta">
        <dt>Phase</dt><dd class="phase-${escapeHtml(d.phase)}">${escapeHtml(d.phase)}</dd>
        <dt>Node</dt><dd>${escapeHtml(d.node)}</dd>
        <dt>IP</dt><dd>${escapeHtml(d.ip || "—")}</dd>
        <dt>Age</dt><dd>${escapeHtml(d.age)}</dd>
      </dl>
      ${labels ? `<div class="labels">${labels}</div>` : ""}
      <h3>Containers</h3>
      <div class="scroll-x">
      <table><thead><tr><th>Name</th><th>Image</th><th>State</th><th>Restarts</th></tr></thead>
      <tbody>${containers || "<tr><td colspan='4'>No containers.</td></tr>"}</tbody></table>
      </div>`;
  } catch (e) {
    els.detailBody.innerHTML = `<p class="error-text">Could not load pod details: ${escapeHtml(String(e))}</p>`;
  }
  loadYaml();
  await startLogs();
}

async function closeDetail() {
  if (!state.selectedPod) return;
  state.selectedPod = null;
  state.detailContainers = [];
  els.detail.classList.add("hidden");
  logs.streamId += 1;
  resetLogs();
  closeTerminal();
  stopPortForward();
  try {
    await invoke("cmd_stop_pod_logs");
  } catch (e) {
    // Stream was already stopped; nothing to do.
  }
}

async function openTerminal() {
  const pod = state.selectedPod;
  if (!pod) return;
  closeTerminal();
  els.terminal.classList.remove("hidden");
  els.termOpen.classList.add("hidden");
  els.termClose.classList.remove("hidden");
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 12,
    theme: { background: "#0d1013" },
  });
  const fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  term.open(els.terminal);
  fitAddon.fit();
  state.term = term;
  state.fitAddon = fitAddon;
  try {
    const container = state.detailContainers[0] ? state.detailContainers[0].name : null;
    const session = await invoke("cmd_exec_start", {
      namespace: pod.namespace,
      name: pod.name,
      container,
      cols: term.cols,
      rows: term.rows,
    });
    state.execSession = session;
    term.onData((data) => {
      invoke("cmd_exec_input", { session, data }).catch(() => {});
    });
    term.focus();
  } catch (e) {
    term.write("\r\n\x1b[31mCould not open terminal: " + e + "\x1b[0m\r\n");
  }
}

function closeTerminal() {
  if (state.execSession) {
    invoke("cmd_exec_close", { session: state.execSession }).catch(() => {});
    state.execSession = null;
  }
  if (state.term) {
    state.term.dispose();
    state.term = null;
    state.fitAddon = null;
  }
  els.terminal.classList.add("hidden");
  els.termOpen.classList.remove("hidden");
  els.termClose.classList.add("hidden");
}

async function startPortForward() {
  const pod = state.selectedPod;
  if (!pod) return;
  const port = parseInt(els.pfPort.value, 10);
  if (!port || port < 1 || port > 65535) {
    els.pfStatus.textContent = "Enter a port between 1 and 65535.";
    return;
  }
  els.pfStatus.textContent = "Starting…";
  try {
    const info = await invoke("cmd_portforward_start", {
      namespace: pod.namespace,
      name: pod.name,
      port,
    });
    state.pfSession = info.session;
    els.pfStatus.textContent = `Forwarding 127.0.0.1:${info.local_port} → ${pod.name}:${port}`;
    els.pfStart.classList.add("hidden");
    els.pfStop.classList.remove("hidden");
  } catch (e) {
    els.pfStatus.textContent = "Could not start port-forward: " + e;
  }
}

function stopPortForward() {
  if (state.pfSession) {
    invoke("cmd_portforward_stop", { session: state.pfSession }).catch(() => {});
    state.pfSession = null;
  }
  els.pfStatus.textContent = "";
  els.pfStart.classList.remove("hidden");
  els.pfStop.classList.add("hidden");
}

async function loadYaml() {
  const pod = state.selectedPod;
  if (!pod) return;
  els.yamlEditor.value = "Loading…";
  els.yamlStatus.textContent = "";
  try {
    els.yamlEditor.value = await invoke("cmd_get_pod_yaml", {
      namespace: pod.namespace,
      name: pod.name,
    });
  } catch (e) {
    els.yamlEditor.value = "";
    els.yamlStatus.textContent = "Could not load YAML: " + e;
  }
}

async function saveYaml() {
  const pod = state.selectedPod;
  if (!pod) return;
  els.yamlStatus.textContent = "Saving…";
  try {
    await invoke("cmd_apply_pod_yaml", {
      namespace: pod.namespace,
      name: pod.name,
      yaml: els.yamlEditor.value,
    });
    els.yamlStatus.textContent = "Saved.";
    await loadYaml();
  } catch (e) {
    els.yamlStatus.textContent = "Could not save YAML: " + e;
  }
}

document.querySelectorAll(".nav-btn").forEach((btn) => {
  btn.addEventListener("click", () => setView(btn.dataset.view));
});
els.refreshBtn.addEventListener("click", () => {
  if (state.view === "overview") loadOverview();
  else loadView();
});
els.contextSelect.addEventListener("change", async (e) => {
  const name = e.target.value || null;
  try {
    await invoke("cmd_set_context", { name });
  } catch (err) {
    showError("Could not switch context: " + err);
    return;
  }
  closeDetail();
  await loadNamespaces();
  if (state.view === "overview") loadOverview();
  else loadView();
});
els.namespaceSelect.addEventListener("change", (e) => {
  state.namespace = e.target.value;
  closeDetail();
  loadView();
});
els.searchInput.addEventListener("input", (e) => {
  state.search = e.target.value;
  loadView();
});
els.autoRefresh.addEventListener("change", scheduleAutoRefresh);
els.body.addEventListener("click", (e) => {
  const row = e.target.closest("tr.clickable");
  if (!row) return;
  openDetail(row.dataset.namespace, row.dataset.name);
});
els.detailClose.addEventListener("click", closeDetail);
els.termOpen.addEventListener("click", openTerminal);
els.termClose.addEventListener("click", closeTerminal);
els.pfStart.addEventListener("click", startPortForward);
els.pfStop.addEventListener("click", stopPortForward);
els.yamlReload.addEventListener("click", loadYaml);
els.yamlSave.addEventListener("click", saveYaml);
window.addEventListener("resize", () => {
  if (state.term && state.fitAddon && state.execSession) {
    state.fitAddon.fit();
    invoke("cmd_exec_resize", {
      session: state.execSession,
      cols: state.term.cols,
      rows: state.term.rows,
    }).catch(() => {});
  }
});
els.logFollow.addEventListener("change", (e) => {
  state.logFollow = e.target.checked;
});
els.logClear.addEventListener("click", resetLogs);
els.logPause.addEventListener("click", () => setLogPaused(!logs.paused));
els.logDownload.addEventListener("click", downloadLogs);
for (const el of [els.logContainer, els.logPrevious, els.logTail, els.logSince, els.logTimestamps]) {
  el.addEventListener("change", startLogs);
}
els.logWrap.addEventListener("change", (e) => {
  els.logOutput.classList.toggle("wrap", e.target.checked);
});
els.logFilter.addEventListener("input", (e) => {
  clearTimeout(logs.filterTimer);
  logs.filterTimer = setTimeout(() => {
    logs.filter = e.target.value;
    renderLogs();
  }, 150);
});

(async () => {
  // Events carry the id of the stream that produced them; ignore stragglers
  // from a stream that has since been replaced.
  const current = (event) => event.payload.streamId === logs.streamId;
  await listen("pod-log", (event) => current(event) && onLogBatch(event.payload.data));
  await listen("pod-log-done", (event) => current(event) && appendLog("[stream ended]", "muted"));
  await listen("pod-log-error", (event) => current(event) && appendLog(event.payload.data, "error-text"));
  // Output can arrive before cmd_exec_start resolves and state.execSession
  // is set, so while a terminal is open without a session yet, accept
  // output for it as well.
  await listen("exec-output", (event) => {
    const { session, data } = event.payload;
    if (state.term && (session === state.execSession || !state.execSession)) {
      state.term.write(data);
    }
  });
  await listen("exec-closed", (event) => {
    if (state.term && (event.payload === state.execSession || !state.execSession)) {
      state.term.write("\r\n\x1b[90m[session closed]\x1b[0m\r\n");
      // Keep state.execSession: a failure message can arrive just after
      // the close, and the exec-output handler drops messages for a
      // session it no longer knows.
    }
  });
  await loadContexts();
  await loadNamespaces();
  scheduleAutoRefresh();
  setView("overview");
})();
