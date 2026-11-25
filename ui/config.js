// Configuration → Keyboard and Configuration → Options dialogs.

import {
  captureRefusal,
  comboFromEvent,
  comboLabel,
  conflictMessage,
  formatDate,
  keyRows,
  MAC_KEYS,
  QUICK_SEARCH_KEYS,
  quickSearchMode,
  modifierChoiceError,
  outputOptions,
  searchSettingsPatch,
  ICON_STYLES,
  iconStyle,
} from "./lib.mjs";
import { colorsSection } from "./colors.js";
import { dialog, popModal, pushModal } from "./dialogs.js";
import { moveStorage } from "./servers.js";

/**
 * {invoke, commands(), keymap(), setKeymap(map), conflicts(), setConflicts(list), settings(),
 *  mods(), applySettings(s), storage(), setStorage(info), defaultColors, status(text)} from app.js.
 */
let ctx = null;

export function initConfig(c) {
  ctx = c;
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(label, fn) {
  const b = el("button", null, label);
  b.addEventListener("click", fn);
  return b;
}

const YES_NO = [
  { label: "Yes", value: "yes", default: true },
  { label: "No", value: null },
];

function message(title, text) {
  return dialog({ title, text, buttons: [{ label: "OK", value: "ok", default: true }] });
}

/** Keeps Tab / Shift+Tab inside `root`. */
function cycleFocus(root, e) {
  const items = [...root.querySelectorAll("button:not(:disabled), input:not(:disabled), select:not(:disabled)")];
  if (!items.length) return;
  let i = items.indexOf(document.activeElement);
  i = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i + 1) % items.length;
  items[i].focus();
}

/** Backdrop + dialog box with a title; returns {modal, box, body, row}. */
function frame(title, cls) {
  const backdrop = el("div", "backdrop");
  const box = el("div", `dialog ${cls}`);
  const body = el("div", "body");
  const row = el("div", "buttons");
  box.append(el("div", "title", title), body, row);
  backdrop.append(box);
  return { modal: { el: backdrop }, box, body, row };
}

function openKeyboardSettings() {
  ctx.invoke("open_keyboard_settings").catch((e) => ctx.status(String(e)));
}

/** Yellow warning block for bindings macOS takes first, or null when there are none. */
function conflictBanner(conflicts) {
  if (!conflicts.length) return null;
  const box = el("div", "warn");
  for (const [combo] of conflicts) box.append(el("p", null, conflictMessage(combo, ctx.mods())));
  box.append(button("Open Keyboard Shortcuts", openKeyboardSettings));
  return box;
}

/** First run: warns once when a (default) binding is taken by macOS. */
export async function firstRunConflictHint() {
  const s = ctx.settings();
  const conflicts = ctx.conflicts();
  if (s.conflictHintShown || !conflicts.length) return;
  s.conflictHintShown = true;
  ctx.invoke("save_settings", { settings: s }).catch((e) => console.error("fxfm: save_settings", e));
  const r = await dialog({
    title: "Keyboard",
    text: conflicts.map(([combo]) => conflictMessage(combo, ctx.mods())),
    buttons: [
      { label: "Open Keyboard Shortcuts", value: "open", default: true },
      { label: "OK", value: null },
    ],
  });
  if (r === "open") openKeyboardSettings();
}

// ---------------------------------------------------------------- Keyboard

/** Waits for one key combo; resolves with it or null (Escape / Cancel). */
function captureKey(label) {
  return new Promise((resolve) => {
    const { modal, box, body, row } = frame("Add key", "capture");
    const hint = el("p", null, "Press the new key for:");
    const note = el("p", "note", "Escape cancels.");
    body.append(hint, el("p", "keyname", label), note);
    let open = true;
    const finish = (v) => {
      if (!open) return;
      open = false;
      popModal(modal);
      resolve(v);
    };
    const cancel = button("Cancel", () => finish(null));
    row.append(cancel);
    modal.noDelay = true;
    modal.onKey = (e) => {
      e.preventDefault();
      if (e.repeat) return; // the held key that opened this dialog
      if (e.key === "Escape") return finish(null);
      const combo = comboFromEvent(e, ctx.mods());
      if (!combo) {
        // A modifier on its own waits; a key that gives no combo says why.
        const why = captureRefusal(e, ctx.mods());
        if (why) note.textContent = why;
        return;
      }
      if (combo.includes("Cmd+")) {
        note.textContent = "Command shortcuts belong to macOS; use the keys chosen for Alt and Ctrl.";
        return;
      }
      finish(combo);
    };
    box.tabIndex = -1;
    pushModal(modal);
    box.focus();
  });
}

