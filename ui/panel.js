// One TC file panel: drive bar, path row, current-path bar, virtual list, status line.

import {
  filterMatch,
  pushHistory,
  quickSearchFind,
  remoteArchiveOf,
  archiveKindOf,
  archiveLeave,
  displayLocation,
  compileTypeRules,
  connectionLabel,
  displayName,
  formatDate,
  formatMode,
  formatSize,
  fullyVisibleRows,
  freeLine,
  groupThousands,
  iconKind,
  isSearchLocation,
  joinLocation,
  lastComponent,
  markSummary,
  maskSelects,
  navigate,
  parentLocation,
  parseRemote,
  remoteArchives,
  remoteRoot,
  sideJump,
  refreshTarget,
  NOT_MOUNTED,
  resultFolder,
  rowStyle,
  visibleRange,
  iconKey,
} from "./lib.mjs";

export const VOLUME_ICON = { root: "💻", home: "🏠", volume: "💾", network: "🌐", server: "☁️" };
const UP = Object.freeze({ name: "..", isDir: true, up: true, size: 0, hidden: false });

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Finder icons by `iconKey`: a data URL, or "" when macOS gave none (emoji then). */
const iconUrls = new Map();
const iconAsked = new Set();
const panels = new Set();

/** Asks for the icons of `keys` once; every panel redraws when they arrive. */
function fetchIcons(keys, api) {
  const missing = keys.filter((k) => !iconAsked.has(k));
  if (!missing.length) return;
  for (const k of missing) iconAsked.add(k);
  api
    .icons(missing)
    .then((got) => {
      for (const k of missing) iconUrls.set(k, got?.[k] ?? "");
      for (const p of panels) {
        p.rowsDirty = true;
        p.renderRows();
      }
    })
    .catch((e) => {
      console.error("fxfm: file_icons", e);
      for (const k of missing) iconUrls.set(k, "");
    });
}

function rowIcon(e) {
  if (e.up) return "⬆";
  if (e.isDir) return "📁";
  const kind = iconKind(e);
  if (kind === "arc") return "🗜";
  if (kind === "med") return "🎬";
  if (/\.pdf$/i.test(e.name)) return "📕";
  return "📄";
}

/** The volume containing `location` (longest path prefix), or null (also on a server and for search results). */
export function volumeFor(volumes, location) {
  if (parseRemote(location) || isSearchLocation(location)) return null;
  let best = null;
  for (const v of volumes) {
    const p = v.path;
    const inside = p === "/" || location === p || location.startsWith(`${p}/`);
    if (inside && (!best || p.length > best.path.length)) best = v;
  }
  return best;
}

/**
 * api: {
 *   listDir(loc) -> Promise<Entry[]>, nearest(loc) -> Promise<string>,
 *   dirSize(loc, side) -> Promise<[bytes, files]>,
 *   openDefault(path, onFolder?) (onFolder: run instead of the error when a server link's
 *   target is a folder),
 *   showHidden() -> bool, onActivate(panel), onNavigated(panel),
 *   onError(panel, error, target), onDriveMenu(panel), onDrive(panel, path) (a drive or
 *   connection button), onCursorMoved(panel),
 *   onRowPress(panel, index, mouseEvent) (left button on a row: a drag may follow),
 *   results(panel) -> {entries, query, back: {location, cursorName}} | null (the search
 *   results this panel shows at SEARCH_LOCATION; `listDir` lists them)
 * }
 */
export class Panel {
  constructor(root, side, api) {
    this.side = side;
    this.api = api;
    this.location = "";
    this.raw = [];
    this.entries = [];
    this.cursorIndex = 0;
    this.marked = new Set();
    this.dirSizes = new Map();
    this.volumes = [];
    /** Open server connections from `list_connections`: [{name, display, root}]. */
    this.connections = [];
    this.active = false;
    this.message = null;
    this.rowH = 25;
    this.iconStyle = "finder";
    panels.add(this);
    this.navToken = 0;
    this.pendingTarget = null;
    this.sizeQueue = [];
    this.sizing = false;
    this.renderQueued = false;
    this.rowsDirty = true;
    this.rendered = { start: 0, end: 0 };
    /** Row highlighted as a drop target (-1: none). */
    this.dropRow = -1;
    /** Settings colors and their compiled file-type matcher (see `setColors`). */
    this.colors = null;
    this.typeMatcher = compileTypeRules([]);
    /** Quick filter text (Ctrl+S); "" shows everything. Cleared when the folder changes. */
    this.filter = "";
    /** Folder history: places left before ([{location, cursorName}], newest last) and after. */
    this.back = [];
    this.forward = [];
    this.build(root);
  }

