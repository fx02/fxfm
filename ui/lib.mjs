// Pure UI helpers (no DOM, testable with node:test).

const NAMED = {
  NumpadAdd: "NumAdd",
  NumpadSubtract: "NumSubtract",
  NumpadMultiply: "NumMultiply",
  NumpadEnter: "Enter",
  Equal: "=",
  Minus: "-",
  Backslash: "\\",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Space: "Space",
  Tab: "Tab",
  Enter: "Enter",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Escape: "Escape",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Home: "Home",
  End: "End",
};

function keyFromCode(code) {
  let m;
  if ((m = /^Key([A-Z])$/.exec(code))) return m[1];
  if ((m = /^Digit([0-9])$/.exec(code))) return m[1];
  if (/^F([1-9]|1[0-2])$/.test(code)) return code;
  return Object.hasOwn(NAMED, code) ? NAMED[code] : null;
}

/** Canonical combo string ("Ctrl+Shift+F6") for a keydown event, or null. */
export function comboFromEvent(e) {
  const key = keyFromCode(e.code);
  if (key === null) return null;
  return (
    (e.ctrlKey ? "Ctrl+" : "") +
    (e.altKey ? "Alt+" : "") +
    (e.shiftKey ? "Shift+" : "") +
    (e.metaKey ? "Cmd+" : "") +
    key
  );
}

/**
 * True when a key pressed in the command line belongs to the panels, as in TC:
 * F-keys, Ctrl/Alt combos and vertical cursor keys. Plain characters,
 * Backspace, Delete, Left/Right, Home/End and Space stay in the input.
 */
export function cmdlineKeyGoesToPanel(combo) {
  if (!combo) return false;
  if (/^(Ctrl|Alt)\+/.test(combo)) return combo !== "Ctrl+Enter";
  const key = combo.replace(/^Shift\+/, "");
  if (/^F([1-9]|1[0-2])$/.test(key)) return true;
  return ["Up", "Down", "PageUp", "PageDown", "Tab"].includes(combo);
}

/** 1234567 -> "1.234.567" (TC thousands separator). */
export function groupThousands(n) {
  return String(Math.trunc(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ".");
}

/** Bytes as TC kilobytes: floor(bytes / 1024) with dots, e.g. "288.594.048 k". */
export function formatK(bytes) {
  return `${groupThousands(Math.floor(bytes / 1024))} k`;
}

/** "[Macintosh HD]  288.594.048 k of 971.350.180 k free". */
export function freeLine(name, free, total) {
  return `[${name}]  ${formatK(free)} of ${formatK(total)} free`;
}

const dateFormats = new Map();

/** Unix seconds as "dd.MM.yyyy HH:mm" in `tz` (default: the system zone). */
export function formatDate(unixSecs, tz) {
  const key = tz ?? "";
  let f = dateFormats.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat("en-GB", {
      timeZone: tz,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23",
    });
    dateFormats.set(key, f);
  }
  const p = {};
  for (const { type, value } of f.formatToParts(new Date(unixSecs * 1000))) p[type] = value;
  return `${p.day}.${p.month}.${p.year} ${p.hour}:${p.minute}`;
}

/** "<DIR>" for folders, otherwise the byte size with dots. */
export function formatSize(entry) {
  return entry.isDir ? "<DIR>" : groupThousands(entry.size);
}

/** Unix permission string: "drwxr-xr-x", "lrwxr-xr-x", "-rw-r--r--". */
export function formatMode(mode, isDir, isSymlink) {
  const bit = (mask, ch) => (mode & mask ? ch : "-");
  const special = (exec, flag, on, off) => (mode & flag ? (mode & exec ? on : off) : bit(exec, "x"));
  return (
    (isSymlink ? "l" : isDir ? "d" : "-") +
    bit(0o400, "r") + bit(0o200, "w") + special(0o100, 0o4000, "s", "S") +
    bit(0o040, "r") + bit(0o020, "w") + special(0o010, 0o2000, "s", "S") +
    bit(0o004, "r") + bit(0o002, "w") + special(0o001, 0o1000, "t", "T")
  );
}

/** Folders are shown as "[name]". */
export function displayName(entry) {
  return entry.isDir ? `[${entry.name}]` : entry.name;
}

const ARCHIVE_EXT = new Set(["zip", "tar", "tgz", "gz", "7z", "bz2", "xz", "rar", "tbz2", "txz"]);
const MEDIA_EXT = new Set(["mp4", "mkv", "mov", "avi", "webm", "mp3", "flac", "wav", "m4a", "ogg", "aac"]);

/** CSS class for the entry's color: "hid" | "arc" | "med" | "" (hidden wins). */
export function colorClass(entry) {
  if (entry.hidden) return "hid";
  if (entry.isDir) return "";
  const dot = entry.name.lastIndexOf(".");
  if (dot <= 0) return "";
  const ext = entry.name.slice(dot + 1).toLowerCase();
  if (ARCHIVE_EXT.has(ext)) return "arc";
  if (MEDIA_EXT.has(ext)) return "med";
  return "";
}

function maskRegex(pattern) {
  let src = "";
  for (const ch of pattern) {
    if (ch === "*") src += ".*";
    else if (ch === "?") src += ".";
    else src += ch.replace(/[\\^$.|+()[\]{}]/g, "\\$&");
  }
  // TC: "name.*" also matches "name" without an extension, so "*.*" matches everything.
  if (pattern.endsWith(".*")) src = `${src.slice(0, -4)}(\\..*)?`;
  return new RegExp(`^${src}$`, "su");
}

/** TC file mask match: masks separated by ";" or spaces, "*" and "?", case- and NFC-insensitive. */
export function matchMask(name, mask) {
  const n = name.normalize("NFC").toLowerCase();
  return mask
    .normalize("NFC")
    .toLowerCase()
    .split(/[;\s]+/)
    .filter(Boolean)
    .some((p) => maskRegex(p).test(n));
}

