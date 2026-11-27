// Ctrl+D directory hotlist: the popup menu and the TC "Change directory menu" dialog.

import { el } from "./dom.js";
import {
  addCurrent,
  comboLabel,
  dragStarted,
  flattenNodes,
  isRemote,
  menuLabel,
  moveNode,
  onRoot,
  parentList,
  parseCd,
  parseRemote,
  pickerStart,
  relocateNode,
  remoteRoot,
  selectedPosition,
  tcModifiers,
  toggleHidden,
  visibleNodes,
} from "./lib.mjs";
import { dialog, popModal, pushModal } from "./dialogs.js";
import { connectFlow } from "./servers.js";

/**
 * {invoke, hotlist(), setHotlist(h), home, active(), folder() (the active panel's real folder:
 * for a search results list, the folder it came from), other(), status(text), mods()} from app.js.
 */
let ctx = null;

export function initHotlist(c) {
  ctx = c;
}

async function save(hotlist) {
  await ctx.invoke("save_hotlist", { hotlist });
  ctx.setHotlist(hotlist);
}

// ---------------------------------------------------------------- choosing

/**
 * Where a hotlist location goes: local paths as they are; a server location on its open
 * connection, after connecting when it is not open (null when that was cancelled or failed).
 * The connection's scheme wins over the one written in the hotlist.
 */
async function resolve(loc) {
  const r = parseRemote(loc);
  if (!r) return loc;
  const open = (await ctx.invoke("list_connections")).find((c) => c.name === r.server);
  if (open) return onRoot(open.root, loc);
  const at = await connectFlow({ server: r.server }, { navigate: false });
  return at ? onRoot(remoteRoot(at), loc) : null;
}

/** Goes to the item's location in the active panel and its target path in the other one. */
async function choose(item) {
  const loc = parseCd(item.command, ctx.home);
  if (loc === null) return ctx.status(`Not a cd command: ${item.command}`);
  const target = item.target.trim() ? parseCd(`cd ${item.target}`, ctx.home) : null;
  const bad = [loc, target].find((l) => l && isRemote(l) && !parseRemote(l));
  if (bad) return ctx.status(`Unsupported location: ${bad}`);
  // The panels as they were when the item was chosen (connecting may take a while).
  const active = ctx.active();
  const other = ctx.other();
  const to = await resolve(loc);
  if (to === null) return;
  const toTarget = target ? await resolve(target) : null;
  active.go(to);
  if (toTarget) other.go(toTarget);
}

async function addCurrentDir() {
  const h = structuredClone(ctx.hotlist());
  addCurrent(h.nodes, ctx.folder(), ctx.home);
  await save(h);
  ctx.status(`Added to hotlist: ${h.nodes[h.nodes.length - 1].title}`);
}

// ---------------------------------------------------------------- popup