  build(root) {
    this.root = root;
    root.classList.add("pane");
    root.replaceChildren();
    this.drivesEl = el("div", "drives");
    const pathRow = el("div", "pathrow");
    this.comboEl = el("span", "combo");
    this.freeEl = el("span", "free");
    const rootBtn = el("span", "tool", "/");
    const upBtn = el("span", "tool", "..");
    pathRow.append(this.comboEl, this.freeEl, rootBtn, upBtn);
    this.curEl = el("div", "cur");
    this.listEl = el("div", "list");
    this.spacerEl = el("div", "spacer");
    this.rowsEl = el("div", "rows");
    this.listEl.append(this.spacerEl, this.rowsEl);
    this.statusEl = el("div", "status");
    this.stName = el("span", "n");
    this.stSize = el("span", "s");
    this.stDate = el("span", "d");
    this.stMode = el("span", "m");
    this.statusEl.append(this.stName, this.stSize, this.stDate, this.stMode);
    // Quick search box (TC Alt+letters): shown over the status line while typing.
    this.qsEl = el("div", "quicksearch");
    this.qsEl.hidden = true;
    this.statusEl.append(this.qsEl);
    // Quick filter box (TC Ctrl+S): shown above the status line while a filter is set.
    this.filterBox = el("div", "qfilter");
    this.filterBox.hidden = true;
    this.filterInput = el("input");
    this.filterInput.type = "text";
    this.filterInput.spellcheck = false;
    this.filterCount = el("span", "count");
    this.filterBox.append(el("span", null, "Filter:"), this.filterInput, this.filterCount);
    this.statusEl.append(this.filterBox);
    this.filterInput.addEventListener("input", () => this.setFilter(this.filterInput.value));
    this.filterInput.addEventListener("keydown", (e) => this.filterKey(e));
    this.filterInput.addEventListener("blur", () => {
      if (!this.filter) this.filterBox.hidden = true;
    });
    root.append(this.drivesEl, pathRow, this.curEl, this.listEl, this.statusEl);

    root.addEventListener("mousedown", () => this.focus());
    this.comboEl.addEventListener("click", () => this.api.onDriveMenu(this));
    rootBtn.addEventListener("click", () => this.goRoot());
    upBtn.addEventListener("click", () => this.goParent());
    this.drivesEl.addEventListener("click", (ev) => {
      const b = ev.target.closest("[data-path]");
      if (b) this.api.onDrive(this, b.dataset.path);
    });
    this.rowsEl.addEventListener("mousedown", (ev) => {
      const r = ev.target.closest("[data-i]");
      if (!r) return;
      const i = Number(r.dataset.i);
      if (ev.button === 0) this.api.onRowPress(this, i, ev);
      this.setCursor(i);
    });
    // Right click: the row under the pointer (the mousedown above put the cursor on it), or
    // empty space below the rows.
    this.listEl.addEventListener("contextmenu", (ev) => {
      ev.preventDefault();
      const r = ev.target.closest("[data-i]");
      this.api.onContextMenu(this, r ? Number(r.dataset.i) : null, ev.clientX, ev.clientY);
    });
    this.rowsEl.addEventListener("dblclick", (ev) => {
      const r = ev.target.closest("[data-i]");
      if (r) this.enter();
    });
    this.listEl.addEventListener("scroll", () => this.queueRender());
    new ResizeObserver(() => this.queueRender()).observe(this.listEl);
  }

  // ---- state ----

  get cursorEntry() {
    return this.entries[this.cursorIndex] ?? null;
  }

  get cursorName() {
    const e = this.cursorEntry;
    return e && !e.up ? e.name : null;
  }

