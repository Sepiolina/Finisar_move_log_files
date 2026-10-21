"use strict";
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;

// ---------- Helpers ----------
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

/** Tiny element builder. Children are appended as text nodes, so backend strings are never parsed as HTML. */
function el(tag, attrs = {}, ...kids) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v === true) node.setAttribute(k, "");
    else if (v !== false && v != null) node.setAttribute(k, v);
  }
  node.append(...kids.flat().filter((k) => k != null));
  return node;
}

const parseList = (text) => text.split(",").map((s) => s.trim()).filter(Boolean);
const fileName = (path) => path.split(/[\\/]/).pop();
const num = (n) => Number(n).toLocaleString();

function fmtBytes(n) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${i ? n.toFixed(n < 10 ? 2 : 1) : n} ${units[i]}`;
}

function fmtDuration(secs) {
  const t = Math.round(secs), h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), s = t % 60;
  if (secs < 60) return `${secs.toFixed(1)} s`;
  const pad = (x) => String(x).padStart(2, "0");
  return h ? `${h}h ${pad(m)}m` : `${m}m ${pad(s)}s`;
}

function toast(message, kind = "ok") {
  const t = el("div", { class: `toast ${kind === "error" ? "error" : ""}`, role: "status" }, message);
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "error" ? 8000 : 3500);
}

function log(message, cls = "") {
  if (!message) return;
  const box = $("#log");
  box.append(el("div", { class: cls }, el("time", {}, new Date().toLocaleTimeString([], { hour12: false })), message));
  while (box.childElementCount > 500) box.firstChild.remove();
  box.scrollTop = box.scrollHeight;
}

// ---------- State ----------
let settings = {};              // last settings object known to the backend
let busy = false;
const paths = {};               // picked paths: source_dir, output_dir, destExtract, destViewer, volume
const view = { volume: null, entries: [], selected: null };
const found = { rows: [], extracted: null };

const DB_KEYS = ["server", "port", "database", "username", "password", "encrypt", "trust_server_certificate", "request_timeout", "retry_attempts", "retry_base_delay"];
const DB_NUMBERS = new Set(["port", "request_timeout", "retry_attempts", "retry_base_delay"]);
const MAX_ROWS = 500;

// ---------- Execution flow ----------
// Each step: [id, label, sub-label]. Ids are emitted by the backend in "progress" events.
const OPEN = ["index", "Open index", "Local SQLite"];
const QUERY = ["query", "Query SQL Server", "Barcode records"];
const SCAN = ["scan", "Scan folders", "Find log files"];
const VOL_OPEN = ["open", "Open archive", "MOCHI volume"];
const FLOWS = {
  archive: [OPEN, QUERY, SCAN, ["stage", "Stage files", "Copy to staging"], ["archive", "Commit to MOCHI", "Write volumes"], ["cleanup", "Clean staging", "Remove temp copies"], ["report", "Report", "Summary"]],
  dryrun: [OPEN, QUERY, SCAN, ["report", "Summary", "What would move"]],
  search: [OPEN, ["search", "Search index", "Match query"]],
  extract: [OPEN, ["search", "Find files", "Match query"], ["extract", "Extract files", "Write to folder"]],
  volume: [VOL_OPEN, ["read", "Read entries", "List contents"]],
  preview: [VOL_OPEN, ["read", "Read file", "Decode text"]],
  volumeExtract: [VOL_OPEN, ["extract", "Extract", "Write to folder"]],
};

let flow = null; // the flow currently shown/running: { ids, nodes, active, status, msg, done, total }

function buildFlow(area, key) {
  const steps = FLOWS[key];
  const nodes = steps.map(([, label, sub]) => {
    const dot = el("div", { class: "dot" }), subEl = el("div", { class: "sub" }, sub);
    const fill = el("i"), bar = el("div", { class: "bar", hidden: true }, fill);
    return { root: el("div", { class: "step" }, dot, el("div", { class: "name" }, label), subEl, bar), dot, subEl, bar, fill, sub };
  });
  const box = $(".steps", area);
  box.className = "steps";
  box.replaceChildren(...nodes.map((n) => n.root));
  flow = { ids: steps.map((s) => s[0]), nodes, active: 0, status: "running", msg: "", done: 0, total: 0 };
  paintFlow();
}

function paintFlow() {
  const f = flow;
  f.nodes.forEach((n, i) => {
    let state = "pending";
    if (f.status === "ok" || i < f.active) state = "done";
    else if (i === f.active) state = f.status === "error" ? "error" : "active";
    n.root.className = `step ${state === "pending" ? "" : state}`;
    n.dot.textContent = state === "done" ? "✓" : state === "error" ? "✕" : i + 1;
    n.subEl.textContent = (state === "active" || state === "error") && f.msg ? f.msg : n.sub;
    const showBar = state === "active" && f.total > 0;
    n.bar.hidden = !showBar;
    if (showBar) n.fill.style.width = `${Math.min(100, (f.done / f.total) * 100)}%`;
  });
}

listen("progress", ({ payload: p }) => {
  log(p.message);
  if (!flow || flow.status !== "running") return;
  const i = p.step ? flow.ids.indexOf(p.step) : -1;
  if (i >= 0) flow.active = i;
  Object.assign(flow, { msg: p.message, done: p.done, total: p.total });
  paintFlow();
});

/** Run one backend action with a flow diagram. Returns the result, or undefined on failure. */
async function run(flowKey, tab, action) {
  if (busy) return toast("Another task is already running.", "error");
  busy = true;
  refresh();
  buildFlow($(`#flow-${tab}`), flowKey);
  try {
    const result = await action();
    flow.status = "ok";
    flow.msg = "";
    return result;
  } catch (e) {
    flow.status = "error";
    flow.msg = String(e);
    log(String(e), "err");
    toast(String(e), "error");
  } finally {
    paintFlow();
    busy = false;
    refresh();
  }
}

