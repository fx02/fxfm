// Main window: wires panels, keys, commands, volumes and the session.

import {
  remoteArchiveOf,
  acceptAudioEvent,
  audioActive,
  audioF3Action,
  cmdlineKeyGoesToPanel,
  comboFromEvent,
  comboLabel,
  compileTypeRules,
  displayLocation,
  discardPendingConfirm,
  escapeStopsAudio,
  isAudioName,
  isSearchLocation,
  joinLocation,
  keepExisting,
  lastComponent,
  groupThousands,
  modifierMap,
  quickSearchChar,
  quickSearchMode,
  NOT_IN_RESULTS,
  passwordArchive,
  panelFolder,
  pathsToCheck,
  pendingEditsDialog as pendingEditsSpec,
  playerStrip,
  rectOnScreen,
  remoteArchives,
  resultEntries,
  resultOpRefusal,
  remoteLinkIsFolder,
  SEARCH_LOCATION,
  seekSeconds,
  sessionLocation,
  textEditKey,
  diskFor,
  driveTarget,
  mergeShares,
  remoteRoot,
  serverDrives,
  serverTarget,
  setFnHeld,
  iconStyle,
  contextMenuItems,
  archiveOf,
  parseRemote,
  keyTableRows,
} from "./lib.mjs";
import {
  clipFiles,
  copyMove,
  copyMoveTo,
  copyNames,
  pasteFiles,
  deleteItems,
  dialog,
  editChanged,
  editNew,
  handleModalKey,
  initDialogs,
  makeDir,
  markByMask,
  packFiles,
  popModal,
  pushModal,
  askPassword,
  withPassword,
  renameInline,
  showTable,
  topModal,
} from "./dialogs.js";
import { cancelDrag, initDragDrop, rowPress } from "./dragdrop.js";
import { configureKeyboard, configureOptions, firstRunConflictHint, initConfig, optionsSearchReload, optionsSearchStatus } from "./config.js";
import { initHotlist, showHotlist } from "./hotlist.js";
import { initMultiRename, multiRename } from "./multirename.js";
import { initSync, syncDirs } from "./sync.js";
import {
  chmodDialog,
  connectFlow,
  disconnectActive,
  autoConnectShares,
  disconnectShare,
  initServers,
  lostServer,
  serversLost,
  showConnect,
} from "./servers.js";
import { Panel, VOLUME_ICON, volumeFor } from "./panel.js";
import { feedFromWindow, initSearchHost, lastResults, openSearch, searchSettingsChanged } from "./searchhost.js";

const T = window.__TAURI__;
const invoke = (cmd, args) => {
  const pending = (window.__fxfmPending ??= new Map());
  const id = Symbol(cmd);
  pending.set(id, cmd);
  return T.core.invoke(cmd, args).finally(() => pending.delete(id));
};
const win = T.window.getCurrentWindow();

const app = {
  boot: null,
  settings: null,
  keymap: {},
  commands: new Map(),
  panels: {},
  activeSide: "left",
  showHidden: true,
  /** Drives: `list_volumes` merged with the saved SMB shares (`mergeShares`). */
  volumes: [],
  /** The last `list_volumes` and `smb_shares` answers. */
  rawVolumes: [],
  shares: [],
  ready: false,
  geom: null,
  scale: 1,
  /** "Continue without settings" after App Translocation: the session is not saved. */
  noSession: false,
  /**
   * "Feed to listbox": side -> {entries, query, back: {location, cursorName}, recheck} for a
   * panel showing SEARCH_LOCATION. `recheck` drops vanished items at the next listing.
   */
  results: {},
};

const other = (side) => (side === "left" ? "right" : "left");
/** Which Mac keys act as TC's Alt and Ctrl (Configuration → Keyboard). */
const keyMods = () => modifierMap(app.settings);
const activePanel = () => app.panels[app.activeSide];

/** Message in the active panel's status line (until the cursor moves). */
export function status(text) {
  activePanel()?.setMessage(text);
}

// Later windows/tasks import the shared modal dialog and the connect flow from here.
export { connectFlow, dialog, withPassword };

/** Volume chooser next to the panel's drive combo (Alt+F1 / Alt+F2). */
/** Last folder visited on each disk (disk path -> location), for picking a drive (TC). */
const lastOnDisk = new Map();

/**
 * Drive bar or drive menu: a disk goes to its current folder (see `driveTarget`); a saved SMB
 * share that is not mounted is mounted first and opens at its folder.
 */
function goDrive(panel, path) {
  const vol = app.volumes.find((v) => v.path === path);
  if (!vol) return panel.go(path); // a server connection button: its root
  if (vol.smb && !vol.mounted) return connectFlow({ server: vol.smb }, { panel });
  const otherPanel = app.panels[other(panel.side)];
  return panel.go(
    driveTarget(vol, { volumes: app.volumes, here: panel.location, other: otherPanel.location, last: lastOnDisk }),
  );
}

/**
 * A saved server from the drive menu: a connected one goes to its current folder (see
 * `serverTarget`); otherwise it connects and opens the server's start folder in this panel.
 */
function goServer(panel, drive) {
  const otherPanel = app.panels[other(panel.side)];
  const to = serverTarget(drive, { here: panel.location, other: otherPanel.location, last: lastOnDisk });
  if (to) return panel.go(to);
  return connectFlow({ server: drive.server }, { panel, localDir: false });
}

/** Drive menu rows: the drives, then the saved SFTP/SCP/FTP servers (connection manager order). */
async function driveRows() {
  try {
    const [list, conns] = await Promise.all([invoke("servers_load"), invoke("list_connections")]);
    return [...app.volumes, ...serverDrives(list.nodes, conns)];
  } catch (e) {
    console.error("fxfm: drive menu servers", e);
    return app.volumes;
  }
}

