// Modal dialogs and the TC file operations behind them (F5/F6/F7/F8, Alt+F5, rename, masks).

import {
  archiveOf,
  copyTargetDefault,
  deleteConfirmText,
  editTexts,
  firstComponent,
  formatDate,
  isLocalFolder,
  formatK,
  groupThousands,
  jobErrorBox,
  jobServers,
  joinLocation,
  jobSucceeded,
  lastComponent,
  modalKeyBlocked,
  newNameError,
  operationPaths,
  operationSources,
  PACK_LOCAL_ONLY,
  packArchiveName,
  packCursor,
  packDialogText,
  packedText,
  packFormError,
  packTargetDir,
  parentLocation,
  parseRemote,
  passwordArchive,
  permanentConfirmText,
  renameDecision,
  renameSelection,
  suggestRename,
  uploadSucceeded,
} from "./lib.mjs";

// ---------------------------------------------------------------- modal stack

const modals = [];

/** The top modal; it gets every keydown first (see app.js onKeyDown). */
export function topModal() {
  return modals[modals.length - 1] ?? null;
}

/** Progress views waiting until no question/prompt is on top of the stack. */
const deferredViews = new Set();

/** True when the top modal is a question, prompt or inline editor (not a progress view). */
function blockingTop() {
  const t = topModal();
  return Boolean(t) && !t.progress;
}

/**
 * m: {el, onKey(e), progress?, noDelay?}; `el` is appended to `host` (default: body).
 * `noDelay` skips the initial key delay (inline editors, where typing starts at once).
 */
export function pushModal(m, host = document.body) {
  m.openedAt = performance.now();
  m.prevFocus = document.activeElement;
  modals.push(m);
  host.append(m.el);
}

export function popModal(m) {
  const i = modals.indexOf(m);
  if (i < 0) return;
  const wasTop = i === modals.length - 1;
  modals.splice(i, 1);
  m.el.remove();
  // Back to what had focus underneath (e.g. the dialog below), never the body.
  if (wasTop && m.prevFocus?.isConnected) m.prevFocus.focus();
  if (!blockingTop()) {
    for (const job of [...deferredViews]) {
      deferredViews.delete(job);
      job.showView();
    }
  }
}

/**
 * Gives a keydown to the top modal. Returns false when no modal is open.
 * Stray activation keys (Enter/Escape/Space-on-button just after opening,
 * auto-repeated Enter) are swallowed; typing always passes.
 */
export function handleModalKey(e) {
  const top = topModal();
  if (!top) return false;
  const onButton = document.activeElement?.tagName === "BUTTON";
  const since = top.noDelay ? Infinity : performance.now() - top.openedAt;
  if (modalKeyBlocked({ key: e.key, repeat: e.repeat, onButton }, since)) {
    e.preventDefault();
    return true;
  }
  top.onKey(e);
  return true;
}

/** Keeps Tab / Shift+Tab inside `root`. */
function cycleFocus(root, e) {
  const items = [...root.querySelectorAll("button:not(:disabled), input:not(:disabled), select:not(:disabled)")];
  if (!items.length) return;
  let i = items.indexOf(document.activeElement);
  i = e.shiftKey ? (i <= 0 ? items.length - 1 : i - 1) : (i + 1) % items.length;
  items[i].focus();
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Backdrop + grey dialog box with a title; returns {backdrop, box, body, row}. */
function frame(title) {
  const backdrop = el("div", "backdrop");
  const box = el("div", "dialog");
  const body = el("div", "body");
  const row = el("div", "buttons");
  box.append(el("div", "title", title), body, row);
  backdrop.append(box);
  return { backdrop, box, body, row };
}

/**
 * Modal dialog. buttons: [{label, value, default?}]; resolves with the clicked
 * value (Enter = focused or default button, Escape = null). With `input`
 * (initial text) it resolves with {value, text}; `select` ({start, end})
 * limits the initial selection. `ctl.close()` (set by the dialog) closes it
 * with null. `danger` shows the title in red (a warning).
 */
export function dialog({ title, text, input, inputType = "text", select, buttons, ctl, danger = false }) {
  return new Promise((resolve) => {
    const { backdrop, box, body, row } = frame(title);
    if (danger) box.classList.add("danger");
    for (const line of [].concat(text ?? [])) body.append(el("p", null, line));
    let field = null;
    if (input !== undefined) {
      field = el("input");
      field.type = inputType;
      field.spellcheck = false;
      if (inputType === "password") field.autocomplete = "off";
      field.value = input;
      body.append(field);
    }
    const modal = { el: backdrop };
    let open = true;
    const finish = (value) => {
      if (!open) return;
      open = false;
      popModal(modal);
      resolve(field && value !== null ? { value, text: field.value } : value);
    };
    if (ctl) ctl.close = () => finish(null);
    let def = null;
    for (const b of buttons) {
      const btn = el("button", null, b.label);
      btn.addEventListener("click", () => finish(b.value));
      if (b.default) def = btn;
      row.append(btn);
    }
    modal.onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        finish(null);
      } else if (e.key === "Tab") {
        e.preventDefault();
        cycleFocus(box, e);
      } else if (e.key === "Enter") {
        e.preventDefault();
        const focused = document.activeElement;
        const btn = focused?.tagName === "BUTTON" && box.contains(focused) ? focused : def;
        if (btn) btn.click();
      }
    };
    pushModal(modal);
    if (field) {
      field.focus();
      if (select) field.setSelectionRange(select.start, select.end);
      else field.select();
    } else {
      (def ?? row.querySelector("button"))?.focus();
    }
  });
}