// ---------- Settings & state ----------
function collectSettings() {
  const text = (id) => $(`#${id}`).value.trim();
  return {
    ...settings,
    source_dir: paths.source_dir ?? null,
    output_dir: paths.output_dir ?? null,
    staging_dir: text("staging_dir") || null,
    db_path: text("db_path") || null,
    extension_filter: parseList($("#extensions").value).map((x) => x.replace(/^\.+/, "")),
    dry_run: $("#dryRun").checked,
    email_stats_enabled: $("#email_stats_enabled").checked,
    email_api_base_url: text("email_api_base_url"),
    email_recipient: text("email_recipient"),
  };
}

async function saveSettings(announce = false) {
  try {
    settings = collectSettings();
    await invoke("save_settings", { settings });
    if (announce) toast("Settings saved");
    return true;
  } catch (e) {
    toast(String(e), "error");
    return false;
  }
}

function setPath(key, value) {
  paths[key] = value || null;
  $$(`[data-show="${key}"]`).forEach((n) => (n.textContent = value || ""));
  refresh();
}

function showLastScan() {
  const d = settings.last_successful_scan_to_date;
  $("#lastScan").textContent = d ? `Last successful scan: ${d}` : "No previous scan";
}

function fillDb(cfg, saved) {
  for (const k of DB_KEYS) {
    const input = $(`#db_${k}`);
    if (input.type === "checkbox") input.checked = !!cfg[k];
    else input.value = cfg[k] ?? "";
  }
  const badge = $("#dbBadge");
  badge.textContent = saved ? "Saved" : "Not saved";
  badge.classList.toggle("ok", saved);
}

function readDb() {
  const cfg = {};
  for (const k of DB_KEYS) {
    const input = $(`#db_${k}`);
    cfg[k] = input.type === "checkbox" ? input.checked : DB_NUMBERS.has(k) ? Number(input.value) : input.value.trim();
  }
  cfg.password = $("#db_password").value; // never trim passwords
  return cfg;
}

function applyState(s) {
  settings = s.settings;
  setPath("source_dir", settings.source_dir);
  setPath("output_dir", settings.output_dir);
  for (const id of ["staging_dir", "db_path", "email_api_base_url", "email_recipient"]) $(`#${id}`).value = settings[id] ?? "";
  $("#email_stats_enabled").checked = !!settings.email_stats_enabled;
  $("#extensions").value = (settings.extension_filter || []).join(", ");
  $("#dryRun").checked = !!settings.dry_run;
  showLastScan();
  fillDb(s.dbConfig, s.dbConfigSaved);
  $("#notice").hidden = !s.notice;
  $("#notice").textContent = s.notice || "";
  if (s.notice) log(s.notice);
}

// ---------- Result rendering ----------
const tiles = (items) => el("div", { class: "tiles" }, items.map(([value, label, cls = ""]) =>
  el("div", { class: `tile ${cls}` }, el("b", {}, String(value)), el("span", {}, label))));

const list = (items, total) => [
  el("ul", { class: "list" }, items.map((x) => el("li", {}, x))),
  total > items.length ? el("p", { class: "hint" }, `...and ${num(total - items.length)} more`) : null,
];

const card = (title, ...body) => el("div", { class: "card" }, el("h3", {}, title), ...body);

