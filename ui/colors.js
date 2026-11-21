// Configuration → Options → Colors (TC Configuration → Color) and the TC "Define colors by
// file type" dialog. Nothing here saves: the Options dialog's OK does.

import { comboLabel, compileTypeRules, dragStarted, moveNode, relocateNode, rowStyle, tcModifiers } from "./lib.mjs";
import { dialog, popModal, pushModal } from "./dialogs.js";

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

function checkbox(label, checked) {
  const l = el("label", "opt");
  const input = el("input");
  input.type = "checkbox";
  input.checked = checked;
  l.append(input, ` ${label}`);
  return { label: l, input };
}

function colorWell(value) {
  const p = el("input");
  p.type = "color";
  p.value = value;
  return p;
}

/** Keeps Tab / Shift+Tab inside `root` (`extra`: more focusable selectors). */
function cycleFocus(root, e, extra = "") {
  const sel = `button:not(:disabled), input:not(:disabled), select:not(:disabled)${extra}`;
  const items = [...root.querySelectorAll(sel)];
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

/** Color wells of the Colors section: [settings key, label]. */
const COLOR_FIELDS = [
  ["font", "Font color"],
  ["background", "Background"],
  ["background2", "Background 2"],
  ["mark", "Mark color"],
  ["cursor", "Cursor color"],
  ["cursorFont", "Cursor font"],
];

/** The Example box rows, drawn like the panel draws them. */
const EXAMPLE_ROWS = [
  { label: "Selected", marked: true, cursor: false },
  { label: "Text", marked: false, cursor: false },
  { label: "Cursor", marked: false, cursor: true },
  { label: "Selected+Cursor", marked: true, cursor: true },
];

/**
 * The Colors fieldset for the Options dialog. `colors` is not changed; `read()` returns the
 * edited copy. {defaultColors, mods} come from the config context.
 */
export function colorsSection(colors, { defaultColors, mods }) {
  let cur = structuredClone(colors);
  const fs = el("fieldset", "colorcfg");
  fs.append(el("legend", null, "Colors"));

  const wells = {};
  const grid = el("div", "colors");
  for (const [key, label] of COLOR_FIELDS) {
    const p = colorWell("#000000");
    p.addEventListener("input", () => {
      cur[key] = p.value;
      paintExample();
    });
    wells[key] = p;
    const l = el("label");
    l.append(p, ` ${label}`);
    const cell = el("div", "color");
    cell.append(l);
    grid.append(cell);
  }
  // Cursor font: "use the file's color" saves an empty string.
  const ownColor = checkbox("use the file's color", false);
  ownColor.input.addEventListener("change", () => {
    cur.cursorFont = ownColor.input.checked ? "" : wells.cursorFont.value;
    wells.cursorFont.disabled = ownColor.input.checked;
    paintExample();
  });
  wells.cursorFont.parentElement.parentElement.append(ownColor.label);

  const inverted = checkbox("Use inverted cursor", true);
  inverted.input.addEventListener("change", () => {
    cur.invertedCursor = inverted.input.checked;
    paintExample();
  });
  const byType = checkbox("Define colors by file type", true);
  byType.input.addEventListener("change", () => {
    cur.byType = byType.input.checked;
    paintExample();
  });
  const defineBtn = button("Define colors by file type…", async () => {
    const rules = await editTypeRules(cur.typeRules, cur, mods);
    if (rules) cur.typeRules = rules;
    defineBtn.focus();
  });
  const typeLine = el("div", "line");
  typeLine.append(byType.label, defineBtn);

  const example = el("div", "colorexample");
  const exampleBox = el("div", "exbox");
  example.append(el("div", "exlabel", "Example:"), exampleBox);
  function paintExample() {
    const matcher = compileTypeRules(cur.typeRules);
    exampleBox.style.backgroundColor = cur.background;
    exampleBox.replaceChildren(
      ...EXAMPLE_ROWS.map((x, index) => {
        const entry = { name: x.label, isDir: false, hidden: false };
        const st = rowStyle(entry, { marked: x.marked, cursor: x.cursor, active: true, index }, cur, matcher);
        const r = el("div", `r${x.marked ? " mark" : ""}${x.cursor ? " sel" : ""}`, x.label);
        r.style.color = st.color;
        r.style.backgroundColor = st.background;
        r.style.outline = st.outline;
        return r;
      }),
    );
  }

  /** Shows `cur` in every widget. */
  function paint() {
    for (const [key] of COLOR_FIELDS) {
      // A color well needs #rrggbb; an empty cursor font shows the default there.
      wells[key].value = cur[key] || defaultColors[key] || "#000000";
    }
    ownColor.input.checked = cur.cursorFont === "";
    wells.cursorFont.disabled = ownColor.input.checked;
    inverted.input.checked = cur.invertedCursor;
    byType.input.checked = cur.byType;
    paintExample();
  }

  const reset = button("Reset to defaults", () => {
    cur = structuredClone(defaultColors);
    paint();
  });
  const left = el("div", "colorleft");
  left.append(grid, inverted.label, typeLine, reset);
  const main = el("div", "colormain");
  main.append(left, example);
  fs.append(main);
  paint();
  return { el: fs, read: () => structuredClone(cur) };
}

/** Add…/Edit… a rule: resolves with {masks, color} or null (Cancel / Escape). */
function editRule(title, rule) {
  return new Promise((resolve) => {
    const { modal, box, body, row } = frame(title, "ruleedit");
    const masks = el("input");
    masks.type = "text";
    masks.spellcheck = false;
    masks.value = rule.masks;
    const color = colorWell(rule.color);
    const field = (label, input) => {
      const f = el("div", "srvfield");
      f.append(el("label", null, label), input);
      return f;
    };
    body.append(field("File types (masks):", masks), field("Color:", color));
    let open = true;
    const finish = (v) => {
      if (!open) return;
      open = false;
      popModal(modal);
      resolve(v);
    };
    const okBtn = button("OK", () => {
      if (masks.value.trim()) finish({ masks: masks.value.trim(), color: color.value });
    });
    const sync = () => {
      okBtn.disabled = !masks.value.trim();
    };
    masks.addEventListener("input", sync);
    sync();
    row.append(okBtn, button("Cancel", () => finish(null)));
    modal.onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        finish(null);
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
    masks.focus();
    masks.select();
  });
}

/**
 * TC "Define colors by file type": edits a copy of `rules` (drawn with `colors`' background
 * and cursor); resolves with the new list on OK, null on Cancel / Escape.
 */
export function editTypeRules(rules, colors, mods) {
  return new Promise((resolve) => {
    const work = structuredClone(rules ?? []);
    let sel = work.length ? 0 : -1;

    const backdrop = el("div", "backdrop");
    const box = el("div", "dialog hotcfg typecfg");
    const list = el("div", "hotlist typelist");
    list.tabIndex = 0;
    list.style.backgroundColor = colors.background;
    const modal = { el: backdrop };
    let open = true;
    const finish = (v) => {
      if (!open) return;
      open = false;
      popModal(modal);
      resolve(v);
    };

    // Reordering by mouse, not HTML5 drag & drop: the webview's native drag-drop handler
    // takes the drag events, so HTML5 `drop` never fires (as in the hotlist dialog).
    let rowDrag = null; // {index, x, y, active}
    let rowEls = [];
    function render() {
      rowEls = work.map((rule, i) => {
        const row = el("div", "hrow trow", rule.masks);
        row.style.color = rule.color;
        row.addEventListener("mousedown", (e) => {
          if (e.button === 0) rowDrag = { index: i, x: e.clientX, y: e.clientY, active: false };
        });
        row.addEventListener("click", () => {
          // No rebuild here: a detached row would swallow the dblclick that follows.
          sel = i;
          paintSel();
          list.focus();
        });
        row.addEventListener("dblclick", () => edit());
        return row;
      });
      list.replaceChildren(...rowEls);
      paintSel();
    }
    function paintSel() {
      rowEls.forEach((row, i) => {
        row.classList.toggle("sel", i === sel);
        row.style.backgroundColor = i === sel ? colors.cursor : "";
      });
      rowEls[sel]?.scrollIntoView({ block: "nearest" });
      editBtn.disabled = delBtn.disabled = sel < 0;
      upBtn.disabled = sel <= 0;
      downBtn.disabled = sel < 0 || sel >= work.length - 1;
    }

    const rowAt = (x, y) => {
      const hit = document.elementFromPoint(x, y)?.closest(".trow");
      return rowEls.indexOf(hit);
    };
    const paintDropRow = (target) => {
      rowEls.forEach((row, i) => row.classList.toggle("dropover", i === target));
    };
    backdrop.addEventListener("mousemove", (e) => {
      if (!rowDrag) return;
      if (!(e.buttons & 1)) {
        // The button went up outside the window.
        rowDrag = null;
        paintDropRow(-1);
        return;
      }
      if (!rowDrag.active) rowDrag.active = dragStarted(rowDrag, { x: e.clientX, y: e.clientY });
      if (rowDrag.active) paintDropRow(rowAt(e.clientX, e.clientY));
    });
    backdrop.addEventListener("mouseup", (e) => {
      const d = rowDrag;
      rowDrag = null;
      paintDropRow(-1);
      if (!d?.active) return;
      const target = rowAt(e.clientX, e.clientY);
      if (target < 0) return;
      const p = relocateNode(work, [d.index], [target]);
      if (p) sel = p[0];
      render();
    });

    const move = (delta) => {
      if (sel < 0) return;
      sel = moveNode(work, [sel], delta)[0];
      render();
    };
    const moveSel = (i) => {
      if (!work.length) return;
      sel = Math.min(work.length - 1, Math.max(0, i));
      paintSel();
    };
    const add = async () => {
      const r = await editRule("Add", { masks: "", color: colors.font || "#ffffff" });
      if (r) {
        sel = sel < 0 ? work.length : sel + 1;
        work.splice(sel, 0, r);
        render();
      }
      list.focus();
    };
    const edit = async () => {
      if (sel < 0) return;
      const r = await editRule("Edit", work[sel]);
      if (r) {
        work[sel] = r;
        render();
      }
      list.focus();
    };
    const del = async () => {
      if (sel < 0) return;
      const yes = await dialog({
        title: "Delete",
        text: `Delete the rule for ${work[sel].masks}?`,
        buttons: [
          { label: "Yes", value: "yes", default: true },
          { label: "No", value: null },
        ],
      });
      if (yes === "yes") {
        work.splice(sel, 1);
        sel = Math.min(sel, work.length - 1);
        render();
      }
      list.focus();
    };

    const keys = `${comboLabel("Ctrl+Up", mods())} / ${comboLabel("Ctrl+Down", mods())}`;
    const addBtn = button("Add…", add);
    const editBtn = button("Edit…", edit);
    const delBtn = button("Delete", del);
    const upBtn = button("Move up", () => move(-1));
    const downBtn = button("Move down", () => move(1));
    upBtn.title = downBtn.title = `Move the rule (${keys}, or drag it)`;
    const okBtn = button("OK", () => finish(work));
    okBtn.classList.add("def");
    const side = el("div", "hotbtns");
    side.append(addBtn, editBtn, delBtn, upBtn, downBtn, el("div", "gap"), okBtn, button("Cancel", () => finish(null)));
    const left = el("div", "hotleft");
    left.append(el("div", "hothead", "First matching rule wins (files and folders):"), list);
    const main = el("div", "hotmain");
    main.append(left, side);
    box.append(el("div", "title", "Define colors by file type"), main);
    backdrop.append(box);

    modal.onKey = (e) => {
      const onList = document.activeElement === list;
      if (e.key === "Escape") {
        e.preventDefault();
        finish(null);
      } else if (e.key === "Tab") {
        e.preventDefault();
        cycleFocus(box, e, ", .typelist");
      } else if (tcModifiers(e, mods()).ctrl && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        move(e.key === "ArrowUp" ? -1 : 1);
      } else if (onList && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        e.preventDefault();
        moveSel(sel + (e.key === "ArrowUp" ? -1 : 1));
      } else if (onList && (e.key === "Home" || e.key === "End")) {
        e.preventDefault();
        moveSel(e.key === "Home" ? 0 : work.length - 1);
      } else if (onList && e.key === "Delete") {
        e.preventDefault();
        del();
      } else if (onList && (e.key === "Enter" || e.key === " ")) {
        e.preventDefault();
        edit();
      } else if (e.key === "Enter" && document.activeElement?.tagName !== "BUTTON") {
        e.preventDefault();
        okBtn.click();
      }
    };

    render();
    pushModal(modal);
    list.focus();
  });
}