const OK_CANCEL = [
  { label: "OK", value: "ok", default: true },
  { label: "Cancel", value: null },
];

/** Single-line input dialog; resolves with the trimmed text, or null (cancelled/empty). */
async function prompt(title, text, input = "", select) {
  const r = await dialog({ title, text, input, select, buttons: OK_CANCEL });
  const t = r?.text.trim();
  return t ? t : null;
}

/** Text of an error that came back from a command (string), a job (`{kind, message}`) or Error. */
function errorText(e) {
  return typeof e === "string" ? e : (e?.message ?? "");
}

/**
 * Asks for the password of the archive named in a "password required: <path>" /
 * "wrong password: <path>" error and hands it to the backend. False when the
 * error is not a password error or the prompt was cancelled.
 */
export async function askPassword(error, ctl) {
  const text = errorText(error);
  const archive = passwordArchive(text);
  if (!archive) return false;
  const name = lastComponent(archive);
  const wrong = text.startsWith("wrong password");
  const r = await dialog({
    title: "Password",
    text: wrong ? `Wrong password for ${name}. Try again:` : `Password for ${name}:`,
    input: "",
    inputType: "password",
    buttons: OK_CANCEL,
    ctl,
  });
  if (!r || !r.text) return false;
  await ctx.invoke("set_archive_password", { archive, password: r.text });
  return true;
}

/**
 * Runs `fn`; on a password error asks for the password and runs it again
 * (again after a wrong one, until it works, fails otherwise or is cancelled).
 */
export async function withPassword(fn) {
  for (;;) {
    try {
      return await fn();
    } catch (e) {
      if (!(await askPassword(e))) throw e;
    }
  }
}

function message(title, text) {
  return dialog({ title, text, buttons: [{ label: "OK", value: "ok", default: true }] });
}

// ---------------------------------------------------------------- jobs

let ctx = null;
const jobs = new Map(); // job id -> Job
const early = new Map(); // job id -> events that arrived before start_job returned

function route(type, payload) {
  const job = jobs.get(payload.job);
  if (job) job.handle(type, payload);
  else {
    if (!early.has(payload.job)) early.set(payload.job, []);
    early.get(payload.job).push([type, payload]);
  }
}

const TITLES = { copy: "Copying", move: "Moving", delete: "Deleting", upload: "Uploading", pack: "Packing" };

/** One background job: progress dialog (after 300 ms), conflict and error questions. */
class Job {
  /** `start()` resolves with the job id (default: `start_job` with `req`). */
  constructor(req, start = () => ctx.invoke("start_job", { req })) {
    this.req = req;
    this.start = start;
    this.id = null;
    this.progress = null;
    this.view = null; // progress dialog
    this.question = null; // ctl of the open conflict/error dialog
    this.followUp = null; // job request to run after this one ends
    this.timer = 0;
  }

  /** Runs the job; resolves with the job://done payload. */
  async run() {
    const done = new Promise((resolve) => {
      this.resolve = resolve;
    });
    this.id = await this.start();
    jobs.set(this.id, this);
    this.timer = setTimeout(() => this.showView(), 300);
    for (const [type, payload] of early.get(this.id) ?? []) this.handle(type, payload);
    early.delete(this.id);
    return done;
  }