/** Ctrl+D: the hotlist popup at the top-left of the active panel. */
export function showHotlist() {
  const hotlist = ctx.hotlist();
  const root = visibleNodes(hotlist.nodes, hotlist.hideChar);
  const panelRect = document.getElementById(ctx.active().side).getBoundingClientRect();
  const backdrop = el("div", "backdrop clear");
  const modal = { el: backdrop };
  const close = () => popModal(modal);
  // levels[0] is the main menu; each deeper level is an open submenu to the right.
  const levels = [];

  const levelEntries = (nodes, top) => {
    const entries = nodes.map((node) => ({ node, ...menuLabel(node.title) }));
    if (top) {
      entries.push({ sep: true });
      entries.push({ action: addCurrentDir, label: "Add current dir", accel: "a", accelIndex: 4 });
      entries.push({ action: () => configureHotlist(), label: "Configure…", accel: "c", accelIndex: 0 });
    }
    return entries;
  };

  const run = (entry) => {
    if (entry.node?.kind === "submenu") return openSub(levels.length - 1);
    close();
    const action = entry.action ?? (() => choose(entry.node));
    Promise.resolve(action()).catch((e) => ctx.status(`Hotlist: ${e?.message ?? e}`));
  };

  const paint = (lv) => lv.rows.forEach((r, i) => r?.classList.toggle("sel", i === lv.sel));
  const select = (lv, i) => {
    lv.sel = i;
    paint(lv);
  };
  const selectable = (lv, i) => !lv.entries[i].sep;
  const step = (lv, d) => {
    let i = lv.sel;
    for (let n = 0; n < lv.entries.length; n++) {
      i = (i + d + lv.entries.length) % lv.entries.length;
      if (selectable(lv, i)) return select(lv, i);
    }
  };

  function openLevel(nodes, top, x, y) {
    const menu = el("div", "popup");
    menu.style.left = `${x}px`;
    menu.style.top = `${y}px`;
    const entries = levelEntries(nodes, top);
    const lv = { menu, entries, rows: [], sel: entries.findIndex((e) => !e.sep) };
    entries.forEach((entry, i) => {
      if (entry.sep) {
        lv.rows.push(null);
        return menu.append(el("div", "sep"));
      }
      const row = el("div", "item");
      const chars = [...entry.label];
      if (entry.accelIndex >= 0) {
        row.append(chars.slice(0, entry.accelIndex).join(""));
        row.append(el("u", null, chars[entry.accelIndex]));
        row.append(chars.slice(entry.accelIndex + 1).join(""));
      } else {
        row.textContent = entry.label;
      }
      if (entry.node?.kind === "submenu") row.append(el("span", "arrow", "▸"));
      row.addEventListener("mouseenter", () => {
        const at = levels.indexOf(lv);
        closeAbove(at);
        select(lv, i);
      });
      row.addEventListener("click", (e) => {
        e.stopPropagation();
        select(lv, i);
        run(entry);
      });
      lv.rows.push(row);
      menu.append(row);
    });
    backdrop.append(menu);
    levels.push(lv);
    paint(lv);
    // Keep the menu on screen.
    const r = menu.getBoundingClientRect();
    if (r.right > window.innerWidth) menu.style.left = `${Math.max(0, window.innerWidth - r.width)}px`;
    if (r.bottom > window.innerHeight) menu.style.top = `${Math.max(0, window.innerHeight - r.height)}px`;
    return lv;
  }

  function closeAbove(i) {
    while (levels.length > i + 1) levels.pop().menu.remove();
  }

  function openSub(i) {
    const lv = levels[i];
    const entry = lv.entries[lv.sel];
    if (entry?.node?.kind !== "submenu") return;
    closeAbove(i);
    const r = lv.rows[lv.sel].getBoundingClientRect();
    const mr = lv.menu.getBoundingClientRect();
    openLevel(entry.node.children, false, mr.right - 2, r.top - 3);
  }

  modal.onKey = (e) => {
    const lv = levels[levels.length - 1];
    if (e.key === "Escape" || e.key === "ArrowLeft") {
      e.preventDefault();
      if (levels.length > 1) closeAbove(levels.length - 2);
      else if (e.key === "Escape") close();
      return;
    }
    if (e.key === "Tab") return e.preventDefault();
    if (tcModifiers(e, ctx.mods()).any) return;
    if (e.key === "ArrowDown") step(lv, 1);
    else if (e.key === "ArrowUp") step(lv, -1);
    else if (e.key === "Home") select(lv, lv.entries.findIndex((x) => !x.sep));
    else if (e.key === "End") select(lv, lv.entries.findLastIndex((x) => !x.sep));
    else if (e.key === "ArrowRight") {
      if (lv.entries[lv.sel]?.node?.kind === "submenu") openSub(levels.length - 1);
    } else if (e.key === "Enter") {
      if (lv.entries[lv.sel]) run(lv.entries[lv.sel]);
    } else if (e.key.length === 1) {
      const k = e.key.toLowerCase();
      const i = lv.entries.findIndex((x) => x.accel === k);
      if (i >= 0) {
        select(lv, i);
        run(lv.entries[i]);
      }
    }
    e.preventDefault();
  };
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) close();
  });
  openLevel(root, true, panelRect.left + 4, panelRect.top + 4);
  pushModal(modal);
}

// ---------------------------------------------------------------- configure dialog

const HELP = [
  "Command: what happens when the item is chosen. Use \"cd <path>\" to change the active panel's folder; ~ stands for your home folder.",
  "Target path: optional folder for the other panel. Leave empty to leave the other panel where it is.",
  "Put & before a letter in the title to make it the item's accelerator key (&& for a literal &).",
];