async function driveMenu(panel) {
  const rows = await driveRows();
  if (!rows.length) return;
  const backdrop = document.createElement("div");
  backdrop.className = "backdrop clear";
  const menu = document.createElement("div");
  menu.className = "popup";
  // Without the panel header the menu opens at the panel's top-left corner.
  const c = panel.comboEl.getBoundingClientRect();
  const r = c.width ? c : panel.root.getBoundingClientRect();
  menu.style.left = `${r.left}px`;
  menu.style.top = `${c.width ? r.bottom + 2 : r.top}px`;
  const here = remoteRoot(panel.location);
  const at = here ? rows.findIndex((v) => v.kind === "server" && v.path === here) : rows.indexOf(volumeFor(app.volumes, panel.location));
  let sel = Math.max(0, at);
  const items = rows.map((v, i) => {
    const it = document.createElement("div");
    it.className = "item";
    it.textContent = `${VOLUME_ICON[v.kind] ?? "💾"} ${v.name}`;
    if (v.smb) it.title = v.mounted ? `${v.path} (Delete: disconnect)` : "Not mounted: picking it mounts the share";
    if (v.kind === "server") it.title = v.connected ? "Connected" : "Not connected: picking it connects";
    it.addEventListener("mouseenter", () => paint(i));
    it.addEventListener("click", (e) => {
      e.stopPropagation();
      choose(i);
    });
    menu.append(it);
    return it;
  });
  const modal = { el: backdrop };
  const paint = (i) => {
    sel = i;
    items.forEach((it, j) => it.classList.toggle("sel", j === i));
  };
  const close = () => popModal(modal);
  const choose = (i) => {
    close();
    setActive(panel.side);
    if (rows[i].kind === "server") goServer(panel, rows[i]);
    else goDrive(panel, rows[i].path);
  };
  modal.onKey = (e) => {
    e.preventDefault();
    if (e.key === "ArrowDown") paint((sel + 1) % items.length);
    else if (e.key === "ArrowUp") paint((sel - 1 + items.length) % items.length);
    else if (e.key === "Home") paint(0);
    else if (e.key === "End") paint(items.length - 1);
    else if (e.key === "Enter") choose(sel);
    else if (e.key === "Delete" && rows[sel]?.smb && rows[sel].mounted) {
      // Disconnect a mounted SMB share (diskutil unmount) without leaving the keyboard.
      close();
      disconnectShare(rows[sel].smb);
    }
    else if (e.key === "Escape" || e.key === "Tab") close();
  };
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) close();
  });
  backdrop.append(menu);
  paint(sel);
  pushModal(modal);
}

// ---------------------------------------------------------------- context menu

/**
 * Right click on a row (or Shift+F10 on the cursor row): Open, Copy name/path, Cut/Copy/Paste,
 * Delete, Rename for the marked items, or only that row when it is not marked (as in Finder);
 * on empty space or `..` only Paste. `at`: the pointer, else below the cursor row.
 */
function contextMenu(panel, index, at = null) {
  if (topModal()) return;
  setActive(panel.side);
  const entry = index == null ? null : panel.entries[index] ?? null;
  if (entry && !entry.up && !panel.marked.has(entry.name) && panel.marked.size) {
    panel.clearMarks();
    panel.render();
  }
  const n = panel.marked.size || 1;
  const path = entry && !entry.up ? (entry.path ?? joinLocation(panel.location, entry.name)) : null;
  // Open with / Quick Look / Show in Finder need a plain local file (not a server, not inside an
  // archive: an archive entry's path does not exist on disk).
  const local = Boolean(path) && !parseRemote(path) && !entry.inArchive && !archiveOf(panel.location);
  const own = {
    open: () => panel.enter(),
    open_with: () => openWithMenu(path, at),
    quick_look: () => invoke("quick_look", { path }).catch((e) => status(`Quick Look: ${e}`)),
    show_in_finder: () => invoke("show_in_finder", { path }).catch((e) => status(`Show in Finder: ${e}`)),
  };
  const items = contextMenuItems(entry, n, app.settings.contextMenuOff ?? [], local).map((i) =>
    i === "-" ? null : { label: i[1], run: () => (own[i[0]] ? own[i[0]]() : dispatch(i[0])) },
  );
  if (!items.length) return;
  if (!at) {
    panel.ensureVisible();
    const row = panel.rowsEl.querySelector(`[data-i="${index}"]`)?.getBoundingClientRect();
    const box = row ?? panel.listEl.getBoundingClientRect();
    at = { x: box.left + 24, y: row ? row.bottom : box.top };
  }
  popupMenu(at, items);
}

/** Open with…: the apps macOS offers for `path` (default first) as a second menu at `at`. */
async function openWithMenu(path, at) {
  const apps = await invoke("open_with_apps", { path }).catch(() => []);
  if (!apps.length) {
    status("No application to open it with");
    return;
  }
  const items = apps.map((a, i) => ({
    label: i === 0 ? `${a.name} (default)` : a.name,
    run: () => invoke("open_with", { path, app: a.path }).catch((e) => status(`Open with: ${e}`)),
  }));
  popupMenu(at, items);
}

/** Alt+Down: the panel's earlier folders, newest first; picking one goes there. */
function historyMenu(panel) {
  if (!panel.back.length) {
    status("No earlier folders");
    return;
  }
  const items = panel.back
    .map((h, i) => ({ label: displayLocation(h.location, remoteArchives), run: () => panel.goBackTo(i) }))
    .reverse();
  const box = panel.listEl.getBoundingClientRect();
  popupMenu({ x: box.left + 24, y: box.top }, items);
}

/** A small menu at `at` ({x, y}, kept inside the window); `items`: {label, run} or null (a line). */
function popupMenu(at, items) {
  const backdrop = document.createElement("div");
  backdrop.className = "backdrop clear";
  const menu = document.createElement("div");
  menu.className = "popup";
  const rows = [];
  for (const it of items) {
    if (!it) {
      menu.append(Object.assign(document.createElement("div"), { className: "sep" }));
      continue;
    }
    const i = rows.length;
    const row = document.createElement("div");
    row.className = "item";
    row.textContent = it.label;
    row.addEventListener("mouseenter", () => paint(i));
    row.addEventListener("click", (e) => {
      e.stopPropagation();
      choose(i);
    });
    menu.append(row);
    rows.push({ row, it });
  }
  let sel = 0;
  const paint = (i) => {
    sel = i;
    rows.forEach((r, j) => r.row.classList.toggle("sel", j === i));
  };
  const modal = { el: backdrop };
  const close = () => popModal(modal);
  const choose = (i) => {
    close();
    rows[i].it.run();
  };
  modal.onKey = (e) => {
    e.preventDefault();
    if (e.key === "ArrowDown") paint((sel + 1) % rows.length);
    else if (e.key === "ArrowUp") paint((sel - 1 + rows.length) % rows.length);
    else if (e.key === "Home") paint(0);
    else if (e.key === "End") paint(rows.length - 1);
    else if (e.key === "Enter") choose(sel);
    else if (e.key === "Escape" || e.key === "Tab") close();
  };
  const dismiss = (e) => {
    if (e.target !== backdrop) return;
    e.preventDefault();
    close();
  };
  backdrop.addEventListener("mousedown", dismiss);
  backdrop.addEventListener("contextmenu", (e) => e.preventDefault());
  backdrop.append(menu);
  paint(0);
  pushModal(modal);
  // Inside the window: flip left / up when it would stick out.
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  menu.style.left = `${Math.max(0, at.x + w > innerWidth ? at.x - w : at.x)}px`;
  menu.style.top = `${Math.max(0, at.y + h > innerHeight ? at.y - h : at.y)}px`;
}