  handle(type, p) {
    if (type === "progress") {
      this.progress = p.progress;
      this.paint();
    } else if (type === "conflict") {
      this.askConflict(p.info);
    } else if (type === "error") {
      this.askError(p.path, p.msg);
    } else if (type === "done") {
      this.finish(p);
    }
  }

  finish(payload) {
    clearTimeout(this.timer);
    jobs.delete(this.id);
    deferredViews.delete(this);
    this.question?.close();
    if (this.view) popModal(this.view.modal);
    this.view = null;
    this.resolve(payload);
  }

  cancel() {
    if (this.view) {
      this.view.cancelBtn.disabled = true;
      this.view.cancelBtn.textContent = "Cancelling…";
    }
    ctx.invoke("cancel_job", { job: this.id }).catch((e) => console.error("fxfm: cancel_job", e));
  }

  showView() {
    if (this.view || !jobs.has(this.id)) return;
    if (blockingTop()) {
      // Shown once the question/prompt is closed, so keys never go to the wrong job.
      deferredViews.add(this);
      return;
    }
    const { backdrop, box, body, row } = frame(TITLES[this.req.kind]);
    box.classList.add("progress");
    const current = el("p", "current");
    const filesLabel = el("p");
    const filesBar = el("div", "bar");
    const filesFill = el("div", "fill");
    filesBar.append(filesFill);
    const bytesLabel = el("p");
    const bytesBar = el("div", "bar");
    const bytesFill = el("div", "fill");
    bytesBar.append(bytesFill);
    body.append(current, filesLabel, filesBar, bytesLabel, bytesBar);
    const cancelBtn = el("button", null, "Cancel");
    cancelBtn.addEventListener("click", () => this.cancel());
    row.append(cancelBtn);
    const modal = { el: backdrop, progress: true };
    modal.onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (!cancelBtn.disabled) this.cancel();
      } else if (e.key === "Tab") {
        e.preventDefault();
        cycleFocus(box, e);
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (document.activeElement === cancelBtn && !cancelBtn.disabled) this.cancel();
      }
    };
    this.view = { modal, current, filesLabel, filesFill, bytesLabel, bytesFill, cancelBtn };
    pushModal(modal);
    this.paint();
  }

  paint() {
    const v = this.view;
    const p = this.progress;
    if (!v || !p) return;
    const ratio = (done, total) => `${total > 0 ? Math.min(100, (done / total) * 100) : 0}%`;
    v.current.textContent = p.current;
    v.filesLabel.textContent = `${groupThousands(p.doneFiles)} / ${groupThousands(p.totalFiles)} file(s)`;
    v.filesFill.style.width = ratio(p.doneFiles, p.totalFiles);
    v.bytesLabel.textContent = `${formatK(p.doneBytes)} / ${formatK(p.totalBytes)}`;
    v.bytesFill.style.width = ratio(p.doneBytes, p.totalBytes);
  }

  /** Opens a question dialog (progress views wait while it is open). */
  async ask(opts) {
    this.question = {};
    const r = await dialog({ ...opts, ctl: this.question });
    this.question = null;
    return r;
  }

  async askConflict(info) {
    const fileLine = (label, size, mtime) => `${label}: ${groupThousands(size)} bytes, ${formatDate(mtime)}`;
    let answer = null;
    while (!answer) {
      const r = await this.ask({
        title: "File exists",
        text: [
          "File exists",
          info.target,
          fileLine("Source", info.sourceSize, info.sourceMtime),
          fileLine("Target", info.targetSize, info.targetMtime),
        ],
        buttons: [
          { label: "Overwrite", value: "overwrite", default: true },
          { label: "Overwrite all", value: "overwriteAll" },
          { label: "Skip", value: "skip" },
          { label: "Skip all", value: "skipAll" },
          { label: "Overwrite older", value: "overwriteAllOlder" },
          { label: "Rename", value: "rename" },
          { label: "Cancel", value: "cancel" },
        ],
      });
      if (!jobs.has(this.id)) return; // the job ended while asking
      if (r === "rename") {
        const newName = await this.askRename(info.target);
        if (!jobs.has(this.id)) return;
        if (newName) answer = { kind: "rename", newName };
      } else {
        answer = { kind: r ?? "cancel" };
      }
    }
    ctx.invoke("answer_conflict", { job: this.id, answer }).catch((e) => console.error("fxfm: answer_conflict", e));
  }

  /** New name for the conflicting target, suggested as "name (2).ext". */
  async askRename(target) {
    const name = lastComponent(target);
    let names = [];
    try {
      names = (await ctx.invoke("list_dir", { location: parentLocation(target) })).map((e) => e.name);
    } catch {
      // The suggestion then only avoids the conflicting name itself.
    }
    const suggestion = suggestRename(name, new Set([name, ...names]));
    this.question = {};
    const r = await dialog({
      title: "Rename",
      text: "New name:",
      input: suggestion,
      select: renameSelection(suggestion),
      buttons: OK_CANCEL,
      ctl: this.question,
    });
    this.question = null;
    return r?.text.trim() || null;
  }

  async askError(path, msg) {
    const trash = this.req.kind === "delete" && this.req.toTrash && msg === "Cannot move to Trash";
    const buttons = [
      { label: "Retry", value: "retry", default: true },
      { label: "Skip", value: "skip" },
      { label: "Skip all", value: "skipAll" },
      { label: "Cancel", value: "cancel" },
    ];
    if (trash) buttons.splice(3, 0, { label: "Delete permanently", value: "permanent" });
    const r = await this.ask({ title: "Error", text: [msg, path], buttons });
    if (!jobs.has(this.id)) return;
    let answer = { kind: r ?? "cancel" };
    if (r === "permanent") {
      // Stop this job; the items from the failed one on are deleted by a new job.
      const at = this.req.paths.indexOf(path);
      const rest = at < 0 ? [path] : this.req.paths.slice(at);
      this.followUp = { kind: "delete", paths: rest, toTrash: false };
      answer = { kind: "cancel" };
    }
    ctx.invoke("answer_error", { job: this.id, answer }).catch((e) => console.error("fxfm: answer_error", e));
  }
}

