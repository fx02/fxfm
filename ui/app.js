// Main window: wires panels, keys, commands, volumes and the session.

import {
  acceptAudioEvent,
  audioActive,
  audioF3Action,
  cmdlineKeyGoesToPanel,
  comboFromEvent,
  comboLabel,
  displayLocation,
  discardPendingConfirm,
  escapeStopsAudio,
  isAudioName,
  joinLocation,
  lastComponent,
  modifierMap,
  passwordArchive,
  pendingEditsDialog as pendingEditsSpec,
  playerStrip,
  rectOnScreen,
  remoteArchiveOf,
  remoteArchives,
  remoteLinkIsFolder,
  seekSeconds,
  textEditKey,
} from "./lib.mjs";
import {
  copyMove,
  copyMoveTo,
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
  topModal,
} from "./dialogs.js";
import { cancelDrag, initDragDrop, rowPress } from "./dragdrop.js";
import { configureKeyboard, configureOptions, firstRunConflictHint, initConfig } from "./config.js";
import { initHotlist, showHotlist } from "./hotlist.js";
import {
  chmodDialog,
  connectFlow,
  disconnectActive,
  initServers,
  lostServer,
  serversLost,
  showConnect,
} from "./servers.js";
import { Panel, VOLUME_ICON, volumeFor } from "./panel.js";

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
  volumes: [],
  ready: false,
  geom: null,
  scale: 1,
  /** "Continue without settings" after App Translocation: the session is not saved. */
  noSession: false,
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
function driveMenu(panel) {
  if (!app.volumes.length) return;
  const backdrop = document.createElement("div");
  backdrop.className = "backdrop clear";
  const menu = document.createElement("div");
  menu.className = "popup";
  const r = panel.comboEl.getBoundingClientRect();
  menu.style.left = `${r.left}px`;
  menu.style.top = `${r.bottom + 2}px`;
  let sel = Math.max(0, app.volumes.indexOf(volumeFor(app.volumes, panel.location)));
  const items = app.volumes.map((v, i) => {
    const it = document.createElement("div");
    it.className = "item";
    it.textContent = `${VOLUME_ICON[v.kind] ?? "💾"} ${v.name}`;
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
    panel.go(app.volumes[i].path);
  };
  modal.onKey = (e) => {
    e.preventDefault();
    if (e.key === "ArrowDown") paint((sel + 1) % items.length);
    else if (e.key === "ArrowUp") paint((sel - 1 + items.length) % items.length);
    else if (e.key === "Home") paint(0);
    else if (e.key === "End") paint(items.length - 1);
    else if (e.key === "Enter") choose(sel);
    else if (e.key === "Escape" || e.key === "Tab") close();
  };
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) close();
  });
  backdrop.append(menu);
  paint(sel);
  pushModal(modal);
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
  cm_parent: () => activePanel().goParent(),
  cm_root: () => activePanel().goRoot(),
  cm_mark: () => activePanel().toggleMark(),
  cm_mark_mask: () => markByMask(true),
  cm_unmark_mask: () => markByMask(false),
  cm_copy: () => copyMove("copy"),
  cm_move: () => copyMove("move"),
  cm_packfiles: () => packFiles(),
  cm_delete: () => deleteItems(true),
  cm_delete_permanent: () => deleteItems(false),
  cm_mkdir: () => makeDir(),
  cm_rename: () => renameInline(),
  cm_view: () => viewOrPlay(),
  cm_edit: () => viewOrEdit("edit_file"),
  cm_edit_new: () => editNew(),
  cm_invert: () => activePanel().invertMarks(),
  cm_mark_all: () => activePanel().markAll(),
  cm_refresh: () => {
    refreshVolumes();
    return activePanel().refresh();
  },
  /** The other panel goes to the active panel's folder; focus stays where it is. */
  cm_target_equal_source: () => app.panels[other(app.activeSide)].go(activePanel().location),
  cm_swap_panels: () => {
    const { left, right } = app.panels;
    const l = { location: left.location, cursor: left.cursorName ?? undefined };
    const r = { location: right.location, cursor: right.cursorName ?? undefined };
    left.go(r.location, r.cursor);
    right.go(l.location, l.cursor);
  },
  cm_toggle_hidden: () => {
    app.showHidden = !app.showHidden;
    for (const p of Object.values(app.panels)) p.rebuild();
  },
  cm_drive_left: () => driveMenu(app.panels.left),
  cm_drive_right: () => driveMenu(app.panels.right),
  cm_hotlist: () => showHotlist(),
  cm_connect: () => showConnect(),
  cm_disconnect: () => disconnectActive(),
  cm_chmod: () => chmodDialog(),
  cm_exit: () => win.close(),
  cm_cmdline_focus: () => {
    if (app.settings.showCommandLine) cmdInput().focus();
  },
  cm_config_keyboard: () => configureKeyboard(),
  cm_config_options: () => configureOptions(),
};

const cmdInput = () => document.getElementById("cmdinput");

/** Runs the command line's text in Terminal in the active panel's folder; false when it is empty. */
function runCommandLine() {
  const input = cmdInput();
  const command = input.value.trim();
  if (!app.settings.showCommandLine || !command) return false;
  input.value = "";
  invoke("run_in_terminal", { dir: activePanel().location, command }).catch((e) => status(`Command line: ${e}`));
  return true;
}