// ---------------------------------------------------------------- commands

function setActive(side) {
  app.activeSide = side;
  for (const s of ["left", "right"]) app.panels[s].setActive(s === side);
  updatePrompt();
  scheduleSave();
}

/** Command handlers; later tasks add entries (dispatch looks them up here). */
export const handlers = {
  cm_switch_panel: () => setActive(other(app.activeSide)),
  // In the results list Ctrl+PgUp goes to the cursor item's folder; Backspace goes back.
  cm_parent: (combo) =>
    combo === "Ctrl+PageUp" && inResults() ? activePanel().goToResult() : activePanel().goParent(),
  cm_root: () => activePanel().goRoot(),
  cm_mark: () => activePanel().toggleMark(),
  cm_mark_mask: () => markByMask(true),
  cm_unmark_mask: () => markByMask(false),
  cm_copy: () => copyMove("copy"),
  cm_move: () => copyMove("move"),
  cm_packfiles: () => notInResults() || packFiles(),
  cm_delete: () => deleteItems(true),
  cm_delete_permanent: () => deleteItems(false),
  cm_mkdir: () => notInResults() || makeDir(),
  cm_rename: () => notInResults() || renameInline(),
  cm_view: () => viewOrPlay(),
  cm_edit: () => viewOrEdit("edit_file"),
  cm_edit_new: () => notInResults() || editNew(),
  cm_invert: () => activePanel().invertMarks(),
  cm_mark_all: () => activePanel().markAll(),
  cm_refresh: () => {
    refreshVolumes();
    // A results list drops the items that no longer exist.
    recheckResults(app.activeSide);
    return activePanel().refresh();
  },
  /** The other panel goes to the active panel's folder (or shows the same results); focus stays. */
  cm_target_equal_source: () => {
    const p = activePanel();
    const o = app.panels[other(app.activeSide)];
    if (inResults()) {
      const r = app.results[p.side];
      app.results[o.side] = { ...r, entries: r.entries.slice(), back: { ...r.back }, recheck: false };
      o.clearMarks();
    }
    return o.go(p.location);
  },
  cm_swap_panels: () => {
    const { left, right } = app.panels;
    // Results lists move with their panels' locations.
    const [l0, r0] = [app.results.left, app.results.right];
    setResults("left", r0);
    setResults("right", l0);
    const l = { location: left.location, cursor: left.cursorName ?? undefined };
    const r = { location: right.location, cursor: right.cursorName ?? undefined };
    // Both show results: the location stays the same, so marks (by name) would stay too.
    if (isSearchLocation(l.location) && isSearchLocation(r.location)) {
      left.clearMarks();
      right.clearMarks();
    }
    left.go(r.location, r.cursor);
    right.go(l.location, l.cursor);
  },
  cm_toggle_hidden: () => {
    app.showHidden = !app.showHidden;
    for (const p of Object.values(app.panels)) p.rebuild();
    scheduleSave();
  },
  cm_drive_left: () => driveMenu(app.panels.left),
  cm_drive_right: () => driveMenu(app.panels.right),
  cm_hotlist: () => showHotlist(),
  cm_connect: () => showConnect(),
  cm_disconnect: () => disconnectActive(),
  cm_chmod: () => chmodDialog(),
  cm_help_keys: () =>
    showTable(
      "Keyboard shortcuts",
      ["Menu", "Command", "Keys"],
      keyTableRows([...app.commands.values()], app.keymap, (k) => comboLabel(k, keyMods())),
    ),
  cm_context_menu: () => contextMenu(activePanel(), activePanel().cursorIndex),
  cm_quick_filter: () => activePanel().openFilter(),
  cm_back: () => activePanel().goHistory(-1),
  cm_forward: () => activePanel().goHistory(1),
  cm_history_list: () => historyMenu(activePanel()),
  cm_cut_to_clipboard: () => clipFiles("move"),
  cm_copy_to_clipboard: () => clipFiles("copy"),
  cm_paste_from_clipboard: () => pasteFiles(),
  cm_copy_names_to_clip: () => copyNames(false),
  cm_copy_full_names_to_clip: () => copyNames(true),
  cm_multi_rename: () => multiRename(),
  cm_sync_dirs: () => syncDirs(),
  cm_exit: () => win.close(),
  cm_cmdline_focus: () => {
    if (app.settings.showCommandLine) cmdInput().focus();
  },
  cm_config_keyboard: () => configureKeyboard(),
  cm_config_options: () => configureOptions(),
  cm_search: () => openSearch(),
  /** The open Find files window's results, else the last one's, else the window itself. */
  cm_feed_listbox: () => {
    if (feedFromWindow()) return;
    const { items, query } = lastResults();
    return items.length ? feed(items, query) : openSearch();
  },
};

/** The active panel shows search results. */
const inResults = () => isSearchLocation(activePanel().location);

/** In a results list: says "not available" and returns true; else false (the command runs). */
function notInResults() {
  if (!inResults()) return false;
  status(NOT_IN_RESULTS);
  return true;
}

/** Gives `side` the results list `r`; none keeps what it has until it navigates away. */
function setResults(side, r) {
  if (r) app.results[side] = r;
}

/**
 * Enter on an archive on a server: downloads it with a progress note (after 300 ms) that has
 * Cancel; resolves with the local copy's path.
 */
async function downloadArchive(location) {
  const backdrop = document.createElement("div");
  backdrop.className = "backdrop";
  const box = document.createElement("div");
  box.className = "dialog busy";
  const title = document.createElement("div");
  title.className = "title";
  title.textContent = "Download";
  const body = document.createElement("div");
  body.className = "body";
  const line = document.createElement("p");
  const name = lastComponent(location);
  line.textContent = `Downloading ${name}…`;
  const bar = document.createElement("div");
  bar.className = "dlbar";
  const fill = document.createElement("div");
  bar.append(fill);
  body.append(line, bar);
  const row = document.createElement("div");
  row.className = "buttons";
  const cancel = document.createElement("button");
  cancel.textContent = "Cancel";
  const stop = () => {
    cancel.disabled = true;
    invoke("cancel_download", { location }).catch((e) => console.error("fxfm: cancel_download", e));
  };
  cancel.addEventListener("click", stop);
  row.append(cancel);
  box.append(title, body, row);
  backdrop.append(box);
  const modal = {
    el: backdrop,
    onKey: (e) => {
      e.preventDefault();
      if (e.key === "Escape") stop();
    },
  };
  const timer = setTimeout(() => pushModal(modal), 300);
  const unlisten = await T.event.listen("download://progress", (ev) => {
    const p = ev.payload;
    if (p.location !== location) return;
    const pct = p.total ? Math.min(100, Math.floor((p.done / p.total) * 100)) : null;
    line.textContent = `Downloading ${name}… ${groupThousands(p.done)} / ${groupThousands(p.total)} bytes`;
    fill.style.width = `${pct ?? 0}%`;
  });
  try {
    return await invoke("open_remote_archive", { location });
  } finally {
    clearTimeout(timer);
    unlisten();
    popModal(modal);
  }
}