/**
 * Connects every server in `servers` that is not connected (the connect flow, no navigation).
 * Resolves with the names connected right before the job, or null when a connect was
 * cancelled or failed (the job must not start then).
 */
async function connectServers(servers) {
  if (!servers.length) return [];
  const open = async () => (await ctx.invoke("list_connections")).map((c) => c.name);
  const before = await open();
  for (const name of servers) {
    if (before.includes(name)) continue;
    if (!(await ctx.connectFlow({ server: name }, { navigate: false }))) {
      ctx.active().setMessage(`Not connected to ${name}`);
      return null;
    }
  }
  return servers.every((s) => before.includes(s)) ? before : open();
}

const INTO_ITSELF = {
  copy: "Cannot copy a folder into itself",
  move: "Cannot move a folder into itself",
};

const YES_NO = [
  { label: "Yes", value: "yes", default: true },
  { label: "No", value: null },
];

const OVERWRITE_CANCEL = [
  { label: "Overwrite", value: "overwrite", default: true },
  { label: "Cancel", value: null },
];

/**
 * Runs `req`, then refreshes both panels; the source panel's marks (if a panel
 * is given) are cleared only when the job ran to the end. Resolves with the last
 * job's `job://done` payload, or null when no job ran.
 */
async function runJob(req, source) {
  // A pack target is never a server (the backend refuses it), so it connects nothing.
  const servers = jobServers([...(req.sources ?? req.paths ?? []), req.dest, req.cwd]);
  const connected = await connectServers(servers);
  if (!connected) return null;
  const job = new Job(req);
  const done = await job.run();
  if (jobSucceeded(done)) source?.clearMarks();
  await Promise.all(ctx.panels().map((p) => p.refresh()));
  const err = done.error;
  // Password needed (or wrong): ask and run the same request again.
  if (err && passwordArchive(err.message) && (await askPassword(err))) return runJob(req, source);
  // Packing never replaces an archive without asking.
  if (err?.kind === "exists" && req.kind === "pack") {
    const r = await dialog({ title: "Pack files", text: `"${lastComponent(req.target)}" already exists.`, buttons: OVERWRITE_CANCEL });
    if (r !== "overwrite") return done;
    const again = { ...req, overwrite: true };
    try {
      return await runJob(again, source);
    } finally {
      again.password = null;
    }
  }
  // A lost connection on servers that were all connected gets no box here:
  // `connections://changed` (sent after job://done) names the dead server(s) exactly, and app.js
  // opens their "connection lost" dialog.
  if (jobErrorBox(err, servers, connected)) {
    await message("Error", err.kind === "intoItself" ? INTO_ITSELF[req.kind] : err.message);
  }
  if (job.followUp) {
    const r = await dialog({ title: "Delete", text: permanentConfirmText(job.followUp.paths), buttons: YES_NO });
    if (r === "yes") return runJob(job.followUp, source);
  }
  return done;
}