/** Configuration → Keyboard: every command with its keys; changes are saved at once. */
export function configureKeyboard() {
  const { modal, box, body, row } = frame("Keyboard", "keycfg");
  const bannerSlot = el("div");
  const table = el("table", "keys");
  const head = el("tr");
  head.append(el("th", null, "Command"), el("th", null, "Keys"), el("th"));
  const tbody = el("tbody");
  table.append(el("thead"), tbody);
  table.tHead.append(head);
  const scroller = el("div", "keyscroll");
  scroller.append(table);

  // Which Mac keys act as TC's Alt and Ctrl; saved at once when the two differ.
  const keySelect = (value) => {
    const sel = el("select");
    for (const [id, name] of MAC_KEYS) {
      const o = el("option", null, name);
      o.value = id;
      sel.append(o);
    }
    sel.value = value;
    return sel;
  };
  const current = ctx.mods();
  const altSel = keySelect(current.alt);
  const ctrlSel = keySelect(current.ctrl);
  const fnNote = el(
    "p",
    "hint",
    "Fn: on keyboards where the F-keys need Fn, Fn+F5 then gives Alt+F5 (or Ctrl+F5); switch the F-keys to standard function keys in System Settings → Keyboard. Fn shortcuts macOS uses (Fn+E, Fn+F, Fn+Q…) stay with macOS.",
  );
  const showFnNote = () => (fnNote.hidden = altSel.value !== "fn" && ctrlSel.value !== "fn");
  showFnNote();
  altSel.addEventListener("change", showFnNote);
  ctrlSel.addEventListener("change", showFnNote);
  const modError = el("p", "error");
  modError.hidden = true;
  const modLine = (label, sel) => {
    const l = el("label", "line");
    l.append(el("span", null, label), sel);
    return l;
  };
  // Quick search in the file list (TC Alt+letters); saved at once.
  const qsSel = el("select");
  for (const [id, name] of QUICK_SEARCH_KEYS) {
    const o = el("option", null, name);
    o.value = id;
    qsSel.append(o);
  }
  qsSel.value = quickSearchMode(ctx.settings());
  qsSel.title = "Jumps to the first name starting with the typed letters (TC Alt+letters)";
  qsSel.addEventListener("change", async () => {
    const settings = { ...structuredClone(ctx.settings()), quickSearch: qsSel.value };
    try {
      const conflicts = await ctx.invoke("save_settings", { settings });
      ctx.applySettings(settings);
      ctx.setConflicts(conflicts);
    } catch (e) {
      await message("Error", `Cannot save the quick search keys: ${e}`);
    }
  });
  const modBox = el("div", "mods");
  modBox.append(
    modLine("Alt key (TC Alt)", altSel),
    modLine("Ctrl key (TC Ctrl)", ctrlSel),
    modLine("Quick search", qsSel),
    fnNote,
    modError,
  );
  const saveMods = async () => {
    const problem = modifierChoiceError(altSel.value, ctrlSel.value);
    modError.textContent = problem ? `${problem} Not saved.` : "";
    modError.hidden = !problem;
    if (problem) return;
    const settings = { ...structuredClone(ctx.settings()), modifiers: { alt: altSel.value, ctrl: ctrlSel.value } };
    try {
      const conflicts = await ctx.invoke("save_settings", { settings });
      ctx.applySettings(settings);
      ctx.setConflicts(conflicts);
    } catch (e) {
      await message("Error", `Cannot save the keys: ${e}`);
    }
    render();
  };
  altSel.addEventListener("change", saveMods);
  ctrlSel.addEventListener("change", saveMods);
  body.append(modBox, bannerSlot, scroller);

  const titleOf = (id) => {
    const c = ctx.commands().find((x) => x.id === id);
    return c ? c.title : id;
  };

  const save = async (bindings) => {
    try {
      const conflicts = await ctx.invoke("save_keymap", { bindings });
      ctx.setKeymap(bindings);
      ctx.setConflicts(conflicts);
    } catch (e) {
      await message("Error", `Cannot save keys: ${e}`);
    }
    render();
  };

  const remove = (combo) => {
    const b = { ...ctx.keymap() };
    delete b[combo];
    return save(b);
  };

  const addKey = async (r) => {
    const combo = await captureKey(r.label);
    if (!combo) return;
    const owner = ctx.keymap()[combo];
    if (owner === r.id) return;
    if (owner) {
      const ok = await dialog({ title: "Keyboard", text: `${comboLabel(combo, ctx.mods())} is used by ${titleOf(owner)}. Reassign?`, buttons: YES_NO });
      if (ok !== "yes") return;
    }
    await save({ ...ctx.keymap(), [combo]: r.id });
  };

  const reset = async () => {
    const ok = await dialog({ title: "Keyboard", text: "Reset all keys to the Total Commander defaults?", buttons: YES_NO });
    if (ok !== "yes") return;
    try {
      const bindings = await ctx.invoke("reset_keymap");
      ctx.setKeymap(bindings);
      ctx.setConflicts([]); // the defaults avoid every key macOS reserves
    } catch (e) {
      await message("Error", `Cannot reset keys: ${e}`);
    }
    render();
  };

  function render() {
    bannerSlot.replaceChildren(...[conflictBanner(ctx.conflicts())].filter(Boolean));
    const top = scroller.scrollTop;
    tbody.replaceChildren();
    for (const r of keyRows(ctx.commands(), ctx.keymap())) {
      const tr = el("tr");
      const keys = el("td", "chips");
      for (const combo of r.keys) {
        const label = comboLabel(combo, ctx.mods());
        const chip = el("span", "chip", label);
        const x = el("button", "x", "×");
        x.title = `Remove ${label}`;
        x.addEventListener("click", () => remove(combo));
        chip.append(x);
        keys.append(chip);
      }
      const add = el("td");
      add.append(button("Add key…", () => addKey(r)));
      tr.append(el("td", null, r.label), keys, add);
      tbody.append(tr);
    }
    scroller.scrollTop = top;
  }

  const close = () => popModal(modal);
  const closeBtn = button("Close", close);
  row.append(button("Reset to TC defaults", reset), closeBtn);
  modal.onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      cycleFocus(box, e);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const f = document.activeElement;
      (f?.tagName === "BUTTON" && box.contains(f) ? f : closeBtn).click();
    }
  };
  render();
  pushModal(modal);
  closeBtn.focus();
}