function renderArchive(r) {
  const out = $("#archiveResult");
  if (r.kind === "dryrun") {
    out.replaceChildren(el("div", { class: "result" },
      tiles([[num(r.barcodesMatched), "Barcodes matched"], [num(r.filesThatWouldMove), "Files that would move", "good"]]),
      card("Files that would move (first 50)", list(r.matchedFilenames.slice(0, 50), r.matchedFilenames.length))));
    return;
  }
  const errors = Array.isArray(r.errors) ? r.errors.length : r.errors;
  const rows = r.archiveBreakdown.map((b) => el("tr", {},
    el("td", { class: "path" }, b.volumeName), el("td", { class: "num" }, num(b.filesArchived)), el("td", { class: "num" }, fmtBytes(b.totalBytes))));
  out.replaceChildren(el("div", { class: "result" },
    tiles([
      [num(r.filesMoved), "Files archived", "good"], [num(r.barcodesMatched), "Barcodes matched"],
      [num(r.totalFilesScanned), "Files scanned"], [num(errors), "Errors", errors > 0 ? "bad" : ""],
      [num(r.dbRecordsRetrieved), "DB records"], [num(r.filesDeleted), "Staging files cleaned"],
      [fmtDuration(r.durationSecs), "Duration"],
    ]),
    r.volumeNames.length ? card("Volumes", el("div", { class: "chips" }, r.volumeNames.map((v) => el("span", { class: "chip" }, v)))) : null,
    rows.length ? card("Archive breakdown", el("table", {},
      el("thead", {}, el("tr", {}, el("th", {}, "Volume"), el("th", {}, "Files"), el("th", {}, "Size"))), el("tbody", {}, rows))) : null,
    r.unmatchedBarcodes.length ? el("details", { class: "card" },
      el("summary", {}, `Unmatched barcodes (${num(r.unmatchedBarcodes.length)})`), list(r.unmatchedBarcodes.slice(0, 500), r.unmatchedBarcodes.length)) : null));
}

function renderExtract() {
  const items = [[num(found.rows.length), "Matches"]];
  if (found.extracted != null) items.push([num(found.extracted), "Files extracted", "good"]);
  const rows = found.rows.slice(0, MAX_ROWS).map((r) => el("tr", {},
    el("td", { class: "path" }, r.relativePath), el("td", { class: "path" }, fileName(r.volumePath)), el("td", { class: "path" }, r.contentHash?.slice(0, 12) ?? "—")));
  $("#extractResult").replaceChildren(el("div", { class: "result" }, tiles(items),
    rows.length ? card("Matches", el("table", {}, el("thead", {}, el("tr", {}, el("th", {}, "Path"), el("th", {}, "Volume"), el("th", {}, "Hash"))), el("tbody", {}, rows)),
      found.rows.length > MAX_ROWS ? el("p", { class: "hint" }, `Showing first ${MAX_ROWS} of ${num(found.rows.length)}`) : null) : null));
}

function renderEntries() {
  const q = $("#filter").value.toLowerCase();
  const shown = view.entries.filter((e) => e.relativePath.toLowerCase().includes(q));
  const pick = (path) => () => { view.selected = path; renderEntries(); refresh(); };
  $("#entries").replaceChildren(...(shown.length
    ? shown.slice(0, MAX_ROWS).map((e) => el("div", { class: `entry ${e.relativePath === view.selected ? "selected" : ""}`, onclick: pick(e.relativePath) },
      el("code", {}, e.relativePath), el("span", {}, fmtBytes(e.size))))
    : [el("p", { class: "hint", style: "padding:0 10px" }, view.volume ? "No matching entries." : "Load an archive to list its contents.")]));
  $("#entryCount").textContent = view.volume ? `${num(shown.length)} of ${num(view.entries.length)} entries${shown.length > MAX_ROWS ? ` (showing first ${MAX_ROWS})` : ""}` : "";
}

// ---------- Actions ----------
async function archive() {
  const dryRun = $("#dryRun").checked, useRange = $("#useRange").checked;
  const dateFrom = useRange ? $("#dateFrom").value || null : null;
  const dateTo = useRange ? $("#dateTo").value || null : null;
  if (useRange && !dateTo) return toast("Pick a 'To' date for the custom range.", "error");
  if (!(await saveSettings())) return; // backend uses the saved dirs / db path / email settings
  const req = { subfolders: parseList($("#subfolders").value), extensions: settings.extension_filter, dryRun, dateFrom, dateTo };
  const result = await run(dryRun ? "dryrun" : "archive", "archive", () => invoke("run_session", { req }));
  if (!result) return;
  renderArchive(result);
  if (result.kind === "archive" && result.scanToDate) {
    settings.last_successful_scan_to_date = result.scanToDate;
    showLastScan();
  }
  toast(dryRun ? "Dry run finished" : "Archive finished");
}

