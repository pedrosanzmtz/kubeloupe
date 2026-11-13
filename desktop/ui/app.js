const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const state = {
  view: "overview",
  namespace: "",
  search: "",
  namespaces: [],
  timer: null,
  selectedPod: null,
  logFollow: true,
};

const COLUMNS = {
  pods: [
    ["name", "Name"],
    ["namespace", "Namespace"],
    ["phase", "Phase"],
    ["node", "Node"],
    ["age", "Age"],
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

function render(rows) {
  const cols = COLUMNS[state.view];
  els.head.innerHTML =
    "<tr>" + cols.map(([, label]) => `<th>${label}</th>`).join("") + "</tr>";

  const visible = filtered(rows);
  els.body.innerHTML = visible
    .map((row) => {
      const cells = cols
        .map(([key]) => {
          const value = row[key] ?? "";
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

function appendLog(text, cls) {
  const atBottom =
    els.logOutput.scrollHeight - els.logOutput.scrollTop - els.logOutput.clientHeight < 40;
  const div = document.createElement("div");
  if (cls) div.className = cls;
  div.textContent = text;
  els.logOutput.appendChild(div);
  if (state.logFollow && atBottom) {
    els.logOutput.scrollTop = els.logOutput.scrollHeight;
  }
}

async function openDetail(namespace, name) {
  state.selectedPod = { namespace, name };
  els.detailTitle.textContent = `${namespace}/${name}`;
  els.detailBody.innerHTML = "<p class='muted'>Loading…</p>";
  els.logOutput.innerHTML = "";
  els.detail.classList.remove("hidden");
  try {
    const d = await invoke("cmd_get_pod_details", { namespace, name });
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
  try {
    await invoke("cmd_stream_pod_logs", { namespace, name });
  } catch (e) {
    appendLog("Could not start log stream: " + e, "error-text");
  }
}

async function closeDetail() {
  if (!state.selectedPod) return;
  state.selectedPod = null;
  els.detail.classList.add("hidden");
  els.logOutput.innerHTML = "";
  try {
    await invoke("cmd_stop_pod_logs");
  } catch (e) {
    // Stream was already stopped; nothing to do.
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
els.logFollow.addEventListener("change", (e) => {
  state.logFollow = e.target.checked;
});
els.logClear.addEventListener("click", () => {
  els.logOutput.innerHTML = "";
});

(async () => {
  await listen("pod-log", (event) => appendLog(event.payload));
  await listen("pod-log-done", () => appendLog("[stream ended]", "muted"));
  await listen("pod-log-error", (event) => appendLog(event.payload, "error-text"));
  await loadContexts();
  await loadNamespaces();
  scheduleAutoRefresh();
  setView("overview");
})();
