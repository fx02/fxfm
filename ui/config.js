// Configuration → Keyboard and Configuration → Options dialogs.

import { comboFromEvent, conflictMessage, keyRows, outputOptions } from "./lib.mjs";
import { dialog, popModal, pushModal } from "./dialogs.js";
import { moveStorage } from "./servers.js";

/**
 * {invoke, commands(), keymap(), setKeymap(map), conflicts(), setConflicts(list), settings(),
 *  applySettings(s), storage(), setStorage(info), defaultColors, status(text)} from app.js.
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
  for (const [combo] of conflicts) box.append(el("p", null, conflictMessage(combo)));
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
    text: conflicts.map(([combo]) => conflictMessage(combo)),
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
      const combo = comboFromEvent(e);
      if (!combo) return; // a modifier on its own
      if (e.metaKey) {
        note.textContent = "Cmd shortcuts belong to macOS; use Ctrl or Option.";
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
  body.append(bannerSlot, scroller);

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
      const ok = await dialog({ title: "Keyboard", text: `${combo} is used by ${titleOf(owner)}. Reassign?`, buttons: YES_NO });
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
        const chip = el("span", "chip", combo);
        const x = el("button", "x", "×");
        x.title = `Remove ${combo}`;
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

const COLOR_LABELS = [
  ["normal", "Folders and files"],
  ["hidden", "Hidden"],
  ["archive", "Archives"],
  ["media", "Video / audio"],
  ["marked", "Marked"],
  ["background", "Background"],
  ["cursor", "Cursor"],
];

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

  // Colors.
  const colors = section("Colors");
  const pickers = {};
  const grid = el("div", "colors");
  for (const [key, label] of COLOR_LABELS) {
    const p = el("input");
    p.type = "color";
    p.value = work.colors[key];
    pickers[key] = p;
    const l = el("label", "color");
    l.append(p, ` ${label}`);
    grid.append(l);
  }
  const resetColors = button("Reset", () => {
    for (const [key] of COLOR_LABELS) pickers[key].value = ctx.defaultColors[key];
  });
  colors.append(grid, resetColors);

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
  view.append(line(fontLabel), line(cmdLabel));

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

  body.append(storage, editor, colors, view, audio);

  const close = () => popModal(modal);
  const ok = async () => {
    if (edApp.input.checked && !appPath.value) return message("Options", "Choose the editor application.");
    if (edTerm.input.checked && !termCmd.value.trim()) return message("Options", "Enter the terminal command.");
    work.editor = edApp.input.checked
      ? { kind: "app", path: appPath.value }
      : edTerm.input.checked
        ? { kind: "terminal", command: termCmd.value.trim() }
        : { kind: "system" };
    for (const [key] of COLOR_LABELS) work.colors[key] = pickers[key].value;
    const size = Number.parseInt(font.value, 10);
    work.fontSize = Number.isFinite(size) ? Math.min(24, Math.max(12, size)) : ctx.settings().fontSize;
    work.showCommandLine = cmdBox.checked;
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