/** Items of `side`'s results list renamed by Multi-rename: `moved` maps old to new paths. */
function renamedInResults(side, moved) {
  const r = app.results[side];
  if (!r) return;
  r.entries = r.entries.map((e) => {
    const now = moved.get(e.path);
    return now ? { ...resultEntries([{ ...e, path: now }], app.boot.home)[0] } : e;
  });
}

/** The next listing of `side`'s results list (if it shows one) drops the vanished items. */
function recheckResults(side) {
  const r = app.results[side];
  if (r && isSearchLocation(app.panels[side].location)) r.recheck = true;
}

/**
 * "Feed to listbox": the Find files results become a list in the active panel. Feeding again
 * from a results list keeps the way back to the folder it came from.
 */
function feed(items, query) {
  const p = activePanel();
  const prev = isSearchLocation(p.location) ? app.results[p.side] : null;
  const back = prev?.back ?? { location: p.location, cursorName: p.cursorName };
  const entries = resultEntries(items, app.boot.home);
  app.results[p.side] = { entries, query, back, recheck: false };
  // The same location again keeps marks by name: a new list starts unmarked.
  p.clearMarks();
  return p.go(SEARCH_LOCATION, entries[0]?.name);
}

/** The panel API's listing of the results list; a pending recheck asks `paths_exist` first. */
async function resultsListing(side) {
  const r = app.results[side];
  if (!r) throw new Error("No search results");
  if (r.recheck) {
    r.recheck = false;
    try {
      r.entries = keepExisting(r.entries, await invoke("paths_exist", { paths: pathsToCheck(r.entries) }));
    } catch (e) {
      console.error("fxfm: paths_exist", e);
    }
  }
  return r.entries;
}

/** Panel `p`'s folder for the command line and the hotlist: a results list uses the one it came from. */
const workFolder = (p) => panelFolder(p.location, app.results[p.side] ?? null, app.boot.home);

/** What the Find files window asks of the main window (searchhost.js). `view` is the panel's F3 path. */
function searchCtx() {
  return {
    invoke,
    home: app.boot.home,
    settings: () => app.settings,
    panel: activePanel,
    status,
    feed,
    view: (path) => viewOrPlayPath(path, lastComponent(path)),
    connectFlow,
    openPrivacy: openFdaSettings,
    focus: () => win.setFocus(),
  };
}

const cmdInput = () => document.getElementById("cmdinput");

/** Runs the command line's text in Terminal in the active panel's folder; false when it is empty. */
function runCommandLine() {
  const input = cmdInput();
  const command = input.value.trim();
  if (!app.settings.showCommandLine || !command) return false;
  input.value = "";
  invoke("run_in_terminal", { dir: workFolder(activePanel()), command }).catch((e) => status(`Command line: ${e}`));
  return true;
}

/** F3/F4: act on the file under the cursor; folders and [..] do nothing. */
function viewOrEdit(command) {
  const p = activePanel();
  const e = p.cursorEntry;
  if (!e || e.up || e.isDir) return;
  if (refusedInResults(p, e, "edit")) return;
  const path = e.path ?? joinLocation(p.location, e.name);
  return withPassword(() => invoke(command, { path }));
}

/** A results list refusing `op` on entry `e` (says why); false elsewhere. */
function refusedInResults(p, e, op) {
  if (!isSearchLocation(p.location)) return false;
  const why = resultOpRefusal([e], op);
  if (why) p.setMessage(why);
  return Boolean(why);
}

// ---------------------------------------------------------------- F3 audio

/**
 * The player strip's state (from `audio://state`). `lastId` is the newest request id seen;
 * events older than `minId` (set at each F3) are dropped.
 */
const player = {
  path: null, name: "", state: null, position: 0, duration: null, info: null, message: null,
  lastId: 0, minId: 0,
};
let playerHideTimer = 0;
/** An error stays on the strip this long. */
const PLAYER_ERROR_MS = 5000;

/** F3: audio files play in place (focus stays in the panel), anything else opens the viewer. */
function viewOrPlay() {
  const p = activePanel();
  const e = p.cursorEntry;
  if (!e || e.up || e.isDir) return;
  if (refusedInResults(p, e, "view")) return;
  if (e.path) return viewOrPlayPath(e.path, lastComponent(e.path));
  return viewOrPlayPath(joinLocation(p.location, e.name), e.name);
}

/** F3 on the file `path` (`name` is its file name): play it when it is audio, else view it. */
function viewOrPlayPath(path, name) {
  if (!isAudioName(name)) return withPassword(() => invoke("open_viewer", { path }));
  if (audioF3Action(player, path) === "stop") return invoke("audio_stop");
  // Ticks of the file playing until now may still be on their way: ignore them.
  Object.assign(player, { path, name, info: null, minId: player.lastId + 1 });
  return withPassword(() => invoke("audio_play", { path }))
    .then((started) => {
      player.minId = Math.max(player.minId, started.id);
      player.lastId = Math.max(player.lastId, started.id);
      if (player.path !== path) return;
      player.info ??= started;
      renderPlayer();
    })
    .catch((err) => {
      // Rust reported the failure on the strip already; a cancelled password prompt or a
      // newer F3 needs nothing.
      const msg = String(err?.message ?? err);
      if (msg === "superseded" || passwordArchive(msg) || player.path !== path) return;
      if (player.state === "error") return;
      const message = /^(Cannot play|Stopped)\b/.test(msg) ? msg : `Cannot play ${name}: ${msg}`;
      setPlayer({ path, state: "error", message });
    });
}

