// Small DOM helpers shared by the windows (main, Find files, Synchronize dirs, viewer dialogs).

export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Tab / Shift+Tab: focus the next / previous of `items` (wrapping). */
export function cycleIn(items, e) {
  if (!items.length) return;
  let i = items.indexOf(document.activeElement);
  i = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i + 1) % items.length;
  items[i].focus();
}

/** Keeps Tab / Shift+Tab inside `root`: enabled buttons, inputs and selects (plus `extra`), not hidden ones. */
export function cycleFocus(root, e, extra = "") {
  const sel = `button:not(:disabled), input:not(:disabled), select:not(:disabled)${extra}`;
  cycleIn([...root.querySelectorAll(sel)].filter((x) => !x.closest("[hidden]")), e);
}

/** A dialog skeleton: backdrop, grey box (class `dialog` plus `cls`), title, body and button row. */
export function frame(title, cls = "") {
  const backdrop = el("div", "backdrop");
  const box = el("div", cls ? `dialog ${cls}` : "dialog");
  const body = el("div", "body");
  const row = el("div", "buttons");
  box.append(el("div", "title", title), body, row);
  backdrop.append(box);
  return { backdrop, modal: { el: backdrop }, box, body, row };
}
