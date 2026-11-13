const { invoke } = window.__TAURI__.core;

const state = {
  view: "pods",
  namespace: "",
  search: "",
  namespaces: [],
  timer: null,
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
  namespaceSelect: document.getElementById("namespace-select"),
  searchInput: document.getElementById("search-input"),
  autoRefresh: document.getElementById("auto-refresh"),
  error: document.getElementById("error"),
  head: document.getElementById("table-head"),
  body: document.getElementById("table-body"),
  empty: document.getElementById("empty"),
  count: document.getElementById("count"),
  updated: document.getElementById("updated"),
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
      return `<tr>${cells}</tr>`;
    })
    .join("");

  els.empty.classList.toggle("hidden", visible.length > 0);
  els.count.textContent = `${visible.length} of ${rows.length} items`;
  els.updated.textContent = "Updated " + new Date().toLocaleTimeString();
}

function escapeHtml(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
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

function scheduleAutoRefresh() {
  if (state.timer) clearInterval(state.timer);
  state.timer = null;
  if (els.autoRefresh.checked) {
    state.timer = setInterval(loadView, 30_000);
  }
}

function setView(view) {
  state.view = view;
  document.querySelectorAll(".nav-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.view === view);
  });
  els.namespaceSelect.style.visibility =
    view === "pods" ? "visible" : "hidden";
  loadView();
}

document.querySelectorAll(".nav-btn").forEach((btn) => {
  btn.addEventListener("click", () => setView(btn.dataset.view));
});
els.refreshBtn.addEventListener("click", loadView);
els.namespaceSelect.addEventListener("change", (e) => {
  state.namespace = e.target.value;
  loadView();
});
els.searchInput.addEventListener("input", (e) => {
  state.search = e.target.value;
  loadView();
});
els.autoRefresh.addEventListener("change", scheduleAutoRefresh);

(async () => {
  await loadNamespaces();
  scheduleAutoRefresh();
  loadView();
})();