function setPlayer(ev) {
  if (!acceptAudioEvent(player, ev)) return;
  if (ev.id != null) player.lastId = Math.max(player.lastId, ev.id);
  if (ev.path && ev.path !== player.path) {
    player.path = ev.path;
    player.name = lastComponent(ev.path);
    player.info = null;
  }
  if (ev.info) player.info = ev.info;
  Object.assign(player, {
    state: ev.state,
    position: ev.position ?? 0,
    duration: ev.duration ?? null,
    message: ev.message ?? null,
  });
  clearTimeout(playerHideTimer);
  if (ev.state === "error") {
    playerHideTimer = setTimeout(() => {
      if (player.state === "error") setPlayer({ state: null });
    }, PLAYER_ERROR_MS);
  }
  renderPlayer();
}

function renderPlayer() {
  const strip = document.getElementById("player");
  const v = playerStrip(player);
  strip.classList.toggle("off", !v);
  if (!v) return;
  strip.classList.toggle("error", !!v.error);
  strip.querySelector(".name").textContent = v.text;
  strip.querySelector(".clock").textContent = v.clock;
  strip.querySelector(".codec").textContent = v.codec;
  const pp = strip.querySelector(".pp");
  pp.classList.toggle("off", player.state !== "playing" && player.state !== "paused");
  pp.textContent = v.paused ? "▶" : "⏸";
  pp.title = v.paused ? "Play" : "Pause";
  strip.classList.toggle("paused", Boolean(v.paused));
  const bar = strip.querySelector(".bar");
  bar.classList.toggle("off", v.progress == null);
  bar.firstElementChild.style.width = `${(v.progress ?? 0) * 100}%`;
}

function initPlayer() {
  const strip = document.getElementById("player");
  // Clicks must not move keyboard focus (the panel keeps it).
  strip.addEventListener("mousedown", (e) => e.preventDefault());
  strip.querySelector(".close").addEventListener("click", () => {
    if (audioActive(player)) invoke("audio_stop").catch((e) => console.error("fxfm: audio_stop", e));
    else setPlayer({ state: null });
  });
  strip.querySelector(".pp").addEventListener("click", () => {
    if (player.state !== "playing" && player.state !== "paused") return;
    invoke("audio_pause", { paused: player.state === "playing" }).catch((e) => console.error("fxfm: audio_pause", e));
  });
  // Volume: live while dragging, saved when let go.
  const vol = strip.querySelector(".vol");
  vol.value = String(Math.round((app.settings.audioVolume ?? 1) * 100));
  vol.addEventListener("input", () => {
    invoke("audio_volume", { volume: Number(vol.value) / 100 }).catch((e) => console.error("fxfm: audio_volume", e));
  });
  vol.addEventListener("change", () => {
    const settings = { ...structuredClone(app.settings), audioVolume: Number(vol.value) / 100 };
    invoke("save_settings", { settings })
      .then(() => {
        app.settings = settings;
      })
      .catch((e) => console.error("fxfm: save volume", e));
  });
  strip.querySelector(".bar").addEventListener("click", (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    const secs = seekSeconds(e.clientX, r.left, r.width, player.duration);
    if (secs != null && (player.state === "playing" || player.state === "paused")) {
      invoke("audio_seek", { secs }).catch((err) => console.error("fxfm: audio_seek", err));
    }
  });
}

/** Runs command `id`; `combo` is the key that asked for it (none from menus and buttons). */
export function dispatch(id, combo = null) {
  const h = handlers[id];
  if (h) {
    const fail = (e) => status(`${id}: ${e?.message ?? e}`);
    try {
      Promise.resolve(h(combo)).catch(fail);
    } catch (e) {
      fail(e);
    }
    return;
  }
  const c = app.commands.get(id);
  if (!c) status(`Unknown command: ${id}`);
  else if (!c.available) status(`${c.title}: coming in a later version`);
  else status(`${c.title}: coming soon`);
}

function onKeyDown(e) {
  // Esc ends a drag between the panels before anything else sees it.
  if (e.key === "Escape" && cancelDrag()) {
    e.preventDefault();
    return;
  }
  if (handleModalKey(e)) return;
  const mods = keyMods();
  const t = e.target;
  if (t instanceof HTMLElement && t.matches("input, textarea, select")) {
    if (textEditKey(e, mods)) {
      // Command+A/C/V/X/Z edit the text even while Command is Alt or Ctrl.
    } else if (e.key === "Escape") {
      // Back to the panels (they take keys whenever no field has focus).
      e.preventDefault();
      t.blur();
    } else if (t === cmdInput() && e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      // TC: after running, keys go to the panels again.
      if (runCommandLine()) t.blur();
    } else if (t === cmdInput() && cmdlineKeyGoesToPanel(comboFromEvent(e, mods))) {
      // TC: F-keys, Ctrl/Alt commands and Up/Down keep working while typing.
      const combo = comboFromEvent(e, mods);
      const id = app.keymap[combo];
      if (id) {
        e.preventDefault();
        if (id === "cm_switch_panel") t.blur();
        dispatch(id, combo);
      } else if (activePanel().handleKey(combo)) {
        e.preventDefault();
      }
    } else if (app.keymap[comboFromEvent(e, mods)] === "cm_switch_panel") {
      // TC: Tab switches panels even from the command line.
      e.preventDefault();
      t.blur();
      dispatch("cm_switch_panel");
    }
    return;
  }
  const combo = comboFromEvent(e, mods);
  if (quickSearchKey(e, combo)) {
    e.preventDefault();
    return;
  }
  if (!combo) return;
  if (combo === "Escape" && escapeStopsAudio({ player, escapeBound: !!app.keymap[combo], commandText: cmdInput().value })) {
    e.preventDefault();
    invoke("audio_stop").catch((err) => console.error("fxfm: audio_stop", err));
    return;
  }
  // TC: with text in the command line, Enter runs it; otherwise Enter acts on the panel.
  if (combo === "Enter" && runCommandLine()) {
    e.preventDefault();
    return;
  }
  const id = app.keymap[combo];
  if (id) {
    e.preventDefault();
    dispatch(id, combo);
  } else if (activePanel().handleKey(combo)) {
    e.preventDefault();
  }
}

/**
 * Quick search (TC Alt+letters) in the active panel. True when the key was used: a quick
 * search character, or Esc/Backspace/Up/Down while the box is open. Any other key closes
 * the box and goes on as usual (Enter opens the found entry). Bound keys win.
 */
