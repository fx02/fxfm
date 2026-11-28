// Modal dialogs and the TC file operations behind them (F5/F6/F7/F8, Alt+F5, rename, masks).

import { el, cycleFocus, frame } from "./dom.js";
import {
  remoteArchives,
  realLocation,
  displayLocation,
  archiveOf,
  CHOOSE_FOLDER,
  NOTHING_TO_PASTE,
  ALREADY_HERE,
  copyName,
  PASTE_NEEDS_FOLDER,
  pasteSource,
  copyTargetDefault,
  deleteConfirmText,
  editTexts,
  firstComponent,
  formatDate,
  isLocalFolder,
  isSearchLocation,
  formatK,
  groupSources,
  groupThousands,
  jobsStrip,
  IS_LINUX,
  jobErrorBox,
  jobServers,
  joinLocation,
  jobSucceeded,
  lastComponent,
  modalKeyBlocked,
  openedByActivationKey,
  newNameError,
  operationPaths,
  operationSources,
  packArchiveName,
  PACK_FORMATS,
  packFormatOf,
  withPackFormat,
  packCursor,
  packDialogText,
  packedText,
  packFormError,
  packTargetDir,
  parentLocation,
  parseRemote,
  connectionLost,
  passwordArchive,
  permanentConfirmText,
  renameDecision,
  renameSelection,
  resultOpRefusal,
  suggestRename,
  uploadSucceeded,
} from "./lib.mjs";

// ---------------------------------------------------------------- modal stack

const modals = [];

/** The top modal; it gets every keydown first (see app.js onKeyDown). */
export function topModal() {
  return modals[modals.length - 1] ?? null;
}

/** The last keydown seen by `handleModalKey` ({key, at}): did an activation key open the next modal? */
let lastKey = null;

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
  // Only a modal opened by an activation key (Enter that closed the previous dialog, Escape…)
  // waits before taking such keys; one opened by F5 & co. takes Enter at once (fast F5, Enter).
  m.noDelay ||= !openedByActivationKey(lastKey, m.openedAt);
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
  lastKey = { key: e.key, at: performance.now() };
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

/**
 * Modal dialog. buttons: [{label, value, default?, key?}] (`key`: a function key that presses
 * the button, e.g. "F2"); resolves with the clicked
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
      } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && document.activeElement?.parentElement === row) {
        // As in TC: Left/Right walk the buttons (Right, Enter = the next one, e.g. Overwrite all).
        e.preventDefault();
        const all = [...row.querySelectorAll("button:not(:disabled)")];
        const i = all.indexOf(document.activeElement);
        all[(i + (e.key === "ArrowRight" ? 1 : all.length - 1)) % all.length]?.focus();
      } else if (!e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && buttons.some((b) => b.key === e.key)) {
        e.preventDefault();
        finish(buttons.find((b) => b.key === e.key).value);
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
  // Other windows (Find files) use the prompts without initDialogs.
  const invoke = ctx?.invoke ?? window.__TAURI__.core.invoke;
  await invoke("set_archive_password", { archive, password: r.text });
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

/** A message with OK; a text with line breaks shows as one paragraph per line. */
export function message(title, text) {
  const lines = typeof text === "string" ? text.split("\n") : text;
  return dialog({ title, text: lines, buttons: [{ label: "OK", value: "ok", default: true }] });
}