// ---------------------------------------------------------------- upload back (F4 in archives)

let uploadQuestions = Promise.resolve();

/** `edit://changed`: one question at a time, in order of arrival. */
export function editChanged(payload) {
  uploadQuestions = uploadQuestions
    .then(() => askUpload(payload))
    .catch((e) => console.error("fxfm: upload back", e));
}

function confirmEdit(id, uploaded) {
  return ctx.invoke("edit_confirm", { id, uploaded }).catch((e) => console.error("fxfm: edit_confirm", e));
}

/** Asks to upload the changed copy; the backend learns whether the archive (or server) now has it. */
async function askUpload({ id, location, name }) {
  const texts = editTexts(location ?? "", name);
  const r = await dialog({
    title: "Edit",
    text: texts.upload,
    buttons: YES_NO,
  });
  if (r !== "yes") return confirmEdit(id, false);
  let done;
  for (let force = false; !done; force = true) {
    try {
      done = await new Job({ kind: "upload" }, () => ctx.invoke("upload_back", { id, force })).run();
    } catch (e) {
      const text = errorText(e);
      if (!force && text.startsWith("conflict: ")) {
        const overwrite = await dialog({
          title: "Edit",
          text: texts.conflict,
          buttons: YES_NO,
        });
        if (overwrite === "yes") continue;
        return confirmEdit(id, false);
      }
      await confirmEdit(id, false);
      return message("Error", `"${name}" was not uploaded: ${text}`);
    }
  }
  const ok = uploadSucceeded(done);
  await confirmEdit(id, ok);
  await Promise.all(ctx.panels().map((p) => p.refresh()));
  if (!ok) {
    const err = done.error && done.error.kind !== "cancelled" ? `: ${done.error.message}` : "";
    await message("Error", `"${name}" was not uploaded${err}`);
  }
}

// ---------------------------------------------------------------- commands

function sourcesOf(panel) {
  return operationSources(panel.entries, panel.marked, panel.cursorIndex);
}

const countText = (sources, files) =>
  sources.length === 1 ? `"${sources[0].name}"` : `${sources.length} ${files}`;

const VERB = { copy: "Copy", move: "Move" };

/** F5 / F6: the active panel's marked entries (or the one under the cursor) to the other panel. */
export async function copyMove(kind) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  // Snapshot before awaiting: a refresh may move the panel while the dialog is open.
  const { cwd, paths } = operationPaths(src.location, sources);
  await copyMoveTo(kind, { paths, cwd, dest: copyTargetDefault(ctx.other().location, sources), source: src });
}

/**
 * The F5/F6 dialog and job for an explicit source list (F5/F6 and drag & drop).
 * `paths`: full source locations; `cwd`: folder a relative target is resolved in; `dest`:
 * the dialog's initial target; `source`: panel whose marks a finished job clears (or null);
 * `switchable` adds a "Move instead" / "Copy instead" button (drops: Shift may be missed).
 */
export async function copyMoveTo(kind, { paths, cwd, dest, source = null, switchable = false }) {
  if (!paths.length) return;
  const sources = paths.map((p) => ({ name: lastComponent(p) }));
  const verb = VERB[kind];
  const otherKind = kind === "copy" ? "move" : "copy";
  const buttons = switchable
    ? [OK_CANCEL[0], { label: `${VERB[otherKind]} instead`, value: "switch" }, OK_CANCEL[1]]
    : OK_CANCEL;
  const r = await dialog({ title: verb, text: `${verb} ${countText(sources, "file(s)")} to:`, input: dest, buttons });
  const target = r?.text.trim();
  if (!target) return;
  await runJob({ kind: r.value === "switch" ? otherKind : kind, sources: paths, dest: target, cwd }, source);
}

/**
 * The Pack files dialog. Resolves with {target, password (or null), encryption} or null.
 * Enter = OK (unless Cancel has focus), Escape = Cancel.
 */