function quickSearchKey(e, combo) {
  const p = activePanel();
  if (["Meta", "Control", "Alt", "Shift", "CapsLock"].includes(e.key)) return false;
  if (p.quickSearching) {
    if (e.key === "Escape") return p.quickSearchEnd(), true;
    if (e.key === "Backspace") return p.quickSearchBack(), true;
    if (e.key === "ArrowUp" && !e.shiftKey) return p.quickSearchStep(-1), true;
    if (e.key === "ArrowDown" && !e.shiftKey) return p.quickSearchStep(1), true;
  }
  const ch = e.isComposing ? null : quickSearchChar(e, quickSearchMode(app.settings));
  if (ch != null && !(combo && app.keymap[combo])) {
    p.quickSearchType(ch);
    return true;
  }
  if (p.quickSearching) p.quickSearchEnd();
  return false;
}

// ---------------------------------------------------------------- session

let saveTimer = 0;

function sessionState() {
  return {
    left: panelState(app.panels.left),
    right: panelState(app.panels.right),
    active: app.activeSide,
    window: app.geom,
    showHidden: app.showHidden,
  };
}

/**
 * Session changed: Rust keeps it in memory right away (written on exit, so
 * Cmd+Q never loses the last cursor move) and it goes to disk after 500 ms.
 */
function scheduleSave() {
  if (!app.ready || app.noSession) return;
  invoke("set_session", { session: sessionState() }).catch((e) => console.error("fxfm: set_session", e));
  clearTimeout(saveTimer);
  saveTimer = setTimeout(saveSession, 500);
}

async function captureGeometry() {
  try {
    app.scale = await win.scaleFactor();
    const pos = (await win.outerPosition()).toLogical(app.scale);
    const size = (await win.innerSize()).toLogical(app.scale);
    setGeometry(pos, size);
  } catch (e) {
    console.error("fxfm: window geometry", e);
  }
}

/** Updates the cached logical geometry (either part may be omitted). */
function setGeometry(pos, size) {
  const g = app.geom ?? { x: 0, y: 0, w: 0, h: 0 };
  if (pos) Object.assign(g, { x: Math.round(pos.x), y: Math.round(pos.y) });
  if (size && size.width > 0 && size.height > 0) {
    Object.assign(g, { w: Math.round(size.width), h: Math.round(size.height) });
  }
  if (g.w > 0 && g.h > 0) app.geom = g;
}

/**
 * The temp copy of a server archive does not survive the session: the server folder is saved.
 * Nor does a results list: the folder it came from is saved.
 */
function panelState(p) {
  return sessionLocation(p, app.results[p.side] ?? null, remoteArchives);
}

async function saveSession() {
  clearTimeout(saveTimer);
  if (app.noSession) return;
  try {
    await invoke("save_session", { session: sessionState() });
  } catch (e) {
    console.error("fxfm: save_session", e);
  }
}

/** Logical rects of the connected monitors, or null when they can't be read. */
async function monitorRects() {
  try {
    let list = await T.window.availableMonitors();
    // availableMonitors() can come back empty on macOS; the primary and
    // current monitors are still reported then.
    if (!list.length) {
      list = [await T.window.primaryMonitor(), await T.window.currentMonitor()].filter(Boolean);
    }
    if (!list.length) return null;
    return list.map((m) => {
      const pos = m.position.toLogical(m.scaleFactor);
      const size = m.size.toLogical(m.scaleFactor);
      return { x: pos.x, y: pos.y, w: size.width, h: size.height };
    });
  } catch (e) {
    console.error("fxfm: monitors", e);
    return null;
  }
}

/** Restores the saved rect if it is still on a connected monitor, else centers the default size. */
async function restoreGeometry(g) {
  if (!g || g.w < 200 || g.h < 150) return;
  const monitors = await monitorRects();
  try {
    if (monitors && !rectOnScreen(g, monitors)) {
      await win.center();
      return;
    }
    await win.setSize(new T.window.LogicalSize(g.w, g.h));
    await win.setPosition(new T.window.LogicalPosition(g.x, g.y));
  } catch (e) {
    console.error("fxfm: restore geometry", e);
  }
}

// ---------------------------------------------------------------- volumes, errors

let volumesInFlight = false;
/** Another refresh was asked for while one ran (a share was just mounted, say). */
let volumesAgain = false;

/** Re-reads volumes (free space changes) and SMB mounts; one request at a time. */
async function refreshVolumes() {
  if (volumesInFlight) {
    volumesAgain = true;
    return;
  }
  volumesInFlight = true;
  try {
    do {
      volumesAgain = false;
      try {
        const [vols, shares] = await Promise.all([invoke("list_volumes"), readShares()]);
        setVolumes(vols, shares);
      } catch (e) {
        console.error("fxfm: list_volumes", e);
      }
    } while (volumesAgain);
  } finally {
    volumesInFlight = false;
  }
}

/** Saved SMB shares and their mounts; the last known ones when that fails. */
async function readShares() {
  try {
    return await invoke("smb_shares");
  } catch (e) {
    console.error("fxfm: smb_shares", e);
    return app.shares;
  }
}

function setVolumes(vols, shares = app.shares) {
  app.rawVolumes = vols;
  app.shares = shares;
  app.volumes = mergeShares(vols, shares);
  for (const p of Object.values(app.panels)) p.setVolumes(app.volumes);
}

/** Drive-bar buttons of the open connections (`list_connections`). */
async function refreshConnections() {
  try {
    const conns = await invoke("list_connections");
    for (const p of Object.values(app.panels)) p.setConnections(conns);
  } catch (e) {
    console.error("fxfm: list_connections", e);
  }
}

async function fdaHint() {
  if (app.settings.fdaHintShown) return;
  app.settings.fdaHintShown = true;
  invoke("save_settings", { settings: app.settings }).catch((e) => console.error("fxfm: save_settings", e));
  const r = await dialog({
    title: "Full Disk Access",
    text: [
      "fxfm needs Full Disk Access to open some folders (Mail, Messages, Safari, other apps' data).",
      "Turn on fxfm in System Settings → Privacy & Security → Full Disk Access, then restart fxfm.",
    ],
    buttons: [
      { label: "Open Settings", value: "open", default: true },
      { label: "Later", value: null },
    ],
  });
  if (r === "open") openFdaSettings();
}

/** System Settings → Privacy & Security → Full Disk Access. */
function openFdaSettings() {
  invoke("open_fda_settings").catch((e) => status(String(e)));
}