/** The TC "Change directory menu" dialog; OK saves, Cancel discards. */
export function configureHotlist() {
  const work = structuredClone(ctx.hotlist());
  let sel = work.nodes.length ? [0] : null;
  const nodeAt = (p) => parentList(work.nodes, p)[p[p.length - 1]];

  const backdrop = el("div", "backdrop");
  const box = el("div", "dialog hotcfg");
  const list = el("div", "hotlist");
  list.tabIndex = 0;
  const count = el("span", "count");
  const head = el("div", "hothead");
  head.append(el("span", null, "Directory menu:"), count);

  const mkBtn = (label, fn) => {
    const b = el("button", null, label);
    b.addEventListener("click", fn);
    return b;
  };
  const cmd = el("input");
  const target = el("input");
  for (const i of [cmd, target]) {
    i.type = "text";
    i.spellcheck = false;
  }
  const browse = mkBtn("🔍", async () => {
    const dir = await window.__TAURI__.dialog.open({
      directory: true,
      defaultPath: pickerStart(cmd.value, ctx.home, ctx.folder()),
    });
    if (typeof dir === "string" && sel && nodeAt(sel).kind === "item") {
      cmd.value = `cd ${dir}`;
      nodeAt(sel).command = cmd.value;
    }
  });
  browse.classList.add("browse");
  browse.title = "Choose a folder";
  const hideBox = el("input");
  hideBox.type = "checkbox";
  hideBox.checked = work.hideChar != null;
  const hideChar = el("input", "hidechar");
  hideChar.maxLength = 1;
  hideChar.spellcheck = false;
  hideChar.value = work.hideChar ?? "?";
  const syncHide = () => {
    work.hideChar = hideBox.checked && hideChar.value ? hideChar.value : null;
  };
  hideBox.addEventListener("change", syncHide);
  hideChar.addEventListener("input", syncHide);

  const modal = { el: backdrop };
  const close = () => popModal(modal);
  const up = mkBtn("↑", () => move(-1));
  const down = mkBtn("↓", () => move(1));
  up.title = down.title = `Move item (${comboLabel("Ctrl+Up", ctx.mods())} / ${comboLabel("Ctrl+Down", ctx.mods())})`;
  up.classList.add("mv");
  down.classList.add("mv");
  head.insertBefore(up, count);
  head.insertBefore(down, count);

  // Reordering by mouse, not HTML5 drag & drop: the webview's native drag-drop handler
  // (file drops onto the panels) takes the drag events, so HTML5 `drop` never fires.
  let rowDrag = null; // {path, x, y, active}
  let rowEls = [];
  function render() {
    const rows = flattenNodes(work.nodes);
    list.replaceChildren();
    rowEls = [];
    rows.forEach((r) => {
      const row = el("div", "hrow", `${"--".repeat(r.depth)}${r.depth ? " " : ""}${menuLabel(r.node.title).label}`);
      if (r.node.kind === "submenu") row.classList.add("submenu");
      rowEls.push({ row, path: r.path });
      row.addEventListener("mousedown", (e) => {
        if (e.button === 0) rowDrag = { path: r.path, x: e.clientX, y: e.clientY, active: false };
      });
      row.addEventListener("click", () => {
        // No rebuild here: a detached row would swallow the dblclick that follows.
        sel = r.path;
        paintSel();
        list.focus();
      });
      row.addEventListener("dblclick", () => {
        if (!hideBox.checked) {
          hideBox.checked = true;
          if (!hideChar.value) hideChar.value = "?";
          syncHide();
        }
        r.node.title = toggleHidden(r.node.title, hideChar.value);
        render();
      });
      list.append(row);
    });
    paintSel();
  }

  /** Row under the pointer during a reorder drag, or null. */
  const rowAt = (x, y) => {
    const hit = document.elementFromPoint(x, y)?.closest(".hrow");
    return rowEls.find((r) => r.row === hit) ?? null;
  };
  const paintDropRow = (target) => {
    for (const r of rowEls) r.row.classList.toggle("dropover", r === target);
  };
  backdrop.addEventListener("mousemove", (e) => {
    if (!rowDrag) return;
    if (!(e.buttons & 1)) {
      // The button went up outside the window.
      rowDrag = null;
      paintDropRow(null);
      return;
    }
    if (!rowDrag.active) rowDrag.active = dragStarted(rowDrag, { x: e.clientX, y: e.clientY });
    if (rowDrag.active) paintDropRow(rowAt(e.clientX, e.clientY));
  });
  backdrop.addEventListener("mouseup", (e) => {
    const d = rowDrag;
    rowDrag = null;
    paintDropRow(null);
    if (!d?.active) return;
    const target = rowAt(e.clientX, e.clientY);
    if (!target) return;
    const p = relocateNode(work.nodes, d.path, target.path);
    if (p) sel = p;
    render();
  });

  /** Selection changed, structure did not: update highlight, count and the fields in place. */
  function paintSel() {
    const key = sel?.join(".");
    for (const { row, path } of rowEls) row.classList.toggle("sel", path.join(".") === key);
    count.textContent = selectedPosition(work.nodes, sel) ?? "";
    list.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
    const n = sel ? nodeAt(sel) : null;
    const isItem = n?.kind === "item";
    cmd.disabled = target.disabled = browse.disabled = !isItem;
    cmd.value = isItem ? n.command : "";
    target.value = isItem ? n.target : "";
  }
  cmd.addEventListener("input", () => {
    if (sel) nodeAt(sel).command = cmd.value;
  });
  target.addEventListener("input", () => {
    if (sel) nodeAt(sel).target = target.value;
  });

  const move = (delta) => {
    if (sel) sel = moveNode(work.nodes, sel, delta);
    render();
  };
  /** Selection moves through the flat rows (not only within a parent). */
  const moveSel = (delta) => {
    const rows = flattenNodes(work.nodes);
    if (!rows.length) return;
    const i = sel ? rows.findIndex((r) => r.path.join(".") === sel.join(".")) : -1;
    sel = rows[Math.min(rows.length - 1, Math.max(0, i + delta))].path;
    paintSel();
  };

  /** Where a new node goes: inside the selected submenu, else right after the selection, else at the end. */
  const insertNew = (node) => {
    if (sel && nodeAt(sel).kind === "submenu") {
      nodeAt(sel).children.push(node);
      sel = [...sel, nodeAt(sel).children.length - 1];
    } else if (sel) {
      parentList(work.nodes, sel).splice(sel[sel.length - 1] + 1, 0, node);
      sel = [...sel.slice(0, -1), sel[sel.length - 1] + 1];
    } else {
      work.nodes.push(node);
      sel = [work.nodes.length - 1];
    }
    render();
  };
  const askTitle = async (title, initial) => {
    const r = await dialog({
      title,
      text: "Menu title (put & before the accelerator letter):",
      input: initial,
      buttons: [
        { label: "OK", value: "ok", default: true },
        { label: "Cancel", value: null },
      ],
    });
    return r?.text.trim() || null;
  };
  const addItem = async () => {
    const loc = ctx.folder();
    const tmp = [];
    addCurrent(tmp, loc, ctx.home);
    const t = await askTitle("Add Item", tmp[0].title);
    if (t) insertNew({ ...tmp[0], title: t });
  };
  const addSub = async () => {
    const t = await askTitle("Add submenu", "");
    if (t) insertNew({ kind: "submenu", title: t, children: [] });
  };
  const del = () => {
    if (!sel) return;
    const l = parentList(work.nodes, sel);
    const i = sel[sel.length - 1];
    l.splice(i, 1);
    if (l.length) sel = [...sel.slice(0, -1), Math.min(i, l.length - 1)];
    else sel = sel.length > 1 ? sel.slice(0, -1) : null;
    render();
  };
  const changeTitle = async () => {
    if (!sel) return;
    const n = nodeAt(sel);
    const t = await askTitle("Change Title", n.title);
    if (t) {
      n.title = t;
      render();
    }
  };
  const ok = async () => {
    try {
      syncHide();
      await save(work);
      close();
    } catch (e) {
      ctx.status(`Cannot save hotlist: ${e?.message ?? e}`);
    }
  };
  const help = () =>
    dialog({ title: "Help", text: HELP, buttons: [{ label: "OK", value: "ok", default: true }] });

  const side = el("div", "hotbtns");
  const okBtn = mkBtn("OK", ok);
  okBtn.classList.add("def");
  side.append(
    mkBtn("Add Item…", addItem),
    mkBtn("Add submenu…", addSub),
    mkBtn("Delete Item", del),
    mkBtn("Change Title…", changeTitle),
    el("div", "gap"),
    okBtn,
    mkBtn("Cancel", close),
    mkBtn("Help", help),
  );
  const field = (label, ...kids) => {
    const r = el("div", "hotfield");
    r.append(el("label", null, label), ...kids);
    return r;
  };
  const left = el("div", "hotleft");
  const cmdRow = field("Command:", cmd, browse);
  const hideRow = el("div", "hotfield hide");
  const hideLabel = el("label", null);
  hideLabel.append(hideBox, " Hide menu items starting with this character:");
  hideRow.append(hideLabel, hideChar);
  left.append(head, list, cmdRow, field("Target path:", target), hideRow);
  left.append(el("div", "hotnote", "Double click menu item to hide/show it (add/remove the character)"));
  const main = el("div", "hotmain");
  main.append(left, side);
  box.append(el("div", "title", "Change directory menu"), main);
  backdrop.append(box);

  modal.onKey = (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      const items = [...box.querySelectorAll("button:not(:disabled), input:not(:disabled), .hotlist")];
      let i = items.indexOf(document.activeElement);
      i = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i + 1) % items.length;
      items[i].focus();
    } else if (tcModifiers(e, ctx.mods()).ctrl && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      move(e.key === "ArrowUp" ? -1 : 1);
    } else if (document.activeElement === list && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      moveSel(e.key === "ArrowUp" ? -1 : 1);
    } else if (document.activeElement === list && (e.key === "Home" || e.key === "End")) {
      e.preventDefault();
      const rows = flattenNodes(work.nodes);
      if (rows.length) sel = rows[e.key === "Home" ? 0 : rows.length - 1].path;
      paintSel();
    } else if (document.activeElement === list && e.key === "Delete") {
      e.preventDefault();
      del();
    } else if (e.key === "Enter" && document.activeElement?.tagName !== "BUTTON") {
      e.preventDefault();
      ok();
    }
  };

  render();
  pushModal(modal);
  list.focus();
}
