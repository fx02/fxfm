// Multi-rename (TC Ctrl+M): masks, search and replace, case and counter, with a live preview.

import { el, cycleFocus } from "./dom.js";
import {
  isLocalFolder,
  isSearchLocation,
  lastComponent,
  MULTI_RENAME_DEFAULTS,
  MULTI_RENAME_HELP,
  multiRenamePlan,
  namesFromText,
  operationSources,
  parentLocation,
} from "./lib.mjs";
import { dialog, popModal, pushModal } from "./dialogs.js";

/** Kept for the session: the last options (TC keeps them too). */
let memory = { ...MULTI_RENAME_DEFAULTS };
/** The last finished rename, for Undo: [{path (now), newName (the old name)}]. */
let lastUndo = null;

let ctx = null;

/**
 * ctx: {invoke, active() (the active panel), panels(), status(text),
 * renamedInResults(side, Map oldPath -> newPath), settings(), applySettings(s)}.
 */
export function initMultiRename(c) {
  ctx = c;
}

function input(value, cls) {
  const i = el("input", cls);
  i.type = "text";
  i.spellcheck = false;
  i.autocomplete = "off";
  i.value = String(value);
  return i;
}

function checkbox(label, checked) {
  const box = el("input");
  box.type = "checkbox";
  box.checked = checked;
  const l = el("label", "opt");
  l.append(box, ` ${label}`);
  return { box, l };
}

/** The items to rename: marked entries (or the cursor one) as {name, isDir, mtime, folder, path}. */
function itemsOf(panel) {
  const sources = operationSources(panel.entries, panel.marked, panel.cursorIndex);
  const results = isSearchLocation(panel.location);
  return sources.map((e) => {
    const path = e.path ?? `${panel.location.replace(/\/$/, "")}/${e.name}`;
    return {
      name: results ? lastComponent(path) : e.name,
      isDir: Boolean(e.isDir),
      mtime: e.mtime,
      folder: parentLocation(path),
      path,
      inArchive: Boolean(e.inArchive),
      mounted: e.mounted !== false,
    };
  });
}

/** Names in `folder` (for "Already exists"): the panel's own listing, else asked once. */
function folderNames(panel) {
  const cache = new Map();
  if (isLocalFolder(panel.location)) cache.set(panel.location, panel.raw.map((e) => e.name));
  return {
    get: (folder) => cache.get(folder) ?? null,
    async load(folders) {
      for (const f of folders) {
        if (cache.has(f)) continue;
        try {
          cache.set(f, (await ctx.invoke("list_dir", { location: f })).map((e) => e.name));
        } catch {
          cache.set(f, null);
        }
      }
    },
  };
}

export async function multiRename() {
  const panel = ctx.active();
  const items = itemsOf(panel);
  if (!items.length) return;
  const results = isSearchLocation(panel.location);
  if (!results && !isLocalFolder(panel.location)) {
    await dialog({ title: "Multi-rename", text: "Multi-rename works in local folders and in search result lists.", buttons: [{ label: "OK", value: "ok", default: true }] });
    return;
  }
  if (items.some((it) => it.inArchive || !it.mounted)) {
    await dialog({ title: "Multi-rename", text: "Items inside archives or on unmounted volumes cannot be renamed.", buttons: [{ label: "OK", value: "ok", default: true }] });
    return;
  }
  const names = folderNames(panel);
  await names.load([...new Set(items.map((it) => it.folder))]);
  showDialog(panel, items, names);
}

