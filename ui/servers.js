// Ctrl+F "Connect to server": the server tree, the connection editor and the connect flow.
//
// Passwords and passphrases live only in <input type="password"> values and in locals of the
// flow that sends them; they are never logged, never put into other DOM, and the inputs are
// emptied when their dialog closes.

import { el, cycleIn } from "./dom.js";
import {
  CHMOD_NOT_IN_ARCHIVE,
  chmodStart,
  chmodSummary,
  comboLabel,
  connectionLost,
  dragStarted,
  duplicateName,
  fieldsFor,
  fitWindow,
  flattenNodes,
  folderKey,
  GROUP_LEVELS,
  groupsTooDeep,
  indentNode,
  hostKeyText,
  lostToShow,
  masterPartial,
  modeBits,
  modeOctal,
  moveNode,
  newServerEntry,
  onRoot,
  operationPaths,
  operationSources,
  outdentNode,
  reorderStep,
  parentList,
  parseModeOctal,
  parseRemote,
  portAfterProtocolChange,
  relocateNode,
  remoteArchiveOf,
  remoteArchives,
  remoteRoot,
  secretChanges,
  serverNames,
  smbHostLabel,
  storageMasterPrompt,
  tcModifiers,
  treeRows,
  urlDisplayName,
  validateServer,
  withModeBit,
} from "./lib.mjs";
import { dialog, message, popModal, pushModal } from "./dialogs.js";

/** {invoke, home, active(), other(), panels(), status(text), mods(), refreshVolumes()} from app.js. */
let ctx = null;

export function initServers(c) {
  ctx = c;
}

const errText = (e) => String(e?.message ?? e);
const OK = [{ label: "OK", value: "ok", default: true }];
const OK_CANCEL = [
  { label: "OK", value: "ok", default: true },
  { label: "Cancel", value: null },
];

function passwordInput() {
  const i = el("input");
  i.type = "password";
  i.autocomplete = "off";
  i.spellcheck = false;
  return i;
}

function checkbox(label) {
  const box = el("input");
  box.type = "checkbox";
  const l = el("label", "check");
  l.append(box, ` ${label}`);
  return { box, label: l };
}

/** Focusable, visible controls of `root` in Tab order. */
/** Tab order of the server dialogs (also the hotlist tree; only what is shown). */
function focusables(root) {
  return [...root.querySelectorAll("button:not(:disabled), input:not(:disabled), select:not(:disabled), .hotlist")].filter(
    (x) => x.offsetParent !== null,
  );
}

const cycleFocus = (root, e) => cycleIn(focusables(root), e);

/**
 * Modal form. `build(body)` fills the body and returns the control to focus first.
 * buttons: [{label, value, default?, submit?}]. A `submit` button first runs `submit()`, which
 * returns an error text (shown in the form, which stays open) or null. Resolves with the
 * button's value; Escape gives null. Password inputs are emptied when the form closes.
 */
function form({ title, cls, build, buttons, submit, danger = false }) {
  return new Promise((resolve) => {
    const backdrop = el("div", "backdrop");
    const box = el("div", `dialog${cls ? ` ${cls}` : ""}`);
    if (danger) box.classList.add("danger");
    const body = el("div", "body");
    const row = el("div", "buttons");
    box.append(el("div", "title", title), body, row);
    backdrop.append(box);
    const first = build(body);
    const err = el("p", "error");
    err.hidden = true;
    body.append(err);

    const modal = { el: backdrop };
    let open = true;
    let running = false;
    const finish = (value) => {
      if (!open) return;
      open = false;
      for (const i of box.querySelectorAll('input[type="password"]')) i.value = "";
      popModal(modal);
      resolve(value);
    };
    const press = async (b) => {
      if (running || !open) return;
      if (b.submit && submit) {
        running = true;
        let problem = null;
        try {
          problem = await submit();
        } catch (e) {
          problem = errText(e);
        } finally {
          running = false;
        }
        if (problem) {
          err.textContent = problem;
          err.hidden = false;
          return;
        }
      }
      finish(b.value);
    };
    let def = null;
    for (const b of buttons) {
      const btn = el("button", null, b.label);
      btn.type = "button";
      btn.addEventListener("click", () => press(b));
      if (b.default) def = btn;
      row.append(btn);
    }
    modal.onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (!running) finish(null);
      } else if (e.key === "Tab") {
        e.preventDefault();
        cycleFocus(box, e);
      } else if (e.key === "Enter" && !e.isComposing) {
        e.preventDefault();
        const f = document.activeElement;
        (f?.tagName === "BUTTON" && box.contains(f) ? f : def)?.click();
      }
    };
    pushModal(modal);
    (first ?? def)?.focus();
  });
}

/**
 * "Connecting to X…" over everything while `connect` runs (it gives up after 15 s).
 * With `onCancel` (mounting an SMB share, which macOS may take long for) it has a Cancel
 * button, also on Escape, that calls it.
 */
function busyNote(text, onCancel = null) {
  const backdrop = el("div", "backdrop");
  const box = el("div", "dialog busy");
  const body = el("div", "body");
  body.append(el("p", null, text));
  box.append(el("div", "title", "Connect"), body);
  if (onCancel) {
    const btns = el("div", "buttons");
    const cancel = el("button", null, "Cancel");
    cancel.type = "button";
    cancel.addEventListener("click", onCancel);
    btns.append(cancel);
    box.append(btns);
  }
  backdrop.append(box);
  const modal = {
    el: backdrop,
    onKey: (e) => {
      e.preventDefault();
      if (onCancel && e.key === "Escape") onCancel();
    },
  };
  // Fast answers (a known host, an open session) do not flash a dialog.
  const timer = setTimeout(() => pushModal(modal), 250);
  return () => {
    clearTimeout(timer);
    popModal(modal);
  };
}

// ---------------------------------------------------------------- master password

/**
 * Asks for the master password and unlocks the vault (`create`: sets a new one, asked twice).
 * True when the vault is unlocked afterwards.
 */