// ---------------------------------------------------------------- Options

/** Set while Options is open: {setStatus(status), reload()}. app.js forwards search://status and volume changes. */
let searchLive = null;

/** A `search://status` payload: the open Options dialog repaints its Search section. */
export function optionsSearchStatus(status) {
  searchLive?.setStatus(status);
}

/** Volumes changed: the open Options dialog re-reads the index state. */
export function optionsSearchReload() {
  searchLive?.reload();
}

const VOL_STATE = { building: "indexing…", ready: "up to date", stale: "stale", off: "off" };
const NO_SEARCH_STATUS = { state: "off", entries: 0, volumes: [], denied: [], unreadable: [], fdaNeeded: false, excludeDefaults: [] };

function radio(name, value, label, checked) {
  const l = el("label", "opt");
  const r = el("input");
  r.type = "radio";
  r.name = name;
  r.value = value;
  r.checked = checked;
  l.append(r, ` ${label}`);
  return { label: l, input: r };
}

function section(title) {
  const fs = el("fieldset");
  fs.append(el("legend", null, title));
  return fs;
}

/** Fills `select` with `outputOptions(devices, saved)` and selects `value`. */
function fillOutputs(select, devices, saved, value) {
  select.replaceChildren(
    ...outputOptions(devices, saved).map((o) => {
      const opt = el("option", null, o.label);
      opt.value = o.value;
      return opt;
    }),
  );
  select.value = value;
}