  /**
   * Lists `location`; on success it becomes the panel's folder. Returns navigate()'s result.
   * A later go() supersedes an earlier one; refresh() never supersedes a user navigation.
   */
  async go(location, cursorName, { refresh = false, history = true } = {}) {
    if (!refresh) this.quickSearchEnd();
    const from = this.location ? { location: this.location, cursorName: this.cursorName } : null;
    const token = ++this.navToken;
    if (!refresh) this.pendingTarget = location;
    const r = await navigate(this, location, this.api.listDir, {
      cursorName,
      nearest: this.api.nearest,
    });
    if (token !== this.navToken) return r;
    this.pendingTarget = null;
    if (!r.ok) {
      this.api.onError(this, r.error, location);
      return r;
    }
    const moved = r.location !== this.location;
    this.location = r.location;
    this.raw = r.entries;
    if (moved) {
      // A new place for Back (not a results list, not when Back/Forward itself moved).
      if (history && from && !isSearchLocation(from.location)) {
        this.back = pushHistory(this.back, from);
        this.forward = [];
      }
      this.filter = "";
      this.filterBox.hidden = true;
      this.marked.clear();
      this.dirSizes.clear();
      this.sizeQueue = [];
      this.listEl.scrollTop = 0;
    }
    this.message = null;
    this.rebuild(r.cursorName, moved ? 0 : this.cursorIndex);
    this.api.onNavigated(this);
    return r;
  }

  /** Re-lists the current folder, keeping cursor and marks (no-op while a user navigation is pending). */
  async refresh() {
    const target = refreshTarget(this);
    if (target === null) return null;
    return this.go(target, this.cursorName ?? undefined, { refresh: true });
  }

  /**
   * Rebuilds the rows (hidden filter, "[..]") and puts the cursor on
   * `cursorName`, or at `fallbackIndex` when that name is not listed.
   */
  rebuild(cursorName = this.cursorName, fallbackIndex = this.cursorIndex) {
    const showHidden = this.api.showHidden();
    const visible = showHidden ? this.raw : this.raw.filter((e) => !e.hidden);
    // Filtered-out items also lose their marks: no command acts on what cannot be seen.
    const shown = this.filter ? visible.filter((e) => filterMatch(e.name, this.filter)) : visible;
    this.filterCount.textContent = this.filter ? `${shown.length} of ${visible.length}` : "";
    // No "[..]" where there is no parent: "/" and a server root. The results list has one (back).
    const noParent = this.location === "/" || parentLocation(this.location) === null;
    this.entries = noParent && !isSearchLocation(this.location) ? shown : [UP, ...shown];
    const names = new Set(shown.map((e) => e.name));
    for (const n of this.marked) if (!names.has(n)) this.marked.delete(n);
    let i = cursorName == null ? -1 : this.entries.findIndex((e) => !e.up && e.name === cursorName);
    if (i < 0) i = Math.min(fallbackIndex, this.entries.length - 1);
    this.cursorIndex = Math.max(0, i);
    this.rowsDirty = true;
    this.spacerEl.style.height = `${this.entries.length * this.rowH}px`;
    this.ensureVisible();
    this.render();
  }

  clearMarks() {
    this.marked.clear();
    this.render();
  }

  setVolumes(volumes) {
    this.volumes = volumes;
    this.renderHeader();
  }

  setConnections(connections) {
    this.connections = connections;
    this.renderHeader();
  }

  setActive(on) {
    if (!on) this.quickSearchEnd();
    this.active = on;
    this.root.classList.toggle("active", on);
    this.curEl.classList.toggle("active", on);
    // The cursor row is drawn differently in the active and the inactive panel.
    this.renderRows();
  }

  /** New panel colors; `matcher` is `compileTypeRules(colors.typeRules)`, compiled once by the caller. */
  setColors(colors, matcher) {
    this.colors = colors;
    this.typeMatcher = matcher;
    this.renderRows();
  }

  /** Options → View → Icons: "finder" or "emoji". */
  setIconStyle(style) {
    if (style === this.iconStyle) return;
    this.iconStyle = style;
    this.rowsDirty = true;
    this.renderRows();
  }

  /** The row's icon: Finder's (once it has arrived; `wanted` collects keys to ask for) or emoji. */
  iconEl(e, wanted) {
    const i = el("i");
    const key = this.iconStyle === "finder" ? iconKey(e, this.location) : null;
    const url = key && iconUrls.get(key);
    if (url) {
      const img = document.createElement("img");
      img.src = url;
      img.alt = "";
      i.append(img);
    } else if (key && url === undefined) {
      wanted.add(key);
    } else {
      i.textContent = rowIcon(e);
    }
    return i;
  }

  setRowHeight(px) {
    this.rowH = px;
    this.rowsDirty = true;
    this.render();
  }

