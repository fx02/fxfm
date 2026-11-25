// The Synchronize dirs window (sync.html): compares the left and right panel's folders, shows
// each side in its own half with the direction in the middle, and asks the main window to run
// the copies and deletes (sync.js). Nothing happens without Synchronize and its confirmation.

import {
  SYNC_ACTION,
  SYNC_MODES,
  SYNC_COLUMNS,
  SYNC_REPLY,
  displayLocation,
  formatDate,
  groupThousands,
  remoteArchives,
  syncActionAllowed,
  syncCounts,
  syncModeAction,
  syncPlan,
  syncSort,
  syncSymbol,
} from "./lib.mjs";
import { dialog, handleModalKey } from "./dialogs.js";
import { SYNC_COMMAND, SYNC_LAYOUT_KEY } from "./sync.js";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const appWin = T.window.getCurrentWindow();

// ------------------------------------------------------------ bridge to the main window

const session = Math.random().toString(36).slice(2);
let nextId = 0;
const waiting = new Map();

function ask(action, args = {}) {
  const id = `${session}:${++nextId}`;
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    T.event.emit(SYNC_ACTION, { id, action, args }).catch((e) => {
      waiting.delete(id);
      reject(e);
    });
  });
}

// ------------------------------------------------------------ kept layout and options

function layout() {
  try {
    return JSON.parse(localStorage.getItem(SYNC_LAYOUT_KEY) ?? "null") ?? {};
  } catch {
    return {};
  }
}

function saveLayout(patch) {
  try {
    localStorage.setItem(SYNC_LAYOUT_KEY, JSON.stringify({ ...layout(), ...patch }));
  } catch {
    // Not kept: the next window opens with the defaults.
  }
}

let geomTimer = 0;
/** Position and size while neither maximized nor full screen, and whether maximized. */
function keepGeometry() {
  clearTimeout(geomTimer);
  geomTimer = setTimeout(async () => {
    try {
      const [max, full] = await Promise.all([appWin.isMaximized(), appWin.isFullscreen()]);
      if (full) return;
      if (max) return saveLayout({ maximized: true });
      const scale = await appWin.scaleFactor();
      const p = (await appWin.outerPosition()).toLogical(scale);
      const z = (await appWin.innerSize()).toLogical(scale);
      saveLayout({ x: Math.round(p.x), y: Math.round(p.y), width: Math.round(z.width), height: Math.round(z.height), maximized: false });
    } catch (e) {
      console.error("fxfm: window geometry", e);
    }
  }, 300);
}

// ------------------------------------------------------------ the window

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function checkbox(label, checked) {
  const box = el("input");
  box.type = "checkbox";
  box.checked = checked;
  const l = el("label", "check");
  l.append(box, ` ${label}`);
  return { box, label: l };
}

const sizeText = (item) => (!item ? "" : item.isDir ? "<DIR>" : groupThousands(item.size));
const dateText = (item) => (item ? formatDate(item.mtime) : "");
const ACTION_TITLE = {
  right: "Copy to the right",
  left: "Copy to the left",
  delRight: "Move the right item to the Trash",
  delLeft: "Move the left item to the Trash",
  none: "Not copied",
};

