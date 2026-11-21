// The main window's side of Find files (Alt+F7): the first-run question, opening or focusing the
// window (search.html, see search.js) and running what it asks for over the `findfiles://`
// bridge (FIND_FILES_ACTION in lib.mjs).

import {
  FIND_FILES_ACTION,
  FIND_FILES_COMMAND,
  FIND_FILES_REPLY,
  findFilesGeometry,
  findFilesReply,
  parseFindFilesAction,
} from "./lib.mjs";
import { dialog } from "./dialogs.js";

const T = window.__TAURI__;

const FIRST_RUN_TEXT =
  "fxfm can index this Mac's disk in the background so search finds files instantly. With Full Disk Access the index is complete; without it macOS may ask about some folders, and refused folders are skipped.";

/** The window keeps its geometry and columns here (search.js writes it). */
const LAYOUT_KEY = "fxfm.findFiles";

/** Kept for the session: the last query and options of the window. */
let memory = { query: "", scope: "everywhere", archives: false };
/** The first-run prompt was answered with Not now / Open Privacy settings in this session. */
let askedThisSession = false;
/** The Find files window is open (between opening it and its `closed`). */
let windowOpen = false;
/** The index state the next window opens with (the first-run answer), else null. */
let openState = null;
/** The rows and query of the last closed window, for "Feed to listbox". */
let last = { items: [], query: "" };

let ctx = null;

/** The result items and the query of the last closed Find files window. */
export function lastResults() {
  return { items: last.items.slice(), query: last.query };
}

/**
 * Commands → Feed search results to listbox: an open window feeds all its rows itself (true);
 * otherwise false and the caller uses `lastResults()`.
 */
export function feedFromWindow() {
  if (!windowOpen) return false;
  emitCommand({ cmd: "feedAll" });
  return true;
}

/** Options changed: the open window shows or hides *Inside archives*. */
export function searchSettingsChanged(settings) {
  if (windowOpen) emitCommand({ cmd: "settings", archives: Boolean(settings.search?.archives) });
}

function emitCommand(c) {
  T.event.emit(FIND_FILES_COMMAND, c).catch((e) => console.error("fxfm: findfiles command", e));
}

/** Starts building the index (first run). Returns false on failure. */
async function buildIndex() {
  try {
    await ctx.invoke("search_build");
    indexOn();
    return true;
  } catch (e) {
    ctx.status(`Cannot build the index: ${e?.message ?? e}`);
    return false;
  }
}

/** The backend saved `search.index = true`; keep the UI's copy in step. */
function indexOn() {
  const s = ctx.settings();
  if (s.search) s.search.index = true;
}

/**
 * The first-run prompt. Resolves with the index state to open the window with: "building"
 * after Build index, otherwise "notAsked" (Current folder, by walk); null to not open it.
 */
async function firstRun() {
  const r = await dialog({
    title: "Find files",
    text: FIRST_RUN_TEXT,
    buttons: [
      { label: "Open Privacy settings", value: "privacy" },
      { label: "Build index", value: "build", default: true },
      { label: "Not now", value: null },
    ],
  });
  if (r === "build") return (await buildIndex()) ? "building" : null;
  askedThisSession = true;
  if (r === "privacy") ctx.openPrivacy();
  return "notAsked";
}

function savedGeometry() {
  try {
    return findFilesGeometry(JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "null"));
  } catch {
    return null;
  }
}

/** Alt+F7: opens the Find files window, or brings the open one to the front. */
export async function openSearch() {
  if (!windowOpen) {
    let status;
    try {
      status = await ctx.invoke("search_status");
    } catch (e) {
      ctx.status(`Search: ${e?.message ?? e}`);
      return;
    }
    if (status.state === "notAsked" && !askedThisSession) {
      const next = await firstRun();
      if (!next) return;
      openState = next;
    }
  }
  let created;
  try {
    created = await ctx.invoke("open_find_files", { geometry: savedGeometry() });
  } catch (e) {
    ctx.status(`Find files: ${e?.message ?? e}`);
    return;
  }
  windowOpen = true;
  // Already open: it takes the active panel's folder as "Current folder".
  if (!created) emitCommand({ cmd: "focus", location: ctx.panel().location });
}

/** What the window asks for; resolves with the reply value. */
async function run(action, args) {
  switch (action) {
    case "context": {
      windowOpen = true;
      const state = openState;
      openState = null;
      const s = ctx.settings();
      return {
        home: ctx.home,
        location: ctx.panel().location,
        mods: s.modifiers ?? {},
        archives: Boolean(s.search?.archives),
        memory,
        state,
      };
    }
    case "go":
      await ctx.panel().go(String(args.folder ?? ""), args.cursor == null ? undefined : String(args.cursor));
      await ctx.focus();
      return null;
    case "feed":
      await ctx.feed(Array.isArray(args.items) ? args.items : [], String(args.query ?? ""));
      await ctx.focus();
      return null;
    case "play":
      await ctx.view(String(args.path ?? ""));
      return null;
    case "connect":
      // Questions and progress of mounting show here.
      await ctx.focus();
      return (await ctx.connectFlow({ server: String(args.server ?? "") }, { navigate: false })) ?? null;
    case "indexOn":
      indexOn();
      return null;
    case "closed":
      windowOpen = false;
      if (args.memory && typeof args.memory === "object") memory = { ...memory, ...args.memory };
      last = { items: Array.isArray(args.items) ? args.items : [], query: String(args.query ?? "") };
      return null;
    default:
      throw new Error(`unknown action ${action}`);
  }
}

/**
 * Subscribes to the window's requests. c: {invoke, home, settings(), panel() (the active
 * panel), status(text), feed(items, query), view(path) (F3 on a result), connectFlow,
 * openPrivacy(), focus() (the main window to the front)}.
 */
export async function initSearchHost(c) {
  ctx = c;
  await T.event.listen(FIND_FILES_ACTION, (ev) => {
    const req = parseFindFilesAction(ev.payload);
    if (!req) return;
    run(req.action, req.args).then(
      (value) => T.event.emit(FIND_FILES_REPLY, findFilesReply(req.id, value)),
      (e) => T.event.emit(FIND_FILES_REPLY, findFilesReply(req.id, null, e ?? "failed")),
    ).catch((e) => console.error("fxfm: findfiles reply", e));
  });
}
