// Drag & drop: out of a panel into other apps (native macOS drag), in from other apps (Tauri
// drag-drop events with paths) and between the panels.
//
// One drop path for everything: a panel drag is a native macOS drag (any app can take it);
// local entries go as files, archive and server entries as file promises (written only when
// the receiver asks for them). When the drag comes back over fxfm it arrives like a Finder
// drop (a promise without paths), and `native` tells which panel started it. If the native
// drag cannot start, archive and server entries are dragged inside the webview with mouse
// events (`internal`), so at least between the panels.

import {
  archiveOf,
  commonParent,
  copyTargetDefault,
  dragDropPoint,
  dragSelection,
  dragStarted,
  dropTarget,
  isRemote,
  isSearchLocation,
  joinLocation,
  operationPaths,
  ownDropPaths,
  resultOpRefusal,
} from "./lib.mjs";

/** A native drop counts as fxfm's own drag this long after the session ended (event order varies). */
const OWN_DROP_GRACE_MS = 2000;

/**
 * ctx: {invoke, listen, webview, panels() -> Panel[], activate(panel), topModal(),
 * copyMoveTo(kind, opts), status(text), focusWindow()}
 */
let ctx = null;
/** Left button pressed on a row: {panel, name, x, y}; a drag starts once it moves > 5 px. */
let press = null;
/**
 * Drag fxfm started: {panel, location, cwd, paths, promised, clearsMarks}; `native` also has
 * `endedAt`. `cwd` is where a relative target resolves (a results list: the first item's
 * folder); `promised`: archive or server entries (file promises, no paths in drop events).
 * `internal` has `outsideError`: why the native drag failed (shown when leaving the window).
 */
let native = null;
let internal = null;
/** Paths of the native drag currently over the window (from the enter event). */
let hoverPaths = null;
/** Panel showing the drop highlight. */
let lit = null;
let buttonDown = false;
let shiftDown = false;

/** Left button went down on row `index` of `panel` (Panel api `onRowPress`). */
export function rowPress(panel, index, ev) {
  if (ctx.topModal() || internal) return;
  const entry = panel.entries[index];
  press = entry && !entry.up ? { panel, name: entry.name, x: ev.clientX, y: ev.clientY } : null;
}

/** Esc: cancels a drag between the panels. True when there was one. */
export function cancelDrag() {
  if (!internal) return false;
  endInternal();
  return true;
}

function begin(p) {
  const names = dragSelection(p.panel.entries, p.panel.marked, p.name);
  if (!names.length) return;
  ctx.activate(p.panel);
  const location = p.panel.location;
  // A results list drags its items from their own folders (`path`): local files, or entries
  // that are all inside archives (file promises).
  const results = isSearchLocation(location);
  const picked = names.map((n) => p.panel.entries.find((e) => !e.up && e.name === n));
  const entries = results ? picked.filter(Boolean) : null;
  if (results) {
    const why = resultOpRefusal(entries, "drag");
    if (why) {
      ctx.status(why);
      return;
    }
  }
  const { cwd, paths } = results
    ? operationPaths(location, entries)
    : { cwd: location, paths: names.map((n) => joinLocation(location, n)) };
  const drag = {
    panel: p.panel,
    location,
    cwd,
    paths,
    // By name only (as elsewhere in the UI); Rust checks every path again.
    promised: results ? entries.every((e) => e.inArchive) : isRemote(location) || archiveOf(location) !== null,
    // A finished copy/move clears the marks only when the drag carried them (as F5 does).
    clearsMarks: p.panel.marked.has(p.name),
  };
  const dirs = results ? entries.map((e) => !!e.isDir) : picked.map((e) => !!e?.isDir);
  native = { ...drag, endedAt: null };
  ctx.invoke("start_drag", { paths: drag.paths, dirs }).catch((e) => {
    if (native?.paths === drag.paths) native = null;
    // Still dragging: at least the panel-to-panel drag works.
    if (buttonDown && drag.promised) startInternal(drag, String(e));
    else ctx.status(`Cannot drag: ${e}`);
  });
}

function startInternal(drag, outsideError) {
  internal = { ...drag, outside: false, outsideError };
  document.body.classList.add("dragging");
}

function endInternal() {
  internal = null;
  document.body.classList.remove("dragging");
  highlight(null);
}

/** Panel and row under the CSS pixel point, or null outside the panels. */
function hitTest(x, y) {
  const el = document.elementFromPoint(x, y);
  if (!el) return null;
  const panel = ctx.panels().find((p) => p.root.contains(el));
  if (!panel) return null;
  const index = panel.rowIndexOf(el);
  return { panel, index, entry: index >= 0 ? panel.entries[index] : null };
}

/** Target of dropping `paths` at `hit` (null: nothing happens); `source`: the drag fxfm started or null. */
function targetOf(hit, paths, source) {
  // Nothing can be dropped into a results list (it is no folder).
  if (!hit || !paths?.length || isSearchLocation(hit.panel.location)) return null;
  return dropTarget({
    panelLocation: hit.panel.location,
    rowEntry: hit.entry,
    draggedLocations: paths,
    sourcePanelLocation: source ? source.location : commonParent(paths),
  });
}