/** F3/F4: act on the file under the cursor; folders and [..] do nothing. */
function viewOrEdit(command) {
  const p = activePanel();
  const e = p.cursorEntry;
  if (!e || e.up || e.isDir) return;
  const path = joinLocation(p.location, e.name);
  return withPassword(() => invoke(command, { path }));
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
  if (!e || e.up || e.isDir || !isAudioName(e.name)) return viewOrEdit("open_viewer");
  const path = joinLocation(p.location, e.name);
  if (audioF3Action(player, path) === "stop") return invoke("audio_stop");
  // Ticks of the file playing until now may still be on their way: ignore them.
  Object.assign(player, { path, name: e.name, info: null, minId: player.lastId + 1 });
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
      const message = /^(Cannot play|Stopped)\b/.test(msg) ? msg : `Cannot play ${e.name}: ${msg}`;
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
  strip.querySelector(".bar").addEventListener("click", (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    const secs = seekSeconds(e.clientX, r.left, r.width, player.duration);
    if (secs != null && player.state === "playing") {
      invoke("audio_seek", { secs }).catch((err) => console.error("fxfm: audio_seek", err));
    }
  });
}

export function dispatch(id) {
  const h = handlers[id];
  if (h) {
    const fail = (e) => status(`${id}: ${e?.message ?? e}`);
    try {
      Promise.resolve(h()).catch(fail);
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
        dispatch(id);
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
    dispatch(id);
  } else if (activePanel().handleKey(combo)) {
    e.preventDefault();
  }
}

// ---------------------------------------------------------------- session

let saveTimer = 0;

function sessionState() {
  return {
    left: panelState(app.panels.left),
    right: panelState(app.panels.right),
    active: app.activeSide,
    window: app.geom,
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

function panelState(p) {
  // The temp copy of a server archive does not survive the session: save the server folder.
  const r = remoteArchiveOf(p.location, remoteArchives);
  if (r) return { location: r.parent, cursorName: lastComponent(r.archive) };
  return p.location ? { location: p.location, cursorName: p.cursorName } : null;
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

/** Re-reads volumes (free space changes); one request at a time. */
async function refreshVolumes() {
  if (volumesInFlight) return;
  volumesInFlight = true;
  try {
    setVolumes(await invoke("list_volumes"));
  } catch (e) {
    console.error("fxfm: list_volumes", e);
  } finally {
    volumesInFlight = false;
  }
}

function setVolumes(vols) {
  app.volumes = vols;
  for (const p of Object.values(app.panels)) p.setVolumes(vols);
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
  if (r === "open") invoke("open_fda_settings").catch((e) => status(String(e)));
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
  app.settings = s;
  const st = document.documentElement.style;
  for (const [k, v] of Object.entries(s.colors)) st.setProperty(`--c-${k}`, v);
  st.setProperty("--font-size", `${s.fontSize}px`);
  const rowH = Math.round(s.fontSize * 1.47);
  st.setProperty("--row-h", `${rowH}px`);
  for (const p of Object.values(app.panels)) p.setRowHeight(rowH);
  document.getElementById("cmdline").classList.toggle("off", !s.showCommandLine);
  // F-key bar entries with a modifier show the Mac keys in use.
  for (const b of document.querySelectorAll("#fkeys [data-combo]")) {
    b.textContent = `${comboLabel(b.dataset.combo, modifierMap(s))} ${b.dataset.label}`;
  }
}

function updatePrompt() {
  document.getElementById("prompt").textContent = `${displayLocation(activePanel().location, remoteArchives)}>`;
}

function makePanel(side) {
  return new Panel(document.getElementById(side), side, {
    listDir: (location) => invoke("list_dir", { location }),
    nearest: (location) => invoke("nearest_existing", { location }),
    dirSize: (location, panel) => invoke("dir_size", { location, panel }),
    status: (text) => status(text),
    openRemoteArchive: (location) => invoke("open_remote_archive", { location }),
    // `onFolder` (a server symlink): called instead of an error when the target is a folder.
    openDefault: (path, onFolder) =>
      withPassword(() => invoke("open_default", { path })).catch((e) =>
        onFolder && remoteLinkIsFolder(String(e?.message ?? e)) ? onFolder() : status(`Cannot open: ${e}`),
      ),
    showHidden: () => app.showHidden,
    onActivate: (p) => {
      if (app.activeSide !== p.side) setActive(p.side);
    },
    onNavigated: (p) => {
      if (p.side === app.activeSide) updatePrompt();
      refreshVolumes();
      scheduleSave();
    },
    onCursorMoved: () => scheduleSave(),
    onError: onPanelError,
    onDriveMenu: (p) => driveMenu(p),
    onRowPress: (p, i, ev) => rowPress(p, i, ev),
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

  app.panels.left = makePanel("left");
  app.panels.right = makePanel("right");
  applySettings(app.settings);
  for (const s of ["left", "right"]) app.panels[s].setActive(s === app.activeSide);

  win.setTitle(`${boot.user} – fxfm ${boot.version}`).catch((e) => console.error("fxfm: title", e));
  await restoreGeometry(app.geom);
  await captureGeometry();

  try {
    setVolumes(await invoke("list_volumes"));
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
    other: () => app.panels[other(app.activeSide)],
    status,
    mods: keyMods,
  });
  initServers({
    invoke,
    home: boot.home,
    active: activePanel,
    other: () => app.panels[other(app.activeSide)],
    panels: () => Object.values(app.panels),
    status,
    mods: keyMods,
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
  await T.event.listen("volumes://changed", (ev) => {
    setVolumes(ev.payload);
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