async function askMaster(create) {
  let pw;
  let again;
  const r = await form({
    title: "Master password",
    cls: "srvask",
    build: (body) => {
      pw = passwordInput();
      if (create) {
        again = passwordInput();
        body.append(
          el("p", null, "Saved passwords are encrypted with a master password. Choose one:"),
          pw,
          el("p", "gap", "Repeat it:"),
          again,
        );
      } else {
        body.append(el("p", null, "Enter the master password to use the saved passwords:"), pw);
      }
      return pw;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: async () => {
      if (!pw.value) return "The master password cannot be empty";
      if (create && pw.value !== again.value) return "The passwords do not match";
      try {
        if (create) await ctx.invoke("master_set", { old: null, new: pw.value });
        else await ctx.invoke("master_unlock", { password: pw.value });
      } catch (e) {
        if (!masterPartial(errText(e))) return errText(e);
        // Unlocked anyway; some queued passwords stay queued.
        await message("Master password", errText(e));
      }
      return null;
    },
  });
  return r === "ok";
}

/** Encrypt…: sets or changes the master password (portable mode only). */
async function encryptDialog() {
  const st = await ctx.invoke("master_status");
  if (st.mode !== "portable") {
    await message("Encrypt", "In system mode passwords are stored in the macOS Keychain.");
    return;
  }
  let old = null;
  let pw;
  let again;
  const r = await form({
    title: "Master password",
    cls: "srvask",
    build: (body) => {
      pw = passwordInput();
      again = passwordInput();
      if (st.exists) {
        old = passwordInput();
        body.append(el("p", null, "Current master password:"), old);
      } else {
        body.append(el("p", null, "Saved passwords are encrypted with a master password."));
      }
      body.append(el("p", "gap", "New master password:"), pw, el("p", "gap", "Repeat the new master password:"), again);
      return old ?? pw;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: async () => {
      if (old && !old.value) return "Enter the current master password";
      if (!pw.value) return "The master password cannot be empty";
      if (pw.value !== again.value) return "The passwords do not match";
      try {
        await ctx.invoke("master_set", { old: old ? old.value : null, new: pw.value });
      } catch (e) {
        if (!masterPartial(errText(e))) return errText(e);
        await message("Master password", errText(e));
      }
      return null;
    },
  });
  if (r === "ok") ctx.status("Master password set");
}

/**
 * Options → Storage: moves the data folder and the saved passwords to `mode` with
 * `set_storage_mode`, first asking for the master password the switch needs
 * (`storageMasterPrompt`): a new one (twice) for Portable, the current one for System when the
 * password file is locked. Resolves with the backend's StorageChange ({...StorageInfo,
 * movedSecrets, message}), or null when the prompt was cancelled (nothing changed). Without a
 * prompt an error is thrown; with one it is shown in the prompt, which stays open to retry.
 * The typed password lives only in the input (emptied on close) and the one invoke call.
 */
export async function moveStorage(mode) {
  const st = await ctx.invoke("master_status");
  const need = storageMasterPrompt(st.mode, mode, st);
  if (need === "none") return ctx.invoke("set_storage_mode", { mode, master: null });
  let change = null;
  let pw;
  let again;
  const r = await form({
    title: "Master password",
    cls: "srvask",
    build: (body) => {
      pw = passwordInput();
      if (need === "new") {
        again = passwordInput();
        body.append(
          el("p", null, "In portable mode saved passwords are encrypted with a master password. Choose one:"),
          pw,
          el("p", "gap", "Repeat it:"),
          again,
        );
      } else {
        body.append(el("p", null, "Enter the master password to move the saved passwords to the macOS Keychain:"), pw);
      }
      return pw;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: async () => {
      if (!pw.value) return "The master password cannot be empty";
      if (need === "new" && pw.value !== again.value) return "The passwords do not match";
      try {
        change = await ctx.invoke("set_storage_mode", { mode, master: pw.value });
      } catch (e) {
        return `Cannot move settings: ${errText(e)}`;
      }
      return null;
    },
  });
  return r === "ok" ? change : null;
}

// ---------------------------------------------------------------- connect flow

/** Password or passphrase prompt; {secret, save} or null when cancelled. */
async function askSecret({ title, text, retryText, offerSave, saveDefault }) {
  let input;
  let save = null;
  let result = null;
  const r = await form({
    title,
    cls: "srvask",
    build: (body) => {
      if (retryText) body.append(el("p", "error", retryText));
      input = passwordInput();
      body.append(el("p", null, text), input);
      if (offerSave) {
        save = checkbox("Save password");
        save.box.checked = saveDefault;
        save.label.classList.add("gap");
        body.append(save.label);
      }
      return input;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: () => {
      result = { secret: input.value, save: !!save?.box.checked };
      return null;
    },
  });
  return r === "ok" ? result : null;
}

function hostKeyDialog(reply) {
  const changed = !!reply.changedFrom;
  return dialog({
    title: changed ? "Host key changed" : "Unknown host key",
    text: hostKeyText(reply).split("\n"),
    danger: changed,
    buttons: changed
      ? [
          { label: "Replace and connect", value: "yes" },
          { label: "Cancel", value: null, default: true },
        ]
      : [
          { label: "Connect", value: "yes", default: true },
          { label: "Cancel", value: null },
        ],
  });
}

/** The saved server named `name`, or null. */
function findServer(nodes, name) {
  return flattenNodes(nodes).find((r) => r.node.kind === "server" && r.node.name === name)?.node ?? null;
}

/**
 * Connects `request` ({server} or {url}; see `ConnectRequest`), answering the backend's
 * questions (host key, password, passphrase, master password) until it is connected.
 * With `navigate` (default) the active panel (or `panel`) opens the server and the other panel
 * its local folder (unless `localDir` is false: picking a drive changes only its panel). Resolves with the server location, or null (cancelled, or the error was
 * shown). A saved SMB share is mounted by macOS instead, and the location is its local folder.
 */
export async function connectFlow(request, { navigate = true, panel = null, localDir = true } = {}) {
  const req = { save: false, ...request };
  // Never the URL itself: it may hold a password.
  const name = req.server ?? urlDisplayName(req.url);
  let entry = null;
  if (req.server) {
    try {
      entry = findServer((await ctx.invoke("servers_load")).nodes, req.server);
    } catch {
      entry = null;
    }
  }
  const smb = entry?.protocol === "smb";
  try {
    for (;;) {
      let reply;
      // Cancel stops waiting: a late SMB mount stays (macOS has it), a late server
      // connection is closed by the backend.
      let cancel = null;
      const cancelled = new Promise((resolve) => {
        cancel = () => resolve(CANCELLED);
      });
      const done = busyNote(smb ? `Mounting ${name}…` : `Connecting to ${name}…`, cancel);
      // Each attempt has an id, so Cancel can tell the backend to drop its late result.
      req.attempt = crypto.randomUUID();
      const call = ctx.invoke("connect", { req });
      call.catch(() => {}); // a cancelled attempt's late error goes nowhere
      try {
        reply = await Promise.race([call, cancelled]);
      } catch (e) {
        done();
        await message("Connect", errText(e));
        return null;
      }
      done();
      if (reply === CANCELLED) {
        // A late success then keeps no password and moves no panel (the mount itself stays).
        ctx.invoke("connect_cancel", { attempt: req.attempt }).catch((e) => console.error("fxfm: connect_cancel", e));
        ctx.status(smb ? `Stopped waiting for ${name}; macOS may still mount it` : `Connecting to ${name} was cancelled`);
        return null;
      }
      if (reply.kind === "connected") {
        closedOnPurpose.delete(parseRemote(reply.location)?.server);
        if (smb) ctx.refreshVolumes?.();
        if (navigate) {
          const target = panel ?? ctx.active();
          await target.go(reply.location);
          const otherPanel = panel ? ctx.panels().find((p) => p !== panel) : ctx.other();
          if (localDir && reply.localDir) await otherPanel?.go(reply.localDir);
          ctx.status(reply.note ?? `Connected to ${parseRemote(reply.location)?.server ?? name}`);
        }
        return reply.location;
      }
      if (reply.kind === "hostKey") {
        if ((await hostKeyDialog(reply)) !== "yes") return null;
        req.acceptFingerprint = reply.fingerprint;
      } else if (reply.kind === "needPassword" || reply.kind === "needPassphrase") {
        const phrase = reply.kind === "needPassphrase";
        const a = await askSecret({
          title: phrase ? "Passphrase" : "Password",
          text: phrase ? `Passphrase for the key of ${name}:` : `Password for ${name}:`,
          retryText: reply.retry
            ? phrase
              ? "Wrong passphrase, try again"
              : smb
                ? "Wrong user name or password."
                : "Wrong password, try again"
            : null,
          offerSave: !!entry,
          saveDefault: !!entry?.savePassword,
        });
        if (!a) return null;
        if (phrase) req.passphrase = a.secret;
        else req.password = a.secret;
        req.save = a.save;
      } else if (reply.kind === "needMaster") {
        if (!(await askMaster(reply.create))) return null;
      } else {
        await message("Connect", `Unexpected answer: ${reply.kind}`);
        return null;
      }
    }
  } finally {
    req.password = undefined;
    req.passphrase = undefined;
  }
}

/** Marks the busy note's Cancel in `connectFlow`. */
const CANCELLED = Symbol("cancelled");

/** Every saved server entry of the tree `nodes`, in order. */
function allServers(nodes) {
  const out = [];
  const walk = (list) => {
    for (const n of list) {
      if (n.kind === "folder") walk(n.children);
      else out.push(n);
    }
  };
  walk(nodes);
  return out;
}

/**
 * At startup: mounts the saved SMB shares marked "Connect at startup" that are not mounted
 * yet, one after another, with their saved passwords. It never asks; a share without a
 * saved password or that fails is named in the status line.
 */
export async function autoConnectShares() {
  let entries;
  try {
    entries = allServers((await ctx.invoke("servers_load")).nodes).filter((e) => e.protocol === "smb" && e.autoConnect);
  } catch (e) {
    console.error("fxfm: servers_load", e);
    return;
  }
  if (!entries.length) return;
  const failed = [];
  let mounted = 0;
  for (const e of entries) {
    try {
      const reply = await ctx.invoke("connect", { req: { server: e.name, save: false } });
      if (reply.kind === "connected") mounted++;
      else failed.push(`${e.name} (${reply.kind === "needMaster" ? "the password vault is locked" : "no saved password"})`);
    } catch (err) {
      failed.push(`${e.name} (${errText(err)})`);
    }
  }
  if (mounted) ctx.refreshVolumes?.();
  if (failed.length) ctx.status(`Not connected at startup: ${failed.join(", ")}`);
}

/**
 * Unmounts the saved SMB share `name` (`diskutil unmount`, never forced). Panels inside it go
 * to the home folder. True when it was unmounted; a busy share shows diskutil's text.
 */
export async function disconnectShare(name) {
  // The panels inside the share are taken before unmounting: the volume watcher may move
  // them to /Volumes before the unmount call returns.
  let affected = [];
  try {
    const share = (await ctx.invoke("smb_shares")).find((sh) => sh.name === name);
    const inside = (loc) => share?.mount && (loc === share.mount || loc.startsWith(`${share.mount}/`));
    affected = ctx.panels().filter((p) => inside(p.location));
  } catch (e) {
    console.error("fxfm: smb_shares", e);
  }
  let mount;
  try {
    mount = await ctx.invoke("smb_unmount", { name });
  } catch (e) {
    await message("Disconnect", errText(e));
    return false;
  }
  const inMount = (loc) => loc === mount || loc.startsWith(`${mount}/`);
  for (const p of ctx.panels()) {
    if (affected.includes(p) || inMount(p.location)) await p.go(ctx.home);
  }
  ctx.refreshVolumes?.();
  ctx.status(`Disconnected ${name}`);
  return true;
}

/** Ctrl+Shift+F: closes the active panel's connection; its panels go to the local folder or home. */
export async function disconnectActive() {
  // Inside the temp copy of a server archive counts as being on that server.
  const archive = remoteArchiveOf(ctx.active().location, remoteArchives);
  const r = parseRemote(archive ? archive.parent : ctx.active().location);
  if (!r) {
    // A mounted SMB share: unmount it (like Disconnect in Ctrl+F).
    const loc = ctx.active().location;
    let share = null;
    try {
      share = (await ctx.invoke("smb_shares")).find((sh) => sh.mount && (loc === sh.mount || loc.startsWith(`${sh.mount}/`)));
    } catch (e) {
      console.error("fxfm: smb_shares", e);
    }
    if (share) {
      await disconnectShare(share.name);
      return;
    }
    ctx.status("Disconnect: this panel is not on a server or a mounted share");
    return;
  }
  let local = null;
  try {
    local = findServer((await ctx.invoke("servers_load")).nodes, r.server)?.localDir ?? null;
  } catch {
    local = null;
  }
  // Taken before the disconnect: it deletes the temp copies the panels may be inside.
  const affected = ctx.panels().filter((p) => onServer(p, r.server) || inServerArchive(p, r.server));
  // A job still running on it may report this name as lost: that is no news to the user.
  noteDisconnect(r.server);
  try {
    await ctx.invoke("disconnect", { name: r.server });
  } catch (e) {
    // Still connected: its real losses must show again.
    closedOnPurpose.delete(r.server);
    ctx.status(`Disconnect: ${errText(e)}`);
    return;
  }
  for (const p of affected) await p.go(local || ctx.home);
  forgetArchives(r.server);
  ctx.status(`Disconnected from ${r.server}`);
}

// ---------------------------------------------------------------- connection lost

/**
 * Servers the user closed on purpose and has not connected again since: a job still running
 * on one may report it as lost (up to 60 s later), which is no news to the user.
 */
const closedOnPurpose = new Set();
/** Server name -> token of its open "connection lost" dialog (one per server). */
const lostOpen = new Map();
let lostToken = 0;

function noteDisconnect(name) {
  closedOnPurpose.add(name);
}

/**
 * The connections `names` died (a `connections://changed` `lost` list, or a panel or chmod
 * error on one server that `connectionLost` recognised). Shows `Connection to <name> lost`
 * once per server. `origin` ({panel, target}): a panel whose navigation to `target` failed;
 * after a reconnect it goes there even if it was not on that server yet.
 */
export function serversLost(names, origin = null) {
  for (const name of lostToShow(names, closedOnPurpose)) {
    if (lostOpen.has(name)) continue;
    const token = ++lostToken;
    lostOpen.set(name, token);
    const release = () => {
      if (lostOpen.get(name) === token) lostOpen.delete(name);
    };
    lostDialog(name, origin, release)
      .catch((e) => ctx.status(`Connection to ${name}: ${errText(e)}`))
      .finally(release);
  }
}

/** Server named in a panel or chmod error location, when the error means the connection is gone. */
export function lostServer(message, ...locations) {
  if (!connectionLost(message)) return null;
  for (const l of locations) {
    const r = l ? parseRemote(l) : null;
    if (r) return r.server;
  }
  return null;
}

const onServer = (p, name) => parseRemote(p.location)?.server === name;

/** The panel is inside the temp copy of an archive that came from server `name`. */
const inServerArchive = (p, name) => {
  const archive = remoteArchiveOf(p.location, remoteArchives);
  return !!archive && parseRemote(archive.parent)?.server === name;
};

/** Forgets the temp archive copies of `name` (its clean copies were removed on disconnect). */
function forgetArchives(name) {
  for (const [copy, parent] of remoteArchives) {
    if (parseRemote(parent)?.server === name) remoteArchives.delete(copy);
  }
}

/**
 * Reconnect: the connect flow (no navigation; saved or session passwords need no prompt), then
 * every panel on that server lists its folder again (or the nearest existing one). A cancelled
 * or failed reconnect asks again. Close (or Escape): the connection is closed and those panels
 * go home. `release()` ends this dialog's claim on the server before the re-list, so a new
 * loss during it gets a dialog of its own.
 */
async function lostDialog(name, origin, release) {
  for (;;) {
    const r = await dialog({
      title: "Connection lost",
      text: `Connection to ${name} lost`,
      buttons: [
        { label: "Reconnect", value: "reconnect", default: true },
        { label: "Close", value: null },
      ],
    });
    if (r !== "reconnect") break;
    const at = await connectFlow({ server: name }, { navigate: false });
    if (!at) continue;
    release();
    const root = remoteRoot(at);
    let ok = true;
    for (const p of ctx.panels()) {
      if (onServer(p, name)) ok = (await p.go(onRoot(root, p.location), p.cursorName ?? undefined)).ok && ok;
    }
    if (origin && !onServer(origin.panel, name)) ok = (await origin.panel.go(onRoot(root, origin.target))).ok && ok;
    if (ok) ctx.status(`Reconnected to ${name}`);
    return;
  }
  noteDisconnect(name);
  const affected = ctx.panels().filter((p) => onServer(p, name) || inServerArchive(p, name));
  try {
    // Also removes the temporary copies of its files.
    await ctx.invoke("disconnect", { name });
  } catch (e) {
    console.error("fxfm: disconnect", e);
  }
  for (const p of affected) await p.go(ctx.home);
  forgetArchives(name);
}

// ---------------------------------------------------------------- change attributes

const RWX = ["Read", "Write", "Execute"];
const WHO = ["Owner", "Group", "Others"];

/**
 * `cm_chmod` (local folders and servers, not inside archives): octal field and nine rwx boxes (kept in sync) for the marked entries or the one
 * under the cursor. The start value is the cursor entry's mode. Folders are not changed
 * recursively.
 */
export async function chmodDialog() {
  const panel = ctx.active();
  const sources = operationSources(panel.entries, panel.marked, panel.cursorIndex);
  if (!sources.length) return;
  const { paths } = operationPaths(panel.location, sources);
  const under = panel.cursorEntry && !panel.cursorEntry.up ? panel.cursorEntry : sources[0];
  let mode = chmodStart(under);
  let octal;
  const boxes = [];
  const paint = () => {
    modeBits(mode).forEach((on, i) => {
      boxes[i].checked = on;
    });
  };
  const r = await form({
    title: "Change attributes",
    cls: "chmod",
    build: (body) => {
      const what = sources.length === 1 ? `"${sources[0].name}"` : `${sources.length} file(s)/dir(s)`;
      body.append(el("p", null, `Change attributes of ${what}:`));
      const grid = el("div", "rwx");
      grid.append(el("span"), ...RWX.map((t) => el("span", "head", t)));
      WHO.forEach((who, w) => {
        grid.append(el("span", null, who));
        for (let k = 0; k < 3; k++) {
          const i = w * 3 + k;
          const box = el("input");
          box.type = "checkbox";
          box.title = `${who}: ${RWX[k]}`;
          box.addEventListener("change", () => {
            mode = withModeBit(mode, i, box.checked);
            octal.value = modeOctal(mode);
          });
          boxes.push(box);
          grid.append(box);
        }
      });
      octal = el("input");
      octal.type = "text";
      octal.spellcheck = false;
      octal.className = "octal";
      octal.value = modeOctal(mode);
      octal.addEventListener("input", () => {
        const m = parseModeOctal(octal.value);
        if (m === null) return;
        mode = m;
        paint();
      });
      const line = el("div", "srvfield");
      line.append(el("label", null, "Octal:"), octal);
      body.append(line, grid);
      if (sources.some((e) => e.isDir)) {
        body.append(el("p", "note gap", "Only the folders themselves change, not what is inside them."));
      }
      paint();
      return octal;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: () => {
      const m = parseModeOctal(octal.value);
      if (m === null) return "Enter 1 to 4 octal digits (0–7)";
      mode = m;
      return null;
    },
  });
  if (r !== "ok") return;
  // Every entry is tried; only a lost connection stops (the rest would fail the same way).
  const results = [];
  let lost = null;
  for (const [i, location] of paths.entries()) {
    try {
      await ctx.invoke("chmod", { location, mode });
      results.push({ name: sources[i].name, error: null });
    } catch (e) {
      if (errText(e) === CHMOD_NOT_IN_ARCHIVE) {
        await message("Change attributes", CHMOD_NOT_IN_ARCHIVE);
        return;
      }
      lost = lostServer(errText(e), location);
      if (lost) break;
      results.push({ name: sources[i].name, error: errText(e) });
    }
  }
  // Entries that changed are unmarked; failed (or not tried) ones stay marked.
  for (const res of results) if (res.error === null) panel.marked.delete(res.name);
  await Promise.all(ctx.panels().map((p) => p.refresh()));
  panel.render();
  if (lost) {
    serversLost([lost]);
    return;
  }
  const summary = chmodSummary(results);
  if (summary) await message("Change attributes", summary.join("\n"));
  else ctx.status(`Attributes set to ${modeOctal(mode)}`);
}

// ---------------------------------------------------------------- secrets of the editor

/**
 * After the editor saved `entry` (replacing `before`): stores or deletes secrets as
 * `secretChanges` says. Every call is sent first; a locked vault queues them in the backend,
 * and the master password is then asked once, which applies the whole queue.
 */
async function storeSecrets(entry, before, secret) {
  let master = null; // {create} of a needMaster reply
  const errors = [];
  for (const [kind, value] of secretChanges(before, entry, secret)) {
    try {
      await ctx.invoke("server_secret_set", { id: entry.id, kind, secret: value });
    } catch (e) {
      if (e?.kind === "needMaster") master = { create: e.create };
      else errors.push(errText(e));
    }
  }
  if (master && !(await askMaster(master.create))) {
    ctx.status("The password change waits until you enter the master password");
  }
  if (errors.length) await message("Password", errors.join("\n"));
}

// ---------------------------------------------------------------- connection editor

const PROTOCOLS = [
  ["sftp", "SFTP"],
  ["scp", "SCP"],
  ["ftp", "FTP"],
  ["ftps", "FTPS"],
  ["smb", "SMB (network share)"],
];
const AUTHS = [
  ["password", "Password"],
  ["key", "Key file"],
  ["keyPassphrase", "Key file + passphrase"],
];
const ENCODINGS = [
  ["utf8", "UTF-8"],
  ["windows1250", "Windows-1250"],
  ["latin1", "Latin-1"],
];

function select(options, value) {
  const s = el("select");
  for (const [v, label] of options) {
    const o = el("option", null, label);
    o.value = v;
    s.append(o);
  }
  s.value = value;
  return s;
}

function textInput(value) {
  const i = el("input");
  i.type = "text";
  i.spellcheck = false;
  i.autocomplete = "off";
  i.value = value ?? "";
  return i;
}

/** The native file/folder picker; the chosen path, or null (cancelled, or the error was shown). */
async function pick(options) {
  try {
    const p = await window.__TAURI__.dialog.open(options);
    return typeof p === "string" ? p : null;
  } catch (e) {
    await message("Choose", errText(e));
    return null;
  }
}

function smallButton(label, fn) {
  const b = el("button", "browse", label);
  b.type = "button";
  b.addEventListener("click", fn);
  return b;
}

/**
 * The "Edit connection" dialog. `save(entry)` stores the list with the entry and returns an
 * error text or null. Resolves with {entry, secret} after a successful save, else null.
 */
async function editServer(initial, { otherNames, save }) {
  const f = {};
  const rows = {};
  let result = null;
  const line = (name, label, ...controls) => {
    const r = el("div", "srvfield");
    r.append(el("label", null, label), ...controls);
    rows[name] = r;
    return r;
  };
  let protocol = initial.protocol;

  const sync = () => {
    const vis = fieldsFor(f.protocol.value);
    for (const [k, r] of Object.entries(rows)) r.hidden = !vis.has(k);
    const auth = vis.has("auth") ? f.auth.value : "password";
    if (auth === "password") rows.keyPath.hidden = true;
    if (auth === "key") rows.savePassword.hidden = true;
    f.secret.placeholder = auth === "keyPassphrase" ? "Passphrase" : "Password";
    if (initial.savePassword) f.secret.placeholder += " (leave empty to keep the saved one)";
    f.secret.disabled = !f.savePassword.checked;
    const smb = f.protocol.value === "smb";
    for (const b of rows.host.querySelectorAll("[data-smb-only]")) b.hidden = !smb;
    rows.remoteDir.firstChild.textContent = smb ? "Folder in share:" : "Remote dir:";
    const kerberos = smb && f.kerberos.checked;
    // Kerberos: the system's ticket logs in, there is no password to save.
    if (kerberos) rows.savePassword.hidden = true;
    f.user.placeholder = kerberos ? "optional with Kerberos" : smb ? "empty: guest; DOMAIN\\user for a domain" : "";
  };

  const r = await form({
    title: "Edit connection",
    cls: "srvedit",
    build: (body) => {
      f.name = textInput(initial.name);
      f.protocol = select(PROTOCOLS, initial.protocol);
      f.host = textInput(initial.host);
      // SMB: computers that share files on the local network (Bonjour, and Windows over NetBIOS).
      const browse = smallButton("Browse…", async () => {
        const done = busyNote("Looking for computers that share files…");
        let hosts = [];
        try {
          hosts = await ctx.invoke("smb_browse");
        } catch (e) {
          console.error("fxfm: smb_browse", e);
        } finally {
          done();
        }
        if (!hosts.length) {
          await message("Browse", "No computers sharing files were found on the local network.");
          return;
        }
        const r = await dialog({
          title: "Browse",
          text: "Computers sharing files on the local network:",
          buttons: [
            ...hosts.slice(0, 12).map((h, i) => ({ label: smbHostLabel(h), value: String(i), default: i === 0 })),
            { label: "Cancel", value: null },
          ],
        });
        if (r == null) return;
        const h = hosts[Number(r)];
        f.host.value = h.host;
        if (!f.name.value.trim()) f.name.value = h.name;
        f.share.focus();
      });
      browse.dataset.smbOnly = "1";
      f.port = el("input", "port");
      f.port.type = "number";
      f.port.min = "1";
      f.port.max = "65535";
      f.port.value = String(initial.port);
      f.user = textInput(initial.user);
      f.share = textInput(initial.share);
      // SMB: the shares the server lists for the entered user (smbutil view).
      const listShares = smallButton("Shares…", async () => {
        const host = f.host.value.trim();
        if (!host) {
          await message("Shares", "Enter the host first.");
          return;
        }
        const user = f.user.value.trim();
        const kerberos = f.kerberos.checked;
        let password = null;
        if (user && !kerberos) {
          password = f.secret.disabled ? "" : f.secret.value;
          if (!password) {
            const s = await askSecret({ title: "Shares", text: `Password of ${user} on ${host}:` });
            if (!s) return;
            password = s.secret;
          }
        }
        const done = busyNote(`Asking ${host} for its shares…`);
        let shares;
        try {
          shares = await ctx.invoke("smb_list_shares", { host, port: Number(f.port.value) || 445, user, kerberos, password });
        } catch (e) {
          done();
          await message("Shares", errText(e));
          return;
        }
        done();
        if (!shares.length) {
          await message("Shares", `${host} lists no shares for ${user || "a guest"}.`);
          return;
        }
        const r = await dialog({
          title: "Shares",
          text: `Shares on ${host}:`,
          buttons: [
            ...shares.slice(0, 12).map((sh, i) => ({ label: sh.comment ? `${sh.name} — ${sh.comment}` : sh.name, value: String(i), default: i === 0 })),
            { label: "Cancel", value: null },
          ],
        });
        if (r == null) return;
        f.share.value = shares[Number(r)].name;
        f.share.focus();
      });
      listShares.title = "List the shares of the server for this user";
      const kerb = checkbox("Use the Kerberos ticket (kinit or Ticket Viewer) instead of a password");
      f.kerberos = kerb.box;
      f.kerberos.checked = Boolean(initial.kerberos);
      f.auth = select(AUTHS, initial.auth);
      f.keyPath = textInput(initial.keyPath);
      const chooseKey = smallButton("Choose…", async () => {
        const p = await pick({ defaultPath: `${ctx.home}/.ssh` });
        if (p) f.keyPath.value = p;
      });
      const importKey = smallButton("Import…", async () => {
        const p = await pick({ defaultPath: `${ctx.home}/.ssh` });
        if (!p) return;
        try {
          f.keyPath.value = await ctx.invoke("import_key", { path: p });
        } catch (e) {
          await message("Import key", errText(e));
        }
      });
      importKey.title = "Copy the key into fxfm's data folder (keys/), readable only by you";
      const sp = checkbox("Save password");
      f.savePassword = sp.box;
      f.savePassword.checked = initial.savePassword;
      f.secret = passwordInput();
      f.remoteDir = textInput(initial.remoteDir);
      f.localDir = textInput(initial.localDir);
      const chooseLocal = smallButton("Choose…", async () => {
        const p = await pick({ directory: true, defaultPath: f.localDir.value || ctx.home });
        if (p) f.localDir.value = p;
      });
      const legacy = checkbox("Allow legacy algorithms (ssh-rsa/SHA1)");
      f.legacyAlgorithms = legacy.box;
      f.legacyAlgorithms.checked = initial.legacyAlgorithms;
      const passive = checkbox("Passive mode");
      f.ftpPassive = passive.box;
      f.ftpPassive.checked = initial.ftpPassive;
      f.ftpEncoding = select(ENCODINGS, initial.ftpEncoding);
      const auto = checkbox("Connect at startup (with the saved password)");
      f.autoConnect = auto.box;
      f.autoConnect.checked = Boolean(initial.autoConnect);

      body.append(
        line("name", "Name:", f.name),
        line("protocol", "Protocol:", f.protocol),
        line("host", "Host:", f.host, browse),
        line("port", "Port:", f.port),
        line("user", "User:", f.user),
        line("share", "Share:", f.share, listShares),
        line("kerberos", "", kerb.label),
        line("auth", "Authentication:", f.auth),
        line("keyPath", "Key file:", f.keyPath, chooseKey, importKey),
        line("savePassword", "", sp.label, f.secret),
        line("remoteDir", "Remote dir:", f.remoteDir),
        line("localDir", "Local dir:", f.localDir, chooseLocal),
        line("legacyAlgorithms", "", legacy.label),
        line("ftpPassive", "", passive.label),
        line("ftpEncoding", "Name encoding:", f.ftpEncoding),
        line("autoConnect", "", auto.label),
      );
      f.protocol.addEventListener("change", () => {
        f.port.value = String(portAfterProtocolChange(protocol, f.protocol.value, f.port.value));
        protocol = f.protocol.value;
        sync();
      });
      f.auth.addEventListener("change", sync);
      f.kerberos.addEventListener("change", sync);
      f.savePassword.addEventListener("change", sync);
      sync();
      return f.name;
    },
    buttons: [
      { label: "OK", value: "ok", default: true, submit: true },
      { label: "Cancel", value: null },
    ],
    submit: async () => {
      const ssh = fieldsFor(f.protocol.value).has("auth");
      const auth = ssh ? f.auth.value : "password";
      const kerberos = f.protocol.value === "smb" && f.kerberos.checked;
      const entry = {
        ...initial,
        name: f.name.value.trim(),
        protocol: f.protocol.value,
        host: f.host.value.trim(),
        port: Number(f.port.value),
        user: f.user.value.trim(),
        auth,
        keyPath: f.keyPath.value.trim() || null,
        savePassword: auth !== "key" && !kerberos && f.savePassword.checked,
        remoteDir: f.remoteDir.value.trim() || null,
        localDir: f.localDir.value.trim() || null,
        legacyAlgorithms: f.legacyAlgorithms.checked,
        ftpPassive: f.ftpPassive.checked,
        ftpEncoding: f.ftpEncoding.value,
        share: f.protocol.value === "smb" ? f.share.value.trim() : "",
        autoConnect: f.protocol.value === "smb" && f.autoConnect.checked,
        kerberos,
      };
      const problem = validateServer(entry, otherNames);
      if (problem) return problem;
      if (!Number.isInteger(entry.port) || entry.port < 1 || entry.port > 65535) {
        return "Port must be a number from 1 to 65535";
      }
      const failed = await save(entry);
      if (failed) return failed;
      result = { entry, secret: entry.savePassword ? f.secret.value : "" };
      return null;
    },
  });
  return r === "ok" ? result : null;
}

// ---------------------------------------------------------------- Connect to server dialog

/** The dialog's help text; the reorder keys are shown as the Mac keys in use (`mods`). */
const help = (mods) => [
  "Enter or double click: connect to the selected server. Esc: close.",
  `↑/↓: move the selection. ←/→: close or open a group. ${comboLabel("Alt+Up", mods)}/${comboLabel("Alt+Down", mods)} or dragging with the mouse: change the order (dropped on a group, an entry goes into it). ${comboLabel("Alt+Left", mods)}: take the entry out of its group. ${comboLabel("Alt+Right", mods)}: put it into the group right above it. Delete: delete the entry.`,
  `Groups (New group…) keep many servers in order, up to ${GROUP_LEVELS} levels: a new group or connection goes into the selected group. Edit… renames a group.`,
  "New URL…: connect once without saving it, e.g. sftp://user@host:2222/var/www (also scp://, ftp:// and ftps://). Without a user name FTP logs in as anonymous.",
  "SMB (network share): macOS mounts the share into /Volumes and fxfm opens it like a disk; every saved share is also in the drive list (Alt+F1/Alt+F2) and the drive bar. Disconnect unmounts it.",
  "Saved passwords are kept in the macOS Keychain (system mode) or, in portable mode, encrypted with a master password (Encrypt…).",
];

/** Size and maximized state of the Connect dialog, kept for the next Ctrl+F (this Mac only). */
const CONNECT_LAYOUT_KEY = "fxfm.connectDialog";
const CONNECT_MIN = { w: 480, h: 300 };

/**
 * Makes `box` (a dialog shown on its backdrop) a window inside the main window: dragged by
 * `title`, made larger (never smaller than its natural size) by its right and bottom edges and corner, maximized (and back) by a double
 * click on `title` or by `maxBtn`. Call after the box is in the page (its natural size is the
 * first size). The size and the maximized state are kept under `key`.
 */
function windowed(box, title, maxBtn, key, min) {
  let saved = {};
  try {
    saved = JSON.parse(localStorage.getItem(key) ?? "{}") ?? {};
  } catch {
    // No stored layout (private storage, bad JSON): the natural size.
  }
  const natural = box.getBoundingClientRect();
  // Never smaller than its natural size: below it the buttons no longer fit.
  min = { w: Math.max(min.w, natural.width), h: Math.max(min.h, natural.height) };
  const w = Number(saved.w) || natural.width;
  const h = Number(saved.h) || natural.height;
  let r = { x: (innerWidth - w) / 2, y: (innerHeight - h) / 2, w, h };
  let max = saved.max === true;
  const apply = () => {
    r = fitWindow(r, innerWidth, innerHeight, min);
    const g = max ? { x: 0, y: 0, w: innerWidth, h: innerHeight } : r;
    Object.assign(box.style, { left: `${g.x}px`, top: `${g.y}px`, width: `${g.w}px`, height: `${g.h}px` });
    box.classList.toggle("max", max);
    maxBtn.textContent = max ? "❐" : "□";
    maxBtn.title = max ? "Restore" : "Maximize";
  };
  const save = () => {
    try {
      localStorage.setItem(key, JSON.stringify({ w: r.w, h: r.h, max }));
    } catch {
      // Kept for this time only.
    }
  };
  const toggleMax = () => {
    max = !max;
    apply();
    save();
  };
  /** Mouse drag from `e`: `step(dx, dy)` changes `r` from where it was at the start. */
  const track = (e, step) => {
    if (e.button !== 0 || max) return;
    e.preventDefault();
    const start = { ...r };
    const move = (m) => {
      r = { ...start };
      step(m.clientX - e.clientX, m.clientY - e.clientY);
      apply();
    };
    const up = () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
      save();
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  };
  title.addEventListener("mousedown", (e) => {
    if (e.target === maxBtn) return;
    track(e, (dx, dy) => {
      r.x += dx;
      r.y += dy;
    });
  });
  title.addEventListener("dblclick", (e) => {
    if (e.target !== maxBtn) toggleMax();
  });
  maxBtn.addEventListener("click", toggleMax);
  for (const [cls, fx, fy] of [["rz-e", 1, 0], ["rz-s", 0, 1], ["rz-se", 1, 1]]) {
    const grip = el("div", `rz ${cls}`);
    grip.addEventListener("mousedown", (e) =>
      track(e, (dx, dy) => {
        r.w += dx * fx;
        r.h += dy * fy;
      }),
    );
    box.append(grip);
  }
  const onResize = () => (box.isConnected ? apply() : removeEventListener("resize", onResize));
  addEventListener("resize", onResize);
  box.classList.add("win");
  apply();
}

/** Folders the user closed in the tree (folder names from the root), for this run. */
const closedFolders = new Set();

/** Ctrl+F: the TC "Connect to server" dialog. Changes are saved right away. */
export async function showConnect() {
  let work;
  try {
    work = await ctx.invoke("servers_load");
  } catch (e) {
    ctx.status(`Cannot read the server list: ${errText(e)}`);
    return;
  }
  let sel = null;
  let busy = false;
  /** Names of the saved SMB shares that are mounted now. */
  let mounted = new Set();
  const readMounts = async () => {
    try {
      const shares = await ctx.invoke("smb_shares");
      mounted = new Set(shares.filter((sh) => sh.mount).map((sh) => sh.name));
    } catch (e) {
      console.error("fxfm: smb_shares", e);
    }
  };
  /** Node at `path`, or null when the path no longer exists (after a reload or a delete). */
  const nodeAt = (p) => {
    if (!p) return null;
    let l = work.nodes;
    for (const i of p.slice(0, -1)) {
      l = l[i]?.children;
      if (!l) return null;
    }
    return l[p[p.length - 1]] ?? null;
  };
  const isOpen = (node, path) => !closedFolders.has(folderKey(work.nodes, path));
  const visible = () => treeRows(work.nodes, isOpen);

  const backdrop = el("div", "backdrop");
  const box = el("div", "dialog hotcfg srvcfg");
  const list = el("div", "hotlist srvlist");
  list.tabIndex = 0;
  const modal = { el: backdrop };
  const close = () => popModal(modal);

  /** Saves a changed copy of the list; `change(next)` returns the new selection (or undefined). */
  const commit = async (change) => {
    const next = structuredClone(work);
    const newSel = change(next);
    if (groupsTooDeep(work.nodes, next.nodes)) {
      await message("Connect to server", `Groups can be at most ${GROUP_LEVELS} levels deep.`);
      return false;
    }
    try {
      await ctx.invoke("servers_save", { list: next });
    } catch (e) {
      await message("Connect to server", errText(e));
      return false;
    }
    work = next;
    if (newSel !== undefined) sel = newSel;
    render();
    // Saved SMB shares are drives: the drive list follows the server list.
    ctx.refreshVolumes?.();
    return true;
  };

  const reload = async () => {
    try {
      work = await ctx.invoke("servers_load");
    } catch (e) {
      ctx.status(`Cannot read the server list: ${errText(e)}`);
    }
    render();
  };

  /** Runs one action at a time; the dialog ignores input meanwhile. */
  const guarded = (fn) => async () => {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } catch (e) {
      await message("Connect to server", errText(e));
    } finally {
      busy = false;
      if (backdrop.isConnected && !box.contains(document.activeElement)) list.focus();
    }
  };

  let rowDrag = null; // {path, x, y, active}
  let rowEls = [];
  const buttons = {};

  function render() {
    if (sel && !nodeAt(sel)) sel = null;
    // The selection is always visible: open the folders above it (a new or dropped entry).
    for (let k = 1; sel && k < sel.length; k++) closedFolders.delete(folderKey(work.nodes, sel.slice(0, k)));
    const rows = visible();
    if (!sel && rows.length) sel = rows[0].path;
    list.replaceChildren();
    rowEls = [];
    for (const r of rows) {
      const row = el("div", "hrow srvrow");
      row.style.paddingLeft = `${4 + r.depth * 16}px`;
      if (r.node.kind === "folder") {
        const open = isOpen(r.node, r.path);
        const arrow = el("span", "twisty", open ? "▾" : "▸");
        arrow.addEventListener("click", (e) => {
          e.stopPropagation();
          toggle(r.path);
        });
        row.append(arrow, `📁 ${r.node.name}`);
        row.classList.add("folder");
      } else if (r.node.protocol === "smb") {
        const on = mounted.has(r.node.name) ? " (mounted)" : "";
        row.append(el("span", "twisty", ""), `🌐 ${r.node.name}${on}`);
      } else {
        row.append(el("span", "twisty", ""), `☁️ ${r.node.name}`);
      }
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
        sel = r.path;
        if (r.node.kind === "folder") toggle(r.path);
        else doConnect();
      });
      list.append(row);
    }
    paintSel();
  }

  function paintSel() {
    const key = sel?.join(".");
    for (const { row, path } of rowEls) row.classList.toggle("sel", path.join(".") === key);
    list.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
    const n = nodeAt(sel);
    buttons.connect.disabled = n?.kind !== "server";
    const isMounted = n?.kind === "server" && n.protocol === "smb" && mounted.has(n.name);
    buttons.disconnect.hidden = !isMounted;
    buttons.duplicate.disabled = n?.kind !== "server";
    buttons.edit.disabled = !n;
    buttons.del.disabled = !n;
  }

  function toggle(path) {
    const node = nodeAt(path);
    if (node?.kind !== "folder") return;
    const key = folderKey(work.nodes, path);
    if (closedFolders.has(key)) closedFolders.delete(key);
    else closedFolders.add(key);
    sel = path;
    render();
  }

  const moveSel = (delta) => {
    const rows = visible();
    if (!rows.length) return;
    const i = sel ? rows.findIndex((r) => r.path.join(".") === sel.join(".")) : -1;
    sel = rows[Math.min(rows.length - 1, Math.max(0, i + delta))].path;
    paintSel();
  };

  /** ←: close an open folder, else go to the parent folder. →: open a closed folder, else into it. */
  const leftRight = (right) => {
    const n = nodeAt(sel);
    if (!n) return;
    const isFolder = n.kind === "folder";
    const open = isFolder && isOpen(n, sel);
    if (right) {
      if (isFolder && !open) toggle(sel);
      else if (isFolder && n.children.length) {
        sel = [...sel, 0];
        paintSel();
      }
    } else if (open) {
      toggle(sel);
    } else if (sel.length > 1) {
      sel = sel.slice(0, -1);
      paintSel();
    }
  };

  const move = (delta) =>
    commit((next) => {
      if (!sel) return undefined;
      return moveNode(next.nodes, sel, delta);
    });

  /** Where a new node goes: into the selected folder, else after the selection, else at the end. */
  const insertAt = (nodes, node, after = sel) => {
    const at = after ? parentList(nodes, after)[after[after.length - 1]] : null;
    if (at?.kind === "folder") {
      at.children.push(node);
      return [...after, at.children.length - 1];
    }
    if (after) {
      parentList(nodes, after).splice(after[after.length - 1] + 1, 0, node);
      return [...after.slice(0, -1), after[after.length - 1] + 1];
    }
    nodes.push(node);
    return [nodes.length - 1];
  };

  /** Opens the editor for `entry` (new when `isNew`), saving it at `place(nodes, entry)`. */
  const editEntry = async (entry, before, place) => {
    const otherNames = serverNames(work.nodes).filter((n) => before === null || n !== before.name);
    let newSel;
    const r = await editServer(entry, {
      otherNames,
      save: async (e) => {
        const next = structuredClone(work);
        newSel = place(next.nodes, e);
        try {
          await ctx.invoke("servers_save", { list: next });
        } catch (err) {
          return errText(err);
        }
        work = next;
        ctx.refreshVolumes?.();
        return null;
      },
    });
    if (!r) return null;
    sel = newSel;
    render();
    await storeSecrets(r.entry, before, r.secret);
    return r.entry;
  };

  const newConnection = () =>
    editEntry(newServerEntry(crypto.randomUUID()), null, (nodes, e) => insertAt(nodes, e));

  const duplicate = async () => {
    const n = nodeAt(sel);
    if (n?.kind !== "server") return;
    const at = sel;
    const copy = {
      ...structuredClone(n),
      id: crypto.randomUUID(),
      name: duplicateName(n.name, serverNames(work.nodes)),
      savePassword: false,
    };
    const saved = await editEntry(copy, null, (nodes, e) => {
      parentList(nodes, at).splice(at[at.length - 1] + 1, 0, e);
      return [...at.slice(0, -1), at[at.length - 1] + 1];
    });
    if (saved) ctx.status(`Copied to '${saved.name}' without the password`);
  };

  const askFolderName = async (title, initial) => {
    const r = await dialog({ title, text: "Group name:", input: initial, buttons: OK_CANCEL });
    return r?.text.trim() || null;
  };

  const newFolder = async () => {
    const name = await askFolderName("New group", "");
    if (name) await commit((next) => insertAt(next.nodes, { kind: "folder", name, children: [] }));
  };

  const edit = async () => {
    const n = nodeAt(sel);
    if (!n) return;
    if (n.kind === "folder") {
      const name = await askFolderName("Rename group", n.name);
      if (name && name !== n.name) {
        const at = sel;
        const wasClosed = closedFolders.delete(folderKey(work.nodes, at));
        await commit((next) => {
          parentList(next.nodes, at)[at[at.length - 1]].name = name;
          return at;
        });
        if (wasClosed) closedFolders.add(folderKey(work.nodes, at));
        render();
      }
      return;
    }
    const at = sel;
    await editEntry(structuredClone(n), n, (nodes, e) => {
      parentList(nodes, at)[at[at.length - 1]] = e;
      return at;
    });
  };

  const del = async () => {
    const n = nodeAt(sel);
    if (!n) return;
    let text;
    if (n.kind === "server") text = `Delete '${n.name}' and its saved password?`;
    else {
      const count = serverNames(n.children).length;
      text = count
        ? `Delete group '${n.name}' and the ${count} server(s) in it, with their saved passwords?`
        : `Delete group '${n.name}'?`;
    }
    const ok = await dialog({
      title: "Delete",
      text,
      buttons: [
        { label: "Delete", value: "yes", default: true },
        { label: "Cancel", value: null },
      ],
    });
    if (ok !== "yes") return;
    const at = sel;
    await commit((next) => {
      const l = parentList(next.nodes, at);
      const i = at[at.length - 1];
      l.splice(i, 1);
      if (l.length) return [...at.slice(0, -1), Math.min(i, l.length - 1)];
      return at.length > 1 ? at.slice(0, -1) : null;
    });
  };

  const doConnect = guarded(async () => {
    const n = nodeAt(sel);
    if (n?.kind === "folder") return toggle(sel);
    if (n?.kind !== "server") return;
    const loc = await connectFlow({ server: n.name, save: false });
    // Success closes the dialog (the next Ctrl+F reads the list again, with any
    // "Save password" the connection turned on); otherwise the user is back here.
    if (loc) close();
    else await reload();
  });

  const disconnect = guarded(async () => {
    const n = nodeAt(sel);
    if (n?.kind !== "server" || n.protocol !== "smb") return;
    await disconnectShare(n.name);
    await readMounts();
    render();
  });

  const newUrl = guarded(async () => {
    const r = await dialog({
      title: "New URL",
      text: "URL (sftp://user@host:port/path):",
      input: "sftp://",
      select: { start: 7, end: 7 },
      buttons: OK_CANCEL,
    });
    const url = r?.text.trim();
    if (!url) return;
    const loc = await connectFlow({ url, save: false });
    if (loc) close();
  });

  const showHelp = () => dialog({ title: "Help", text: help(ctx.mods()), buttons: OK });

  // Reordering by mouse, not HTML5 drag & drop: the webview's native drag-drop handler
  // takes the drag events, so HTML5 `drop` never fires (same as the hotlist dialog).
  const rowAt = (x, y) => {
    const hit = document.elementFromPoint(x, y)?.closest(".srvrow");
    return rowEls.find((r) => r.row === hit) ?? null;
  };
  const paintDropRow = (target) => {
    for (const r of rowEls) r.row.classList.toggle("dropover", r === target);
  };
  backdrop.addEventListener("mousemove", (e) => {
    if (!rowDrag) return;
    if (!(e.buttons & 1)) {
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
    if (!d?.active || busy) return;
    const target = rowAt(e.clientX, e.clientY);
    if (!target) return;
    guarded(() => commit((next) => relocateNode(next.nodes, d.path, target.path) ?? undefined))();
  });

  const mkBtn = (key, label, fn) => {
    const b = el("button", null, label);
    b.type = "button";
    b.addEventListener("click", fn);
    if (key) buttons[key] = b;
    return b;
  };
  const side = el("div", "hotbtns");
  const connectBtn = mkBtn("connect", "Connect", doConnect);
  connectBtn.classList.add("def");
  side.append(
    connectBtn,
    mkBtn("disconnect", "Disconnect", disconnect),
    mkBtn(null, "New connection…", guarded(newConnection)),
    mkBtn(null, "New URL…", newUrl),
    mkBtn("duplicate", "Duplicate entry…", guarded(duplicate)),
    mkBtn(null, "New group…", guarded(newFolder)),
    mkBtn("edit", "Edit…", guarded(edit)),
    mkBtn("del", "Delete", guarded(del)),
    mkBtn(null, "Encrypt…", guarded(encryptDialog)),
    el("div", "gap"),
    mkBtn(null, "Cancel", close),
    mkBtn(null, "Help", showHelp),
  );
  const left = el("div", "hotleft");
  left.append(el("div", "hothead", "Servers:"), list);
  const main = el("div", "hotmain");
  main.append(left, side);
  const titleBar = el("div", "title", "Connect to server");
  // A span, not a button: it stays out of the Tab order and of Enter.
  const maxBtn = el("span", "winmax");
  titleBar.append(maxBtn);
  box.append(titleBar, main);
  backdrop.append(box);

  modal.onKey = (e) => {
    if (busy) {
      e.preventDefault();
      return;
    }
    const onList = document.activeElement === list;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      e.preventDefault();
      cycleFocus(box, e);
    } else if (reorderStep(e, ctx.mods())) {
      e.preventDefault();
      const step = reorderStep(e, ctx.mods());
      guarded(() => move(step))();
    } else if (tcModifiers(e, ctx.mods()).alt && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      // Alt, not Ctrl: macOS takes Control+←/→ for switching Spaces.
      e.preventDefault();
      const shift = e.key === "ArrowLeft" ? outdentNode : indentNode;
      // Nothing to save when the entry cannot go that way (top level, no group above).
      if (sel && shift(structuredClone(work.nodes), sel)) guarded(() => commit((next) => shift(next.nodes, sel)))();
    } else if (onList && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      moveSel(e.key === "ArrowUp" ? -1 : 1);
    } else if (onList && (e.key === "ArrowLeft" || e.key === "ArrowRight")) {
      e.preventDefault();
      leftRight(e.key === "ArrowRight");
    } else if (onList && (e.key === "Home" || e.key === "End")) {
      e.preventDefault();
      const rows = visible();
      if (rows.length) sel = rows[e.key === "Home" ? 0 : rows.length - 1].path;
      paintSel();
    } else if (onList && e.key === "Delete") {
      e.preventDefault();
      guarded(del)();
    } else if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      const f = document.activeElement;
      if (f?.tagName === "BUTTON" && box.contains(f)) f.click();
      else doConnect();
    }
  };

  await readMounts();
  render();
  pushModal(modal);
  windowed(box, titleBar, maxBtn, CONNECT_LAYOUT_KEY, CONNECT_MIN);
  list.focus();
}