function onPanelError(panel, error, target) {
  // A listing that needs a password: ask, then list again (a wrong one asks again via this same path).
  if (passwordArchive(error?.message ?? error)) {
    askPassword(error)
      .then((ok) => (ok ? panel.go(target) : panel.setMessage(String(error?.message ?? error))))
      .catch((e) => panel.setMessage(String(e)));
    return;
  }
  const loc = error?.path ?? target;
  // A dead connection: Reconnect / Close (once per server, however many panels report it).
  const lost = lostServer(error?.path ?? error?.message ?? error, target, panel.location);
  if (lost) serversLost([lost], { panel, target });
  const text = {
    permissionDenied: `Access denied: ${loc}`,
    notFound: `Not found: ${loc}`,
    notADirectory: `Not a folder: ${loc}`,
    io: `Cannot read: ${loc}`,
  }[error?.kind] ?? `Cannot open ${target}: ${error}`;
  panel.setMessage(text);
  if (error?.kind === "permissionDenied") fdaHint();
}

// ---------------------------------------------------------------- setup

function applySettings(s) {
  const keysChanged = JSON.stringify(app.settings?.modifiers) !== JSON.stringify(s.modifiers);
  app.settings = s;
  // Open viewer windows take the new Alt/Ctrl keys at once.
  if (keysChanged) T.event.emit("keys://modifiers", s.modifiers ?? {}).catch((e) => console.error("fxfm: emit keys", e));
  searchSettingsChanged(s);
  const st = document.documentElement.style;
  st.setProperty("--c-font", s.colors.font);
  st.setProperty("--c-background", s.colors.background);
  // File-type rules are compiled once per settings change, not per row.
  const matcher = compileTypeRules(s.colors.typeRules);
  for (const p of Object.values(app.panels)) p.setColors(s.colors, matcher);
  st.setProperty("--font-size", `${s.fontSize}px`);
  const rowH = Math.round(s.fontSize * 1.47);
  st.setProperty("--row-h", `${rowH}px`);
  for (const p of Object.values(app.panels)) p.setRowHeight(rowH);
  for (const p of Object.values(app.panels)) p.setIconStyle(iconStyle(s));
  document.getElementById("cmdline").classList.toggle("off", !s.showCommandLine);
  document.body.classList.toggle("nodrives", s.showDriveBar !== true);
  document.body.classList.toggle("nohead", s.showPanelHeader === false);
  document.body.classList.toggle("nofoot", s.showPanelFooter === false);
  document.body.classList.toggle("nofkeys", s.showFkeyBar === false);
  // F-key bar entries with a modifier show the Mac keys in use.
  for (const b of document.querySelectorAll("#fkeys [data-combo]")) {
    b.textContent = `${comboLabel(b.dataset.combo, modifierMap(s))} ${b.dataset.label}`;
  }
}

/** The command line's prompt: the folder a command runs in (for a results list, the one it came from). */
function updatePrompt() {
  document.getElementById("prompt").textContent = `${displayLocation(workFolder(activePanel()), remoteArchives)}>`;
}

function makePanel(side) {
  return new Panel(document.getElementById(side), side, {
    // The results list (SEARCH_LOCATION) is listed from `app.results`, never by `list_dir`.
    listDir: (location) => (isSearchLocation(location) ? resultsListing(side) : invoke("list_dir", { location })),
    nearest: (location) => invoke("nearest_existing", { location }),
    icons: (keys) => invoke("file_icons", { keys }),
    dirSize: (location, panel) => invoke("dir_size", { location, panel }),
    status: (text) => status(text),
    openRemoteArchive: (location) => downloadArchive(location),
    // `onFolder` (a server symlink): called instead of an error when the target is a folder.
    openDefault: (path, onFolder) =>
      withPassword(() => invoke("open_default", { path })).catch((e) =>
        onFolder && remoteLinkIsFolder(String(e?.message ?? e)) ? onFolder() : status(`Cannot open: ${e}`),
      ),
    showHidden: () => app.showHidden,
    onActivate: (p) => {
      if (app.activeSide !== p.side) setActive(p.side);
    },
    results: (p) => app.results[p.side] ?? null,
    onNavigated: (p) => {
      // Leaving a results list forgets it (Feed to listbox again brings it back).
      if (!isSearchLocation(p.location)) delete app.results[p.side];
      const disk = diskFor(app.volumes, p.location);
      if (disk) lastOnDisk.set(disk.path, p.location);
      const root = remoteRoot(p.location);
      if (root) lastOnDisk.set(root, p.location);
      if (p.side === app.activeSide) updatePrompt();
      refreshVolumes();
      scheduleSave();
    },
    onCursorMoved: () => scheduleSave(),
    onError: onPanelError,
    onDriveMenu: (p) => driveMenu(p),
    onDrive: (p, path) => goDrive(p, path),
    onRowPress: (p, i, ev) => rowPress(p, i, ev),
    onContextMenu: (p, i, x, y) => contextMenu(p, i, { x, y }),
    onFilterDone: (p) => p.focus(),
  });
}

async function openPanel(panel, saved, home) {
  const r = await panel.go(saved?.location ?? home, saved?.cursorName ?? undefined);
  if (!r.ok && panel.location === "") await panel.go(home);
}

/**
 * Started from an App Translocation path: before anything else, offer the
 * quarantine fix (it restarts fxfm) or a run without the portable settings.
 */
async function translocationDialog(boot) {
  const appPath = boot.originalApp ?? "/path/to/fxfm.app";
  const quoted = `'${appPath.replace(/'/g, "'\\''")}'`;
  await win.show();
  const onKey = (e) => handleModalKey(e);
  document.addEventListener("keydown", onKey);
  try {
    for (;;) {
      const r = await dialog({
        title: "fxfm",
        text: [
          "macOS started fxfm from a temporary read-only location, so your settings next to the app can't be found.",
          "To fix it, fxfm removes the quarantine flag and starts again. The same in Terminal:",
          `xattr -dr com.apple.quarantine ${quoted}`,
        ],
        buttons: [
          { label: "Fix and restart", value: "fix", default: true },
          { label: "Continue without settings", value: null },
        ],
      });
      if (r !== "fix") break;
      // fxfm exits during the call; no session save may race that exit.
      app.noSession = true;
      try {
        await invoke("fix_quarantine");
        return; // fxfm quits and starts again
      } catch (e) {
        await dialog({ title: "fxfm", text: `Cannot fix it: ${e}`, buttons: [{ label: "OK", value: "ok", default: true }] });
      }
    }
  } finally {
    document.removeEventListener("keydown", onKey);
  }
  app.noSession = true;
}

