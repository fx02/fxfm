// Find files window (Alt+F7): its own native window (search.html, label `findfiles`), opened by
// searchhost.js in the main window. Searching, the index buttons, F3 and the password prompts
// run here; going to a file, Feed to listbox, mounting a share and F3 on audio run in the main
// window over the `findfiles://` bridge (see FIND_FILES_ACTION in lib.mjs).

import {
  FIND_FILES_ACTION,
  FIND_FILES_COMMAND,
  FIND_FILES_REPLY,
  findFilesAction,
  goTarget,
  isAudioName,
  lastComponent,
  missingAction,
  modifierMap,
  moveIndex,
  mountedAt,
  parseFindFilesCommand,
  parseFindFilesReply,
  resultRow,
  RESULT_COLUMNS,
  RESULT_COLUMN_MIN,
  resultClick,
  resultColumns,
  resultGridTemplate,
  feedItems,
  searchRequest,
  searchScopes,
  searchUnder,
  searchWindowState,
  setFnHeld,
  smbServerForVolume,
  staleVolume,
  statusText,
  indexButton,
  tcModifiers,
  comboLabel,
  volumeMounted,
  volumeName,
  volumeOf,
} from "./lib.mjs";
import { dialog, handleModalKey, withPassword } from "./dialogs.js";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const appWin = T.window.getCurrentWindow();

/** Typing waits this long before a search starts. */
const DEBOUNCE_MS = 120;

/** Window geometry and result columns, kept between starts (browser storage; a view preference). */
const LAYOUT_KEY = "fxfm.findFiles";

function loadLayout() {
  try {
    const v = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "null");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

function saveLayout(patch) {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify({ ...loadLayout(), ...patch }));
  } catch {
    // Not kept: the defaults come back next time.
  }
}

// ------------------------------------------------------------ bridge to the main window

/** Request ids are unique per window instance: a late reply meant for a closed window is ignored. */
const session = Math.random().toString(36).slice(2, 10);
let nextId = 0;
const waiting = new Map();

/** Asks the main window to run `action` (FIND_FILES_ACTIONS); resolves with its answer. */
function ask(action, args = {}) {
  const id = `${session}:${++nextId}`;
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    T.event.emit(FIND_FILES_ACTION, findFilesAction(id, action, args)).catch((e) => {
      waiting.delete(id);
      reject(e);
    });
  });
}

/** The open window (null before it is drawn and after it closed); events go to it. */
let win = null;
/** Which Mac keys act as TC's Alt and Ctrl (from the main window, then `keys://modifiers`). */
let mods = modifierMap({});

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Keeps Tab / Shift+Tab inside `root`. */
function cycleFocus(root, e) {
  const items = [...root.querySelectorAll("button:not(:disabled), input:not(:disabled)")].filter((x) => !x.closest("[hidden]"));
  if (!items.length) return;
  let i = items.indexOf(document.activeElement);
  i = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i + 1) % items.length;
  items[i].focus();
}

/** The window's Build index button. Returns false on failure (said with `report`). */
async function buildIndex(report) {
  try {
    await invoke("search_build");
    // The backend saved `search.index = true`; the main window keeps its copy in step.
    ask("indexOn").catch((e) => console.error("fxfm: indexOn", e));
    return true;
  } catch (e) {
    report(`Cannot build the index: ${e?.message ?? e}`);
    return false;
  }
}

/**
 * Draws the window. ctx: {home, location (the active panel's), archives (Options → Search →
 * inside archives), memory ({query, scope, archives} of this session)}.
 */
