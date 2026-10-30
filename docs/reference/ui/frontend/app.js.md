# `ui/frontend/app.js`

[Reference index](../../README.md) · [Source](../../../../ui/frontend/app.js)

Plain JavaScript frontend using global Tauri `invoke`, event `listen`, and dialog `open`. State is held in `settings`, `paths`, `busy`, archive-viewer `view`, search `found`, and current `flow`. Backend strings are rendered through text nodes/textContent. `MAX_ROWS` limits displayed search/viewer rows to 500 while full results remain in memory. Functions below follow source order, including named local helpers.

## Functions

### `$`

[Source, line 7](../../../../ui/frontend/app.js#L7)

```javascript
const $ = (sel, root = document) => root.querySelector(sel);
```

Returns the first element matching `sel` in `root` (default document), or null. Callers generally expect the matching HTML element to exist.

### `$$`

[Source, line 8](../../../../ui/frontend/app.js#L8)

```javascript
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
```

Returns an array of all elements matching `sel` within the supplied root, defaulting to document.

### `el`

[Source, line 11](../../../../ui/frontend/app.js#L11)

```javascript
function el(tag, attrs = {}, ...kids)
```

Creates an element, registers `on*` attributes as event listeners, applies boolean/ordinary attributes, flattens children one level, and appends them. String children become text nodes, so backend content is not parsed as HTML. Returns the DOM node.

### `parseList`

[Source, line 22](../../../../ui/frontend/app.js#L22)

```javascript
const parseList = (text) => text.split(",").map((s) => s.trim()).filter(Boolean);
```

Splits comma-separated text, trims each item, and drops empty entries. Returns an array without deduplication.

### `fileName`

[Source, line 23](../../../../ui/frontend/app.js#L23)

```javascript
const fileName = (path) => path.split(/[\\/]/).pop();
```

Splits a Windows- or Unix-style path string on slash/backslash and returns its final component.

### `num`

[Source, line 24](../../../../ui/frontend/app.js#L24)

```javascript
const num = (n) => Number(n).toLocaleString();
```

Coerces the value to Number and returns its locale-formatted string. Invalid input formats as the locale's NaN representation.

### `fmtBytes`

[Source, line 26](../../../../ui/frontend/app.js#L26)

```javascript
function fmtBytes(n)
```

Returns a display string using 1,024-based B/KB/MB/GB/TB scaling. Scaled values use two decimals below ten and one otherwise; byte values are unscaled.

### `fmtDuration`

[Source, line 33](../../../../ui/frontend/app.js#L33)

```javascript
function fmtDuration(secs)
```

Formats durations below 60 seconds with one decimal; otherwise rounds to whole seconds and returns minutes/seconds or hours/minutes.

### `pad`

[Source, line 36](../../../../ui/frontend/app.js#L36)

```javascript
const pad = (x) => String(x).padStart(2, "0");
```

Local helper of `fmtDuration`: converts a value to a string padded to two characters with leading zero.

### `toast`

[Source, line 40](../../../../ui/frontend/app.js#L40)

```javascript
function toast(message, kind = "ok")
```

Appends an accessible status message to `#toasts` and schedules removal after 3.5 seconds, or eight seconds for errors. Returns no result.

### `log`

[Source, line 46](../../../../ui/frontend/app.js#L46)

```javascript
function log(message, cls = "")
```

Adds a timestamped line to the activity log, keeps at most 500 lines, and scrolls to the bottom. Empty messages are ignored.

### `buildFlow`

[Source, line 83](../../../../ui/frontend/app.js#L83)

```javascript
function buildFlow(area, key)
```

Builds nodes for the selected `FLOWS` definition in the supplied area, replaces previous steps, initializes running flow state, and paints it. Flow IDs must match emitted backend step names.

### `paintFlow`

[Source, line 97](../../../../ui/frontend/app.js#L97)

```javascript
function paintFlow()
```

Updates each flow node's pending/active/done/error classes, marker, sublabel, and optional progress bar. Caps bar width at 100%; assumes `flow` has been initialized.

### `run`

[Source, line 122](../../../../ui/frontend/app.js#L122)

```javascript
async function run(flowKey, tab, action)
```

Serializes a frontend action with `busy`, disables relevant buttons, builds its flow, and awaits `action()`. Returns its result on success; on rejection logs/toasts the error and returns undefined. Finally repaints, clears busy, and refreshes controls.

### `collectSettings`

[Source, line 145](../../../../ui/frontend/app.js#L145)

```javascript
function collectSettings()
```

Reads path/form state into a new backend settings object while preserving cached fields via object spread. Removes leading dots from extensions, trims ordinary text fields, and uses null for empty optional paths.

### `text`

[Source, line 146](../../../../ui/frontend/app.js#L146)

```javascript
const text = (id) => $(`#${id}`).value.trim();
```

Local helper of `collectSettings`: reads and trims the value of an input selected by ID.

### `saveSettings`

[Source, line 161](../../../../ui/frontend/app.js#L161)

```javascript
async function saveSettings(announce = false)
```

Collects settings and invokes `save_settings`, optionally showing a success toast. Returns a boolean; catches IPC failures and shows an error. Backend disk failures may still be hidden by its settings persistence implementation.

### `setPath`

[Source, line 173](../../../../ui/frontend/app.js#L173)

```javascript
function setPath(key, value)
```

Sets a chosen path or null, updates every matching `data-show` display, and refreshes action availability. Does not persist settings itself.

### `showLastScan`

[Source, line 179](../../../../ui/frontend/app.js#L179)

```javascript
function showLastScan()
```

Renders the cached query end date or `No previous scan` in the archive tab.

### `fillDb`

[Source, line 184](../../../../ui/frontend/app.js#L184)

```javascript
function fillDb(cfg, saved)
```

Populates all DB fields with backend values and sets the saved-status badge. Handles checkbox inputs as booleans and other fields as text/numbers.

### `readDb`

[Source, line 195](../../../../ui/frontend/app.js#L195)

```javascript
function readDb()
```

Collects DB fields into a config object, converting numeric fields and reading checkbox booleans. Restores the password's untrimmed value; does not validate numeric ranges or connectivity.

### `applyState`

[Source, line 205](../../../../ui/frontend/app.js#L205)

```javascript
function applyState(s)
```

Applies initial backend settings, selected paths, extension/dry-run/email controls, DB form, last-scan badge, and optional warning banner/log message. Mutates frontend state and DOM.

### `tiles`

[Source, line 221](../../../../ui/frontend/app.js#L221)

```javascript
const tiles = (items) => el("div", { class: "tiles" }, items.map(([value, label, cls = ""]) =>
```

Builds a DOM container of metric tiles from `[value,label,optionalClass]` tuples. Values are converted to text.

### `list`

[Source, line 224](../../../../ui/frontend/app.js#L224)

```javascript
const list = (items, total) => [
```

Returns a list element and an optional omitted-count hint based on `total`; callers choose how many items to pass.

### `card`

[Source, line 229](../../../../ui/frontend/app.js#L229)

```javascript
const card = (title, ...body) => el("div", { class: "card" }, el("h3", {}, title), ...body);
```

Builds a titled card element containing the supplied body nodes.

### `renderArchive`

[Source, line 231](../../../../ui/frontend/app.js#L231)

```javascript
function renderArchive(r)
```

Replaces archive results with dry-run counts/first 50 filenames or real-run counters, volume chips, archive breakdown, and up to 500 unmatched barcodes. `durabilityNotes` from the backend are not explicitly rendered here; degraded messages remain available in progress logs.

### `renderExtract`

[Source, line 255](../../../../ui/frontend/app.js#L255)

```javascript
function renderExtract()
```

Renders search count, optional extracted count, and the first 500 index locations with volume basename and shortened hash. Adds an omission hint when more rows exist.

### `renderEntries`

[Source, line 265](../../../../ui/frontend/app.js#L265)

```javascript
function renderEntries()
```

Filters loaded volume paths case-insensitively using the filter input, renders the first 500 matching rows with selection handlers, and updates total/shown counts. The complete loaded list remains in `view.entries`.

### `pick`

[Source, line 268](../../../../ui/frontend/app.js#L268)

```javascript
const pick = (path) => () => { view.selected = path; renderEntries(); refresh(); };
```

Local helper of `renderEntries`: returns a click callback capturing an archive path. The callback selects that path, rerenders entries, and refreshes button availability.

### `archive`

[Source, line 277](../../../../ui/frontend/app.js#L277)

```javascript
async function archive()
```

Reads dry-run/manual range controls, requires To when a manual range is enabled, saves settings, and invokes `run_session` through `run`. Renders successful results, updates the cached scan end for real runs, and shows completion. Manual From may be absent.

### `search`

[Source, line 294](../../../../ui/frontend/app.js#L294)

```javascript
async function search()
```

Invokes backend `search` using the trimmed query through a search flow. Replaces found rows, clears prior extraction count, and rerenders even for an empty result array.

### `extract`

[Source, line 299](../../../../ui/frontend/app.js#L299)

```javascript
async function extract()
```

Invokes backend `extract` with query and chosen destination. On success updates extracted count, renders search results, and shows completion; zero extracted is still success.

### `loadVolume`

[Source, line 304](../../../../ui/frontend/app.js#L304)

```javascript
async function loadVolume()
```

Invokes `list_volume` for the selected file, replaces loaded volume/entry state, clears selection, hides preview, and refreshes entry rendering/buttons.

### `preview`

[Source, line 314](../../../../ui/frontend/app.js#L314)

```javascript
async function preview()
```

Invokes `preview_entry` for the selected loaded entry, displays returned text/name via textContent, and toggles the truncation note and preview card.

### `extractFile`

[Source, line 324](../../../../ui/frontend/app.js#L324)

```javascript
async function extractFile()
```

Invokes `extract_entry` for the selected loaded entry and destination, then shows the returned destination in a toast.

### `extractAll`

[Source, line 330](../../../../ui/frontend/app.js#L330)

```javascript
async function extractAll()
```

Invokes `extract_all` for the loaded archive and destination, then shows the extracted count, including zero.

### `saveDb`

[Source, line 335](../../../../ui/frontend/app.js#L335)

```javascript
async function saveDb()
```

Invokes encrypted-config saving with the current DB form, marks the badge saved, and shows success or catches/reports failure. Does not test the connection.

### `clearDb`

[Source, line 343](../../../../ui/frontend/app.js#L343)

```javascript
async function clearDb()
```

Invokes config clearing, reloads backend state, and resets only the DB form/badge from defaults. Catches and displays errors.

### `togglePassword`

[Source, line 352](../../../../ui/frontend/app.js#L352)

```javascript
function togglePassword(btn)
```

Toggles the password input between password/text and updates the invoking button's Show/Hide label.

### `pickPath`

[Source, line 365](../../../../ui/frontend/app.js#L365)

```javascript
async function pickPath(btn)
```

Opens a file dialog for typed MOCHI/SQLite targets or a directory dialog otherwise. Cancellation leaves state unchanged; selected text-input targets update their value while path displays use `setPath`.

### `refresh`

[Source, line 376](../../../../ui/frontend/app.js#L376)

```javascript
function refresh()
```

Sets Start text for dry-run mode and enables action buttons from required path/query/selection state, disabling those actions while busy. Settings and navigation controls are not part of this readiness map.

### `showTab`

[Source, line 391](../../../../ui/frontend/app.js#L391)

```javascript
function showTab(name)
```

Marks the chosen navigation item active and hides all other tab sections. Does not cancel a running task.

### `target`

[Source, line 397](../../../../ui/frontend/app.js#L397)

```javascript
const target = (attr) => e.target.closest(`[${attr}]`);
```

Local helper in the delegated document-click handler: finds the closest clicked ancestor bearing a requested data attribute.


## Event callbacks and startup

- **`listen("progress", ...)`** logs each payload. While a flow is running, it selects a recognized step, updates message/counters, and repaints; otherwise it only logs.
- **Document click callback** resolves `data-tab`, then `data-pick`, then enabled `data-do` ancestors, routing navigation, dialog selection, or `ACTIONS`. This callback does not await promises returned by those actions.
- **Document input callback** invokes `refresh` for readiness updates. The filter input additionally invokes `renderEntries`.
- **Dry-run change callback** saves the checkbox preference through `saveSettings`.
- **Startup async callback** invokes `load_state`, applies it or displays its error, then renders entries and refreshes controls.
- **`ACTIONS.saveSettings` callback** calls `saveSettings(true)` to request a visible saved toast.
- **Toast timeout callback** removes its toast after the configured display time. Per-row event callbacks are produced by `pick`.

Other inline mapping/filtering callbacks transform data locally inside the documented enclosing function. They do not define separate named APIs.

## IPC and data naming

`invoke` arguments use camelCase for Tauri command argument names such as `volumePath`, `relativePath`, and `destDir`. `RunRequest` and archive/search result fields are camelCase. The nested `UiSettings` and `DbConfig` objects retain snake_case fields. Frontend timeout/backoff form values are numeric milliseconds.

The UI's flow sublabel says “Copy to staging”; the real backend uses filesystem rename. A successful flow does not imply the archive is an independent backup, and degraded directory durability is delivered through log messages.