/** Configuration → Options: storage (applied at once), editor, colors, font, command line, audio output (OK saves). */
export function configureOptions() {
  const work = structuredClone(ctx.settings());
  const { modal, box, body, row } = frame("Options", "optcfg");

  // Storage: applied with its own button, it moves the files.
  const storage = section("Storage");
  const st = ctx.storage();
  const portable = radio("storage", "portable", "Portable (fxfm-data next to fxfm.app)", st.mode === "portable");
  const system = radio("storage", "system", "System (~/Library/Application Support/fxfm)", st.mode === "system");
  const where = el("p", "path");
  const note = el("p", "note");
  const apply = button("Apply", async () => {
    const mode = portable.input.checked ? "portable" : "system";
    if (mode === ctx.storage().mode) return;
    const target = mode === "portable" ? ctx.storage().portableDir : ctx.storage().systemDir;
    const ok = await dialog({ title: "Storage", text: mode === "system" ? `Move all settings to ${target}? An older copy there is kept as fxfm.old.` : `Move all settings to ${target}?`, buttons: YES_NO });
    // A No or a cancelled master prompt changes nothing: the radios show the current mode again.
    let change = null;
    if (ok === "yes") {
      try {
        change = await moveStorage(mode);
      } catch (e) {
        await message("Error", `Cannot move settings: ${e}`);
      }
    }
    if (change) ctx.setStorage(change);
    paintStorage();
    if (change?.message) await message("Storage", change.message);
  });
  const paintStorage = () => {
    const s = ctx.storage();
    where.textContent = `Current folder: ${s.dir}`;
    portable.input.checked = s.mode === "portable";
    system.input.checked = s.mode === "system";
    portable.input.disabled = !s.portableWritable && s.mode !== "portable";
    note.textContent = portable.input.disabled
      ? `Portable is not available: ${s.portableDir ? `the folder next to fxfm.app is not writable (${s.portableDir})` : "fxfm.app has no folder"}.`
      : "";
  };
  paintStorage();
  storage.append(portable.label, system.label, where, note, apply);

  // Editor (F4).
  const editor = section("Editor (F4)");
  const kind = work.editor.kind;
  const edSystem = radio("editor", "system", "System default (VS Code if installed, else TextEdit)", kind === "system");
  const edApp = radio("editor", "app", "Application:", kind === "app");
  const appPath = el("input");
  appPath.type = "text";
  appPath.readOnly = true;
  appPath.value = kind === "app" ? work.editor.path : "";
  const choose = button("Choose…", async () => {
    const p = await window.__TAURI__.dialog.open({
      defaultPath: "/Applications",
      filters: [{ name: "Applications", extensions: ["app"] }],
    });
    if (typeof p === "string") {
      appPath.value = p;
      edApp.input.checked = true;
    }
  });
  const edTerm = radio("editor", "terminal", "Terminal command:", kind === "terminal");
  const termCmd = el("input");
  termCmd.type = "text";
  termCmd.spellcheck = false;
  termCmd.placeholder = "nano";
  termCmd.value = kind === "terminal" ? work.editor.command : "";
  termCmd.addEventListener("input", () => {
    edTerm.input.checked = true;
  });
  const line = (...kids) => {
    const d = el("div", "line");
    d.append(...kids);
    return d;
  };
  editor.append(line(edSystem.label), line(edApp.label, appPath, choose), line(edTerm.label, termCmd));

  // Colors: panel colors and colors by file type.
  const colors = colorsSection(work.colors, { defaultColors: ctx.defaultColors, mods: ctx.mods });

  // Font size and command line.
  const view = section("View");
  const font = el("input");
  font.type = "number";
  font.min = "12";
  font.max = "24";
  font.value = String(work.fontSize);
  const fontLabel = el("label", "opt");
  fontLabel.append("Font size (12–24): ", font);
  const cmdBox = el("input");
  cmdBox.type = "checkbox";
  cmdBox.checked = work.showCommandLine;
  const cmdLabel = el("label", "opt");
  cmdLabel.append(cmdBox, " Show command line");
  const check = (checked, text) => {
    const box = el("input");
    box.type = "checkbox";
    box.checked = checked;
    const label = el("label", "opt");
    label.append(box, ` ${text}`);
    return { box, label };
  };
  const drive = check(work.showDriveBar === true, "Show drive button bar");
  const head = check(work.showPanelHeader !== false, "Show panel header (drive list, free space, path)");
  const foot = check(work.showPanelFooter !== false, "Show panel status line");
  const fkeys = check(work.showFkeyBar !== false, "Show function key bar");
  const iconSel = el("select");
  for (const [id, name] of ICON_STYLES) {
    const o = el("option", null, name);
    o.value = id;
    iconSel.append(o);
  }
  iconSel.value = iconStyle(work);
  const iconLabel = el("label", "opt");
  iconLabel.append("Icons: ", iconSel);
  view.append(line(fontLabel), line(iconLabel), line(cmdLabel), line(fkeys.label), line(drive.label), line(head.label), line(foot.label));

  // Audio: the device list is read once, when the dialog opens.
  const audio = section("Audio");
  const savedOutput = work.audioOutput ?? "";
  const output = el("select");
  // Until the list arrives: the saved choice only, so OK keeps it.
  fillOutputs(output, savedOutput ? [savedOutput] : [], savedOutput, savedOutput);
  output.disabled = true;
  ctx
    .invoke("audio_outputs")
    .catch((e) => {
      console.error("fxfm: audio_outputs", e);
      return [];
    })
    .then((devices) => {
      fillOutputs(output, devices, savedOutput, savedOutput);
      output.disabled = false;
    });
  const outputLabel = el("label", "opt", "Output device");
  audio.append(line(outputLabel, output));

  // Search: edits go to work.search (OK saves); the buttons act at once.
  const search = section("Search");
  const sline = (...kids) => {
    const d = el("div", "line");
    d.append(...kids);
    return d;
  };
  const patch = (change) => {
    work.search = searchSettingsPatch(work, change);
  };
  const s0 = work.search ?? {};
  const wasOn = () => ctx.settings().search?.index === true;
  const indexBox = el("input");
  indexBox.type = "checkbox";
  indexBox.checked = s0.index === true;
  const indexLabel = el("label", "opt");
  indexLabel.append(indexBox, " Index while fxfm is running");
  const indexNote = el("p", "note", "Off: no index is built or used, and searches walk the chosen folder.");
  const sInfo = el("p", "note");
  const volsBox = el("div", "svols");
  const exDefaults = el("p", "note");
  const exList = el("div", "sex");
  const exAdd = button("Add folder…", async () => {
    const dir = await window.__TAURI__.dialog.open({ directory: true });
    if (typeof dir === "string" && dir) {
      patch({ addExclude: dir });
      paintExcludes();
    }
  });
  const archBox = el("input");
  archBox.type = "checkbox";
  archBox.checked = Boolean(s0.archives);
  const archLabel = el("label", "opt");
  archLabel.append(archBox, " Index files inside archives");
  const archNote = el("p", "note", "Turning this on re-reads every indexed volume; turning it off drops the archive entries. Changing the options below re-reads only the archives they affect.");
  const gz = el("input");
  gz.type = "number";
  gz.min = "1";
  gz.value = String(s0.gzLimitMb ?? 500);
  const gzLabel = el("label", "opt");
  gzLabel.append("Skip tar.gz/gz larger than ", gz, " MB");
  const netArchBox = el("input");
  netArchBox.type = "checkbox";
  netArchBox.checked = Boolean(s0.networkArchives);
  const netArchLabel = el("label", "opt");
  netArchLabel.append(netArchBox, " Also read archives on network volumes");
  const hours = el("input");
  hours.type = "number";
  hours.min = "0";
  hours.value = String(s0.networkRefreshHours ?? 24);
  const hoursLabel = el("label", "opt");
  hoursLabel.append("Refresh network volumes every ", hours, " hours (0 = only by hand)");
  const deniedBox = el("div", "sdenied");
  const rebuild = button("Rebuild index", async () => {
    try {
      await ctx.invoke("search_rebuild", { path: null });
    } catch (e) {
      await message("Search", `Cannot rebuild the index: ${e?.message ?? e}`);
    }
  });

  let sstatus = NO_SEARCH_STATUS;
  const indexOn = () => work.search?.index === true;
  const paintExcludes = () => {
    exDefaults.textContent = `Always skipped: ${sstatus.excludeDefaults.join("  ")}`;
    exDefaults.hidden = !sstatus.excludeDefaults.length;
    exList.replaceChildren(
      ...(work.search?.exclude ?? []).map((path) => {
        const r = el("div", "line");
        const t = el("span", "path", path);
        r.append(t, button("Remove", () => {
          patch({ removeExclude: path });
          paintExcludes();
        }));
        return r;
      }),
    );
  };
  const paintStatus = () => {
    const on = indexOn();
    sInfo.textContent =
      sstatus.state === "building"
        ? "Building index…"
        : sstatus.state === "ready"
          ? `Index: ${sstatus.entries.toLocaleString("en-US")} files, up to date`
          : sstatus.state === "notAsked"
            ? "The index has not been built yet. Turn on “Index while fxfm is running” to build it."
            : "";
    sInfo.hidden = !sInfo.textContent;
    rebuild.disabled = !on;
    volsBox.replaceChildren(
      ...sstatus.volumes.map((v) => {
        const r = el("div", "line svol");
        const box = el("input");
        box.type = "checkbox";
        const saved = work.search?.volumes?.find((x) => x.path === v.path);
        box.checked = v.kind === "startup" ? true : (saved?.enabled ?? v.enabled);
        box.disabled = v.kind === "startup";
        box.title = v.kind === "startup" ? "The startup disk is always indexed" : v.path;
        box.addEventListener("change", () => patch({ volume: { path: v.path, enabled: box.checked } }));
        const state = on ? (VOL_STATE[v.state] ?? v.state) : "off";
        let text = `${v.name} · ${v.kind} · ${v.entries.toLocaleString("en-US")} files · ${state}`;
        if (v.kind === "network" && v.refreshed > 0) text += ` · last refreshed ${formatDate(v.refreshed)}`;
        if (!v.mounted) text += " · not mounted";
        const lab = el("label", "opt", "");
        lab.append(box, ` ${text}`);
        r.append(lab);
        if (v.kind === "network") {
          const b = button("Refresh now", async () => {
            try {
              await ctx.invoke("search_refresh", { path: v.path });
            } catch (e) {
              await message("Search", `Cannot refresh ${v.name}: ${e?.message ?? e}`);
            }
          });
          b.disabled = !on || !v.mounted || !(saved?.enabled ?? v.enabled);
          r.append(b);
        }
        return r;
      }),
    );
    deniedBox.replaceChildren();
    if (sstatus.denied.length) {
      deniedBox.append(el("p", "note", "Folders macOS privacy protection blocked (skipped; Full Disk Access opens them):"));
      for (const d of sstatus.denied) deniedBox.append(el("div", "path", d));
      const retry = button("Retry", async () => {
        for (const d of sstatus.denied) {
          try {
            await ctx.invoke("search_rebuild", { path: d });
          } catch (e) {
            console.error("fxfm: search_rebuild", e);
          }
        }
      });
      retry.disabled = !on;
      deniedBox.append(retry);
    }
    if (sstatus.unreadable.length) {
      deniedBox.append(el("p", "note", "Not readable (permissions; skipped):"));
      for (const d of sstatus.unreadable) deniedBox.append(el("div", "path", d));
    }
    if (sstatus.fdaNeeded) {
      deniedBox.append(
        el("p", "note", "Some folders were skipped: Full Disk Access is needed for a complete index."),
        button("Open Privacy settings", () => ctx.invoke("open_fda_settings").catch((e) => ctx.status(String(e)))),
      );
    }
    paintExcludes();
  };
  const reloadStatus = () =>
    ctx
      .invoke("search_status")
      .then((st) => {
        sstatus = { ...NO_SEARCH_STATUS, ...st };
        paintStatus();
      })
      .catch((e) => console.error("fxfm: search_status", e));
  searchLive = {
    setStatus: (st) => {
      sstatus = { ...NO_SEARCH_STATUS, ...st };
      paintStatus();
    },
    reload: reloadStatus,
  };

  indexBox.addEventListener("change", async () => {
    if (indexBox.checked) {
      if (!wasOn()) {
        try {
          await ctx.invoke("search_build");
        } catch (e) {
          indexBox.checked = false;
          await message("Search", `Cannot build the index: ${e?.message ?? e}`);
          return;
        }
        // The backend saved search.index = true; keep the UI's copy in step.
        const cur = ctx.settings();
        cur.search = { ...(cur.search ?? {}), index: true };
      }
      patch({ index: true });
    } else {
      const r = await dialog({
        title: "Search",
        text: "Turn off the index? Its file is deleted; searches then walk the current folder.",
        buttons: YES_NO,
      });
      if (r !== "yes") {
        indexBox.checked = true;
        return;
      }
      patch({ index: false });
    }
    paintStatus();
  });
  archBox.addEventListener("change", () => patch({ archives: archBox.checked }));
  netArchBox.addEventListener("change", () => patch({ networkArchives: netArchBox.checked }));
  const number = (input, key) => {
    input.addEventListener("change", () => {
      patch({ [key]: input.value });
      input.value = String(work.search[key]); // an invalid entry shows the kept value
    });
  };
  number(gz, "gzLimitMb");
  number(hours, "networkRefreshHours");
  paintStatus();
  reloadStatus();
  search.append(
    sline(indexLabel),
    indexNote,
    sInfo,
    el("p", null, "Volumes"),
    volsBox,
    el("p", null, "Excluded folders"),
    exDefaults,
    exList,
    sline(exAdd),
    archLabel,
    archNote,
    gzLabel,
    netArchLabel,
    hoursLabel,
    deniedBox,
    sline(rebuild),
  );

  // Search (indexing) stays last: it is the longest section.
  body.append(storage, editor, colors.el, view, audio, search);

  const close = () => {
    searchLive = null;
    popModal(modal);
  };
  const ok = async () => {
    if (edApp.input.checked && !appPath.value) return message("Options", "Choose the editor application.");
    if (edTerm.input.checked && !termCmd.value.trim()) return message("Options", "Enter the terminal command.");
    work.editor = edApp.input.checked
      ? { kind: "app", path: appPath.value }
      : edTerm.input.checked
        ? { kind: "terminal", command: termCmd.value.trim() }
        : { kind: "system" };
    work.colors = colors.read();
    // Enter in a number field may not fire its change event: take both values now.
    patch({ gzLimitMb: gz.value });
    patch({ networkRefreshHours: hours.value });
    const size = Number.parseInt(font.value, 10);
    work.fontSize = Number.isFinite(size) ? Math.min(24, Math.max(12, size)) : ctx.settings().fontSize;
    work.showCommandLine = cmdBox.checked;
    work.showDriveBar = drive.box.checked;
    work.icons = iconSel.value;
    work.showPanelHeader = head.box.checked;
    work.showPanelFooter = foot.box.checked;
    work.showFkeyBar = fkeys.box.checked;
    work.audioOutput = output.value || null;
    try {
      await ctx.invoke("save_settings", { settings: work });
    } catch (e) {
      return message("Error", `Cannot save settings: ${e}`);
    }
    ctx.applySettings(work);
    close();
  };
  const okBtn = button("OK", ok);
  row.append(okBtn, button("Cancel", close));
  modal.onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      cycleFocus(box, e);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const f = document.activeElement;
      (f?.tagName === "BUTTON" && box.contains(f) ? f : okBtn).click();
    }
  };
  pushModal(modal);
  okBtn.focus();
}