function showWindow(ctx, initialStatus) {
  let status = initialStatus;
  const memory = { query: "", scope: "everywhere", archives: false, ...ctx.memory };
  let state = searchWindowState(undefined, { type: "clear" });
  let sel = -1;
  /** Selected rows (paths) for Feed to listbox; `anchor` is where a Shift range starts. */
  let marked = new Set();
  let anchor = -1;
  let columns = resultColumns(loadLayout().columns);
  let timer = 0;
  let seq = 0;
  let message = null;
  let under = searchUnder(ctx.location);
  let archivesOn = Boolean(ctx.archives);

  // The whole window is the old dialog box (its CSS), without a title: macOS draws that.
  const box = el("div", "dialog search window");
  const body = el("div", "body");

  const name = el("input");
  name.type = "text";
  name.spellcheck = false;
  name.autocomplete = "off";
  name.value = memory.query;
  const nameLine = el("label", "line");
  nameLine.append(el("span", null, "Name:"), name);

  const radio = (label, value) => {
    const r = el("input");
    r.type = "radio";
    r.name = "search-in";
    r.value = value;
    const l = el("label", "opt");
    l.append(r, ` ${label}`);
    return { r, l };
  };
  const everywhere = radio("Everywhere", "everywhere");
  const folder = radio("Current folder", "folder");
  const folderTip = () => {
    folder.l.title = under ?? "The active panel's folder cannot be searched (a server or the results list)";
  };
  folderTip();
  const inLine = el("div", "line in");
  inLine.append(el("span", null, "In:"), everywhere.l, folder.l);

  const archives = el("input");
  archives.type = "checkbox";
  archives.checked = memory.archives;
  const archLine = el("label", "opt arch");
  archLine.append(archives, " Inside archives");
  archLine.hidden = !archivesOn;

  const note = el("p", "note");
  const fda = el("div", "fda");
  fda.append(el("span", "note", "Some folders were skipped: Full Disk Access is needed for a complete index."));

  // Result list: a header with resizable columns (right-click hides or shows them) over the rows.
  const list = el("div", "results");
  const head = el("div", "fhead");
  const rowsEl = el("div", "frows");
  list.append(head, rowsEl);
  const colMenu = el("div", "colmenu");
  colMenu.hidden = true;
  // Bottom line: the index state, and the index and privacy buttons (always reachable, not
  // only from the first-run prompt).
  const statusLine = el("div", "sstatus");
  const indexBtn = el("button");
  const privacyBtn = el("button", null, "Open Privacy settings");
  privacyBtn.title = "Full Disk Access lets fxfm index every folder without macOS asking";
  privacyBtn.addEventListener("click", () => invoke("open_fda_settings").catch((e) => say(String(e))));
  const foot = el("div", "sfoot");
  foot.append(statusLine, indexBtn, privacyBtn);
  body.append(nameLine, inLine, archLine, note, fda, list, foot);

  const row = el("div", "buttons");
  const hint = el("span", "hint", "Enter: go to file  F3: view  Esc: close");
  const feedBtn = el("button", null, "Feed to listbox");
  feedBtn.title = comboLabel("Ctrl+Enter", mods);
  const closeBtn = el("button", null, "Close");
  row.append(hint, feedBtn, closeBtn);
  box.append(body, row, colMenu);

  const scope = () => (everywhere.r.checked ? "everywhere" : "folder");

  // ------------------------------------------------------------ rendering

  /**
   * Enables the "In" choices for the index state. On opening the remembered choice is taken
   * when it is possible; later (a status event) the choice only changes when it was disabled.
   * Returns true when the checked choice changed.
   */
  const applyScopes = (initial) => {
    const s = searchScopes(status.state);
    const was = everywhere.r.checked ? "everywhere" : folder.r.checked ? "folder" : null;
    everywhere.r.disabled = !s.everywhere;
    folder.r.disabled = !under;
    let pick = initial ? memory.scope : was;
    if (pick === "everywhere" && !s.everywhere) pick = "folder";
    if (pick === "folder" && !under && s.everywhere) pick = "everywhere";
    everywhere.r.checked = pick === "everywhere";
    folder.r.checked = pick === "folder";
    // The index is off: "Current folder" is the only choice, so Everywhere is not shown at all.
    everywhere.l.hidden = status.state === "off";
    const text = s.note ?? (!under && !s.everywhere ? "The active panel's folder cannot be searched (a server or the results list)." : null);
    note.textContent = text ?? "";
    note.hidden = !text;
    fda.hidden = !status.fdaNeeded;
    const b = indexButton(status.state);
    indexBtn.textContent = b.label;
    indexBtn.disabled = b.disabled;
    indexBtn.dataset.action = b.action ?? "";
    return !initial && pick !== was;
  };

  const paintStatus = () => {
    if (message) {
      statusLine.textContent = message;
      statusLine.classList.add("msg");
      return;
    }
    statusLine.classList.remove("msg");
    const searching = state.id != null || state.pending || state.rows.length > 0;
    statusLine.textContent = statusText({
      total: name.value.trim() && searching ? state.total : undefined,
      shown: state.rows.length,
      state: status.state,
      walking: (state.pending || !state.done) && scope() === "folder",
      entries: status.entries,
      stale: staleVolume(status),
    });
  };

  const say = (text) => {
    message = text;
    paintStatus();
  };

  indexBtn.addEventListener("click", async () => {
    const action = indexBtn.dataset.action;
    if (action === "build") {
      if (!(await buildIndex(say))) return;
    } else if (action === "rebuild") {
      const r = await dialog({
        title: "Rebuild index",
        text: "Read the whole disk again? The current index stays usable until the new one is ready.",
        buttons: [
          { label: "Rebuild", value: "yes", default: true },
          { label: "Cancel", value: null },
        ],
      });
      if (r !== "yes") return;
      try {
        await invoke("search_rebuild", { path: null });
      } catch (e) {
        return say(`Cannot rebuild the index: ${e?.message ?? e}`);
      }
    } else return;
    message = null;
    win?.reloadStatus();
    name.focus();
  });

  const paintSel = () => {
    const many = marked.size > 1;
    [...rowsEl.children].forEach((r, i) => {
      r.classList.toggle("sel", i === sel);
      r.classList.toggle("mark", many && marked.has(state.rows[i]?.path));
    });
    const r = rowsEl.children[sel];
    if (r) {
      // Keep the row clear of the sticky header.
      const top = r.offsetTop - head.offsetHeight;
      if (top < list.scrollTop) list.scrollTop = top;
      else if (r.offsetTop + r.offsetHeight > list.scrollTop + list.clientHeight) {
        list.scrollTop = r.offsetTop + r.offsetHeight - list.clientHeight;
      }
    }
    feedBtn.textContent = many ? `Feed ${marked.size} to listbox` : "Feed to listbox";
  };

  /** Moves the cursor to `i`; `extend` (Shift) selects from the anchor, else only that row. */
  const moveTo = (i, extend) => {
    if (i < 0) return;
    if (extend) ({ marked, anchor } = resultClick(state.rows, marked, anchor, i, { range: true }));
    else ({ marked, anchor } = resultClick(state.rows, marked, anchor, i));
    sel = i;
    paintSel();
  };

  const shown = () => columns.filter((c) => !c.hidden);

  const applyColumns = () => {
    list.style.setProperty("--rcols", resultGridTemplate(columns));
  };

  const saveColumns = () => saveLayout({ columns });

  const renderHead = () => {
    const cells = shown().map((c) => {
      const label = RESULT_COLUMNS.find((d) => d.id === c.id).label;
      const cell = el("span", `h ${c.id}`, label);
      cell.title = "Drag the edge to resize; right-click to show or hide columns";
      const grip = el("span", "grip");
      grip.addEventListener("mousedown", (e) => {
        e.preventDefault();
        e.stopPropagation();
        const x0 = e.clientX;
        const w0 = cell.getBoundingClientRect().width;
        const move = (ev) => {
          c.width = Math.max(RESULT_COLUMN_MIN, Math.round(w0 + ev.clientX - x0));
          applyColumns();
        };
        const up = () => {
          document.removeEventListener("mousemove", move);
          document.removeEventListener("mouseup", up);
          saveColumns();
        };
        document.addEventListener("mousemove", move);
        document.addEventListener("mouseup", up);
      });
      cell.append(grip);
      return cell;
    });
    head.replaceChildren(...cells);
  };

  const closeColMenu = () => {
    colMenu.hidden = true;
  };

  head.addEventListener("mousedown", (e) => e.preventDefault());
  head.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    colMenu.replaceChildren(
      ...columns.map((c) => {
        const l = el("label", "opt");
        const cb = el("input");
        cb.type = "checkbox";
        cb.checked = !c.hidden;
        cb.disabled = c.id === "name";
        cb.addEventListener("change", () => {
          c.hidden = !cb.checked;
          saveColumns();
          applyColumns();
          renderHead();
          renderRows();
        });
        l.append(cb, ` ${RESULT_COLUMNS.find((d) => d.id === c.id).label}`);
        return l;
      }),
    );
    const b = box.getBoundingClientRect();
    colMenu.style.left = `${e.clientX - b.left}px`;
    colMenu.style.top = `${e.clientY - b.top}px`;
    colMenu.hidden = false;
  });
  box.addEventListener("mousedown", (e) => {
    if (!colMenu.hidden && !colMenu.contains(e.target)) closeColMenu();
  });

  /** A cut-off cell shows its whole text as the tooltip; otherwise the row's full path. */
  const cellTip = (e) => {
    const c = e.target.closest(".frow > span");
    if (!c) return;
    c.title = c.scrollWidth > c.clientWidth ? c.textContent : c.parentElement.dataset.path;
  };
  rowsEl.addEventListener("mouseover", cellTip);

  const renderRows = () => {
    const frag = document.createDocumentFragment();
    const cols = shown();
    state.rows.forEach((it, i) => {
      const v = resultRow(it, ctx.home);
      const r = el("div", "frow");
      if (it.mounted === false) r.classList.add("stale");
      if (it.isDir) r.classList.add("dir");
      r.append(...cols.map((c) => el("span", c.id, v[c.id])));
      r.dataset.path = it.path;
      r.addEventListener("mousedown", (e) => {
        e.preventDefault(); // focus stays in the Name field
        if (e.button !== 0) return;
        ({ marked, anchor } = resultClick(state.rows, marked, anchor, i, {
          toggle: e.metaKey || e.ctrlKey,
          range: e.shiftKey,
        }));
        sel = i;
        paintSel();
      });
      // Control+click is a right click on a Mac: it toggles above, so no context menu.
      r.addEventListener("contextmenu", (e) => e.preventDefault());
      r.addEventListener("dblclick", (e) => {
        if (!e.metaKey && !e.ctrlKey && !e.shiftKey) go(i);
      });
      frag.append(r);
    });
    rowsEl.replaceChildren(frag);
    // Rows that left the list leave the selection.
    const paths = new Set(state.rows.map((r) => r.path));
    for (const p of marked) if (!paths.has(p)) marked.delete(p);
    if (sel >= state.rows.length) sel = state.rows.length - 1;
    if (sel < 0 && state.rows.length) sel = 0;
    if (!marked.size && sel >= 0) ({ marked, anchor } = resultClick(state.rows, marked, anchor, sel));
    paintSel();
  };

  const dispatch = (event) => {
    const before = state;
    state = searchWindowState(state, event);
    if (state === before) return;
    if (state.rows !== before.rows) {
      // A new search's first batch starts at the top; a walk's batches keep the selection.
      if (before.fresh || event.type === "started") {
        sel = state.rows.length ? 0 : -1;
        marked = new Set();
        anchor = -1;
      }
      renderRows();
    }
    paintStatus();
  };

  // ------------------------------------------------------------ searching

  const run = async () => {
    clearTimeout(timer);
    message = null;
    const req = searchRequest({
      query: name.value,
      scope: scope(),
      location: under,
      archives: !archLine.hidden && archives.checked,
    });
    const mine = ++seq;
    if (!req || (req.under == null && (scope() === "folder" || everywhere.r.disabled))) {
      if (state.id != null) invoke("search_cancel", { id: state.id }).catch(() => {});
      dispatch({ type: "clear" });
      paintStatus();
      return;
    }
    dispatch({ type: "start" });
    paintStatus();
    try {
      const id = await invoke("search_start", { req });
      if (!open) {
        invoke("search_cancel", { id }).catch(() => {});
        return;
      }
      if (mine !== seq) return; // a newer search took over (the backend cancelled this one)
      dispatch({ type: "started", id });
    } catch (e) {
      if (mine !== seq) return;
      dispatch({ type: "failed" });
      const msg = String(e?.message ?? e);
      say(msg === "no index" ? "The index is not ready yet; Current folder searches work now." : `Search: ${msg}`);
    }
  };

  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(run, DEBOUNCE_MS);
  };

  // ------------------------------------------------------------ actions

  let open = true;
  /** Esc, Close, Ctrl+W: the close handler (in main below) tidies up first. */
  const close = () => appWin.close().catch((e) => console.error("fxfm: close", e));
  /** The window goes: the search stops and the main window keeps the results and options. */
  const finish = async () => {
    if (!open) return;
    open = false;
    win = null;
    clearTimeout(timer);
    memory.query = name.value;
    if (state.id != null && !state.done) await invoke("search_cancel", { id: state.id }).catch(() => {});
    await ask("closed", { memory, items: state.rows, query: name.value.trim() }).catch((e) => console.error("fxfm: closed", e));
  };

  const missing = (it) => {
    invoke("search_forget", { path: it.path }).catch((e) => console.error("fxfm: search_forget", e));
    message = `${it.name} no longer exists.`;
    dispatch({ type: "remove", path: it.path });
    paintStatus();
  };

  /** An unmounted result: mount its saved SMB share, then go; else say it is not mounted. */
  const mountFirst = async (it) => {
    const vol = volumeOf(it.path, status.volumes);
    const volName = volumeName(it.path, vol);
    let server = null;
    try {
      server = smbServerForVolume(vol, (await invoke("servers_load")).nodes);
    } catch (e) {
      console.error("fxfm: servers_load", e);
    }
    if (!server) return say(`Volume ${volName} is not mounted.`);
    let loc = null;
    try {
      loc = await ask("connect", { server });
    } catch (e) {
      return say(`Cannot mount ${server}: ${e?.message ?? e}`);
    } finally {
      // Mounting asked in the main window: come back here.
      if (open) appWin.setFocus().catch(() => {});
    }
    if (!loc || !open) return;
    if (!mountedAt(loc, vol.path)) return say(`${server} is mounted at ${loc}, not at ${vol.path}.`);
    return go(state.rows.indexOf(it), true);
  };

  /** Enter: the result must still exist; then the active panel opens its folder at it. */
  const go = async (i, mounted = false) => {
    const it = state.rows[i];
    if (!it) return;
    if (it.mounted === false && !mounted) return mountFirst(it);
    const { folder, name: cursor } = goTarget(it);
    let entries = null;
    try {
      entries = await withPassword(() => invoke("list_dir", { location: folder }));
    } catch (e) {
      if (!open) return;
      if (e?.kind !== "notFound" && e?.kind !== "notADirectory") {
        return say(`Cannot read ${folder}: ${e?.message ?? e?.kind ?? e}`);
      }
    }
    if (!open) return;
    const listOk = entries != null;
    const found = listOk && entries.some((x) => x.name === cursor);
    let mountedNow = true;
    if (!found) {
      // `mounted` on the row dates from the search: ask again before forgetting anything.
      try {
        status = await invoke("search_status");
        mountedNow = volumeMounted(it.path, status.volumes);
      } catch (e) {
        console.error("fxfm: search_status", e);
        return say(`Cannot check the volume of ${it.name}.`);
      }
      if (!open) return;
    }
    const action = missingAction({ listOk, found, volumeMounted: mountedNow });
    if (action === "forget") return missing(it);
    if (action === "mount") return mounted ? say(`Volume ${volumeName(it.path, volumeOf(it.path, status.volumes))} is not mounted.`) : mountFirst(it);
    try {
      await ask("go", { folder, cursor });
    } catch (e) {
      return say(`Cannot go to ${it.name}: ${e?.message ?? e}`);
    }
    close();
  };

  /** F3: view or play the result like F3 in a panel; the window stays open. */
  const view = () => {
    const it = state.rows[sel];
    if (!it || it.isDir) return;
    if (it.mounted === false) return say(`Volume ${volumeName(it.path, volumeOf(it.path, status.volumes))} is not mounted.`);
    message = null;
    paintStatus();
    // Audio plays in the main window's player strip; anything else opens a viewer window.
    const shown = isAudioName(lastComponent(it.path))
      ? ask("play", { path: it.path })
      : withPassword(() => invoke("open_viewer", { path: it.path }));
    shown.catch((e) => say(`Cannot view ${it.name}: ${e?.message ?? e}`));
  };

  /** Feed to listbox: the selected rows (else all; `all`: every row) go to the main window. */
  const feed = async (all = false) => {
    const items = all ? state.rows.slice() : feedItems(state.rows, marked);
    if (!items.length) return;
    try {
      await ask("feed", { items, query: name.value.trim() });
    } catch (e) {
      return say(`Feed to listbox: ${e?.message ?? e}`);
    }
    close();
  };

  feedBtn.addEventListener("click", () => feed());
  closeBtn.addEventListener("click", close);
  name.addEventListener("input", () => {
    memory.query = name.value;
    schedule();
  });
  for (const c of [everywhere.r, folder.r, archives]) {
    c.addEventListener("change", () => {
      memory.scope = scope();
      memory.archives = archives.checked;
      run();
      name.focus();
    });
  }

  const pageRows = () => {
    const h = rowsEl.firstElementChild?.offsetHeight || 18;
    return Math.max(1, Math.floor((list.clientHeight - head.offsetHeight) / h) - 1);
  };

  const onKey = (e) => {
    const m = tcModifiers(e, mods);
    if (e.key === "Escape") {
      e.preventDefault();
      if (!colMenu.hidden) return closeColMenu();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      cycleFocus(box, e);
    } else if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      if (m.ctrl) return feed();
      const focused = document.activeElement;
      if (focused?.tagName === "BUTTON" && box.contains(focused)) return focused.click();
      go(sel);
    } else if (m.ctrl && e.code === "KeyW") {
      e.preventDefault();
      close();
    } else if (["ArrowUp", "ArrowDown", "PageUp", "PageDown"].includes(e.key) && !m.any) {
      e.preventDefault();
      // Shift extends the selection (Feed to listbox takes the selected rows).
      moveTo(moveIndex(sel, e.key, state.rows.length, pageRows()), e.shiftKey);
    } else if (e.key === "F3" && !m.any && !e.shiftKey) {
      e.preventDefault();
      view();
    }
    // Anything else is typing in the Name field (or a key the focused control handles).
  };

  /** Alt+F7 again in the main window: its active panel's folder becomes "Current folder". */
  const setLocation = (location) => {
    const next = searchUnder(location);
    if (next === under) return;
    under = next;
    folderTip();
    if (applyScopes(false) || scope() === "folder") run();
    else paintStatus();
  };

  win = {
    dispatch,
    onKey,
    finish,
    feed,
    setLocation,
    focusName: () => {
      name.focus();
      name.select();
    },
    setArchives: (on) => {
      if (on === archivesOn) return;
      archivesOn = on;
      archLine.hidden = !on;
      if (archives.checked) run();
    },
    setMods: () => {
      feedBtn.title = comboLabel("Ctrl+Enter", mods);
    },
    setStatus: (s) => {
      status = s;
      if (applyScopes(false)) run();
      else paintStatus();
    },
    reloadStatus: () => {
      invoke("search_status")
        .then((s) => win?.setStatus(s))
        .catch((e) => console.error("fxfm: search_status", e));
    },
  };

  applyScopes(true);
  applyColumns();
  renderHead();
  renderRows();
  paintStatus();
  document.body.append(box);
  name.focus();
  name.select();
  if (name.value.trim()) run();
}