  /** Drop feedback: border around the panel and, with `row` >= 0, that folder row. */
  setDropHighlight(on, row = -1) {
    this.root.classList.toggle("droptarget", on);
    const r = on ? row : -1;
    if (r === this.dropRow) return;
    this.dropRow = r;
    this.renderRows();
  }

  /** Row index of the element `el` (a row or inside one), or -1. */
  rowIndexOf(el) {
    const r = el?.closest?.("[data-i]");
    return r && this.rowsEl.contains(r) ? Number(r.dataset.i) : -1;
  }

  setMessage(text) {
    this.message = text;
    this.renderStatus();
  }

  focus() {
    this.api.onActivate(this);
  }

  // ---- navigation ----

  setCursor(i) {
    if (!this.entries.length) return;
    this.cursorIndex = Math.max(0, Math.min(this.entries.length - 1, i));
    this.message = null;
    this.api.onCursorMoved(this);
    this.ensureVisible();
    this.renderRows();
    this.renderStatus();
  }

  /** First and last row fully visible in the list. */
  visibleRows() {
    const l = this.listEl;
    return fullyVisibleRows(l.scrollTop, this.rowH, l.clientHeight, this.entries.length);
  }

  pageRows() {
    return Math.max(1, Math.floor(this.listEl.clientHeight / this.rowH) - 1);
  }

  /** The search results this panel shows, or null (not a results panel). */
  get results() {
    return isSearchLocation(this.location) ? (this.api.results?.(this) ?? null) : null;
  }

  /** Backspace / [..]: nothing at "/" or at a server root; the results list goes back where it came from. */
  goParent() {
    if (isSearchLocation(this.location)) {
      const back = this.results?.back;
      return back ? this.go(back.location, back.cursorName ?? undefined) : undefined;
    }
    // The root of a server archive's copy goes back to the server folder, not the temp folder.
    const left = archiveLeave(this.location, remoteArchives);
    if (left) return this.go(left.location, left.cursor);
    const up = this.location === "/" ? null : parentLocation(this.location);
    if (up !== null) return this.go(up);
  }

  /** Root of the current volume ("/" for the home folder and the boot disk), or of the server. */
  rootLocation() {
    // The results list: the root of the folder it came from.
    const here = isSearchLocation(this.location) ? (this.results?.back.location ?? "/") : this.location;
    const server = remoteRoot(here);
    if (server) return server;
    const v = volumeFor(this.volumes, here);
    return v && v.kind !== "home" ? v.path : "/";
  }

  goRoot() {
    return this.go(this.rootLocation());
  }

  enter() {
    const e = this.cursorEntry;
    if (!e) return;
    if (e.up) return this.goParent();
    // A search result (file or folder): its folder, cursor on it (TC).
    if (isSearchLocation(this.location)) return this.goToResult();
    const path = joinLocation(this.location, e.name);
    if (e.isDir) return this.go(path);
    // An archive opens as a folder, also one inside an archive (nested).
    if (archiveKindOf(e.name)) {
      if (!parseRemote(this.location)) return this.go(path);
      return this.enterRemoteArchive(path);
    }
    // Server listings show a link to a folder as a link (not followed): Enter opens it, and
    // when the backend refuses because the target is a folder, the panel enters it instead.
    if (e.isSymlink && parseRemote(this.location)) {
      const here = this.location;
      return this.api.openDefault(path, () => (this.location === here ? this.go(path) : null));
    }
    return this.api.openDefault(path);
  }

  /**
   * Enter / Ctrl+PgUp in the results list: the cursor item's folder with the cursor on it
   * (an archive entry: the folder inside the archive).
   */
  goToResult() {
    const e = this.cursorEntry;
    if (!isSearchLocation(this.location) || !e || e.up || !e.path) return;
    if (e.mounted === false) return this.setMessage(NOT_MOUNTED);
    const { folder, name } = resultFolder(e);
    return this.go(folder, name);
  }

  /** Enter on an archive on a server: a tracked local copy is downloaded and entered. */
  async enterRemoteArchive(path) {
    if (this.downloading) return null;
    const parent = this.location;
    let local;
    this.downloading = true;
    try {
      this.api.status?.(`Downloading ${lastComponent(path)}…`);
      local = await this.api.openRemoteArchive(path);
    } catch (e) {
      if (String(e?.message ?? e) === "Download cancelled") this.setMessage(`Download of ${lastComponent(path)} cancelled`);
      else this.api.onError(this, e, path);
      return null;
    } finally {
      this.downloading = false;
    }
    // Only if the panel is still where Enter was pressed.
    if (this.location !== parent) return null;
    remoteArchives.set(local, parent);
    return this.go(local);
  }