/** Highlights the panel (and folder row) a drop at `hit` would go to; null clears. */
function highlight(hit, paths, source) {
  const target = ctx.topModal() ? null : targetOf(hit, paths, source);
  const panel = target === null ? null : hit.panel;
  if (lit && lit !== panel) lit.setDropHighlight(false);
  lit = panel;
  if (!panel) return;
  const onRow = hit.entry?.isDir && !hit.entry.up;
  panel.setDropHighlight(true, onRow ? hit.index : -1);
}

/** A native drop that came from fxfm's own drag (its paths, session not long over). */
function ownDrag(paths) {
  if (!ownDropPaths(native, paths)) return null;
  if (native.endedAt !== null && performance.now() - native.endedAt > OWN_DROP_GRACE_MS) return null;
  return native;
}

/**
 * Opens the copy (move with Shift) dialog for `paths` dropped at `hit`. The panel that got
 * the drop becomes active; same folder, a dragged entry or a folder inside one: nothing.
 */
function drop(hit, paths, source, shift) {
  if (!hit || ctx.topModal()) return;
  const target = targetOf(hit, paths, source);
  ctx.activate(hit.panel);
  if (target === null) return;
  // A drop from another app leaves that app in front: the dialog needs the keyboard.
  ctx.focusWindow();
  const cwd = source ? source.cwd : (commonParent(paths) ?? hit.panel.location);
  ctx
    .copyMoveTo(shift ? "move" : "copy", {
      paths,
      cwd,
      dest: copyTargetDefault(target, []),
      source: source?.clearsMarks ? source.panel : null,
      switchable: true,
    })
    .catch((e) => ctx.status(`${shift ? "Move" : "Copy"}: ${e?.message ?? e}`));
}

function onMouseMove(ev) {
  buttonDown = (ev.buttons & 1) === 1;
  shiftDown = ev.shiftKey;
  if (internal) {
    if (!buttonDown) {
      // The button went up where no mouseup reached the page.
      endInternal();
      return;
    }
    const outside = ev.clientX < 0 || ev.clientY < 0 || ev.clientX >= innerWidth || ev.clientY >= innerHeight;
    if (outside && !internal.outside) ctx.status(`Cannot drag out: ${internal.outsideError}`);
    internal.outside = outside;
    highlight(outside ? null : hitTest(ev.clientX, ev.clientY), internal.paths, internal);
    return;
  }
  if (!press) return;
  if (!buttonDown || ctx.topModal()) {
    press = null;
    return;
  }
  if (dragStarted(press, { x: ev.clientX, y: ev.clientY })) {
    const p = press;
    press = null;
    begin(p);
  }
}

function onMouseUp(ev) {
  buttonDown = false;
  press = null;
  if (!internal) return;
  const drag = internal;
  const hit = drag.outside ? null : hitTest(ev.clientX, ev.clientY);
  endInternal();
  drop(hit, drag.paths, drag, ev.shiftKey);
}

/** Tauri drag-drop events: native drags from other apps and fxfm's own drag-out. */
async function onNativeDrag(ev) {
  const p = ev.payload;
  if (p.type === "leave") {
    hoverPaths = null;
    highlight(null);
    return;
  }
  if (p.type === "enter") hoverPaths = p.paths ?? [];
  const { x, y } = dragDropPoint(p.position, devicePixelRatio, true);
  if (p.type === "enter" || p.type === "over") {
    const own = ownDrag(hoverPaths ?? []);
    // fxfm's own file promises carry no paths: the drag knows them.
    highlight(hitTest(x, y), own ? own.paths : hoverPaths, own);
    return;
  }
  if (p.type !== "drop") return;
  // Read the keys first: the drop event has no modifiers and Shift may go up soon.
  const keys = ctx.invoke("drag_modifiers").catch(() => ({ shift: shiftDown }));
  hoverPaths = null;
  highlight(null);
  const hit = hitTest(x, y);
  const source = ownDrag(p.paths ?? []);
  const paths = source ? source.paths : (p.paths ?? []);
  if (source) native = null;
  if (!paths.length || !hit || ctx.topModal()) return;
  drop(hit, paths, source, (await keys).shift);
}

/** Delays of the panel refreshes after a drag-out the receiver moved (Finder may finish later). */
const MOVED_REFRESH_MS = [0, 1000];

function onDragEnded(ev) {
  const { result, operation, message } = ev.payload;
  if (result === "error") {
    // Also after the drop: a file promise that could not be written.
    native = null;
    ctx.status(`Cannot drag: ${message}`);
    return;
  }
  if (!native) return;
  if (operation === "move") {
    const panel = native.panel;
    for (const ms of MOVED_REFRESH_MS) {
      setTimeout(() => panel.refresh().catch((e) => console.error("fxfm: refresh", e)), ms);
    }
  }
  native.endedAt = performance.now();
}

export async function initDragDrop(c) {
  ctx = c;
  document.addEventListener("mousemove", onMouseMove);
  document.addEventListener("mouseup", onMouseUp);
  const keys = (ev) => {
    shiftDown = ev.shiftKey;
  };
  document.addEventListener("keydown", keys, true);
  document.addEventListener("keyup", keys, true);
  window.addEventListener("blur", () => {
    // A Shift released in another app never reaches this window.
    shiftDown = false;
    press = null;
    if (internal) endInternal();
  });
  await ctx.webview.onDragDropEvent(onNativeDrag);
  await ctx.listen("drag://ended", onDragEnded);
}