/** Rows to render for a virtual list: [start, end) including `overscan` rows each side. */
export function visibleRange(scrollTop, rowH, viewH, count, overscan = 10) {
  const first = Math.floor(scrollTop / rowH);
  const shown = Math.ceil(viewH / rowH);
  return {
    start: Math.max(0, Math.min(count, first - overscan)),
    end: Math.max(0, Math.min(count, first + shown + overscan)),
  };
}

/** First and last row fully inside the view (Left/Right jump there). */
export function fullyVisibleRows(scrollTop, rowH, viewH, count) {
  if (count <= 0) return { first: 0, last: 0 };
  const clamp = (i) => Math.max(0, Math.min(count - 1, i));
  const first = clamp(Math.ceil(scrollTop / rowH - 0.01));
  const last = clamp(Math.floor((scrollTop + viewH) / rowH + 0.01) - 1);
  return { first, last: Math.max(first, last) };
}

/**
 * Left/Right target: first press goes to the first/last visible row; when the
 * cursor is already there, a further press moves a whole window up/down so
 * the cursor lands on the first/last row of the previous/next window.
 */
export function sideJump(cursor, first, last, dir) {
  const page = last - first + 1;
  if (dir < 0) return cursor !== first ? first : first - page;
  return cursor !== last ? last : last + page;
}

/** Without trailing slashes; "/" and a server root ("sftp://m/") keep theirs. */
function trimSlash(loc) {
  const r = parseRemote(loc);
  if (r) return `${r.scheme}://${r.server}${r.path}`;
  return loc.length > 1 && loc.endsWith("/") ? loc.replace(/\/+$/, "") || "/" : loc;
}

/** "/a/b" -> "/a"; "/" stays "/". On a server: "sftp://m/a" -> "sftp://m/", and the root -> null. */
export function parentLocation(loc) {
  const r = parseRemote(loc);
  if (r) {
    const path = r.path.replace(/\/{2,}/g, "/");
    if (path === "/") return null;
    return `${r.scheme}://${r.server}${path.slice(0, path.lastIndexOf("/")) || "/"}`;
  }
  const l = trimSlash(loc);
  const i = l.lastIndexOf("/");
  return i <= 0 ? "/" : l.slice(0, i);
}