  // ---- quick search (TC Alt+letters) ----

  // ---- quick filter (Ctrl+S) and folder history ----

  /** Ctrl+S: opens the filter box (or goes back into it) with the current filter. */
  openFilter() {
    this.quickSearchEnd();
    this.filterBox.hidden = false;
    this.filterInput.value = this.filter;
    this.filterInput.focus();
    this.filterInput.select();
  }

  setFilter(text) {
    this.filter = text;
    this.rebuild(this.cursorName, 0);
  }

  /**
   * Keys in the filter box: Escape clears the filter and closes it; Enter, Tab, Up and Down
   * keep it and give the keys back to the list (Up/Down also move the cursor).
   */
  filterKey(e) {
    const back = () => {
      e.preventDefault();
      e.stopPropagation();
      this.filterInput.blur();
      this.api.onFilterDone?.(this);
    };
    if (e.key === "Escape") {
      this.filterInput.value = "";
      this.setFilter("");
      back();
    } else if (e.key === "Enter" || e.key === "Tab") {
      back();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      back();
      this.setCursor(this.cursorIndex + (e.key === "ArrowDown" ? 1 : -1));
    }
  }

  /** Alt+Left / Alt+Right: the previous / next place in this panel's history. */
  async goHistory(dir) {
    const from = dir < 0 ? this.back : this.forward;
    if (!from.length) return false;
    const to = from[from.length - 1];
    const here = { location: this.location, cursorName: this.cursorName };
    const r = await this.go(to.location, to.cursorName ?? undefined, { history: false });
    if (!r.ok) return false;
    if (dir < 0) {
      this.back = this.back.slice(0, -1);
      if (!isSearchLocation(here.location)) this.forward = pushHistory(this.forward, here);
    } else {
      this.forward = this.forward.slice(0, -1);
      if (!isSearchLocation(here.location)) this.back = pushHistory(this.back, here);
    }
    return true;
  }

  /** Alt+Down list: jumps to `index` in `back` (older places move to Forward like repeated Back). */
  async goBackTo(index) {
    const to = this.back[index];
    if (!to) return;
    const here = { location: this.location, cursorName: this.cursorName };
    const r = await this.go(to.location, to.cursorName ?? undefined, { history: false });
    if (!r.ok) return;
    const skipped = this.back.slice(index + 1).reverse();
    this.back = this.back.slice(0, index);
    let fwd = this.forward;
    if (!isSearchLocation(here.location)) fwd = pushHistory(fwd, here);
    for (const h of skipped) fwd = pushHistory(fwd, h);
    this.forward = fwd;
  }

  /** True while the quick search box is open. */
  get quickSearching() {
    return this.qsText != null;
  }

  /**
   * Adds `ch` to the quick search text and puts the cursor on the first name starting with
   * it. A character that matches nothing is not added (TC); the box shows the miss.
   */
  quickSearchType(ch) {
    const text = (this.qsText ?? "") + ch;
    const i = quickSearchFind(this.entries, text);
    if (i >= 0) {
      this.qsText = text;
      this.setCursor(i);
    } else if (this.qsText == null) {
      this.qsText = "";
    }
    this.quickSearchShow(i < 0 ? text : null);
  }

  /** Backspace: drops the last character; the cursor goes to the first match of the rest. */
  quickSearchBack() {
    if (!this.qsText) return this.quickSearchShow();
    this.qsText = this.qsText.slice(0, -1);
    const i = quickSearchFind(this.entries, this.qsText);
    if (i >= 0) this.setCursor(i);
    this.quickSearchShow();
  }

  /** Up/Down: the previous/next name with the same start, wrapping. */
  quickSearchStep(dir) {
    const i = quickSearchFind(this.entries, this.qsText ?? "", this.cursorIndex, dir);
    if (i >= 0) this.setCursor(i);
    this.quickSearchShow();
  }

  quickSearchEnd() {
    clearTimeout(this.qsTimer);
    this.qsText = null;
    if (this.qsEl) this.qsEl.hidden = true;
  }