/** A read-only table (`head`: column titles, `rows`: arrays of cell texts) with OK; Enter/Escape close it. */
export function showTable(title, head, rows) {
  return new Promise((resolve) => {
    const { backdrop, box, body, row } = frame(title);
    box.classList.add("tablebox");
    const table = el("table");
    const tr = el("tr");
    for (const h of head) tr.append(el("th", null, h));
    table.append(tr);
    for (const r of rows) {
      const line = el("tr");
      for (const c of r) line.append(el("td", null, c));
      table.append(line);
    }
    const scroll = el("div", "scroll");
    scroll.append(table);
    body.append(scroll);
    const ok = el("button", null, "OK");
    row.append(ok);
    const modal = { el: backdrop };
    const close = () => {
      popModal(modal);
      resolve();
    };
    ok.addEventListener("click", close);
    modal.onKey = (e) => {
      if (e.key === "Enter" || e.key === "Escape") {
        e.preventDefault();
        close();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "PageDown" || e.key === "PageUp") {
        e.preventDefault();
        const step = e.key.startsWith("Page") ? scroll.clientHeight - 20 : 24;
        scroll.scrollTop += e.key === "ArrowDown" || e.key === "PageDown" ? step : -step;
      }
    };
    pushModal(modal);
    ok.focus();
  });
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

// ---------------------------------------------------------------- background jobs and the queue

/** F5/F6 "Queue": transfers waiting to run in the background, one after the other. */
const queued = [];
let queueBusy = false;

/** Repaints the operations strip above the F-key bar (background jobs and the queue). */
function paintJobs() {
  const strip = document.getElementById("jobs");
  if (!strip) return;
  const running = [...jobs.values()].filter((j) => j.background);
  const s = jobsStrip(
    running.map((j) => ({ kind: j.req.kind, progress: j.progress })),
    queued.length,
  );
  strip.classList.toggle("off", !s);
  if (!s) return;
  strip.querySelector(".what").textContent = s.text;
  strip.querySelector(".bar > div").style.width = `${s.ratio * 100}%`;
  strip.querySelector(".clear").hidden = !queued.length;
}

/** A click on the strip: the first background job's progress window again. */
export function showBackgroundJob() {
  [...jobs.values()].find((j) => j.background)?.toForeground();
}

/** × on the strip: drops the operations still waiting (the running one goes on). */
export async function clearQueue() {
  if (!queued.length) return;
  const n = queued.length;
  const r = await dialog({ title: "Queue", text: `Remove the ${n} queued operation(s)? The running one goes on.`, buttons: YES_NO });
  if (r !== "yes") return;
  queued.splice(0, n);
  paintJobs();
}

/** How many operations wait in the queue (the quit question counts them). */
export function queuedCount() {
  return queued.length;
}

/** F5/F6 "Queue": the transfer runs in the background after the ones queued before it. */
function enqueue(item) {
  queued.push(item);
  // The marked items are on their way: the panel is free for the next ones.
  item.source?.clearMarks();
  item.source = null;
  paintJobs();
  if (!queueBusy) runQueue();
}

async function runQueue() {
  queueBusy = true;
  try {
    while (queued.length) {
      const it = queued.shift();
      paintJobs();
      try {
        await transfer(it.kind, { ...it, background: true });
      } catch (e) {
        console.error("fxfm: queued operation", e);
      }
    }
  } finally {
    queueBusy = false;
    paintJobs();
  }
}

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
    /** Shared by the jobs of one F5/F6 from a results list: an "all" answer holds for all. */
    this.shared = null;
    /** Runs without its progress window, shown on the strip above the F-key bar. */
    this.background = false;
  }

  /** Runs the job; resolves with the job://done payload. */
  async run() {
    const done = new Promise((resolve) => {
      this.resolve = resolve;
    });
    this.id = await this.start();
    jobs.set(this.id, this);
    if (!this.background) this.timer = setTimeout(() => this.showView(), 300);
    paintJobs();
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
    paintJobs();
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

  /** "Background": the window goes, the job goes on (the strip shows it). */
  toBackground() {
    this.background = true;
    clearTimeout(this.timer);
    deferredViews.delete(this);
    if (this.view) popModal(this.view.modal);
    this.view = null;
    paintJobs();
  }

  /** A click on the strip: the progress window again. */
  toForeground() {
    this.background = false;
    paintJobs();
    this.showView();
  }

  showView() {
    if (this.background || this.view || !jobs.has(this.id)) return;
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
    const bgBtn = el("button", null, "F2 Background");
    bgBtn.title = "Go on working; the operation runs on, shown above the function keys (click there to see it again)";
    bgBtn.addEventListener("click", () => this.toBackground());
    const cancelBtn = el("button", null, "Cancel");
    cancelBtn.addEventListener("click", () => this.cancel());
    row.append(bgBtn, cancelBtn);
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
        else if (document.activeElement === bgBtn) this.toBackground();
      } else if (e.key === "F2" && !e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
        e.preventDefault();
        this.toBackground();
      }
    };
    this.view = { modal, current, filesLabel, filesFill, bytesLabel, bytesFill, cancelBtn };
    pushModal(modal);
    this.paint();
  }

  paint() {
    if (this.background) paintJobs();
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
    // An earlier job of the same operation was told "Overwrite all" / "Skip all" / "Overwrite older".
    let answer = this.shared?.all ?? null;
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
    if (this.shared && ["overwriteAll", "skipAll", "overwriteAllOlder"].includes(answer.kind)) this.shared.all = answer;
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
 * job's `job://done` payload, or null when no job ran. `recheck: false` leaves the search
 * results lists alone after a move/delete (the caller rechecks once at its end).
 */
async function runJob(req, source, { recheck = true, shared = null, background = false } = {}) {
  // A pack target may be on a server too: it is connected first like a copy's destination.
  const servers = jobServers([...(req.sources ?? req.paths ?? []), req.dest, req.cwd, req.target]);
  const connected = await connectServers(servers);
  if (!connected) return null;
  const job = new Job(req);
  job.shared = shared;
  job.background = background;
  const done = await job.run();
  // Local moves and Trash deletes can be undone; jobs of one operation (shared answers) add up.
  const pairs = done?.summary?.undo ?? [];
  if (pairs.length) {
    const what = req.kind === "delete" ? "move to the Trash" : "move";
    if (shared && lastUndo?.shared === shared) lastUndo.plan.moves.push(...pairs);
    else recordUndo(what, { moves: pairs.slice() }, shared);
  }
  if (jobSucceeded(done)) source?.clearMarks();
  // Moved or deleted items vanish from the search results lists (also after a partial job).
  if (recheck && (req.kind === "move" || req.kind === "delete")) ctx.recheckResults?.();
  await Promise.all(ctx.panels().map((p) => p.refresh()));
  const err = done.error;
  // Password needed (or wrong): ask and run the same request again.
  if (err && passwordArchive(err.message) && (await askPassword(err))) return runJob(req, source, { recheck });
  // Packing never replaces an archive without asking; it can also add to it.
  if (err?.kind === "exists" && req.kind === "pack") {
    const canAdd = !req.password && packFormatOf(req.target) !== "gz";
    const buttons = canAdd ? [{ label: "Add to archive", value: "add", default: true }, ...OVERWRITE_CANCEL.map((b) => ({ ...b, default: false }))] : OVERWRITE_CANCEL;
    const r = await dialog({ title: "Pack files", text: `"${lastComponent(req.target)}" already exists.`, buttons });
    if (r !== "overwrite" && r !== "add") return done;
    const again = r === "add" ? { ...req, add: true } : { ...req, overwrite: true };
    try {
      const res = await runJob(again, source, { recheck });
      if (r === "add" && res) res.added = true;
      return res;
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
    if (r === "yes") return runJob(job.followUp, source, { recheck });
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
  const server = parseRemote(location ?? "")?.server ?? null;
  /** A server file whose connection is gone: offers to reconnect; true = try the upload again. */
  const reconnect = async (why) => {
    if (!server) return false;
    const r2 = await dialog({
      title: "Edit",
      text: `"${name}" was not uploaded: ${why}`,
      buttons: [
        { label: "Reconnect and upload", value: "again", default: true },
        { label: "Not now", value: null },
      ],
    });
    if (r2 !== "again") return false;
    return Boolean(await connectServers([server]));
  };
  let done;
  for (let force = false; !done; ) {
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
        if (overwrite === "yes") {
          force = true;
          continue;
        }
        return confirmEdit(id, false);
      }
      if (text.startsWith("Not connected to ") && (await reconnect(text))) continue;
      await confirmEdit(id, false);
      if (server && text.startsWith("Not connected to ")) return null;
      return message("Error", `"${name}" was not uploaded: ${text}`);
    }
    // The connection broke during the upload: the same offer, then a new attempt.
    if (!uploadSucceeded(done) && server && connectionLost(done.error?.message) && (await reconnect(done.error.message))) {
      done = null;
      continue;
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

/** In a results list: says why `op` is refused for `sources` and returns true; else false. */
function refusedInResults(panel, sources, op) {
  if (!isSearchLocation(panel.location)) return false;
  const why = resultOpRefusal(sources, op);
  if (why) panel.setMessage(why);
  return Boolean(why);
}

const countText = (sources, files) =>
  sources.length === 1 ? `"${sources[0].name}"` : `${sources.length} ${files}`;

const VERB = { copy: "Copy", move: "Move" };

/** F5 / F6: the active panel's marked entries (or the one under the cursor) to the other panel. */
export async function copyMove(kind) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  // A results list is no folder to copy into.
  if (isSearchLocation(ctx.other().location)) {
    src.setMessage(CHOOSE_FOLDER);
    return;
  }
  if (refusedInResults(src, sources, kind)) return;
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
  // Queue: runs in the background, after the operations queued before it (TC's F2).
  const queueBtn = { label: "F2 Queue", value: "queue", key: "F2" };
  const buttons = switchable
    ? [OK_CANCEL[0], queueBtn, { label: `${VERB[otherKind]} instead`, value: "switch" }, OK_CANCEL[1]]
    : [OK_CANCEL[0], queueBtn, OK_CANCEL[1]];
  // A server archive's copy shows as its place on the server; the job writes into the copy
  // (and the edit watcher offers the upload).
  const r = await dialog({ title: verb, text: `${verb} ${countText(sources, "file(s)")} to:`, input: displayLocation(dest, remoteArchives), buttons });
  const target = r?.text.trim() ? realLocation(r.text.trim(), remoteArchives) : "";
  if (!target) return;
  if (r.value === "queue") {
    enqueue({ kind, paths, cwd, target, source });
    return;
  }
  await transfer(r.value === "switch" ? otherKind : kind, { paths, cwd, target, source });
}

/**
 * Copies or moves `paths` into `target` without a dialog (after F5/F6 and for Paste); `source`:
 * the panel whose marks a finished job clears (or null).
 */
async function transfer(jobKind, { paths, cwd, target, source, background = false }) {
  // A search results list can mix local items and entries of several archives; one job takes
  // one archive (or none), so each group gets its own job, one after the other.
  const groups = groupSources(paths, cwd);
  if (groups.length <= 1) {
    const done = await runJob({ kind: jobKind, sources: paths, dest: target, cwd }, source, { background });
    return Boolean(done && jobSucceeded(done));
  }
  let ok = true;
  // One answer "for all" holds for every group's job.
  const shared = {};
  try {
    for (let i = 0; i < groups.length; i++) {
      const g = groups[i];
      // Marks go only when the last group finished too.
      const last = i === groups.length - 1;
      const done = await runJob({ kind: jobKind, sources: g.paths, dest: target, cwd: g.cwd }, last ? source : null, {
        recheck: false,
        shared,
        background,
      });
      // A cancelled, failed or never started job stops the rest.
      if (!done || !jobSucceeded(done)) {
        ok = false;
        break;
      }
    }
  } finally {
    if (jobKind === "move") {
      ctx.recheckResults?.();
      await Promise.all(ctx.panels().map((p) => p.refresh()));
    }
  }
  return ok;
}

// ---------------------------------------------------------------- synchronize dirs

/**
 * Runs the copy jobs of a synchronization one after the other. Every row was given its
 * direction by the user, so an existing target is overwritten without asking (one shared
 * "Overwrite all"); a cancelled or failed job stops the rest. Resolves with how many ran fully.
 */
export async function runSyncJobs(jobs) {
  const shared = { all: { kind: "overwriteAll" } };
  let ok = 0;
  try {
    for (const req of jobs) {
      const done = await runJob(req, null, { recheck: false, shared });
      if (!done || !jobSucceeded(done)) break;
      ok += 1;
    }
  } finally {
    await Promise.all(ctx.panels().map((p) => p.refresh()));
  }
  return ok;
}

// ---------------------------------------------------------------- undo (Ctrl+Z)

/** The last undoable local operation: {what, plan: {moves, removeDirs}, shared}. */
let lastUndo = null;

function recordUndo(what, plan, shared = null) {
  lastUndo = { what, plan: { moves: plan.moves ?? [], removeDirs: plan.removeDirs ?? [] }, shared };
}

/**
 * Ctrl+Z: after a confirmation, the last local move, Trash delete, rename (F2) or new folder
 * (F7) is undone; an item whose old place is taken again stays where it is.
 */
export async function undoLast() {
  if (!lastUndo) {
    ctx.active().setMessage("Nothing to undo");
    return;
  }
  const u = lastUndo;
  const n = u.plan.moves.length + u.plan.removeDirs.length;
  const r = await dialog({
    title: "Undo",
    text: [`Undo the last ${u.what}?`, `${n} item(s) go back where they were. Nothing is overwritten.`],
    buttons: YES_NO,
  });
  if (r !== "yes") return;
  lastUndo = null;
  let res;
  try {
    res = await ctx.invoke("undo_last", { plan: u.plan });
  } catch (e) {
    await message("Undo", String(e));
    return;
  }
  ctx.recheckResults?.();
  await Promise.all(ctx.panels().map((p) => p.refresh()));
  if (res.failed.length) await message("Undo", [`${res.done} of ${n} put back.`, ...res.failed.slice(0, 10)].join("\n"));
  else ctx.active().setMessage(`Undone: ${u.what}`);
}

// ---------------------------------------------------------------- clipboard

/** fxfm's own last Copy/Cut to the clipboard: {kind, paths, cwd, change} (see `pasteSource`). */
let clip = null;

/** Copy names / Copy names with path: the active panel's marked items (or the cursor one) as text lines. */
export async function copyNames(full) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  const { paths } = operationPaths(src.location, sources);
  // A results entry is named by a (shortened) path: its own name is the path's last part.
  const lines = full ? paths.map((p) => displayLocation(p, remoteArchives)) : sources.map((e) => (e.path ? lastComponent(e.path) : e.name));
  await ctx.invoke("clipboard_write", { text: lines.join("\n"), files: [] });
  src.setMessage(`Copied ${full ? "path" : "name"}${lines.length > 1 ? "s" : ""} to the clipboard`);
}

/**
 * The `smb://` link and Windows path of each item the command acts on (Copy network path), or
 * null with a message when one of them is not on a mounted SMB share.
 */
export async function shareLinks() {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return null;
  const { paths } = operationPaths(src.location, sources);
  const local = paths.every((p) => p.startsWith("/") && !archiveOf(p));
  const links = local ? await ctx.invoke("share_links", { paths }) : [];
  if (!links.length || links.some((l) => !l)) {
    src.setMessage(paths.length > 1 ? "Not all of them are on an SMB share" : "Not on an SMB share");
    return null;
  }
  return links;
}

/** Copy / Cut to clipboard: Paste in a panel copies / moves them; local files also paste in Finder. */
export async function clipFiles(kind) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  if (refusedInResults(src, sources, kind)) return;
  const { cwd, paths } = operationPaths(src.location, sources);
  const text = paths.map((p) => displayLocation(p, remoteArchives)).join("\n");
  const change = await ctx.invoke("clipboard_write", { text, files: paths });
  clip = { kind, paths, cwd, change };
  src.setMessage(`${kind === "move" ? "Cut" : "Copied"} ${countText(sources, "file(s)")} to the clipboard`);
}

/** Paste from clipboard into the active panel's folder (no dialog; conflicts are asked as in F5). */
export async function pasteFiles() {
  const dst = ctx.active();
  if (isSearchLocation(dst.location)) {
    dst.setMessage(PASTE_NEEDS_FOLDER);
    return;
  }
  const p = pasteSource(clip, await ctx.invoke("clipboard_files"));
  if (!p) {
    dst.setMessage(NOTHING_TO_PASTE);
    return;
  }
  const here = dst.location;
  const same = p.paths.filter((x) => parentLocation(x) === here);
  const rest = p.paths.filter((x) => parentLocation(x) !== here);
  if (p.kind === "move" && !rest.length) {
    dst.setMessage(ALREADY_HERE);
    return;
  }
  if (rest.length) await transfer(p.kind, { paths: rest, cwd: p.cwd, target: here, source: null });
  // Moved items are gone from where they were: a second Paste has nothing to move.
  if (p.kind === "move") clip = null;
  // Copied into their own folder: each gets Finder's "name copy" (a target without "/" is the new name).
  if (p.kind === "copy" && same.length) {
    // Every name in the folder counts, also hidden and filtered-out ones (`raw`), so a copy
    // never lands on an existing "x copy".
    const taken = new Set(dst.raw.map((e) => e.name));
    for (const x of same) {
      const old = lastComponent(x);
      const name = copyName(old, taken, dst.raw.find((e) => e.name === old)?.isDir);
      taken.add(name);
      // Cancel (or a failure) stops the rest.
      if (!(await transfer("copy", { paths: [x], cwd: here, target: joinLocation(here, name), source: null }))) break;
    }
  }
}

/**
 * The Pack files dialog. Resolves with {target, password (or null), encryption} or null.
 * Enter = OK (unless Cancel has focus), Escape = Cancel.
 */
/** The format of the last Pack files dialog, for the next one's default name (this session). */
let lastPackFormat = "zip";

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
    for (const [id, label] of PACK_FORMATS) {
      const o = el("option", null, label);
      o.value = id;
      format.append(o);
    }
    format.value = packFormatOf(initial) ?? "zip";
    // Picking a format renames the target; typing another suffix picks its format.
    format.addEventListener("change", () => {
      target.value = withPackFormat(target.value.trim(), format.value);
      lastPackFormat = format.value;
      showPw();
    });
    target.addEventListener("input", () => {
      const f = packFormatOf(target.value.trim());
      if (f && f !== format.value) {
        format.value = f;
        showPw();
      }
    });
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
      el("p", "note", IS_LINUX ? "Some archive tools cannot open AES-256 zips; fxfm and 7-Zip can." : "AES-256 zips do not open with a double-click in Finder; use fxfm, Keka or 7-Zip."),
    );
    const error = el("p", "error");
    error.hidden = true;
    const showPw = () => {
      // Only ZIP can be encrypted.
      encryptLabel.hidden = format.value !== "zip";
      if (encryptLabel.hidden) encrypt.checked = false;
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
      const problem = packFormError({ target: target.value, encrypt: encrypt.checked, password: password.value, repeat: repeat.value, sources });
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
  if (refusedInResults(src, sources, "copy")) return;
  // Snapshot before awaiting: a refresh may move the panel while the dialog is open.
  const { cwd, paths } = operationPaths(src.location, sources);
  const dir = packTargetDir(src.location, ctx.other().location, cwd);
  const ext = PACK_FORMATS.find(([id]) => id === lastPackFormat)?.[2] ?? ".zip";
  const fmt = lastPackFormat === "gz" && (sources.length !== 1 || sources[0].isDir) ? ".zip" : ext;
  const r = await packDialog(sources, joinLocation(dir, packArchiveName(sources, src.location, fmt)));
  if (!r) return;
  const req = { kind: "pack", sources: paths, cwd, target: r.target, password: r.password, encryption: r.encryption };
  r.password = null;
  const done = await runJob(req, src);
  req.password = null;
  if (!done?.summary || done.summary.cancelled || done.error) return;
  // The cursor goes to the new archive in the panel showing its folder (the target panel first).
  const target = r.target.startsWith("/") || r.target.includes("://") ? r.target : joinLocation(cwd, r.target);
  const folder = parentLocation(target);
  const name = lastComponent(target);
  const panel = [ctx.other(), src].find((p) => p.location === folder);
  if (panel) await panel.go(folder, name, { refresh: true });
  src.setMessage(packedText(done.summary, target, Boolean(done.added)));
}