function showDialog(panel, items, names) {
  const opts = { ...memory };
  // A names list (from a file) is for this dialog only.
  delete opts.names;
  let fileNames = null;
  const backdrop = el("div", "backdrop");
  const box = el("div", "dialog mrename");
  const body = el("div", "body");

  const nameIn = input(opts.name);
  const extIn = input(opts.ext, "short");
  const masks = el("div", "line");
  const fromFile = el("button", null, "Names from file…");
  fromFile.title = "A text file with one new name per line, in the order of the list";
  const fileNote = el("span", "note");
  masks.append(el("span", null, "Name:"), nameIn, el("span", "lbl", "Ext:"), extIn, fromFile);
  const help = el("p", "note", MULTI_RENAME_HELP);

  const searchIn = input(opts.search);
  const replaceIn = input(opts.replace);
  const sr = el("div", "line");
  sr.append(el("span", null, "Search:"), searchIn, el("span", "lbl", "Replace:"), replaceIn);
  const regex = checkbox("Regular expression", opts.regex);
  const matchCase = checkbox("Match case", opts.matchCase);

  const caseSel = el("select");
  for (const [id, label] of [
    ["same", "Unchanged"],
    ["lower", "lowercase"],
    ["upper", "UPPERCASE"],
    ["first", "First letter uppercase"],
    ["words", "First Of Each Word"],
  ]) {
    const o = el("option", null, label);
    o.value = id;
    caseSel.append(o);
  }
  caseSel.value = opts.case;
  const startIn = input(opts.start, "num");
  const stepIn = input(opts.step, "num");
  const digitsIn = input(opts.digits, "num");
  const perFolder = checkbox("per folder", opts.perFolder);
  perFolder.l.title = "The counter starts again in each folder";
  const opt2 = el("div", "line");
  opt2.append(
    regex.l,
    matchCase.l,
    el("span", "lbl", "Case:"),
    caseSel,
    el("span", "lbl", "Counter [C] start:"),
    startIn,
    el("span", "lbl", "step:"),
    stepIn,
    el("span", "lbl", "digits:"),
    digitsIn,
    perFolder.l,
  );

  // Presets: named sets of these options, kept in the settings.
  const presetSel = el("select");
  const presetLine = el("div", "line");
  const savePreset = el("button", null, "Save…");
  const deletePreset = el("button", null, "Delete");
  presetLine.append(el("span", null, "Preset:"), presetSel, savePreset, deletePreset);
  const presets = () => ctx.settings?.()?.renamePresets ?? {};
  const fillPresets = (pick = "") => {
    presetSel.replaceChildren();
    const none = el("option", null, "(none)");
    none.value = "";
    presetSel.append(none);
    for (const n of Object.keys(presets()).sort((a, b) => a.localeCompare(b))) {
      const o = el("option", null, n);
      o.value = n;
      presetSel.append(o);
    }
    presetSel.value = pick;
    deletePreset.disabled = !presetSel.value;
  };
  const storePresets = async (next) => {
    const settings = { ...structuredClone(ctx.settings()), renamePresets: next };
    await ctx.invoke("save_settings", { settings });
    ctx.applySettings(settings);
  };
  const setFields = (p) => {
    const o = { ...MULTI_RENAME_DEFAULTS, ...p };
    nameIn.value = o.name;
    extIn.value = o.ext;
    searchIn.value = o.search;
    replaceIn.value = o.replace;
    regex.box.checked = Boolean(o.regex);
    matchCase.box.checked = Boolean(o.matchCase);
    caseSel.value = o.case;
    startIn.value = String(o.start);
    stepIn.value = String(o.step);
    digitsIn.value = String(o.digits);
    perFolder.box.checked = Boolean(o.perFolder);
  };

  const list = el("div", "mrlist");
  const head = el("div", "mrrow head");
  head.append(el("span", null, "Old name"), el("span", null, "New name"));
  const rowsEl = el("div", "mrrows");
  list.append(head, rowsEl);
  const statusLine = el("div", "sstatus");
  body.append(presetLine, masks, fileNote, help, sr, opt2, list, statusLine);

  const row = el("div", "buttons");
  const undoBtn = el("button", null, "Undo last rename");
  undoBtn.disabled = !lastUndo;
  undoBtn.title = "Gives the items of the last multi-rename their old names back";
  const startBtn = el("button", "default", "Start");
  const closeBtn = el("button", null, "Close");
  row.append(undoBtn, el("span", "spacer"), startBtn, closeBtn);
  box.append(el("div", "title", `Multi-rename (${items.length} ${items.length === 1 ? "item" : "items"})`), body, row);
  backdrop.append(box);
  const modal = { el: backdrop };

  let plan = null;
  let busy = false;

  const read = () => {
    opts.name = nameIn.value;
    opts.ext = extIn.value;
    opts.search = searchIn.value;
    opts.replace = replaceIn.value;
    opts.regex = regex.box.checked;
    opts.matchCase = matchCase.box.checked;
    opts.case = caseSel.value;
    opts.start = startIn.value;
    opts.step = stepIn.value;
    opts.digits = digitsIn.value;
    opts.perFolder = perFolder.box.checked;
    memory = { ...opts };
    if (fileNames) opts.names = fileNames;
    else delete opts.names;
    nameIn.disabled = extIn.disabled = Boolean(fileNames);
  };

  const preview = () => {
    read();
    plan = multiRenamePlan(items, opts, (f) => names.get(f));
    const frag = document.createDocumentFragment();
    for (const r of plan.rows) {
      const line = el("div", "mrrow");
      if (r.error) line.classList.add("bad");
      else if (r.from !== r.to) line.classList.add("changed");
      const to = el("span", null, r.error ? `${r.to}  — ${r.error}` : r.to);
      const from = el("span", null, r.from);
      from.title = r.entry.path;
      to.title = r.error ?? r.to;
      line.append(from, to);
      frag.append(line);
    }
    rowsEl.replaceChildren(frag);
    const changed = plan.rows.filter((r) => r.from !== r.to).length;
    const bad = plan.rows.filter((r) => r.error).length;
    statusLine.classList.toggle("msg", Boolean(plan.error || bad));
    statusLine.textContent = plan.error ?? (bad ? `${bad} name(s) cannot be used.` : `${changed} of ${items.length} will be renamed.`);
    startBtn.disabled = busy || !plan.ok;
  };

  const close = () => {
    popModal(modal);
  };

  /** Applies [{path, newName}] and refreshes the panels; returns the backend's results. */
  const apply = async (batch) => {
    const res = await ctx.invoke("multi_rename", { items: batch });
    const moved = new Map();
    batch.forEach((b, i) => {
      if (res[i]?.path && res[i].path !== b.path) moved.set(b.path, res[i].path);
    });
    if (moved.size) {
      for (const p of ctx.panels()) {
        if (isSearchLocation(p.location)) ctx.renamedInResults(p.side, moved);
      }
      // The cursor follows its item when it was renamed.
      const cur = panel.cursorEntry;
      const curPath = cur && !cur.up ? (cur.path ?? `${panel.location.replace(/\/$/, "")}/${cur.name}`) : null;
      const now = curPath && moved.get(curPath);
      panel.clearMarks();
      for (const p of ctx.panels()) {
        if (p === panel && now) {
          await p.go(p.location, isSearchLocation(p.location) ? undefined : lastComponent(now), { refresh: true });
        } else await p.refresh();
      }
    }
    return res;
  };

  const report = (batch, res) => {
    const failed = res.map((r, i) => (r.error ? `${lastComponent(batch[i].path)}: ${r.error}` : null)).filter(Boolean);
    const done = res.filter((r) => r.path).length;
    return { failed, done };
  };

  const start = async () => {
    preview();
    if (!plan.ok || busy) return;
    busy = true;
    startBtn.disabled = true;
    const batch = plan.rows.filter((r) => r.from !== r.to).map((r) => ({ path: r.entry.path, newName: r.to }));
    try {
      const res = await apply(batch);
      const { failed, done } = report(batch, res);
      lastUndo = batch
        .map((b, i) => (res[i]?.path && res[i].path !== b.path ? { path: res[i].path, newName: lastComponent(b.path) } : null))
        .filter(Boolean);
      if (!lastUndo.length) lastUndo = null;
      if (!failed.length) {
        close();
        ctx.status(`Renamed ${done} item(s).`);
        return;
      }
      busy = false;
      undoBtn.disabled = !lastUndo;
      statusLine.classList.add("msg");
      statusLine.textContent = `Renamed ${done}; not renamed: ${failed.join("; ")}`;
    } catch (e) {
      busy = false;
      startBtn.disabled = false;
      statusLine.classList.add("msg");
      statusLine.textContent = `Multi-rename: ${e?.message ?? e}`;
    }
  };

  const undo = async () => {
    if (!lastUndo || busy) return;
    busy = true;
    const batch = lastUndo;
    try {
      const res = await apply(batch);
      const { failed, done } = report(batch, res);
      lastUndo = null;
      close();
      ctx.status(failed.length ? `Undo: ${done} back; failed: ${failed.join("; ")}` : `Undo: ${done} item(s) have their old names again.`);
    } catch (e) {
      busy = false;
      statusLine.classList.add("msg");
      statusLine.textContent = `Undo: ${e?.message ?? e}`;
    }
  };

  for (const i of [nameIn, extIn, searchIn, replaceIn, startIn, stepIn, digitsIn]) i.addEventListener("input", preview);
  for (const c of [regex.box, matchCase.box, caseSel, perFolder.box]) c.addEventListener("change", preview);
  fromFile.addEventListener("click", async () => {
    if (fileNames) {
      fileNames = null;
      fromFile.textContent = "Names from file…";
      fileNote.textContent = "";
      return preview();
    }
    let path = null;
    try {
      const p = await window.__TAURI__.dialog.open({ multiple: false, directory: false });
      path = typeof p === "string" ? p : null;
    } catch (e) {
      statusLine.textContent = `Choose: ${e?.message ?? e}`;
    }
    if (!path) return;
    try {
      const c = await ctx.invoke("view_content", { path });
      if (c.kind !== "text") throw new Error("not a text file");
      fileNames = namesFromText(c.text);
      fromFile.textContent = "Use masks";
      fileNote.textContent = `${lastComponent(path)}: ${fileNames.length} name(s)`;
      preview();
    } catch (e) {
      statusLine.classList.add("msg");
      statusLine.textContent = `Cannot read ${lastComponent(path)}: ${e?.message ?? e}`;
    }
  });
  presetSel.addEventListener("change", () => {
    deletePreset.disabled = !presetSel.value;
    if (!presetSel.value) return;
    setFields(presets()[presetSel.value]);
    preview();
  });
  savePreset.addEventListener("click", async () => {
    const r = await dialog({
      title: "Save preset",
      text: "Name of the preset:",
      input: presetSel.value || "",
      buttons: [
        { label: "Save", value: "save", default: true },
        { label: "Cancel", value: null },
      ],
    });
    const name = r?.value === "save" ? r.text.trim() : "";
    if (!name) return;
    read();
    try {
      await storePresets({ ...presets(), [name]: { ...opts } });
      fillPresets(name);
    } catch (e) {
      statusLine.textContent = `Cannot save the preset: ${e?.message ?? e}`;
    }
  });
  deletePreset.addEventListener("click", async () => {
    const name = presetSel.value;
    if (!name) return;
    const next = { ...presets() };
    delete next[name];
    try {
      await storePresets(next);
      fillPresets();
    } catch (e) {
      statusLine.textContent = `Cannot delete the preset: ${e?.message ?? e}`;
    }
  });
  fillPresets();
  startBtn.addEventListener("click", start);
  closeBtn.addEventListener("click", close);
  undoBtn.addEventListener("click", undo);

  modal.onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      cycleFocus(box, e);
    } else if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      const focused = document.activeElement;
      if (focused?.tagName === "BUTTON" && box.contains(focused)) return focused.click();
      start();
    }
  };

  preview();
  pushModal(modal);
  nameIn.focus();
  nameIn.select();
}