async function main() {
  const boot = await invoke("bootstrap");
  if (boot.translocated) await translocationDialog(boot);
  app.boot = boot;
  app.settings = boot.settings;
  app.keymap = boot.keymap;
  app.commands = new Map(boot.commands.map((c) => [c.id, c]));
  app.geom = boot.session.window ?? null;
  app.activeSide = boot.session.active === "right" ? "right" : "left";
  app.showHidden = boot.session.showHidden ?? true;

  app.panels.left = makePanel("left");
  app.panels.right = makePanel("right");
  applySettings(app.settings);
  for (const s of ["left", "right"]) app.panels[s].setActive(s === app.activeSide);

  win.setTitle(`${boot.user} – fxfm ${boot.version}`).catch((e) => console.error("fxfm: title", e));
  await restoreGeometry(app.geom);
  await captureGeometry();

  try {
    const [vols, shares] = await Promise.all([invoke("list_volumes"), readShares()]);
    setVolumes(vols, shares);
  } catch (e) {
    console.error("fxfm: list_volumes", e);
  }
  await Promise.all([
    openPanel(app.panels.left, boot.session.left, boot.home),
    openPanel(app.panels.right, boot.session.right, boot.home),
  ]);
  updatePrompt();
  await initDialogs({
    invoke,
    // After F6/F8: the results lists drop the items that no longer exist.
    recheckResults: () => {
      recheckResults("left");
      recheckResults("right");
    },
    listen: T.event.listen,
    active: activePanel,
    other: () => app.panels[other(app.activeSide)],
    panels: () => Object.values(app.panels),
    connectFlow,
  });
  await initDragDrop({
    invoke,
    listen: T.event.listen,
    webview: T.webview.getCurrentWebview(),
    panels: () => Object.values(app.panels),
    activate: (p) => {
      if (app.activeSide !== p.side) setActive(p.side);
    },
    topModal,
    copyMoveTo,
    status,
    focusWindow: () => win.setFocus().catch((e) => console.error("fxfm: set focus", e)),
  });
  initHotlist({
    invoke,
    hotlist: () => app.boot.hotlist,
    setHotlist: (h) => {
      app.boot.hotlist = h;
    },
    home: boot.home,
    active: activePanel,
    // The active panel's real folder (a results list: the folder it came from).
    // In a server archive's copy: the server folder holding the archive.
    folder: () => remoteArchiveOf(workFolder(activePanel()), remoteArchives)?.parent ?? workFolder(activePanel()),
    other: () => app.panels[other(app.activeSide)],
    status,
    mods: keyMods,
  });
  await initSync({ invoke, panels: () => app.panels, status, focusMain: () => win.setFocus().catch(() => {}) });
  initMultiRename({
    invoke,
    active: activePanel,
    panels: () => Object.values(app.panels),
    status,
    renamedInResults,
    settings: () => app.settings,
    applySettings,
  });
  initServers({
    invoke,
    home: boot.home,
    active: activePanel,
    other: () => app.panels[other(app.activeSide)],
    panels: () => Object.values(app.panels),
    status,
    mods: keyMods,
    refreshVolumes,
  });
  initConfig({
    invoke,
    commands: () => boot.commands,
    keymap: () => app.keymap,
    setKeymap: (k) => {
      app.keymap = k;
    },
    conflicts: () => app.boot.systemConflicts,
    setConflicts: (c) => {
      app.boot.systemConflicts = c;
    },
    settings: () => app.settings,
    mods: keyMods,
    applySettings,
    storage: () => app.boot.storage,
    setStorage: (s) => {
      app.boot.storage = s;
    },
    defaultColors: boot.defaultColors,
    status,
  });
  app.ready = true;
  scheduleSave();

  document.addEventListener("keydown", onKeyDown);
  document.getElementById("fkeys").addEventListener("click", (e) => {
    const b = e.target.closest("[data-cmd]");
    if (b) dispatch(b.dataset.cmd);
  });

  // Fn as Alt or Ctrl: WebKit never reports Fn, the native monitor does.
  await T.event.listen("keys://fn", (ev) => setFnHeld(ev.payload?.down));
  await T.event.listen("menu://command", (ev) => {
    // A menu click while a dialog is open would stack a second operation on it.
    if (topModal()) return;
    dispatch(ev.payload.id);
  });
  await T.event.listen("edit://changed", (ev) => editChanged(ev.payload));
  // Opened, closed or lost connections: the drive bars follow; a lost one gets its dialog.
  await T.event.listen("connections://changed", (ev) => {
    refreshConnections();
    serversLost(ev.payload?.lost ?? []);
  });
  await refreshConnections();
  initPlayer();
  await T.event.listen("audio://state", (ev) => setPlayer(ev.payload));
  // The Find files window listens to search:// and volumes:// itself.
  await initSearchHost(searchCtx());
  await T.event.listen("search://status", (ev) => optionsSearchStatus(ev.payload));
  await T.event.listen("volumes://changed", async (ev) => {
    optionsSearchReload();
    // A share mounted or unmounted: its drive changes with the volume list.
    setVolumes(ev.payload, await readShares());
    // A panel whose folder vanished (ejected disk) falls back to the nearest existing folder.
    for (const p of Object.values(app.panels)) {
      p.refresh().catch((e) => console.error("fxfm: refresh", e));
    }
  });
  await win.onMoved((e) => {
    setGeometry(e.payload.toLogical(app.scale), null);
    scheduleSave();
  });
  await win.onResized((e) => {
    setGeometry(null, e.payload.toLogical(app.scale));
    scheduleSave();
  });
  await win.onScaleChanged((e) => {
    app.scale = e.payload.scaleFactor;
  });

  await win.show();
  window.__fxfmBooted = true;
  if (boot.pendingEdits?.length) {
    await pendingEditsDialog(boot.pendingEdits).catch((e) => console.error("fxfm: pending edits", e));
  }
  firstRunConflictHint().catch((e) => console.error("fxfm: keyboard hint", e));
  autoConnectShares().catch((e) => console.error("fxfm: auto-connect", e));
}

/**
 * Edited archive files a previous session did not upload. Keep (default, also Escape)
 * leaves them for the next start; Discard moves them to the Trash after a confirmation.
 */
async function pendingEditsDialog(edits) {
  for (;;) {
    const r = await dialog(pendingEditsSpec(edits));
    if (r === "show") {
      for (const e of edits) {
        await invoke("reveal_file", { path: e.local }).catch((err) => status(`Show in Finder: ${err}`));
      }
      continue;
    }
    if (r !== "discard") return;
    if ((await dialog(discardPendingConfirm())) !== "yes") continue;
    try {
      await invoke("discard_pending_edits");
    } catch (err) {
      await dialog({ title: "Error", text: String(err?.message ?? err), buttons: [{ label: "OK", value: "ok", default: true }] });
    }
    return;
  }
}

main().catch(async (e) => {
  console.error("fxfm: startup failed", e);
  document.body.textContent = `fxfm failed to start: ${e?.message ?? e}`;
  await win.show();
});