function showWindow(start) {
  let L = start.left;
  let R = start.right;
  const saved = layout();
  const opts = {
    subdirs: saved.subdirs !== false,
    byContent: saved.byContent === true,
    ignoreDates: saved.ignoreDates === true,
    showEqual: saved.showEqual === true,
    mode: SYNC_MODES.some(([id]) => id === saved.mode) ? saved.mode : "twoWay",
  };
  let rows = [];
  let actions = [];
  let shown = [];
  let sel = 0;
  let busy = false;

  // The whole window is the dialog box (its CSS), without a title: macOS draws that.
  const box = el("div", "dialog syncdlg window");
  const body = el("div", "body");
  const subdirs = checkbox("Subfolders", opts.subdirs);
  const byContent = checkbox("By content (local files)", opts.byContent);
  const ignoreDates = checkbox("Ignore dates", opts.ignoreDates);
  const showEqual = checkbox("Show equal", opts.showEqual);
  const mode = el("select");
  for (const [id, label] of SYNC_MODES) {
    const o = el("option", null, label);
    o.value = id;
    mode.append(o);
  }
  mode.value = opts.mode;
  const modeLabel = el("label", "mode");
  modeLabel.append("Mode: ", mode);
  const optLine = el("div", "opts");
  optLine.append(modeLabel, subdirs.label, byContent.label, ignoreDates.label, showEqual.label);
  const sides = el("div", "srow sides");
  const leftPath = el("span", "side left");
  const rightPath = el("span", "side right");
  sides.append(leftPath, el("span", "mid"), rightPath);
  // Column headers sort (again: the other way); the middle one sorts by direction.
  let sortCol = SYNC_COLUMNS.includes(saved.sortCol) ? saved.sortCol : null;
  let sortDesc = saved.sortDesc === true;
  const head = el("div", "srow head");
  const heads = ["Left", "Size", "Date", "Dir.", "Date", "Size", "Right"].map((t, i) => {
    const h = el("span", "sortable", t);
    h.dataset.col = SYNC_COLUMNS[i];
    h.dataset.label = t;
    h.title = i === 3 ? "Sort by direction: → first, then ←, equal, not copied" : `Sort by ${t.toLowerCase()}`;
    head.append(h);
    return h;
  });
  const paintHeads = () => {
    for (const h of heads) h.textContent = h.dataset.col === sortCol ? `${h.dataset.label} ${sortDesc ? "▼" : "▲"}` : h.dataset.label;
  };
  const list = el("div", "slist");
  list.tabIndex = 0;
  const note = el(
    "p",
    "note",
    "→ / ← copy the selected row that way, Shift+→ / Shift+← move the item on that side to the Trash, Space skips it; or click the symbol. Mode sets all rows; Synchronize asks before doing anything.",
  );
  const statusLine = el("div", "sstatus");
  body.append(optLine, sides, head, list, note, statusLine);
  const buttons = el("div", "buttons");
  const compareBtn = el("button", null, "Compare");
  const syncBtn = el("button", null, "Synchronize");
  const defaultsBtn = el("button", null, "Reset to mode");
  const clearBtn = el("button", null, "Skip all");
  const closeBtn = el("button", null, "Close");
  buttons.append(compareBtn, syncBtn, defaultsBtn, clearBtn, closeBtn);
  box.append(body, buttons);
  document.body.append(box);

  // Long paths are cut at the start (the folder at the end matters); the LRM keeps a trailing
  // "/" or ")" at either end in place in the right-to-left box.
  const showPath = (span, loc) => {
    const text = displayLocation(loc, remoteArchives);
    span.textContent = `\u200e${text}\u200e`;
    span.title = text;
  };
  const showPaths = () => {
    showPath(leftPath, L);
    showPath(rightPath, R);
  };

  const setStatus = (text, error = false) => {
    statusLine.textContent = text;
    statusLine.classList.toggle("msg", error);
  };

  const summary = () => {
    const c = syncCounts(rows, actions);
    const parts = [`→ ${c.right} item(s), ${groupThousands(c.rightBytes)} bytes`, `← ${c.left} item(s), ${groupThousands(c.leftBytes)} bytes`];
    if (c.delRight || c.delLeft) parts.push(`to the Trash: ${c.delLeft} left, ${c.delRight} right`);
    parts.push(`not copied: ${rows.filter((r, i) => r.state !== "equal" && actions[i] === "none").length}`);
    return parts.join(" · ");
  };

  const paintRow = (line, i, k) => {
    const row = rows[i];
    const a = actions[i];
    line.className = `srow ${row.state} act-${a}`;
    if (k === sel) line.classList.add("sel");
    const act = el("span", "act", syncSymbol(a, row.state));
    act.title = ACTION_TITLE[a];
    line.replaceChildren(
      el("span", "name", row.left ? row.rel : ""),
      el("span", "num", sizeText(row.left)),
      el("span", null, dateText(row.left)),
      act,
      el("span", null, dateText(row.right)),
      el("span", "num", sizeText(row.right)),
      el("span", "name", row.right ? row.rel : ""),
    );
    line.title = row.rel;
  };

  const render = () => {
    const visible = rows.map((_, i) => i).filter((i) => showEqual.box.checked || rows[i].state !== "equal");
    shown = syncSort(rows, actions, visible, sortCol, sortDesc);
    paintHeads();
    sel = Math.min(sel, Math.max(0, shown.length - 1));
    const lines = shown.map((i, k) => {
      const line = el("div");
      line.dataset.k = String(k);
      paintRow(line, i, k);
      return line;
    });
    list.replaceChildren(...lines);
    if (!rows.length) list.append(el("div", "empty", busy ? "Comparing…" : "Press Compare."));
    else if (!shown.length) list.append(el("div", "empty", "No differences."));
    syncBtn.disabled = busy || !rows.length;
    if (rows.length && !busy) setStatus(summary());
    list.children[sel]?.scrollIntoView?.({ block: "nearest" });
  };

  const select = (k) => {
    if (!shown.length) return;
    const old = sel;
    sel = Math.max(0, Math.min(shown.length - 1, k));
    if (list.children[old]) paintRow(list.children[old], shown[old], old);
    paintRow(list.children[sel], shown[sel], sel);
    list.children[sel].scrollIntoView?.({ block: "nearest" });
  };

  const setAction = (k, a) => {
    const i = shown[k];
    if (i === undefined || !syncActionAllowed(rows[i], a)) return;
    actions[i] = a;
    paintRow(list.children[k], i, k);
    setStatus(summary());
  };

  /** A click on the symbol: the next allowed direction (→ ← →✕ ✕← none). */
  const cycle = (k) => {
    const i = shown[k];
    const order = ["right", "left", "delRight", "delLeft", "none"];
    let a = actions[i];
    for (let n = 0; n < order.length; n++) {
      a = order[(order.indexOf(a) + 1) % order.length];
      if (syncActionAllowed(rows[i], a)) break;
    }
    setAction(k, a);
  };

  const applyMode = () => {
    actions = rows.map((r) => syncModeAction(mode.value, r));
    render();
  };

  const remember = () =>
    saveLayout({
      subdirs: subdirs.box.checked,
      byContent: byContent.box.checked,
      ignoreDates: ignoreDates.box.checked,
      showEqual: showEqual.box.checked,
      mode: mode.value,
    });

  const compare = async () => {
    if (busy) {
      invoke("sync_cancel").catch(() => {});
      return;
    }
    remember();
    busy = true;
    compareBtn.textContent = "Stop";
    rows = [];
    actions = [];
    render();
    setStatus("Comparing…");
    try {
      const got = await invoke("sync_compare", {
        left: L,
        right: R,
        subdirs: subdirs.box.checked,
        ignoreDates: ignoreDates.box.checked,
        byContent: byContent.box.checked,
      });
      busy = false;
      rows = got;
      sel = 0;
      applyMode();
    } catch (e) {
      busy = false;
      rows = [];
      actions = [];
      render();
      const stopped = String(e) === "cancelled";
      setStatus(stopped ? "Stopped." : `Cannot compare: ${e}`, !stopped);
    } finally {
      busy = false;
      compareBtn.textContent = "Compare";
      syncBtn.disabled = !rows.length;
    }
  };

  const synchronize = async () => {
    if (busy || !rows.length) return;
    const c = syncCounts(rows, actions);
    if (!c.right && !c.left && !c.delRight && !c.delLeft) {
      setStatus("Nothing is set to copy or delete.", true);
      return;
    }
    const text = [];
    if (c.right) text.push(`Copy ${c.right} item(s) to the right (${groupThousands(c.rightBytes)} bytes).`);
    if (c.left) text.push(`Copy ${c.left} item(s) to the left (${groupThousands(c.leftBytes)} bytes).`);
    if (c.delRight) text.push(`Move ${c.delRight} item(s) on the right to the Trash.`);
    if (c.delLeft) text.push(`Move ${c.delLeft} item(s) on the left to the Trash.`);
    if (c.delRight || c.delLeft) text.push("On a server or inside an archive deleting is permanent.");
    if (c.right || c.left) text.push("Existing files in the chosen direction are overwritten.");
    const r = await dialog({
      title: "Synchronize dirs",
      text,
      danger: Boolean(c.delRight || c.delLeft),
      buttons: [
        { label: "Synchronize", value: "ok", default: true },
        { label: "Cancel", value: null },
      ],
    });
    if (r !== "ok") return;
    const jobs = syncPlan(rows, actions);
    busy = true;
    syncBtn.disabled = true;
    setStatus("Synchronizing in the main window…");
    let ok = 0;
    try {
      ok = await ask("run", { jobs });
    } catch (e) {
      setStatus(`Cannot synchronize: ${e?.message ?? e}`, true);
    } finally {
      busy = false;
    }
    await appWin.setFocus().catch(() => {});
    await compare();
    if (ok < jobs.length) setStatus(`Stopped after ${ok} of ${jobs.length} job(s). ${summary()}`, true);
  };

  list.addEventListener("click", (e) => {
    const line = e.target.closest("[data-k]");
    if (!line) return;
    const k = Number(line.dataset.k);
    select(k);
    if (e.target.classList.contains("act")) cycle(k);
  });
  head.addEventListener("click", (e) => {
    const col = e.target.closest("[data-col]")?.dataset.col;
    if (!col) return;
    // Ascending, descending, then back to the compare order.
    if (col !== sortCol) [sortCol, sortDesc] = [col, false];
    else if (!sortDesc) sortDesc = true;
    else [sortCol, sortDesc] = [null, false];
    saveLayout({ sortCol, sortDesc });
    // The selected row stays selected.
    const keep = shown[sel];
    render();
    const k = shown.indexOf(keep);
    if (k >= 0) select(k);
  });
  showEqual.box.addEventListener("change", () => {
    remember();
    render();
  });
  mode.addEventListener("change", () => {
    remember();
    applyMode();
  });
  compareBtn.addEventListener("click", compare);
  syncBtn.addEventListener("click", synchronize);
  defaultsBtn.addEventListener("click", applyMode);
  clearBtn.addEventListener("click", () => {
    actions = rows.map(() => "none");
    render();
  });
  closeBtn.addEventListener("click", () => appWin.close());

  const onKey = (e) => {
    const inList = document.activeElement === list;
    const k = e.key;
    if (k === "Escape") {
      e.preventDefault();
      appWin.close();
    } else if (inList && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"].includes(k)) {
      e.preventDefault();
      const page = Math.max(1, Math.floor(list.clientHeight / 22) - 1);
      select({ ArrowUp: sel - 1, ArrowDown: sel + 1, PageUp: sel - page, PageDown: sel + page, Home: 0, End: shown.length - 1 }[k]);
    } else if (inList && (k === "ArrowRight" || k === "ArrowLeft")) {
      e.preventDefault();
      const right = k === "ArrowRight";
      setAction(sel, e.shiftKey ? (right ? "delRight" : "delLeft") : right ? "right" : "left");
    } else if (inList && (k === " " || k === "Delete" || k === "Backspace")) {
      e.preventDefault();
      setAction(sel, "none");
    } else if (k === "Enter" && !e.isComposing && document.activeElement?.tagName !== "BUTTON" && document.activeElement?.tagName !== "SELECT") {
      e.preventDefault();
      if (document.activeElement?.type === "checkbox") document.activeElement.click();
      else compare();
    }
  };

  showPaths();
  render();
  list.focus();
  return {
    onKey,
    compare,
    setFolders(l, r) {
      L = l;
      R = r;
      showPaths();
      compare();
    },
    cancel: () => busy && invoke("sync_cancel").catch(() => {}),
  };
}