  /** Shows the box (`missed`: typed text that matched nothing); a 3 s pause closes it. */
  quickSearchShow(missed = null) {
    this.qsEl.textContent = missed ?? this.qsText;
    this.qsEl.classList.toggle("miss", missed != null);
    this.qsEl.hidden = false;
    clearTimeout(this.qsTimer);
    this.qsTimer = setTimeout(() => this.quickSearchEnd(), 3000);
  }

  /** Fixed panel keys (not remappable). Returns true when handled. */
  handleKey(combo) {
    switch (combo) {
      case "Up": this.setCursor(this.cursorIndex - 1); return true;
      case "Down": this.setCursor(this.cursorIndex + 1); return true;
      case "PageUp": this.setCursor(this.cursorIndex - this.pageRows()); return true;
      case "PageDown": this.setCursor(this.cursorIndex + this.pageRows()); return true;
      case "Home": this.setCursor(0); return true;
      case "End": this.setCursor(this.entries.length - 1); return true;
      case "Left":
      case "Right": {
        const { first, last } = this.visibleRows();
        this.setCursor(sideJump(this.cursorIndex, first, last, combo === "Left" ? -1 : 1));
        return true;
      }
      case "Enter": this.enter(); return true;
      default: return false;
    }
  }

  // ---- marking ----

  /** Space/Insert: toggle the entry under the cursor, size folders, move down. */
  toggleMark() {
    const e = this.cursorEntry;
    if (e && !e.up) {
      if (this.marked.has(e.name)) this.marked.delete(e.name);
      else {
        this.marked.add(e.name);
        if (e.isDir) this.queueDirSize(e);
      }
    }
    this.setCursor(this.cursorIndex + 1);
  }

  markAll() {
    for (const e of this.entries) if (!e.up) this.marked.add(e.name);
    this.render();
  }

  invertMarks() {
    for (const e of this.entries) {
      if (e.up) continue;
      if (this.marked.has(e.name)) this.marked.delete(e.name);
      else this.marked.add(e.name);
    }
    this.render();
  }

  /** Marks (on=true) or unmarks entries matching `mask` (folders only via "name/" patterns). */
  markMask(mask, on) {
    for (const e of this.entries) {
      if (e.up || !maskSelects(e, mask)) continue;
      if (!on) this.marked.delete(e.name);
      else if (!this.marked.has(e.name)) {
        this.marked.add(e.name);
        if (e.isDir) this.queueDirSize(e);
      }
    }
    this.render();
  }

  /** `entry`: a marked folder; a search result is sized at its own path. */
  queueDirSize(entry) {
    const path = entry.path ?? joinLocation(this.location, entry.name);
    this.sizeQueue.push({ location: this.location, name: entry.name, path });
    if (!this.sizing) this.drainDirSizes();
  }

  // One request at a time: a new dir_size call for this panel would cancel the running one.
  async drainDirSizes() {
    this.sizing = true;
    while (this.sizeQueue.length) {
      const { location, name, path } = this.sizeQueue.shift();
      try {
        const [bytes] = await this.api.dirSize(path, this.side);
        if (location === this.location) {
          this.dirSizes.set(name, bytes);
          this.renderStatus();
        }
      } catch {
        // Unreadable or cancelled: the folder keeps showing <DIR>.
      }
    }
    this.sizing = false;
  }

  // ---- rendering ----

  render() {
    this.renderHeader();
    this.renderRows();
    this.renderStatus();
  }

  queueRender() {
    if (this.renderQueued) return;
    this.renderQueued = true;
    requestAnimationFrame(() => {
      this.renderQueued = false;
      this.renderRows();
    });
  }