function packDialog(sources, initial) {
  return new Promise((resolve) => {
    const { backdrop, box, body, row } = frame("Pack files");
    box.classList.add("pack");
    const target = el("input");
    target.type = "text";
    target.spellcheck = false;
    target.value = initial;
    const line = (label, field) => {
      const l = el("label", "line");
      l.append(el("span", null, label), field);
      return l;
    };
    const format = el("select");
    const zipOpt = el("option", null, "ZIP");
    zipOpt.value = "zip";
    format.append(zipOpt);
    const encrypt = el("input");
    encrypt.type = "checkbox";
    const encryptLabel = el("label", "check gap");
    encryptLabel.append(encrypt, " Encrypt with a password");
    const password = el("input");
    const repeat = el("input");
    for (const f of [password, repeat]) {
      f.type = "password";
      f.autocomplete = "off";
      f.spellcheck = false;
    }
    const method = el("select");
    for (const [value, label] of [
      ["aes256", "AES-256 (secure)"],
      ["zipCrypto", "ZipCrypto (opens everywhere, weak)"],
    ]) {
      const o = el("option", null, label);
      o.value = value;
      method.append(o);
    }
    const pwBox = el("div", "pw");
    pwBox.append(
      line("Password", password),
      line("Repeat password", repeat),
      line("Encryption", method),
      el("p", "note", "AES-256 zips do not open with a double-click in Finder; use fxfm, Keka or 7-Zip."),
    );
    const error = el("p", "error");
    error.hidden = true;
    const showPw = () => {
      pwBox.hidden = !encrypt.checked;
      for (const f of [password, repeat, method]) f.disabled = !encrypt.checked;
    };
    encrypt.addEventListener("change", showPw);
    showPw();
    body.append(el("p", null, packDialogText(sources)), target, line("Format", format), encryptLabel, pwBox, error);

    const modal = { el: backdrop };
    let open = true;
    const finish = (value) => {
      if (!open) return;
      open = false;
      popModal(modal);
      // The fields are gone with the dialog; the value object is all that holds the password.
      password.value = "";
      repeat.value = "";
      resolve(value);
    };
    const accept = () => {
      const problem = packFormError({ target: target.value, encrypt: encrypt.checked, password: password.value, repeat: repeat.value });
      if (problem) {
        error.textContent = problem;
        error.hidden = false;
        (problem.includes("password") ? password : target).focus();
        return;
      }
      finish({
        target: target.value.trim(),
        password: encrypt.checked ? password.value : null,
        encryption: method.value,
      });
    };
    const okBtn = el("button", null, "OK");
    okBtn.addEventListener("click", accept);
    const cancelBtn = el("button", null, "Cancel");
    cancelBtn.addEventListener("click", () => finish(null));
    row.append(okBtn, cancelBtn);
    modal.onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        finish(null);
      } else if (e.key === "Tab") {
        e.preventDefault();
        cycleFocus(box, e);
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (document.activeElement === cancelBtn) finish(null);
        else accept();
      }
    };
    pushModal(modal);
    target.focus();
    const at = packCursor(target.value);
    target.setSelectionRange(at.start, at.end);
  });
}

/** Alt+F5: packs the marked entries (or the one under the cursor) into a zip. */
export async function packFiles() {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  if (!isLocalFolder(src.location)) {
    await message("Pack files", PACK_LOCAL_ONLY);
    return;
  }
  // Snapshot before awaiting: a refresh may move the panel while the dialog is open.
  const { cwd, paths } = operationPaths(src.location, sources);
  const dir = packTargetDir(src.location, ctx.other().location);
  const r = await packDialog(sources, joinLocation(dir, packArchiveName(sources, src.location)));
  if (!r) return;
  const req = { kind: "pack", sources: paths, cwd, target: r.target, password: r.password, encryption: r.encryption };
  r.password = null;
  const done = await runJob(req, src);
  req.password = null;
  if (!done?.summary || done.summary.cancelled || done.error) return;
  // The cursor goes to the new archive in the panel showing its folder (the target panel first).
  const target = r.target.startsWith("/") ? r.target : joinLocation(cwd, r.target);
  const folder = parentLocation(target);
  const name = lastComponent(target);
  const panel = [ctx.other(), src].find((p) => p.location === folder);
  if (panel) await panel.go(folder, name, { refresh: true });
  src.setMessage(packedText(done.summary, target));
}

