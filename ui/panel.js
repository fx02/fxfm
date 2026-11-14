// One TC file panel: drive bar, path row, current-path bar, virtual list, status line.

import {
  archiveKindOf,
  archiveOf,
  archiveLeave,
  displayLocation,
  colorClass,
  connectionLabel,
  displayName,
  formatDate,
  formatMode,
  formatSize,
  fullyVisibleRows,
  freeLine,
  groupThousands,
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
  visibleRange,
} from "./lib.mjs";

export const VOLUME_ICON = { root: "💻", home: "🏠", volume: "💾", network: "🌐" };
const UP = Object.freeze({ name: "..", isDir: true, up: true, size: 0, hidden: false });

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function rowIcon(e, cls) {
  if (e.up) return "⬆";
  if (e.isDir) return "📁";
  if (cls === "arc") return "🗜";
  if (cls === "med") return "🎬";
  if (/\.pdf$/i.test(e.name)) return "📕";
  return "📄";
}

/** The volume containing `location` (longest path prefix), or null (also on a server). */
export function volumeFor(volumes, location) {
  if (parseRemote(location)) return null;
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
 *   onError(panel, error, target), onDriveMenu(panel), onCursorMoved(panel),
 *   onRowPress(panel, index, mouseEvent) (left button on a row: a drag may follow)
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
    this.navToken = 0;
    this.pendingTarget = null;
    this.sizeQueue = [];
    this.sizing = false;
    this.renderQueued = false;
    this.rowsDirty = true;
    this.rendered = { start: 0, end: 0 };
    /** Row highlighted as a drop target (-1: none). */
    this.dropRow = -1;
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
    root.append(this.drivesEl, pathRow, this.curEl, this.listEl, this.statusEl);

    root.addEventListener("mousedown", () => this.focus());
    this.comboEl.addEventListener("click", () => this.api.onDriveMenu(this));
    rootBtn.addEventListener("click", () => this.goRoot());
    upBtn.addEventListener("click", () => this.goParent());
    this.drivesEl.addEventListener("click", (ev) => {
      const b = ev.target.closest("[data-path]");
      if (b) this.go(b.dataset.path);
    });
    this.rowsEl.addEventListener("mousedown", (ev) => {
      const r = ev.target.closest("[data-i]");
      if (!r) return;
      const i = Number(r.dataset.i);
      if (ev.button === 0) this.api.onRowPress(this, i, ev);
      this.setCursor(i);
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
  async go(location, cursorName, { refresh = false } = {}) {
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
    const shown = showHidden ? this.raw : this.raw.filter((e) => !e.hidden);
    // No "[..]" where there is no parent: "/" and a server root.
    this.entries = this.location === "/" || parentLocation(this.location) === null ? shown : [UP, ...shown];
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
    this.active = on;
    this.root.classList.toggle("active", on);
    this.curEl.classList.toggle("active", on);
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

  /** Backspace / [..]: nothing at "/" or at a server root. */
  goParent() {
    // The root of a server archive's copy goes back to the server folder, not the temp folder.
    const left = archiveLeave(this.location, remoteArchives);
    if (left) return this.go(left.location, left.cursor);
    const up = this.location === "/" ? null : parentLocation(this.location);
    if (up !== null) return this.go(up);
  }

  /** Root of the current volume ("/" for the home folder and the boot disk), or of the server. */
  rootLocation() {
    const server = remoteRoot(this.location);
    if (server) return server;
    const v = volumeFor(this.volumes, this.location);
    return v && v.kind !== "home" ? v.path : "/";
  }

  goRoot() {
    return this.go(this.rootLocation());
  }

  enter() {
    const e = this.cursorEntry;
    if (!e) return;
    if (e.up) return this.goParent();
    const path = joinLocation(this.location, e.name);
    if (e.isDir) return this.go(path);
    // An archive opens as a folder; inside an archive (nested) it is a plain file.
    if (archiveKindOf(e.name) && !archiveOf(this.location)) {
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
      this.api.onError(this, e, path);
      return null;
    } finally {
      this.downloading = false;
    }
    // Only if the panel is still where Enter was pressed.
    if (this.location !== parent) return null;
    remoteArchives.set(local, parent);
    return this.go(local);
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
        if (e.isDir) this.queueDirSize(e.name);
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
        if (e.isDir) this.queueDirSize(e.name);
      }
    }
    this.render();
  }

  queueDirSize(name) {
    this.sizeQueue.push({ location: this.location, name });
    if (!this.sizing) this.drainDirSizes();
  }

  // One request at a time: a new dir_size call for this panel would cancel the running one.
  async drainDirSizes() {
    this.sizing = true;
    while (this.sizeQueue.length) {
      const { location, name } = this.sizeQueue.shift();
      try {
        const [bytes] = await this.api.dirSize(joinLocation(location, name), this.side);
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
    const vol = volumeFor(this.volumes, this.location);
    const server = parseRemote(this.location)?.server ?? null;
    const conn = this.connections.find((c) => c.name === server) ?? null;
    this.drivesEl.replaceChildren(
      ...this.volumes.map((v) => {
        const b = el("span", v === vol ? "drive on" : "drive", `${VOLUME_ICON[v.kind] ?? "💾"} ${v.name}`);
        b.dataset.path = v.path;
        b.title = v.path;
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
    this.curEl.textContent = joinLocation(displayLocation(this.location, remoteArchives), "*.*");
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
    const color = colorClass(e);
    if (color) c += ` ${color}`;
    if (!e.up && this.marked.has(e.name)) c += " mark";
    if (i === this.cursorIndex) c += " sel";
    if (i === this.dropRow) c += " droptarget";
    return c;
  }

  /** Draws the rows in view; when the range and the data are unchanged only classes are updated. */
  renderRows() {
    const list = this.listEl;
    const count = this.entries.length;
    const { start, end } = visibleRange(list.scrollTop, this.rowH, list.clientHeight, count);
    if (!this.rowsDirty && start === this.rendered.start && end === this.rendered.end) {
      const rows = this.rowsEl.children;
      for (let i = start; i < end; i++) {
        const c = this.rowClass(i);
        if (rows[i - start].className !== c) rows[i - start].className = c;
      }
      return;
    }
    this.rowsDirty = false;
    this.rendered = { start, end };
    this.spacerEl.style.height = `${count * this.rowH}px`;
    this.rowsEl.style.transform = `translateY(${start * this.rowH}px)`;
    const rows = [];
    for (let i = start; i < end; i++) {
      const e = this.entries[i];
      const r = el("div", this.rowClass(i));
      r.dataset.i = i;
      r.append(el("i", null, rowIcon(e, colorClass(e))), document.createTextNode(e.up ? "[..]" : displayName(e)));
      rows.push(r);
    }
    this.rowsEl.replaceChildren(...rows);
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