async function search() {
  const rows = await run("search", "extract", () => invoke("search", { query: $("#query").value.trim() }));
  if (rows) { Object.assign(found, { rows, extracted: null }); renderExtract(); }
}

async function extract() {
  const count = await run("extract", "extract", () => invoke("extract", { query: $("#query").value.trim(), destDir: paths.destExtract }));
  if (count != null) { found.extracted = count; renderExtract(); toast(`Extracted ${num(count)} file(s)`); }
}

async function loadVolume() {
  const volumePath = paths.volume;
  const entries = await run("volume", "viewer", () => invoke("list_volume", { volumePath }));
  if (!entries) return;
  Object.assign(view, { volume: volumePath, entries, selected: null });
  $("#previewCard").hidden = true;
  renderEntries();
  refresh();
}

async function preview() {
  const relativePath = view.selected;
  const res = await run("preview", "viewer", () => invoke("preview_entry", { volumePath: view.volume, relativePath }));
  if (!res) return;
  $("#previewName").textContent = relativePath;
  $("#preview").textContent = res.text;
  $("#truncNote").hidden = !res.truncated;
  $("#previewCard").hidden = false;
}

async function extractFile() {
  const written = await run("volumeExtract", "viewer", () =>
    invoke("extract_entry", { volumePath: view.volume, relativePath: view.selected, destDir: paths.destViewer }));
  if (written) toast(`Extracted to ${written}`);
}

async function extractAll() {
  const count = await run("volumeExtract", "viewer", () => invoke("extract_all", { volumePath: view.volume, destDir: paths.destViewer }));
  if (count != null) toast(`Extracted ${num(count)} file(s)`);
}

async function saveDb() {
  try {
    await invoke("save_db_config", { config: readDb() });
    fillDb(readDb(), true);
    toast("Connection saved");
  } catch (e) { toast(String(e), "error"); }
}

async function clearDb() {
  try {
    await invoke("clear_db_config");
    const s = await invoke("load_state"); // refill the form with the backend defaults
    fillDb(s.dbConfig, s.dbConfigSaved);
    toast("Connection cleared");
  } catch (e) { toast(String(e), "error"); }
}

function togglePassword(btn) {
  const input = $("#db_password");
  input.type = input.type === "password" ? "text" : "password";
  btn.textContent = input.type === "password" ? "Show" : "Hide";
}

const ACTIONS = { archive, search, extract, load: loadVolume, preview, extractFile, extractAll, saveDb, clearDb, togglePw: togglePassword, saveSettings: () => saveSettings(true) };

const FILTERS = {
  mochi: [{ name: "MOCHI archive", extensions: ["mochi"] }],
  sqlite: [{ name: "SQLite index", extensions: ["sqlite", "db"] }],
};

async function pickPath(btn) {
  const key = btn.dataset.pick;
  const chosen = await open(btn.dataset.kind ? { filters: FILTERS[btn.dataset.kind] } : { directory: true });
  if (!chosen) return;
  const input = $(`#${key}`); // text-input targets (index DB) vs. path displays
  if (input) input.value = chosen;
  else setPath(key, chosen);
}

// ---------- UI wiring ----------
/** Enable/disable buttons from current state. */
function refresh() {
  $("#startBtn").textContent = $("#dryRun").checked ? "Start dry run" : "Start archive";
  const query = $("#query").value.trim();
  const ready = {
    startBtn: paths.source_dir && paths.output_dir,
    searchBtn: query,
    extractBtn: query && paths.destExtract,
    loadBtn: paths.volume,
    previewBtn: view.volume && view.selected,
    extractFileBtn: view.volume && view.selected && paths.destViewer,
    extractAllBtn: view.volume && paths.destViewer,
  };
  for (const [id, ok] of Object.entries(ready)) $(`#${id}`).disabled = busy || !ok;
}

function showTab(name) {
  $$(".nav-item").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  $$(".tab").forEach((t) => (t.hidden = t.id !== `tab-${name}`));
}

document.addEventListener("click", (e) => {
  const target = (attr) => e.target.closest(`[${attr}]`);
  if (target("data-tab")) showTab(target("data-tab").dataset.tab);
  else if (target("data-pick")) pickPath(target("data-pick"));
  else if (target("data-do") && !target("data-do").disabled) ACTIONS[target("data-do").dataset.do](target("data-do"));
});
document.addEventListener("input", refresh);
$("#filter").addEventListener("input", renderEntries);
$("#dryRun").addEventListener("change", () => saveSettings());

(async () => {
  try { applyState(await invoke("load_state")); }
  catch (e) { toast(String(e), "error"); }
  renderEntries();
  refresh();
})();