/** F8 (Trash) / Shift+F8 (permanent). */
export async function deleteItems(toTrash) {
  const src = ctx.active();
  const sources = sourcesOf(src);
  if (!sources.length) return;
  if (refusedInResults(src, sources, "del")) return;
  const { paths } = operationPaths(src.location, sources);
  // The backend ignores toTrash inside archives and on servers (always permanent); for
  // archives it also decides the wording. Results lists hold no archive entries here.
  let inArchive = false;
  if (!parseRemote(src.location) && !isSearchLocation(src.location)) {
    try {
      inArchive = await ctx.invoke("location_in_archive", { location: src.location });
    } catch {
      // Wording falls back to the normal text; the backend still deletes correctly.
    }
  }
  // A network share has no Trash: the backend deletes its items permanently, the question says so.
  let onNetwork = 0;
  if (toTrash && !inArchive && !parseRemote(src.location)) {
    onNetwork = await ctx.invoke("count_on_network", { paths }).catch(() => 0);
  }
  const r = await dialog({
    title: "Delete",
    text: deleteConfirmText(src.location, sources.length, { inArchive, toTrash, name: sources[0].name, onNetwork }),
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
  // F7: Undo removes the new folders while they are empty — only the ones that did not exist
  // (a chain "a/b/c" counts only when "a" was not there).
  const created = command === "mkdir" && isLocalFolder(dir) && !panel.raw.some((e) => e.name === firstComponent(name));
  try {
    await ctx.invoke(command, nested ? { parent: dir, name } : { dir, name });
    if (created) {
      const parts = name.split("/").filter(Boolean);
      const chain = parts.map((_, i) => joinLocation(dir, parts.slice(0, i + 1).join("/"))).reverse();
      recordUndo(`new folder "${name}"`, { removeDirs: chain });
    }
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
    const d = renameDecision(entry.name, input.value, dir);
    if (d.kind === "invalid") {
      panel.setMessage(`Cannot rename: ${d.reason}`);
      return;
    }
    if (!close() || d.kind === "same") return;
    if (d.kind === "move") {
      // A path moves the item there (as F6 with that target: conflicts asked, Undo kept).
      await runJob({ kind: "move", sources: [joinLocation(dir, entry.name)], dest: d.target, cwd: dir }, panel);
      return;
    }
    try {
      await ctx.invoke("rename", { path: joinLocation(dir, entry.name), newName: d.name });
    } catch (e) {
      panel.setMessage(`Cannot rename: ${e}`);
      return;
    }
    if (isLocalFolder(dir)) recordUndo(`rename of "${entry.name}"`, { moves: [[joinLocation(dir, d.name), joinLocation(dir, entry.name)]] });
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
  // The whole name, extension included (the owner's choice).
  input.select();
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
 *     connectFlow(request, opts) -> Promise<location|null>, recheckResults()} (`recheckResults`:
 *     the search results lists drop vanished items at their next listing).
 * Subscribes to the job events.
 */
export async function initDialogs(c) {
  ctx = c;
  for (const type of ["progress", "conflict", "error", "done"]) {
    await ctx.listen(`job://${type}`, (ev) => route(type, ev.payload));
  }
}