// ------------------------------------------------------------ window

/** Kept geometry: position and size while neither maximized nor full screen, and whether maximized. */
let geomTimer = 0;
function keepGeometry() {
  clearTimeout(geomTimer);
  geomTimer = setTimeout(async () => {
    try {
      const [max, full] = await Promise.all([appWin.isMaximized(), appWin.isFullscreen()]);
      if (full) return;
      if (max) return saveLayout({ maximized: true });
      const scale = await appWin.scaleFactor();
      const [pos, size] = await Promise.all([appWin.outerPosition(), appWin.innerSize()]);
      const p = pos.toLogical(scale);
      const z = size.toLogical(scale);
      saveLayout({ x: Math.round(p.x), y: Math.round(p.y), width: Math.round(z.width), height: Math.round(z.height), maximized: false });
    } catch (e) {
      console.error("fxfm: window geometry", e);
    }
  }, 300);
}

async function main() {
  await T.event.listen(FIND_FILES_REPLY, (ev) => {
    const r = parseFindFilesReply(ev.payload);
    const w = r && waiting.get(r.id);
    if (!w) return;
    waiting.delete(r.id);
    if (r.ok) w.resolve(r.value);
    else w.reject(new Error(r.error));
  });
  await T.event.listen("keys://modifiers", (ev) => {
    mods = modifierMap({ modifiers: ev.payload });
    win?.setMods();
  });
  await T.event.listen("keys://fn", (ev) => setFnHeld(ev.payload?.down));
  await T.event.listen("search://batch", (ev) => win?.dispatch(ev.payload));
  await T.event.listen("search://status", (ev) => win?.setStatus(ev.payload));
  // Volumes changed: re-read the index state.
  await T.event.listen("volumes://changed", () => win?.reloadStatus());
  await T.event.listen(FIND_FILES_COMMAND, (ev) => {
    const c = parseFindFilesCommand(ev.payload);
    if (!c || !win) return;
    if (c.cmd === "focus") {
      if (typeof c.location === "string") win.setLocation(c.location);
      win.focusName();
    } else if (c.cmd === "feedAll") win.feed(true);
    else if (c.cmd === "settings") win.setArchives(Boolean(c.archives));
  });
  // Esc, Close, Ctrl+W and the red button all come here before the window goes.
  await appWin.onCloseRequested(() => win?.finish());
  await appWin.onMoved(keepGeometry);
  await appWin.onResized(keepGeometry);

  // A dialog on top (Rebuild index, a password) gets the keys first.
  document.addEventListener("keydown", (e) => {
    if (handleModalKey(e)) return;
    win?.onKey(e);
  });

  const ctx = await ask("context");
  mods = modifierMap({ modifiers: ctx.mods ?? {} });
  let status = await invoke("search_status");
  // The first-run question in the main window decided the state to open with.
  if (ctx.state) status = { ...status, state: ctx.state };
  showWindow(ctx, status);
  await appWin.show();
  await appWin.setFocus();
}

main().catch(async (e) => {
  console.error("fxfm: Find files failed", e);
  document.body.textContent = `Find files could not start: ${e?.message ?? e}`;
  await appWin.show().catch(() => {});
});
