// The main window's side of Synchronize dirs: opens the window (sync.html, see syncwin.js) for
// the two panels' folders and runs the copy and delete jobs it asks for, so their progress,
// questions and panel refresh stay in the main window. Nothing is deleted without the window's
// own confirmation.

import { SYNC_ACTION, SYNC_REPLY, findFilesGeometry, isSearchLocation } from "./lib.mjs";
import { runSyncJobs } from "./dialogs.js";

const T = window.__TAURI__;

/** The window keeps its geometry here (syncwin.js writes it). */
export const SYNC_LAYOUT_KEY = "fxfm.syncDirs";
/** Main → window: compare these folders now (the window was already open). */
export const SYNC_COMMAND = "syncdirs://command";

let ctx = null;
/** The folders the window compares: {left, right}. */
let folders = null;

/** ctx: {invoke, panels: () => ({left, right}), status(text), focusMain()}. */
export async function initSync(c) {
  ctx = c;
  await T.event.listen(SYNC_ACTION, async (ev) => {
    const { id, action, args } = ev.payload ?? {};
    if (typeof id !== "string") return;
    const reply = (r) => T.event.emit(SYNC_REPLY, { id, ...r }).catch((e) => console.error("fxfm: sync reply", e));
    try {
      reply({ ok: true, value: await answer(action, args ?? {}) });
    } catch (e) {
      reply({ ok: false, error: String(e?.message ?? e) });
    }
  });
}

async function answer(action, args) {
  if (action === "context") return folders;
  if (action === "run") {
    if (!Array.isArray(args.jobs)) throw new Error("no jobs");
    ctx.focusMain();
    return runSyncJobs(args.jobs);
  }
  throw new Error(`unknown action ${action}`);
}

function savedGeometry() {
  try {
    return findFilesGeometry(JSON.parse(localStorage.getItem(SYNC_LAYOUT_KEY) ?? "null"));
  } catch {
    return null;
  }
}

/** Commands → Synchronize dirs: the window for the left and right panel's folders. */
export async function syncDirs() {
  const { left, right } = ctx.panels();
  if ([left.location, right.location].some((l) => !l || isSearchLocation(l))) {
    ctx.status("Synchronize dirs needs a folder in both panels");
    return;
  }
  if (left.location === right.location) {
    ctx.status("Both panels show the same folder");
    return;
  }
  folders = { left: left.location, right: right.location };
  let created;
  try {
    created = await ctx.invoke("open_sync_dirs", { geometry: savedGeometry() });
  } catch (e) {
    ctx.status(`Synchronize dirs: ${e?.message ?? e}`);
    return;
  }
  // Already open: it compares the panels' folders now.
  if (!created) T.event.emit(SYNC_COMMAND, folders).catch((e) => console.error("fxfm: sync command", e));
}