async function main() {
  await T.event.listen(SYNC_REPLY, (ev) => {
    const r = ev.payload ?? {};
    const w = waiting.get(r.id);
    if (!w) return;
    waiting.delete(r.id);
    if (r.ok) w.resolve(r.value);
    else w.reject(new Error(r.error));
  });
  const folders = await ask("context");
  if (!folders?.left || !folders?.right) throw new Error("no folders");
  const win = showWindow(folders);
  await T.event.listen(SYNC_COMMAND, (ev) => {
    const f = ev.payload ?? {};
    if (typeof f.left === "string" && typeof f.right === "string") win.setFolders(f.left, f.right);
    appWin.setFocus().catch(() => {});
  });
  // A dialog on top (the confirmation) gets the keys first.
  document.addEventListener("keydown", (e) => {
    if (handleModalKey(e)) return;
    win.onKey(e);
  });
  await appWin.onCloseRequested(() => win.cancel());
  await appWin.onMoved(keepGeometry);
  await appWin.onResized(keepGeometry);
  await appWin.show();
  await appWin.setFocus();
  await win.compare();
}

main().catch(async (e) => {
  console.error("fxfm: Synchronize dirs failed", e);
  document.body.textContent = `Synchronize dirs could not start: ${e?.message ?? e}`;
  await appWin.show().catch(() => {});
});