/** F8 (Trash) / Shift+F8 (permanent). */
export async function deleteItems(toTrash) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  const { paths } = operationPaths(src.location, sources);
  // The backend ignores toTrash inside archives and on servers (always permanent); for
  // archives it also decides the wording.
  let inArchive = false;
  if (!parseRemote(src.location)) {
    try {
      inArchive = await ctx.invoke("location_in_archive", { location: src.location });
    } catch {
      // Wording falls back to the normal text; the backend still deletes correctly.
    }
  }
  const r = await dialog({
    title: "Delete",
    text: deleteConfirmText(src.location, sources.length, { inArchive, toTrash }),
    buttons: YES_NO,
  });
  if (r !== "yes") return;
  await runJob({ kind: "delete", paths, toTrash }, src);
}

async function refreshOther(panel) {
  const other = ctx.panels().find((p) => p !== panel);
  await other?.refresh();
}

/**
 * Creates `name` in the active panel's folder with `command` (mkdir/edit_new)
 * and puts the cursor on it (on the first component of "a/b/c").
 */
async function createNew(title, nested, command, what) {
  const panel = ctx.active();
  const dir = panel.location;
  const name = await prompt(title, undefined);
  if (!name) return;
  const invalid = newNameError(name, nested);
  if (invalid) {
    await message("Error", `Cannot create ${what}: ${invalid}`);
    return;
  }
  try {
    await ctx.invoke(command, nested ? { parent: dir, name } : { dir, name });
  } catch (e) {
    // Not possible inside archives: the backend's reason goes to the status line.
    if (archiveOf(dir) && command === "edit_new") panel.setMessage(String(e));
    else await message("Error", `Cannot create ${what}: ${e}`);
    return;
  }
  if (panel.location === dir) await panel.go(dir, firstComponent(name), { refresh: true });
  else await panel.refresh();
  await refreshOther(panel);
}

/** F7: creates the folder (nested "a/b/c" too). */
export function makeDir() {
  return createNew("New folder", true, "mkdir", "folder");
}

/** Shift+F4: creates the file and opens it in the editor. */
export function editNew() {
  return createNew("New file name", false, "edit_new", "file");
}

/** F2 / Shift+F6: inline rename over the cursor row. */
export function renameInline() {
  const panel = ctx.active();
  const entry = panel.cursorEntry;
  if (!entry || entry.up) return;
  const dir = panel.location;
  panel.ensureVisible();
  const input = el("input", "rename");
  input.type = "text";
  input.spellcheck = false;
  input.value = entry.name;
  input.style.top = `${panel.cursorIndex * panel.rowH}px`;
  input.style.height = `${panel.rowH}px`;
  const modal = { el: input, noDelay: true };
  let open = true;
  const close = () => {
    if (!open) return false;
    open = false;
    popModal(modal);
    return true;
  };
  const save = async () => {
    const d = renameDecision(entry.name, input.value);
    if (d.kind === "invalid") {
      panel.setMessage(`Cannot rename: ${d.reason}`);
      return;
    }
    if (!close() || d.kind === "same") return;
    try {
      await ctx.invoke("rename", { path: joinLocation(dir, entry.name), newName: d.name });
    } catch (e) {
      panel.setMessage(`Cannot rename: ${e}`);
      return;
    }
    if (panel.location === dir) {
      if (panel.marked.delete(entry.name)) panel.marked.add(d.name);
      await panel.go(dir, d.name, { refresh: true });
    } else {
      await panel.refresh();
    }
    await refreshOther(panel);
  };
  modal.onKey = (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      save().catch((err) => panel.setMessage(`Cannot rename: ${err}`));
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
    }
  };
  input.addEventListener("blur", close);
  pushModal(modal, panel.listEl);
  input.focus();
  const sel = renameSelection(entry.name, entry.isDir);
  input.setSelectionRange(sel.start, sel.end);
}

let lastMask = "*.*";

/** Num+ / Num−: marks or unmarks by mask (remembered for the session). */
export async function markByMask(on) {
  const mask = await prompt(on ? "Select files" : "Unselect files", undefined, lastMask);
  if (!mask) return;
  lastMask = mask;
  ctx.active().markMask(mask, on);
}

/**
 * c: {invoke, listen, active() -> Panel, other() -> Panel, panels() -> Panel[],
 *     connectFlow(request, opts) -> Promise<location|null>}.
 * Subscribes to the job events.
 */
export async function initDialogs(c) {
  ctx = c;
  for (const type of ["progress", "conflict", "error", "done"]) {
    await ctx.listen(`job://${type}`, (ev) => route(type, ev.payload));
  }
}