export function joinLocation(dir, name) {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

/** "/a/b" -> "b"; "/" and a server root -> "". */
export function lastComponent(loc) {
  const l = trimSlash(loc);
  return l === "/" || l.endsWith("/") ? "" : l.slice(l.lastIndexOf("/") + 1);
}

/**
 * TC selection summary: "4.218 k / 288.594 k in 2 / 31 file(s)", plus
 * ", 1 / 3 dir(s)" when the listing has folders. Folder bytes count only
 * when computed (`dirSizes`: name -> bytes). The "[..]" row is ignored.
 */
export function markSummary(entries, marked, dirSizes = new Map()) {
  let mBytes = 0, tBytes = 0, mFiles = 0, tFiles = 0, mDirs = 0, tDirs = 0;
  for (const e of entries) {
    if (e.up) continue;
    const on = marked.has(e.name);
    const bytes = e.isDir ? dirSizes.get(e.name) ?? 0 : e.size;
    tBytes += bytes;
    if (on) mBytes += bytes;
    if (e.isDir) {
      tDirs++;
      if (on) mDirs++;
    } else {
      tFiles++;
      if (on) mFiles++;
    }
  }
  const head = `${formatK(mBytes)} / ${formatK(tBytes)} in ${mFiles} / ${tFiles} file(s)`;
  return tDirs ? `${head}, ${mDirs} / ${tDirs} dir(s)` : head;
}

/**
 * Lists `target` via `listFn`. On notFound, `opts.nearest(target)` gives the
 * closest existing folder to list instead. Never mutates `state`.
 * Returns {ok:true, location, entries, cursorName} or {ok:false, error}.
 * cursorName: `opts.cursorName`, else the folder we came from when going up.
 */
export async function navigate(state, target, listFn, opts = {}) {
  let location = trimSlash(target);
  let entries;
  try {
    entries = await listFn(location);
  } catch (error) {
    if (error?.kind !== "notFound" || !opts.nearest) return { ok: false, error };
    try {
      location = trimSlash(await opts.nearest(location));
      entries = await listFn(location);
    } catch (error2) {
      return { ok: false, error: error2 };
    }
  }
  let cursorName = opts.cursorName ?? null;
  if (cursorName === null) {
    // Going up (or falling back): put the cursor on the child we came from.
    const from = location === trimSlash(target) ? trimSlash(state.location ?? "") : trimSlash(target);
    const prefix = location.endsWith("/") ? location : `${location}/`;
    if (from.startsWith(prefix) && from !== location) {
      cursorName = from.slice(prefix.length).split("/")[0] || null;
    }
  }
  return { ok: true, location, entries, cursorName };
}

/**
 * Folder a refresh should re-list, or null: nothing for an empty panel, and
 * nothing while a user navigation is pending (a refresh would cancel it, and
 * that navigation brings a fresh listing anyway).
 */
export function refreshTarget({ location, pendingTarget }) {
  return location && pendingTarget == null ? location : null;
}

/** True when at least 100×50 (logical px) of `rect` is on one of `monitors` ({x,y,w,h}). */
export function rectOnScreen(rect, monitors) {
  return monitors.some((m) => {
    const w = Math.min(rect.x + rect.w, m.x + m.w) - Math.max(rect.x, m.x);
    const h = Math.min(rect.y + rect.h, m.y + m.h) - Math.max(rect.y, m.y);
    return w >= 100 && h >= 50;
  });
}

/**
 * Where the name's extension starts: the ".tar." of "x.tar.gz", else the
 * last dot. A leading dot (".bashrc") is not an extension. -1 when none.
 */
function extensionStart(name) {
  const tar = name.toLowerCase().lastIndexOf(".tar.");
  if (tar > 0) return tar;
  const dot = name.lastIndexOf(".");
  return dot > 0 ? dot : -1;
}

/** First free "name (n).ext" (n ≥ 2) not in `existing` (Set or array of names). */
export function suggestRename(name, existing) {
  const taken = existing instanceof Set ? existing : new Set(existing);
  const at = extensionStart(name);
  const base = at < 0 ? name : name.slice(0, at);
  const ext = at < 0 ? "" : name.slice(at);
  for (let n = 2; ; n++) {
    const candidate = `${base} (${n})${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** Inline rename selection: the name without its extension; whole name for folders. */
export function renameSelection(name, isDir = false) {
  const at = isDir ? -1 : extensionStart(name);
  return { start: 0, end: at < 0 ? name.length : at };
}

/** F5/F6 target: the other panel's folder with a trailing "/", plus the name of a single file. */
export function copyTargetDefault(otherLoc, sources) {
  const dir = otherLoc.endsWith("/") ? otherLoc : `${otherLoc}/`;
  return sources.length === 1 && !sources[0].isDir ? dir + sources[0].name : dir;
}

/** Entries an operation works on: the marked ones, else the one under the cursor; never "[..]". */
export function operationSources(entries, marked, cursor) {
  const real = entries.filter((e) => !e.up);
  if (real.some((e) => marked.has(e.name))) return real.filter((e) => marked.has(e.name));
  const e = entries[cursor];
  return e && !e.up ? [e] : [];
}

/**
 * Num+/Num− mask test: patterns ending in "/" select folders (matched
 * without the slash), all other patterns select files.
 */
export function maskSelects(entry, mask) {
  return mask
    .split(/[;\s]+/)
    .filter(Boolean)
    .some((p) => {
      const dirPattern = p.endsWith("/");
      if (dirPattern !== Boolean(entry.isDir)) return false;
      const pattern = dirPattern ? p.replace(/\/+$/, "") : p;
      return pattern !== "" && matchMask(entry.name, pattern);
    });
}

/** Folder and full source paths of an operation, taken before any dialog is awaited. */
export function operationPaths(location, sources) {
  return { cwd: location, paths: sources.map((e) => joinLocation(location, e.name)) };
}

/** Activation/dismissal keys ignored for this long after a modal opens (stray Enter from the previous one). */
export const MODAL_KEY_DELAY = 150;

/**
 * True when a modal must ignore the key: Enter, Escape or Space on a button
 * (activation / dismissal) in the first MODAL_KEY_DELAY ms, an auto-repeated
 * Enter, or an auto-repeated Space on a button. Typing always passes.
 * k: {key, repeat, onButton}.
 */
export function modalKeyBlocked(k, msSinceOpen) {
  const activates = k.key === "Enter" || (k.key === " " && k.onButton);
  if (msSinceOpen < MODAL_KEY_DELAY && (activates || k.key === "Escape")) return true;
  return k.repeat && activates;
}

/** Inline rename: {kind:"same"} | {kind:"invalid", reason} | {kind:"rename", name} (no trimming). */
export function renameDecision(original, value) {
  if (value === original) return { kind: "same" };
  if (!value.trim()) return { kind: "invalid", reason: "empty name" };
  if (value.includes("/")) return { kind: "invalid", reason: "name contains '/'" };
  if (value === "." || value === "..") return { kind: "invalid", reason: `invalid name '${value}'` };
  return { kind: "rename", name: value };
}

/** First real component of a relative path ("./a/b" -> "a"). */
export function firstComponent(name) {
  return name.split("/").find((c) => c && c !== ".") ?? "";
}

/**
 * Error text for a new folder (nested "a/b/c" allowed) or file name, or null
 * when valid: no absolute paths, no ".." and at least one real component.
 */
export function newNameError(name, nested) {
  if (name.startsWith("/")) return "Absolute paths are not allowed";
  if (!nested && name.includes("/")) return "The name must not contain '/'";
  const parts = name.split("/");
  if (parts.includes("..")) return "'..' is not allowed";
  if (!firstComponent(name)) return `Invalid name '${name}'`;
  return null;
}

/** Confirmation before a "Delete permanently" follow-up job. */
export function permanentConfirmText(paths) {
  return paths.length === 1
    ? `Delete "${lastComponent(paths[0])}" permanently?`
    : `Delete ${paths.length} remaining item(s) permanently?`;
}

/** A job://done payload of a job that ran to the end (skips allowed). */
export function jobSucceeded(done) {
  return Boolean(done.summary) && !done.summary.cancelled && !done.error;
}

/** An upload-back job (one file) ended with the file in the archive (not skipped after an error). */
export function uploadSucceeded(done) {
  return jobSucceeded(done) && done.summary.done > 0 && done.summary.skipped === 0;
}

/** Hex dump rows, 16 bytes each: `00000000  48 65 6c …  |Hel…|`. `offset` is the file offset of bytes[0]. */
export function hexLines(bytes, offset) {
  const rows = [];
  for (let i = 0; i < bytes.length; i += 16) {
    const chunk = bytes.slice(i, i + 16);
    const cells = Array.from({ length: 16 }, (_, k) => (k < chunk.length ? chunk[k].toString(16).padStart(2, "0") : "  "));
    const hex = `${cells.slice(0, 8).join(" ")}  ${cells.slice(8).join(" ")}`;
    const ascii = chunk.map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    rows.push(`${(offset + i).toString(16).padStart(8, "0")}  ${hex}  |${ascii}|`);
  }
  return rows;
}

/** Viewer status line, e.g. `UTF-8 · 12.345 bytes (truncated)`. */
export function formatViewStatus(encoding, bytes, truncated) {
  return `${encoding} · ${groupThousands(bytes)} bytes${truncated ? " (truncated)" : ""}`;
}

// ---------------------------------------------------------------- hotlist (mirrors crates/core hotlist.rs)

const SCHEME_RE = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//;

/** True for locations with a URL-like scheme (`sftp://...`). */
export function isRemote(location) {
  return SCHEME_RE.test(location);
}

/** `cd X` -> location (case-insensitive, trimmed, `~` expanded, schemes untouched) or null. */
export function parseCd(command, home) {
  const c = command.trim();
  if (c.slice(0, 2).toLowerCase() !== "cd") return null;
  const rest = c.slice(2);
  if (!/^\s/.test(rest)) return null;
  const x = rest.trim();
  if (!x) return null;
  if (isRemote(x)) return x;
  if (x === "~") return home;
  if (x.startsWith("~/")) return `${home.replace(/\/+$/, "")}/${x.slice(2)}`;
  return x;
}

/** Title -> {label, accel (lowercase), accelIndex (in label)}; `&&` is a literal `&`. */
export function menuLabel(title) {
  let label = "";
  let accel = null;
  let accelIndex = -1;
  const chars = [...title];
  for (let i = 0; i < chars.length; i++) {
    if (chars[i] !== "&") {
      label += chars[i];
    } else if (chars[i + 1] === "&") {
      label += "&";
      i++;
    } else if (i + 1 < chars.length && accel === null) {
      accel = chars[i + 1].toLowerCase();
      accelIndex = [...label].length;
    }
  }
  return { label, accel, accelIndex };
}

/** Appends the current location: title = last component, command = `cd <loc>` with `~` for home. */
export function addCurrent(nodes, location, home) {
  const trimmed = location.replace(/\/+$/, "");
  const title = trimmed.split("/").pop() || location;
  let shown = location;
  if (!isRemote(location)) {
    const h = home.replace(/\/+$/, "");
    if (trimmed === h) shown = "~";
    else if (trimmed.startsWith(`${h}/`)) shown = `~/${trimmed.slice(h.length + 1)}`;
  }
  nodes.push({ kind: "item", title, command: `cd ${shown}`, target: "" });
}

/** Nodes whose title does not start with `hideChar` (recursive); returns copies. */
export function visibleNodes(nodes, hideChar) {
  const out = [];
  for (const n of nodes) {
    if (hideChar && n.title.startsWith(hideChar)) continue;
    out.push(n.kind === "submenu" ? { ...n, children: visibleNodes(n.children, hideChar) } : { ...n });
  }
  return out;
}

/** Hotlist submenus and server folders: nodes that hold other nodes. */
function isContainer(node) {
  return Array.isArray(node?.children);
}

/** The node array that holds the node at `path` (array of indices). */
export function parentList(nodes, path) {
  let list = nodes;
  for (const i of path.slice(0, -1)) list = list[i].children;
  return list;
}

/** Moves the node at `path` by `delta` inside its parent; no-op out of bounds. Returns the new path. */
export function moveNode(nodes, path, delta) {
  const list = parentList(nodes, path);
  const i = path[path.length - 1];
  const j = i + delta;
  if (j < 0 || j >= list.length) return path;
  [list[i], list[j]] = [list[j], list[i]];
  return [...path.slice(0, -1), j];
}

/**
 * Drag & drop: moves the node at `from` onto the row at `to`. Dropping on a submenu (or server folder) puts the
 * node at the end of it; otherwise it takes the target's slot (after it when moving down within
 * one list). Returns the new path, or null when impossible (onto itself or its own subtree).
 */
export function relocateNode(nodes, from, to) {
  if (from.length <= to.length && from.every((v, k) => v === to[k])) return null;
  const src = parentList(nodes, from);
  const node = src[from[from.length - 1]];
  const dstParent = parentList(nodes, to);
  const target = dstParent[to[to.length - 1]];
  src.splice(from[from.length - 1], 1);
  if (isContainer(target)) {
    target.children.push(node);
    return pathOf(nodes, node);
  }
  const idx = dstParent.indexOf(target);
  dstParent.splice(src === dstParent && from[from.length - 1] <= idx ? idx + 1 : idx, 0, node);
  return pathOf(nodes, node);
}

/** Path of the node object `node` in the tree, or null. */
export function pathOf(nodes, node) {
  for (let i = 0; i < nodes.length; i++) {
    if (nodes[i] === node) return [i];
    if (isContainer(nodes[i])) {
      const p = pathOf(nodes[i].children, node);
      if (p) return [i, ...p];
    }
  }
  return null;
}

/** Double click on a menu item: adds `ch` at the start of the title, or removes it. */
export function toggleHidden(title, ch) {
  return title.startsWith(ch) ? title.slice(ch.length) : ch + title;
}

/** The rows of the configure list: [{path, node, depth}] with submenu children following their parent. */
export function flattenNodes(nodes, depth = 0, prefix = []) {
  const rows = [];
  nodes.forEach((node, i) => {
    const path = [...prefix, i];
    rows.push({ path, node, depth });
    if (isContainer(node)) rows.push(...flattenNodes(node.children, depth + 1, path));
  });
  return rows;
}

/** 1-based position of the node at `path` in the flattened list (what the config dialog shows), or null. */
export function selectedPosition(nodes, path) {
  if (!path) return null;
  const key = path.join(".");
  const i = flattenNodes(nodes).findIndex((r) => r.path.join(".") === key);
  return i < 0 ? null : i + 1;
}

/** Start folder of the folder picker: the Command's local `cd` target, else the active panel's location. */
export function pickerStart(command, home, activeLocation) {
  const loc = parseCd(command, home);
  return loc && !isRemote(loc) ? loc : activeLocation;
}

/** What macOS does with each reserved combo (keymap SYSTEM_RESERVED). */
const SYSTEM_USES = {
  "Ctrl+Up": "Mission Control",
  "Ctrl+Down": "Application windows",
  "Ctrl+Left": "Move left a space",
  "Ctrl+Right": "Move right a space",
  "Ctrl+Space": "Select the previous input source",
  "Ctrl+Alt+Space": "Select the next input source",
};

/** Warning for a binding macOS takes before fxfm sees it. */
export function conflictMessage(combo) {
  const use = SYSTEM_USES[combo] ?? "a system shortcut";
  return `macOS uses ${combo} for ${use}. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key.`;
}

/**
 * Configuration → Keyboard rows, in registry order:
 * [{id, label: "Files › Copy", keys: [combo...]}] with keys sorted.
 */
export function keyRows(commands, keymap) {
  const keys = new Map(commands.map((c) => [c.id, []]));
  for (const [combo, id] of Object.entries(keymap)) keys.get(id)?.push(combo);
  return commands.map((c) => ({ id: c.id, label: `${c.menu} › ${c.title}`, keys: keys.get(c.id).sort() }));
}

/** Archive kind by file name, same rules as Rust `ArchiveKind::from_name`; null when unsupported. */
export function archiveKindOf(name) {
  const n = name.toLowerCase();
  if (n.endsWith(".tar.gz") || n.endsWith(".tgz")) return "tgz";
  if (n.endsWith(".gz")) return "gz";
  if (n.endsWith(".tar")) return "tar";
  if (n.endsWith(".zip")) return "zip";
  if (n.endsWith(".7z")) return "7z";
  return null;
}

/** Location prefix up to the first archive component, or null. By name only: UI decisions, the backend decides. */
export function archiveOf(location) {
  const parts = location.split("/");
  for (let i = 1; i <= parts.length; i++) {
    if (parts[i - 1] && archiveKindOf(parts[i - 1])) return parts.slice(0, i).join("/");
  }
  return null;
}

/**
 * Server archives open as a tracked local copy; this map remembers `copy path -> folder on the
 * server` (in memory only) so "[..]" at the copy's root, a disconnect and the header know it.
 */
export const remoteArchives = new Map();

/** {archive, parent} when `location` is a server archive's temp copy or inside it, else null. */
export function remoteArchiveOf(location, remoteArchives) {
  const loc = location.length > 1 ? location.replace(/\/+$/, "") : location;
  for (const [archive, parent] of remoteArchives) {
    if (loc === archive || loc.startsWith(`${archive}/`)) return { archive, parent };
  }
  return null;
}

/** The folder on the server to return to from the root of a server archive's copy, else null. */
export function archiveParent(location, remoteArchives) {
  const r = remoteArchiveOf(location, remoteArchives);
  if (!r) return null;
  const loc = location.length > 1 ? location.replace(/\/+$/, "") : location;
  return loc === r.archive ? r.parent : null;
}

/** [..] from the root of a server archive's copy: {location: server folder, cursor: archive name}, else null. */
export function archiveLeave(location, remoteArchives) {
  const up = archiveParent(location, remoteArchives);
  if (up === null) return null;
  return { location: up, cursor: lastComponent(remoteArchiveOf(location, remoteArchives).archive) };
}

/** What the panel shows as path: inside a server archive's copy, the server location. */
export function displayLocation(location, remoteArchives) {
  const r = remoteArchiveOf(location, remoteArchives);
  if (!r) return location;
  const rest = location.slice(r.archive.length).replace(/\/+$/, "");
  return joinLocation(r.parent, lastComponent(r.archive)) + rest;
}

/** Archive path from "password required: <path>" / "wrong password: <path>", else null. */
export function passwordArchive(message) {
  const m = /^(?:password required|wrong password): ([\s\S]+)$/.exec(String(message ?? ""));
  return m ? m[1] : null;
}

/**
 * Startup dialog for edited archive copies a previous session did not upload. Keep is the
 * default (Enter), and Escape keeps them too: the dialog appears again at the next start.
 */
export function pendingEditsDialog(edits) {
  return {
    title: "fxfm",
    text: [
      "Edited copies from a previous session were not uploaded:",
      ...edits.map((e) => `${e.location} → ${e.local}`),
    ],
    buttons: [
      { label: "Show in Finder", value: "show" },
      { label: "Keep", value: "keep", default: true },
      { label: "Discard", value: "discard" },
    ],
  };
}

/** Confirmation of Discard; Enter and Escape answer No. */
export function discardPendingConfirm() {
  return {
    title: "fxfm",
    text: "Move these copies to the Trash?",
    buttons: [
      { label: "Yes", value: "yes" },
      { label: "No", value: null, default: true },
    ],
  };
}

// ---------------------------------------------------------------- F3 audio

/** Extensions F3 plays instead of opening the viewer. */
export const AUDIO_EXTENSIONS = new Set([
  "wav", "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "aif", "aiff", "au", "gsm",
]);

/** True when F3 plays `name` (by extension, any case; `.wav` alone is a hidden file, not audio). */
export function isAudioName(name) {
  const s = String(name ?? "");
  const dot = s.lastIndexOf(".");
  return dot > 0 && AUDIO_EXTENSIONS.has(s.slice(dot + 1).toLowerCase());
}

/** Seconds as `m:ss` (whole seconds, minutes not wrapped). */
export function formatClock(secs) {
  const s = Number.isFinite(secs) && secs > 0 ? Math.floor(secs) : 0;
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** `GSM 6.10, 8000 Hz, mono` from the AudioInfo `audio_play` returns. */
export function codecSummary(info) {
  if (!info) return "";
  const ch = info.channels === 1 ? "mono" : info.channels === 2 ? "stereo" : `${info.channels} channels`;
  return `${info.codec}, ${info.sampleRate} Hz, ${ch}`;
}

/**
 * True when an `audio://state` event may update the strip: request ids only grow, and events
 * of requests older than the newest F3 (`player.minId`) are late leftovers. UI-made states
 * (no id) always pass.
 */
export function acceptAudioEvent(player, ev) {
  return ev.id == null || ev.id >= (player.minId ?? 0);
}

/** Loading or playing: F3 on that file again, or Escape, stops it. */
export function audioActive(player) {
  return player.state === "loading" || player.state === "playing";
}

/** What F3 on `path` does: "stop" when that file is the one loading or playing, else "play". */
export function audioF3Action(player, path) {
  return audioActive(player) && player.path === path ? "stop" : "play";
}

/**
 * True when Escape in the panels stops playback: something plays and Escape has no other
 * job there (no key binding on it, no text in the command line). Dialogs, popups and focused
 * fields get Escape before this is asked.
 */
export function escapeStopsAudio({ player, escapeBound, commandText }) {
  return audioActive(player) && !escapeBound && !String(commandText ?? "").trim();
}

/** The player strip for a state: null when hidden, else its texts and the progress (0..1). */
export function playerStrip(player) {
  const { state, name, position = 0, duration = null } = player;
  if (state === "loading") return { text: `Loading ${name}…`, clock: "", codec: "", progress: null };
  if (state === "error") return { text: player.message ?? "", clock: "", codec: "", progress: null, error: true };
  if (state !== "playing") return null;
  const total = duration ?? null;
  return {
    text: name,
    clock: `${formatClock(position)} / ${total == null ? "-:--" : formatClock(total)}`,
    // A note from the player (the chosen output device is missing) takes the codec's place.
    codec: player.message || codecSummary(player.info),
    progress: total ? Math.min(1, Math.max(0, position / total)) : null,
  };
}

/**
 * Options → Audio → Output device: `System default` (value "") first, then each device name
 * once in the system's order; a saved device that is not connected stays as
 * `<name> (not connected)`.
 */
export function outputOptions(devices, saved) {
  const out = [{ value: "", label: "System default" }];
  const seen = new Set([""]);
  for (const name of devices ?? []) {
    if (seen.has(name)) continue;
    seen.add(name);
    out.push({ value: name, label: name });
  }
  if (saved && !seen.has(saved)) out.push({ value: saved, label: `${saved} (not connected)` });
  return out;
}

/** Seek target for a click at `x` on a bar spanning `left..left+width`. */
export function seekSeconds(x, left, width, duration) {
  if (!duration || !(width > 0)) return null;
  return Math.min(1, Math.max(0, (x - left) / width)) * duration;
}

// ---------------------------------------------------------------- drag & drop

/** Names a drag starting on `pressedName` carries: all marks when it is marked, else just it; never "..". */
export function dragSelection(entries, marks, pressedName) {
  const pressed = entries.find((e) => !e.up && e.name === pressedName);
  if (!pressed) return [];
  if (!marks.has(pressed.name)) return [pressed.name];
  return entries.filter((e) => !e.up && marks.has(e.name)).map((e) => e.name);
}

/** Folder location without trailing slashes ("sftp://h/" -> "sftp://h", "/" stays "/"). */
function plainLocation(loc) {
  return loc.length > 1 ? loc.replace(/\/+$/, "") || "/" : loc;
}

/**
 * Where a drop goes: the folder row under the pointer (not ".."), else the panel's folder.
 * null ("do nothing") for the sources' own folder, a dragged entry itself, or a folder
 * inside a dragged folder. `sourcePanelLocation` is null for drops from other apps.
 */
export function dropTarget({ panelLocation, rowEntry, draggedLocations, sourcePanelLocation }) {
  const target = rowEntry && rowEntry.isDir && !rowEntry.up ? joinLocation(panelLocation, rowEntry.name) : panelLocation;
  const t = plainLocation(target);
  if (sourcePanelLocation != null && t === plainLocation(sourcePanelLocation)) return null;
  const dragged = draggedLocations.map(plainLocation);
  if (dragged.some((d) => t === d || t.startsWith(`${d}/`))) return null;
  if (dragged.length && dragged.every((d) => plainLocation(parentLocation(d)) === t)) return null;
  return target;
}

/** Folder all `paths` are in, or null when they differ (or there are none). */
export function commonParent(paths) {
  const parents = new Set(paths.map((p) => parentLocation(p)));
  return parents.size === 1 ? [...parents][0] : null;
}

/** Pixels the pointer must move with the button down before a drag starts. */
export const DRAG_THRESHOLD = 5;

/** True once the pointer moved more than DRAG_THRESHOLD px from the press. */
export function dragStarted(from, to) {
  return Math.hypot(to.x - from.x, to.y - from.y) > DRAG_THRESHOLD;
}

/**
 * CSS pixel point of a Tauri drag-drop position. On macOS wry reports the position in
 * points from the webview's top-left (already CSS px) although the type says physical;
 * `logical` = false divides by the scale factor instead.
 */
export function dragDropPoint(position, scale, logical) {
  if (logical || !(scale > 0)) return { x: position.x, y: position.y };
  return { x: position.x / scale, y: position.y / scale };
}

/** Same non-empty set of paths (order ignored). */
export function samePaths(a, b) {
  if (!a.length || a.length !== b.length) return false;
  const s = new Set(a);
  return b.every((p) => s.has(p));
}

// ---------------------------------------------------------------- servers (mirrors crates/core servers.rs)

const SSH_PROTOCOLS = new Set(["sftp", "scp"]);
const DEFAULT_PORT = { sftp: 22, scp: 22, ftp: 21, ftps: 21 };

/** Port and login of a new server for `protocol`. */
export function serverDefaults(protocol) {
  return { port: DEFAULT_PORT[protocol] ?? 22, auth: "password" };
}

/** A new SFTP server entry (as `servers.json` stores it) with the given id. */
export function newServerEntry(id) {
  return {
    kind: "server",
    id,
    name: "",
    protocol: "sftp",
    host: "",
    port: 22,
    user: "",
    auth: "password",
    keyPath: null,
    savePassword: false,
    remoteDir: null,
    localDir: null,
    legacyAlgorithms: false,
    ftpPassive: true,
    ftpEncoding: "utf8",
  };
}

/**
 * First problem with `entry`, or null; the texts of `ServerList::validate`.
 * `allNames`: the names of every other server in the list (all folders).
 */
export function validateServer(entry, allNames) {
  if (!entry.name.trim()) return "Name is required";
  if (entry.name.includes("/")) return "Name cannot contain '/'";
  if (allNames.includes(entry.name)) return `Another server is already named '${entry.name}'`;
  if (!entry.host.trim()) return "Host is required";
  if ((entry.auth === "key" || entry.auth === "keyPassphrase") && !entry.keyPath?.trim()) {
    return "Choose a key file";
  }
  return null;
}

/** The editor fields shown for `protocol`. */
export function fieldsFor(protocol) {
  const f = ["name", "protocol", "host", "port", "user", "savePassword", "remoteDir", "localDir"];
  if (SSH_PROTOCOLS.has(protocol)) f.push("auth", "keyPath", "legacyAlgorithms");
  else f.push("ftpPassive", "ftpEncoding");
  return new Set(f);
}

/** A changed protocol takes its own default port only when the port was the old one's default. */
export function portAfterProtocolChange(oldProto, newProto, port) {
  const n = Number(port);
  return n === serverDefaults(oldProto).port ? serverDefaults(newProto).port : n;
}

/** Drive-bar label of a connection from `list_connections`. */
export function connectionLabel(info) {
  return `☁️ ${info.name}`;
}

/** Question text of a `hostKey` connect reply (new or changed key). */
export function hostKeyText(reply) {
  const at = `${reply.host}:${reply.port}`;
  if (reply.changedFrom) {
    return [
      `WARNING: THE HOST KEY FOR '${at}' HAS CHANGED!`,
      "Someone may be intercepting the connection, or the server was reinstalled.",
      `Old: ${reply.changedFrom}`,
      `New: ${reply.fingerprint}`,
      "Replace the stored key and connect?",
    ].join("\n");
  }
  return [
    `The authenticity of host '${at}' can't be established.`,
    `${reply.keyType} key fingerprint is ${reply.fingerprint}.`,
    "Connect and remember this key?",
  ].join("\n");
}

/** `scheme://server/path` -> {scheme, server, path} (path absolute, no trailing /), else null. */
export function parseRemote(loc) {
  const m = /^(sftp|scp|ftp|ftps):\/\/([^/]*)(\/.*)?$/s.exec(loc);
  if (!m || !m[2]) return null;
  const path = (m[3] ?? "/").replace(/\/+$/, "") || "/";
  return { scheme: m[1], server: m[2], path };
}

/** Root of the server `loc` is on (`sftp://m/`), or null for local paths. */
export function remoteRoot(loc) {
  const r = parseRemote(loc);
  return r ? `${r.scheme}://${r.server}/` : null;
}

/** `loc`'s server path on the connection `root` (`list_connections` root: its scheme wins). */
export function onRoot(root, loc) {
  const path = parseRemote(loc)?.path ?? "/";
  return root + path.slice(1);
}

const LOST_PREFIXES = ["connection lost", "server not responding"];

/**
 * True for an error meaning the connection is gone: the message starts with `connection lost`
 * or `server not responding`, also after a `<location>: ` prefix (panel listing errors).
 */
export function connectionLost(message) {
  const m = String(message ?? "");
  return LOST_PREFIXES.some((p) => m.startsWith(p) || m.endsWith(`: ${p}`));
}

/**
 * Names of a `connections://changed` `lost` list that need the "connection lost" dialog:
 * each once, and not those in `closed` (closed on purpose and not connected again since).
 */
export function lostToShow(lost, closed) {
  return [...new Set(lost)].filter((n) => !closed.has(n));
}

/** Unique server names of a job's locations (sources, destination, cwd); others are skipped. */
export function jobServers(locations) {
  const names = [];
  for (const l of locations) {
    const s = l ? parseRemote(l)?.server : null;
    if (s && !names.includes(s)) names.push(s);
  }
  return names;
}

/**
 * A finished job's error that needs the plain Error box: not a cancel. A lost connection gets no
 * box only when the job used servers and all of them (`servers`) were in `connectedAtStart`:
 * then the backend's `connections://changed` names the dead one(s) exactly and app.js opens
 * their dialog. Otherwise no such event comes, so the box shows.
 */
export function jobErrorBox(err, servers, connectedAtStart) {
  if (!err || err.kind === "cancelled") return false;
  if (!connectionLost(err.message)) return true;
  return !(servers.length > 0 && servers.every((s) => connectedAtStart.includes(s)));
}

/** `open_default` refusal for a folder on a server (`materialize_remote` in crates/core/src/vfs/mod.rs). */
export const REMOTE_FOLDER_REFUSED = "Only files can be opened from a server";

/**
 * Enter on a symlink on a server: listings show links as they are (not followed), so a link to
 * a folder looks like a file. Enter opens it; when `open_default` refuses because the target is
 * a folder (`message`), the panel enters it instead.
 */
export function remoteLinkIsFolder(message) {
  return String(message ?? "") === REMOTE_FOLDER_REFUSED;
}

/** Backend refusal of `chmod` outside servers; same as `CHMOD_ONLY_REMOTE` in src-tauri/src/remote.rs. */
export const CHMOD_ONLY_REMOTE = "Change attributes works on SFTP and SCP servers";

/** Lines of the chmod result box ([{name, error|null}]), or null when every entry changed. */
export function chmodSummary(results) {
  const failed = results.filter((r) => r.error != null);
  if (!failed.length) return null;
  return [`Changed ${results.length - failed.length} of ${results.length}`, ...failed.map((r) => `${r.name}: ${r.error}`)];
}

/** F8 / Shift+F8 question: on a server always permanent, else the archive / Trash / permanent text. */
export function deleteConfirmText(location, n, { inArchive = false, toTrash = false } = {}) {
  if (parseRemote(location)) return `Delete ${n} file(s) from the server? This cannot be undone.`;
  if (inArchive) return `Delete ${n} file(s) from archive? This cannot be undone.`;
  const what = `${n} file(s)/dir(s)`;
  return toTrash ? `Do you really want to move ${what} to the Trash?` : `Do you really want to delete ${what} permanently?`;
}

/** F4 upload-back questions for the edited copy of `location` (a server or an archive file). */
export function editTexts(location, name) {
  const remote = !!parseRemote(location);
  const where = remote ? "on the server" : "in the archive";
  return {
    upload: `"${name}" was changed. Upload it back ${remote ? "to the server" : "into the archive"}?`,
    conflict: `"${name}" was changed ${where} since you opened it. Overwrite it with your edited copy?`,
  };
}

/** Permission bits as octal text: 3 digits, 4 when setuid/setgid/sticky is set. */
export function modeOctal(mode) {
  const m = mode & 0o7777;
  return m.toString(8).padStart(m > 0o777 ? 4 : 3, "0");
}

/** Octal text (1 to 4 digits 0–7, spaces trimmed) -> mode, else null. */
export function parseModeOctal(text) {
  const t = String(text).trim();
  return /^[0-7]{1,4}$/.test(t) ? parseInt(t, 8) : null;
}

/** The nine rwx boxes (owner r,w,x, group r,w,x, others r,w,x) of `mode`. */
export function modeBits(mode) {
  return Array.from({ length: 9 }, (_, i) => (mode & (0o400 >> i)) !== 0);
}

/** `mode` with box `i` (modeBits order) set or cleared; other bits are kept. */
export function withModeBit(mode, i, on) {
  const bit = 0o400 >> i;
  return on ? mode | bit : mode & ~bit;
}

/** Start value of the chmod dialog: the entry's permissions, else 644 (file) / 755 (folder). */
export function chmodStart(entry) {
  const m = (entry?.mode ?? 0) & 0o7777;
  if (m) return m;
  return entry?.isDir ? 0o755 : 0o644;
}

/** Name of a duplicated server: `<name> (2)`, or the next free number. */
export function duplicateName(name, allNames) {
  for (let n = 2; ; n++) {
    const candidate = `${name} (${n})`;
    if (!allNames.includes(candidate)) return candidate;
  }
}

/** Names of every server in the tree (folders included). */
export function serverNames(nodes) {
  return flattenNodes(nodes)
    .filter((r) => r.node.kind === "server")
    .map((r) => r.node.name);
}

/** Visible rows of a folder tree: like flattenNodes, but a folder's children only when `isOpen(node, path)`. */
export function treeRows(nodes, isOpen, depth = 0, prefix = []) {
  const rows = [];
  nodes.forEach((node, i) => {
    const path = [...prefix, i];
    rows.push({ path, node, depth });
    if (isContainer(node) && isOpen(node, path)) rows.push(...treeRows(node.children, isOpen, depth + 1, path));
  });
  return rows;
}

/** `F/G`: the folder names down to the node at `path`; names an open folder across reloads. */
export function folderKey(nodes, path) {
  const names = [];
  let list = nodes;
  for (const i of path) {
    names.push(list[i].name);
    list = list[i].children ?? [];
  }
  return names.join("/");
}

/** `master_unlock` / `master_set` errors that still leave the vault unlocked. */
export function masterPartial(message) {
  return /, but \d+ password\(s\) could not be saved$/.test(String(message));
}

/**
 * Master password prompt an Options → Storage switch from `from` to `to` needs (`status` is
 * `master_status`: {mode, exists, unlocked}), as `set_storage_mode` requires it:
 * "new" to Portable (a new password file is created; asked twice), "current" to System when
 * the portable password file exists and is locked, otherwise "none".
 */
export function storageMasterPrompt(from, to, status) {
  if (from === to) return "none";
  if (to === "portable") return "new";
  return status?.exists && !status?.unlocked ? "current" : "none";
}

/**
 * Name shown for a "New URL…" request: `user@host`, or `user@host:port` when the port is not
 * the protocol's default (the name `parse_url` gives the quick connection). The URL's password
 * and path are never part of it; an URL `parse_url` would reject gives "the server".
 */
export function urlDisplayName(url) {
  const fallback = "the server";
  const s = String(url ?? "").trim();
  const i = s.indexOf("://");
  if (i < 0) return fallback;
  const protocol = s.slice(0, i).toLowerCase();
  if (!(protocol in DEFAULT_PORT)) return fallback;
  const rest = s.slice(i + 3);
  // Userinfo as `split_userinfo` in crates/core/src/servers.rs: when it may hold a password
  // (a `:` before the first `@`, or a `:` and no `@`, before the first `/`) it ends at the
  // URL's last `@`, so a password with `/`, `@` or digits never reaches host, port or name.
  const slash = rest.indexOf("/");
  const head = slash < 0 ? rest : rest.slice(0, slash);
  const colon1 = head.indexOf(":");
  const at1 = head.indexOf("@");
  const password = !head.startsWith("[") && colon1 >= 0 && (at1 < 0 || colon1 < at1);
  const at = password ? rest.lastIndexOf("@") : head.lastIndexOf("@");
  const rawUser = at < 0 ? "" : rest.slice(0, at).split(":")[0];
  const after = at < 0 ? rest : rest.slice(at + 1);
  const slash2 = after.indexOf("/");
  const hostport = slash2 < 0 ? after : after.slice(0, slash2);
  let host = hostport;
  let port = DEFAULT_PORT[protocol];
  const colon = hostport.lastIndexOf(":");
  if (colon >= 0 && !hostport.slice(colon + 1).includes("]")) {
    const p = hostport.slice(colon + 1);
    if (!/^\d+$/.test(p) || Number(p) > 65535) return fallback;
    host = hostport.slice(0, colon);
    port = Number(p);
  }
  host = host.replace(/^\[+/, "").replace(/\]+$/, "");
  if (!host) return fallback;
  let user = rawUser || "anonymous";
  try {
    user = decodeURIComponent(user);
  } catch {
    // Not valid percent-encoding: shown as typed (it holds no password).
  }
  return port === DEFAULT_PORT[protocol] ? `${user}@${host}` : `${user}@${host}:${port}`;
}

/** Secret kind a server's login uses, or null (key file without passphrase). */
function secretKind(entry) {
  if (entry.auth === "keyPassphrase") return "passphrase";
  return entry.auth === "key" ? null : "password";
}

/**
 * `server_secret_set` calls after the editor saved `entry` (`before`: the saved entry it
 * replaced, or null), as [kind, secret|null]. A typed secret is stored when Save password is
 * on (an empty field keeps the stored one); turning Save password off deletes both kinds; a
 * changed login deletes the stored kind that no longer applies.
 */
export function secretChanges(before, entry, secret) {
  const out = [];
  const kind = entry.savePassword ? secretKind(entry) : null;
  if (kind && secret) out.push([kind, secret]);
  if (before?.savePassword) {
    if (!entry.savePassword) out.push(["password", null], ["passphrase", null]);
    else {
      const old = secretKind(before);
      if (old && old !== kind) out.push([old, null]);
    }
  }
  return out;
}