  renderHeader() {
    // Inside the local copy of an archive from a server: the drive is that server.
    const where = remoteArchiveOf(this.location, remoteArchives)?.parent ?? this.location;
    const vol = parseRemote(where) ? null : volumeFor(this.volumes, where);
    const server = parseRemote(where)?.server ?? null;
    const conn = this.connections.find((c) => c.name === server) ?? null;
    this.drivesEl.replaceChildren(
      ...this.volumes.map((v) => {
        const b = el("span", v === vol ? "drive on" : "drive", `${VOLUME_ICON[v.kind] ?? "💾"} ${v.name}`);
        b.dataset.path = v.path;
        // A saved SMB share that is not mounted has no folder yet.
        b.title = v.smb && !v.mounted ? "Not mounted: click to mount the share" : v.path;
        return b;
      }),
      // One button per open connection; it opens the server's root.
      ...this.connections.map((c) => {
        const b = el("span", c === conn ? "drive on" : "drive", connectionLabel(c));
        b.dataset.path = c.root;
        b.title = c.display;
        return b;
      }),
    );
    if (server) {
      // On a server: its name instead of a disk, its address instead of the free space.
      this.comboEl.textContent = connectionLabel({ name: server });
      this.freeEl.textContent = conn ? `[${conn.display}]` : "";
    } else if (vol) {
      this.comboEl.textContent = `${VOLUME_ICON[vol.kind] ?? "💾"} ${vol.name}`;
      // The home folder lives on the boot disk: show that disk's label.
      const label = vol.kind === "home" ? (this.volumes.find((v) => v.kind === "root")?.name ?? vol.name) : vol.name;
      this.freeEl.textContent = freeLine(label, vol.free, vol.total);
    } else {
      this.comboEl.textContent = "";
      this.freeEl.textContent = "";
    }
    this.curEl.textContent = isSearchLocation(this.location)
      ? displayLocation(this.location, remoteArchives, this.results)
      : joinLocation(displayLocation(this.location, remoteArchives), "*.*");
  }

  ensureVisible() {
    const list = this.listEl;
    const top = this.cursorIndex * this.rowH;
    const viewH = list.clientHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + this.rowH > list.scrollTop + viewH) list.scrollTop = top + this.rowH - viewH;
  }

  rowClass(i) {
    const e = this.entries[i];
    let c = "r";
    if (!e.up && this.marked.has(e.name)) c += " mark";
    if (i === this.cursorIndex) c += " sel";
    if (i === this.dropRow) c += " droptarget";
    return c;
  }

  /** Class and inline colors of row element `r` showing entry `i`; touches the DOM only on change. */
  paintRow(r, i) {
    const c = this.rowClass(i);
    if (r.className !== c) r.className = c;
    if (!this.colors) return;
    const e = this.entries[i];
    const st = rowStyle(
      e,
      { marked: !e.up && this.marked.has(e.name), cursor: i === this.cursorIndex, active: this.active, index: i },
      this.colors,
      this.typeMatcher,
    );
    const key = `${st.color}|${st.background}|${st.outline}`;
    if (r.colorKey === key) return;
    r.colorKey = key;
    r.style.color = st.color;
    r.style.backgroundColor = st.background;
    r.style.outline = st.outline;
  }

  /** Draws the rows in view; when the range and the data are unchanged only classes and colors are updated. */
  renderRows() {
    const list = this.listEl;
    const count = this.entries.length;
    const { start, end } = visibleRange(list.scrollTop, this.rowH, list.clientHeight, count);
    if (!this.rowsDirty && start === this.rendered.start && end === this.rendered.end) {
      const rows = this.rowsEl.children;
      for (let i = start; i < end; i++) this.paintRow(rows[i - start], i);
      return;
    }
    this.rowsDirty = false;
    this.rendered = { start, end };
    this.spacerEl.style.height = `${count * this.rowH}px`;
    this.rowsEl.style.transform = `translateY(${start * this.rowH}px)`;
    const rows = [];
    const wanted = new Set();
    for (let i = start; i < end; i++) {
      const e = this.entries[i];
      const r = el("div");
      this.paintRow(r, i);
      r.dataset.i = i;
      r.append(this.iconEl(e, wanted), document.createTextNode(e.up ? "[..]" : displayName(e)));
      rows.push(r);
    }
    this.rowsEl.replaceChildren(...rows);
    if (wanted.size) fetchIcons([...wanted], this.api);
  }

  renderStatus() {
    let name = "", size = "", date = "", mode = "";
    const e = this.cursorEntry;
    if (this.message) {
      name = this.message;
    } else if (this.marked.size) {
      name = markSummary(this.entries, this.marked, this.dirSizes);
    } else if (e?.up) {
      name = "[..]";
      size = "<DIR>";
    } else if (e) {
      name = displayName(e);
      size = e.isDir && this.dirSizes.has(e.name) ? groupThousands(this.dirSizes.get(e.name)) : formatSize(e);
      date = formatDate(e.mtime);
      mode = formatMode(e.mode, e.isDir, e.isSymlink);
    }
    this.stName.textContent = name;
    this.stSize.textContent = size;
    this.stDate.textContent = date;
    this.stMode.textContent = mode;
  }
}
