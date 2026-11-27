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

/** Which Mac key acts as TC's Alt and Ctrl (settings `modifiers`); the TC default. */
export const DEFAULT_MODIFIERS = Object.freeze({ alt: "option", ctrl: "control" });

/**
 * Mac keys that can act as Alt or Ctrl, with their names. WebKit never reports Fn, so the app
 * tracks it natively (`keys://fn` → `setFnHeld`).
 */
export const MAC_KEYS = [
  ["option", "Option"],
  ["control", "Control"],
  ["command", "Command"],
  ["fn", "Fn (globe)"],
];

let fnHeld = false;

/** The Fn key went down or up (`keys://fn` from the native monitor). */
export function setFnHeld(down) {
  fnHeld = Boolean(down);
}

const MAC_KEY_IDS = MAC_KEYS.map(([id]) => id);

/** The Alt/Ctrl mapping of `settings`; unknown or doubled keys give the default. */
export function modifierMap(settings) {
  const m = settings?.modifiers ?? {};
  const pick = (v, def) => (MAC_KEY_IDS.includes(v) ? v : def);
  const alt = pick(m.alt, DEFAULT_MODIFIERS.alt);
  const ctrl = pick(m.ctrl, DEFAULT_MODIFIERS.ctrl);
  return alt === ctrl ? { ...DEFAULT_MODIFIERS } : { alt, ctrl };
}

/** Why the chosen Alt and Ctrl keys cannot be saved, or null. */
export function modifierChoiceError(alt, ctrl) {
  if (alt !== ctrl) return null;
  const name = MAC_KEYS.find(([id]) => id === alt)?.[1] ?? alt;
  return `Alt and Ctrl need different keys: ${name} is chosen for both.`;
}

/**
 * The TC modifiers of a key event under `map`: {ctrl, alt, shift, cmd, stray, any}. `cmd` is
 * the Command key when it is neither Alt nor Ctrl; `stray` is a held Control or Option that is
 * neither (it makes no TC combo); `any` is any of Control, Option or Command held.
 */
export function tcModifiers(e, map = DEFAULT_MODIFIERS) {
  const held = { control: Boolean(e.ctrlKey), option: Boolean(e.altKey), command: Boolean(e.metaKey), fn: Boolean(e.fnKey ?? fnHeld) };
  const mapped = (k) => k === map.alt || k === map.ctrl;
  return {
    ctrl: held[map.ctrl] ?? false,
    alt: held[map.alt] ?? false,
    shift: Boolean(e.shiftKey),
    cmd: held.command && !mapped("command"),
    stray: (held.control && !mapped("control")) || (held.option && !mapped("option")),
    any: held.control || held.option || held.command,
  };
}

/**
 * Command+these stay with the native menu (fxfm › Quit, Hide; Option+Command+H Hide Others)
 * even when Command is Alt or Ctrl. The menu has no Close or Minimize item.
 */
const MAC_OWNED = new Set(["Q", "H"]);

/**
 * Canonical TC combo string ("Ctrl+Shift+F6") for a keydown event under the Alt/Ctrl `map`,
 * or null (a modifier on its own, an unknown key, a Control/Option that is neither Alt nor
 * Ctrl, or a Command shortcut macOS owns while Command is Alt or Ctrl).
 */
export function comboFromEvent(e, map = DEFAULT_MODIFIERS) {
  return keyCombo(e, map).combo;
}

/** {combo} for a key event, or {combo: null, why} with "stray" / "macOwned" / "noKey". */
function keyCombo(e, map) {
  const key = keyFromCode(e.code);
  if (key === null) return { combo: null, why: "noKey" };
  const m = tcModifiers(e, map);
  if (m.stray) return { combo: null, why: "stray", key };
  const commandMapped = map.alt === "command" || map.ctrl === "command";
  if (e.metaKey && commandMapped && MAC_OWNED.has(key)) return { combo: null, why: "macOwned", key };
  return { combo: (m.ctrl ? "Ctrl+" : "") + (m.alt ? "Alt+" : "") + (m.shift ? "Shift+" : "") + (m.cmd ? "Cmd+" : "") + key };
}

/**
 * Configuration → Keyboard, "Add key": why a real key (not a lone modifier) gives no combo,
 * or null when it gives one or is no key at all.
 */
export function captureRefusal(e, map = DEFAULT_MODIFIERS) {
  const r = keyCombo(e, map);
  if (r.why === "macOwned") {
    const physical = (e.ctrlKey ? "⌃" : "") + (e.altKey ? "⌥" : "") + (e.shiftKey ? "⇧" : "") + "⌘";
    return `${physical}${r.key} is reserved by macOS.`;
  }
  if (r.why === "stray") {
    const name = e.ctrlKey && map.alt !== "control" && map.ctrl !== "control" ? "Control" : "Option";
    return `${name} is neither Alt nor Ctrl now; choose the Alt and Ctrl keys above.`;
  }
  return null;
}

/** Splits a canonical combo into {fn, ctrl, alt, shift, cmd, key}. */
function splitCombo(combo) {
  const parts = combo.split("+");
  // "Ctrl++" does not occur (the key is "=" / "-" / NumAdd), but a trailing "+" would be the key.
  const key = parts.pop() || "+";
  const has = (m) => parts.includes(m);
  return { fn: has("Fn"), ctrl: has("Ctrl"), alt: has("Alt"), shift: has("Shift"), cmd: has("Cmd"), key };
}

/**
 * The physical keys of a TC combo under `map`, written like a combo with Ctrl = Control,
 * Alt = Option, Cmd = Command (what macOS's reserved shortcuts are listed in) and a leading
 * "Fn+" when Fn is held.
 */
export function physicalCombo(combo, map = DEFAULT_MODIFIERS) {
  const c = splitCombo(combo);
  const held = new Set();
  if (c.ctrl) held.add(map.ctrl);
  if (c.alt) held.add(map.alt);
  if (c.cmd) held.add("command");
  return (
    (held.has("fn") ? "Fn+" : "") +
    (held.has("control") ? "Ctrl+" : "") +
    (held.has("option") ? "Alt+" : "") +
    (c.shift ? "Shift+" : "") +
    (held.has("command") ? "Cmd+" : "") +
    c.key
  );
}

const ARROWS = { Up: "↑", Down: "↓", Left: "←", Right: "→" };

/** Label of a TC combo with the Mac keys in use, as macOS menus write it: "⌥F5", "⌃⇧F". */
export function comboLabel(combo, map = DEFAULT_MODIFIERS) {
  const p = splitCombo(physicalCombo(combo, map));
  return (p.fn ? "fn " : "") + (p.ctrl ? "⌃" : "") + (p.alt ? "⌥" : "") + (p.shift ? "⇧" : "") + (p.cmd ? "⌘" : "") + (ARROWS[p.key] ?? p.key);
}

/**
 * True when a key pressed in the command line belongs to the panels, as in TC:
 * F-keys, Ctrl/Alt combos and vertical cursor keys. Plain characters,
 * Backspace, Delete, Left/Right, Home/End and Space stay in the input.
 */
export function cmdlineKeyGoesToPanel(combo) {
  if (!combo) return false;
  // Alt+Left/Right (Option+arrows on a Mac) move the caret by word in the field.
  if (/^Alt\+(Shift\+)?(Left|Right)$/.test(combo)) return false;
  if (/^(Ctrl|Alt)\+/.test(combo)) return combo !== "Ctrl+Enter";
  const key = combo.replace(/^Shift\+/, "");
  if (/^F([1-9]|1[0-2])$/.test(key)) return true;
  return ["Up", "Down", "PageUp", "PageDown", "Tab"].includes(combo);
}

/** 1234567 -> "1,234,567". */
export function groupThousands(n) {
  return String(Math.trunc(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Bytes as TC kilobytes: floor(bytes / 1024) with commas, e.g. "288,594,048 k". */
export function formatK(bytes) {
  return `${groupThousands(Math.floor(bytes / 1024))} k`;
}

/** "[Macintosh HD]  288,594,048 k of 971,350,180 k free". */
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

/** "<DIR>" for folders, otherwise the byte size with commas. */
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

/** Which icon a file gets: "arc" | "med" | "" (colors come from the file-type rules). */
export function iconKind(entry) {
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
  return maskRegexes(mask).some((re) => re.test(n));
}

/** Splits TC masks (";" or spaces) into compiled regexes. */
function maskRegexes(mask) {
  return String(mask ?? "")
    .normalize("NFC")
    .toLowerCase()
    .split(/[;\s]+/)
    .filter(Boolean)
    .map(maskRegex);
}

/**
 * Compiles "Define colors by file type" rules once: returns `(name) => color | null`, the
 * color of the first rule with a matching mask (case- and NFC-insensitive).
 */
export function compileTypeRules(rules) {
  const compiled = (rules ?? []).map((r) => ({ res: maskRegexes(r.masks), color: r.color }));
  return (name) => {
    const n = name.normalize("NFC").toLowerCase();
    for (const r of compiled) if (r.res.some((re) => re.test(n))) return r.color;
    return null;
  };
}

/** An entry's own color: the first matching file-type rule (when on), else hidden, else null (font). */
export function entryColor(entry, colors, matcher) {
  if (entry.up) return null;
  if (colors.byType) {
    const c = matcher(entry.name);
    if (c) return c;
  }
  return entry.hidden ? colors.hidden : null;
}

/** The cursor in the inactive panel: only this frame. */
export const INACTIVE_CURSOR_OUTLINE = "1px dashed #8a8a8a";

/**
 * Inline style of a panel row, {color, background, outline}. Text: marked > cursorFont on the
 * cursor bar (unless empty) > entry color > font. Rows alternate background / background2; the
 * active panel's cursor is a bar (inverted cursor) or a frame in the cursor color, the
 * inactive panel's cursor only a dashed frame.
 */
export function rowStyle(entry, { marked, cursor, active, index }, colors, matcher) {
  const bar = cursor && active && colors.invertedCursor;
  let color;
  if (marked) color = colors.mark;
  else if (bar && colors.cursorFont) color = colors.cursorFont;
  else color = entryColor(entry, colors, matcher) ?? colors.font;
  const background = bar ? colors.cursor : index % 2 ? colors.background2 : colors.background;
  const outline = !cursor || bar ? "" : active ? `1px solid ${colors.cursor}` : INACTIVE_CURSOR_OUTLINE;
  return { color, background, outline };
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

/**
 * "/a/b" -> "/a"; "/" stays "/". On a server: "sftp://m/a" -> "sftp://m/", and the root -> null.
 * The search results list has no parent folder (its [..] goes back where the panel was).
 */
export function parentLocation(loc) {
  if (isSearchLocation(loc)) return null;
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
 * TC selection summary: "4,218 k / 288,594 k in 2 / 31 file(s)", plus
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

/**
 * Finder's name for a copy pasted into its own folder: "a copy.txt", then "a copy 2.txt"…;
 * a folder keeps any dots ("v1.2 copy"). `taken`: Set of the names already there.
 */
export function copyName(name, taken, isDir = false) {
  const at = isDir ? -1 : extensionStart(name);
  const base = at < 0 ? name : name.slice(0, at);
  const ext = at < 0 ? "" : name.slice(at);
  for (let n = 1; ; n++) {
    const candidate = `${base} copy${n > 1 ? ` ${n}` : ""}${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** Status text when a Cut is pasted into the folder it came from. */
export const ALREADY_HERE = "The items are already in this folder";

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

/** Selection in the conflict dialog's suggested new name: the name without its extension; whole name for folders. */
export function renameSelection(name, isDir = false) {
  const at = isDir ? -1 : extensionStart(name);
  return { start: 0, end: at < 0 ? name.length : at };
}

/** F5/F6 target: the other panel's folder with a trailing "/", plus the name of a single file. */
export function copyTargetDefault(otherLoc, sources) {
  const dir = otherLoc.endsWith("/") ? otherLoc : `${otherLoc}/`;
  return sources.length === 1 && !sources[0].isDir ? dir + fileName(sources[0]) : dir;
}

/** An entry's own file name: a search result's `name` is its shown path, its `path` the real one. */
function fileName(entry) {
  return entry.path ? lastComponent(entry.path) : entry.name;
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
      return pattern !== "" && matchMask(fileName(entry), pattern);
    });
}

/** Folder and full source paths of an operation, taken before any dialog is awaited. */
export function operationPaths(location, sources) {
  if (sources.length && sources.every((e) => e.path)) {
    // A results list: entries carry full paths; cwd is the folder of the first one.
    return { cwd: parentLocation(sources[0].path) ?? location, paths: sources.map((e) => e.path) };
  }
  return { cwd: location, paths: sources.map((e) => e.path ?? joinLocation(location, e.name)) };
}

/** Pseudo-location of the "Feed to listbox" search results panel (never matches parseRemote). */
export const SEARCH_LOCATION = "search:results";

export function isSearchLocation(loc) {
  return loc === SEARCH_LOCATION;
}

/** "/Users/x/Docs/a" -> "~/Docs/a"; only an exact `home` prefix followed by "/" or the end counts. */
export function homeDisplay(path, home) {
  if (!home || home === "/") return path;
  if (path === home) return "~";
  return path.startsWith(home + "/") ? "~" + path.slice(home.length) : path;
}

/** Search `Item`s -> panel entries. `name` is the home-shortened full path: unique per path, so marks stay separate. */
export function resultEntries(items, home) {
  return items.map((it) => ({
    name: homeDisplay(it.path, home),
    path: it.path,
    isDir: Boolean(it.isDir),
    size: it.size,
    mtime: it.mtime,
    hidden: false,
    inArchive: Boolean(it.inArchive),
    mounted: it.mounted !== false,
    mode: 0,
    isSymlink: false,
  }));
}

/**
 * Operations allowed on the selected result entries:
 * - any entry with `mounted === false` -> everything false;
 * - else any entry `inArchive` -> only `copy` (true), `view` (true only when exactly one entry)
 *   and `drag` (true when every entry is in an archive: they go out as file promises);
 * - otherwise everything true.
 */
export function resultOps(entries) {
  if (entries.some((e) => e.mounted === false)) {
    return { view: false, edit: false, copy: false, move: false, del: false, drag: false };
  }
  if (entries.some((e) => e.inArchive)) {
    return { view: entries.length === 1, edit: false, copy: true, move: false, del: false, drag: entries.every((e) => e.inArchive) };
  }
  return { view: true, edit: true, copy: true, move: true, del: true, drag: true };
}

/** Keeps the entries whose path is in `existsSet` (after F6/F8). */
export function dropMissing(entries, existsSet) {
  return entries.filter((e) => existsSet.has(e.path));
}

/** Ops refused in a results panel ("Feed to listbox"), and F5/F6 into a results panel. */
export const NOT_IN_ARCHIVES = "Not available for items inside archives";
export const NOT_MOUNTED = "Not available for items on a volume that is not mounted";
export const NOT_IN_RESULTS = "Not available in search results";
export const CHOOSE_FOLDER = "Choose a folder in the other panel.";

/**
 * Why `op` ("view" | "edit" | "copy" | "move" | "del" | "drag", see `resultOps`) is refused for
 * the selected result `entries`, or null when it is allowed (also for no entries).
 */
export function resultOpRefusal(entries, op) {
  if (!entries.length || resultOps(entries)[op]) return null;
  return entries.some((e) => e.mounted === false) ? NOT_MOUNTED : NOT_IN_ARCHIVES;
}

/** Enter / Ctrl+PgUp on a result: its folder (inside an archive: the inner folder) and its name. */
export function resultFolder(entry) {
  return { folder: parentLocation(entry.path), name: lastComponent(entry.path) };
}

/** Result paths a recheck asks `paths_exist` about: not those on an unmounted volume. */
export function pathsToCheck(entries) {
  return entries.filter((e) => e.mounted !== false).map((e) => e.path);
}

/** The entries left after a recheck: `existing` paths, plus those on an unmounted volume (never dropped). */
export function keepExisting(entries, existing) {
  const keep = new Set(existing);
  for (const e of entries) if (e.mounted === false) keep.add(e.path);
  return dropMissing(entries, keep);
}

/**
 * What the session saves for a panel ({location, cursorName}, or null for an empty panel):
 * - a results panel saves the folder it came from (`results.back`);
 * - a server archive's temp copy saves the server folder, cursor on the archive.
 */
export function sessionLocation(panel, results, archives = remoteArchives) {
  if (isSearchLocation(panel.location)) {
    const back = results?.back;
    if (!back?.location) return null;
    return sessionLocation({ location: back.location, cursorName: back.cursorName ?? null }, null, archives);
  }
  const r = remoteArchiveOf(panel.location ?? "", archives);
  if (r) return { location: r.parent, cursorName: lastComponent(r.archive) };
  return panel.location ? { location: panel.location, cursorName: panel.cursorName } : null;
}

/**
 * The folder panel `location` stands for: a results list stands for the folder it came from
 * (`results.back`, else `home`). Hotlist entries and the command line use it.
 */
export function panelFolder(location, results, home) {
  if (!isSearchLocation(location)) return location;
  return results?.back?.location || home;
}

/**
 * F5/F6 sources split into the groups one copy/move job can take (the backend wants all items
 * from one archive, or none from an archive): the local items as one group, then one group per
 * archive, in order of first appearance. By name, like `archiveOf`; an archive file itself is a
 * local item. Every group keeps `cwd`, so a relative target means the same folder for all.
 */
export function groupSources(paths, cwd) {
  const groups = new Map();
  for (const p of paths) {
    const plain = p.length > 1 ? p.replace(/\/+$/, "") : p;
    const archive = archiveOf(plain);
    const key = archive && archive !== plain ? archive : "";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(p);
  }
  return [...groups.values()].map((g) => ({ paths: g, cwd }));
}

/** Row of the search window: name, folder (home-shortened) and date. */
export function resultRow(item, home) {
  return {
    name: item.name,
    folder: homeDisplay(item.folder ?? "", home),
    size: item.size == null && !item.isDir ? "" : formatSize(item),
    date: formatDate(item.mtime),
  };
}

/** Columns of the Find files result list, in order, with their default widths (px). */
export const RESULT_COLUMNS = [
  { id: "name", label: "Name", width: 250 },
  { id: "folder", label: "Folder", width: 280 },
  { id: "size", label: "Size", width: 90 },
  { id: "date", label: "Date", width: 140 },
];

/** Narrowest a result column can be dragged (px). */
export const RESULT_COLUMN_MIN = 40;

/**
 * The result list's column layout from a saved one (junk tolerated): [{id, width, hidden}]
 * in RESULT_COLUMNS order. Name is never hidden.
 */
export function resultColumns(saved) {
  const byId = new Map((Array.isArray(saved) ? saved : []).filter((c) => c && typeof c.id === "string").map((c) => [c.id, c]));
  return RESULT_COLUMNS.map((d) => {
    const c = byId.get(d.id) ?? {};
    const w = Number.isFinite(c.width) ? Math.round(c.width) : d.width;
    return { id: d.id, width: Math.max(RESULT_COLUMN_MIN, w), hidden: d.id !== "name" && c.hidden === true };
  });
}

/**
 * CSS grid-template-columns of the visible columns at their widths, plus an empty track that
 * takes the spare width (so a column is never squeezed: a wide layout scrolls sideways).
 */
export function resultGridTemplate(layout) {
  return [...layout.filter((c) => !c.hidden).map((c) => `${c.width}px`), "1fr"].join(" ");
}

/**
 * A click on result row `i` (Finder/TC listbox rules): plain selects only it; `toggle`
 * (Cmd/Ctrl) adds or removes it; `range` (Shift) selects from the anchor to it. `marked` is a
 * Set of paths; returns {marked, anchor} (new Set).
 */
export function resultClick(rows, marked, anchor, i, { toggle = false, range = false } = {}) {
  const path = rows[i]?.path;
  if (path == null) return { marked: new Set(marked), anchor };
  if (range) {
    const a = anchor != null && anchor >= 0 && anchor < rows.length ? anchor : i;
    const [lo, hi] = a <= i ? [a, i] : [i, a];
    const next = toggle ? new Set(marked) : new Set();
    for (let k = lo; k <= hi; k++) next.add(rows[k].path);
    return { marked: next, anchor: a };
  }
  if (toggle) {
    const next = new Set(marked);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    return { marked: next, anchor: i };
  }
  return { marked: new Set([path]), anchor: i };
}

/** Feed to listbox: the selected rows (in list order) when more than one is selected, else all. */
export function feedItems(rows, marked) {
  if (marked.size < 2) return rows.slice();
  return rows.filter((r) => marked.has(r.path));
}

// ---------------------------------------------------------------- Find files window

/** Results shown by the Find files window (the backend's `limit`). */
export const SEARCH_LIMIT = 1000;


/**
 * Reducer of the Find files window's results: {rows, total, done, id, pending, fresh, early, floor}.
 * Events:
 * - `{type: "start"}`: `search_start` was called; its id is not known yet. Batches with an id
 *   above `floor` are kept in `early` (the search thread may emit before the call returns).
 * - `{type: "started", id}`: the id came back; the kept batches of that id are applied.
 * - `{type: "failed"}`: the call failed; the rows go and nothing is awaited any more.
 * - `{type: "clear"}`: empty query; rows go and late batches of the old search are ignored.
 * - `{type: "remove", path}`: drops one row (a result that no longer exists).
 * - a `search://batch` payload `{id, items, total, done}`: ignored unless `id` is current. The
 *   first batch of a search replaces the rows; later ones (a walk) append.
 */
export function searchWindowState(prev, event) {
  const s = prev ?? { rows: [], total: 0, done: true, id: null, pending: false, fresh: false, early: [], floor: 0 };
  switch (event.type) {
    case "start":
      return { ...s, pending: true, early: [] };
    case "started": {
      let next = { ...s, id: event.id, floor: Math.max(s.floor ?? 0, event.id), pending: false, fresh: true, done: false, early: [] };
      for (const b of s.early ?? []) if (b.id === event.id) next = searchWindowState(next, b);
      return next;
    }
    case "failed":
      return { ...s, rows: [], total: 0, id: null, pending: false, fresh: false, done: true, early: [] };
    case "clear":
      return { ...s, rows: [], total: 0, id: null, pending: false, fresh: false, done: true, early: [] };
    case "remove": {
      const rows = s.rows.filter((r) => r.path !== event.path);
      return rows.length === s.rows.length ? s : { ...s, rows, total: Math.max(0, s.total - 1) };
    }
    default: {
      if (s.pending) {
        return event.id > (s.floor ?? 0) ? { ...s, early: [...(s.early ?? []), event] } : s;
      }
      if (s.id == null || event.id !== s.id) return s;
      const rows = (s.fresh ? event.items : s.rows.concat(event.items)).slice(0, SEARCH_LIMIT);
      return { ...s, rows, total: event.total, done: event.done, fresh: false };
    }
  }
}

/**
 * The Find files status line: "N found" (plus "· showing first 1,000" when `total` > `shown`),
 * then the index state: "searching folder…" while `walking`, "building index…", "volume <name>
 * last refreshed <date>" (`stale`, see `staleVolume`), "index: N files, up to date"
 * (`entries`) or "index not built"; nothing when the index is off (the window shows that note).
 * `total` undefined: no search, no count.
 */
export function statusText({ total, shown, state, walking, entries, stale }) {
  const parts = [];
  if (total != null) {
    let found = `${groupThousands(total)} found`;
    if (shown != null && total > shown) found += ` · showing first ${groupThousands(shown)}`;
    parts.push(found);
  }
  if (walking) parts.push("searching folder…");
  else if (state === "building") parts.push("building index…");
  else if (state === "ready" && stale) parts.push(`volume ${stale.name} last refreshed ${stale.date}`);
  else if (state === "ready") parts.push(`index: ${groupThousands(entries ?? 0)} files, up to date`);
  else if (state === "notAsked") parts.push("index not built");
  return parts.join(" · ");
}

const SEARCH_OFF_NOTE = "Index is off (Options → Search)";

/** The first enabled, unmounted volume of a `search_status` answer as {name, date}, or null. */
export function staleVolume(status, tz) {
  const v = status?.volumes?.find((x) => x.enabled && !x.mounted);
  if (!v) return null;
  return { name: v.name, date: v.refreshed > 0 ? formatDate(v.refreshed, tz) : "never" };
}

/**
 * Which "In" choices the window offers for the index state, and the note it shows. Everywhere
 * needs a complete startup-disk index ("ready"); while it builds, Current folder walks.
 */
export function searchScopes(state) {
  const everywhere = state === "ready";
  return { everywhere, note: state === "off" ? SEARCH_OFF_NOTE : null };
}

/**
 * The index button at the bottom of the Find files window for an index state: "Build index"
 * before the index exists or while it is off, "Rebuild index" when it is ready, disabled while
 * it is being built.
 */
export function indexButton(state) {
  if (state === "ready") return { label: "Rebuild index", action: "rebuild", disabled: false };
  if (state === "building") return { label: "Building index…", action: null, disabled: true };
  return { label: "Build index", action: "build", disabled: false };
}

/** The folder "Current folder" searches for a panel location, or null (servers, the results list). */
export function searchUnder(location) {
  if (!location || isSearchLocation(location) || parseRemote(location)) return null;
  return location;
}

/** The `search_start` request, or null for an empty query (the window clears its rows then). */
export function searchRequest({ query, scope, location, archives }) {
  const q = query.trim();
  if (!q) return null;
  return { query: q, under: scope === "folder" ? location : null, archives: Boolean(archives), limit: SEARCH_LIMIT };
}

/** Where Enter on a result goes: its folder (inside an archive: the inner parent) and the cursor name. */
export function goTarget(item) {
  return { folder: item.folder, name: item.path.slice(item.path.lastIndexOf("/") + 1) };
}

/**
 * The `search_status` volume holding `path` (the longest prefix), or null. "/" holds everything
 * but /Volumes/<x> (the startup index excludes /Volumes; such a path is another volume's).
 */
export function volumeOf(path, volumes) {
  let best = null;
  const otherVolume = /^\/Volumes\/[^/]+/.test(path);
  for (const v of volumes ?? []) {
    const inside = v.path === "/" ? !otherVolume : path === v.path || path.startsWith(`${v.path}/`);
    if (inside && (!best || v.path.length > best.path.length)) best = v;
  }
  return best;
}

/** Name of the volume of `path` for messages: `vol.name`, else the /Volumes/<name> component. */
export function volumeName(path, vol) {
  if (vol?.name) return vol.name;
  const m = /^\/Volumes\/([^/]+)/.exec(path);
  return m ? m[1] : "/";
}

/**
 * {host, share} of an SMB `mntfrom` ("//[DOMAIN;][user@]host[:port]/share", the share
 * percent-encoded), lower case; null for anything else.
 */
export function parseMountSource(source) {
  const m = /^\/\/(?:[^@/]*@)?([^/:]+)(?::\d+)?\/([^/]+)\/?$/.exec(source ?? "");
  if (!m) return null;
  let share = m[2];
  try {
    share = decodeURIComponent(share);
  } catch {
    // Keep the raw text.
  }
  return { host: m[1].toLowerCase(), share: share.toLowerCase() };
}

/**
 * The saved SMB entry (its name, for `connectFlow`) that mounts the network volume `vol`, or
 * null. When the volume's `source` (mntfrom) is known, an entry with the same host and share
 * wins, so a share mounted under another name is still found. Otherwise macOS mounts a share
 * at /Volumes/<share>, or /Volumes/<share>-N when that name is taken, so the mount point's
 * last component is compared with each entry's `share`, ignoring case. The first match in the
 * connection manager's order wins.
 */
export function smbServerForVolume(vol, nodes) {
  if (!vol || vol.kind !== "network") return null;
  const find = (pred) => {
    let found = null;
    const walk = (list) => {
      for (const n of list ?? []) {
        if (found) return;
        if (n.kind === "folder") walk(n.children);
        else if (n.kind === "server" && n.protocol === "smb" && pred(n)) found = n.name;
      }
    };
    walk(nodes);
    return found;
  };
  const src = parseMountSource(vol.source);
  const shareOf = (n) => (n.share ?? "").trim().toLowerCase();
  if (src) {
    const byHost = find((n) => (n.host ?? "").trim().toLowerCase() === src.host && shareOf(n) === src.share);
    if (byHost) return byHost;
  }
  const base = vol.path.slice(vol.path.lastIndexOf("/") + 1).toLowerCase();
  for (const pass of [src?.share, base, base.replace(/-\d+$/, "")]) {
    if (!pass) continue;
    const found = find((n) => shareOf(n) === pass);
    if (found) return found;
  }
  return null;
}

/**
 * Whether the volume holding `path` is mounted now (`search_status` volumes). A path that no
 * listed volume holds (an ejected /Volumes/<x>) is not mounted.
 */
export function volumeMounted(path, volumes) {
  const v = volumeOf(path, volumes);
  return Boolean(v) && v.mounted !== false;
}

/**
 * What Enter does after listing the result's folder: "go" when it is there; otherwise "forget"
 * only when its volume is still mounted (the file is really gone), else "mount" (the volume
 * went away after the search; its index entries stay).
 */
export function missingAction({ listOk, found, volumeMounted }) {
  if (listOk && found) return "go";
  return volumeMounted ? "forget" : "mount";
}

/** True when a share mounted at `location` (`connectFlow`'s answer) is the volume `volPath`. */
export function mountedAt(location, volPath) {
  return location === volPath || location.startsWith(`${volPath}/`);
}

/** The selected row after Up/Down/PgUp/PgDn (`page` rows per page); -1 when there are no rows. */
export function moveIndex(sel, key, count, page) {
  if (count <= 0) return -1;
  const step = { ArrowUp: -1, ArrowDown: 1, PageUp: -Math.max(1, page), PageDown: Math.max(1, page) }[key] ?? 0;
  return Math.min(count - 1, Math.max(0, sel + step));
}

/** Activation/dismissal keys ignored for this long after a modal opens (stray Enter from the previous one). */
export const MODAL_KEY_DELAY = 150;

/** A modal opened this long after an activation key counts as opened by it. */
export const OPENED_BY_KEY_MS = 1000;

/**
 * True when the last keydown (`last`: {key, at} or null) was Enter, Escape or Space shortly
 * before a modal opened at `openedAt`: only then may that key still be a stray one.
 */
export function openedByActivationKey(last, openedAt) {
  return Boolean(last) && ["Enter", "Escape", " "].includes(last.key) && openedAt - last.at < OPENED_BY_KEY_MS;
}

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

/** Viewer status line, e.g. `UTF-8 · 12,345 bytes (truncated)`. */
export function formatViewStatus(encoding, bytes, truncated) {
  return `${encoding} · ${groupThousands(bytes)} bytes${truncated ? " (truncated)" : ""}`;
}

// ---------------------------------------------------------------- Find files window bridge

/**
 * The Find files window (search.html, label `findfiles`) and the main window talk over three
 * broadcast events:
 * - `findfiles://action` (window → main) `{id, action, args}`: the main window runs `action`
 *   (one of FIND_FILES_ACTIONS) and answers with
 * - `findfiles://reply` (main → window) `{id, ok, value}` or `{id, ok: false, error}`;
 * - `findfiles://command` (main → window) `{cmd, ...}`: `focus {location}` (Alt+F7 again),
 *   `feedAll` (Commands → Feed search results to listbox), `settings {archives}`.
 * Actions: `context` → `{home, location, mods, archives, memory, state}`; `go {folder, cursor}`;
 * `feed {items, query}`; `play {path}` (F3 on audio: the main window's player); `connect
 * {server}` → the mounted location or null; `indexOn` (the index was turned on); `closed
 * {memory, items, query}`. Commands without a main window (search_*, list_dir, open_viewer)
 * are invoked by the window itself.
 */
export const FIND_FILES_ACTION = "findfiles://action";
export const FIND_FILES_REPLY = "findfiles://reply";
export const FIND_FILES_COMMAND = "findfiles://command";
export const FIND_FILES_ACTIONS = ["context", "go", "feed", "play", "connect", "indexOn", "closed"];
export const FIND_FILES_COMMANDS = ["focus", "feedAll", "settings"];

const isPlainObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const validId = (id) => typeof id === "string" && id.length > 0 && id.length <= 64;

/** A `findfiles://action` payload; throws on an unknown action or a bad id. */
export function findFilesAction(id, action, args = {}) {
  if (!validId(id)) throw new Error(`bad request id: ${id}`);
  if (!FIND_FILES_ACTIONS.includes(action)) throw new Error(`unknown Find files action: ${action}`);
  return { id, action, args: isPlainObject(args) ? args : {} };
}

/** A received `findfiles://action` payload as `{id, action, args}`, or null when malformed. */
export function parseFindFilesAction(p) {
  if (!isPlainObject(p) || !validId(p.id) || !FIND_FILES_ACTIONS.includes(p.action)) return null;
  return { id: p.id, action: p.action, args: isPlainObject(p.args) ? p.args : {} };
}

/** The `findfiles://reply` to request `id`: `value` on success, else the error's text. */
export function findFilesReply(id, value, error) {
  if (error !== undefined) return { id, ok: false, error: String(error?.message ?? error) };
  return { id, ok: true, value: value ?? null };
}

/** A received `findfiles://reply` as `{id, ok, value, error}`, or null when malformed. */
export function parseFindFilesReply(p) {
  if (!isPlainObject(p) || !validId(p.id) || typeof p.ok !== "boolean") return null;
  return p.ok ? { id: p.id, ok: true, value: p.value ?? null } : { id: p.id, ok: false, error: String(p.error ?? "failed") };
}

/** A received `findfiles://command` as `{cmd, ...}`, or null when malformed. */
export function parseFindFilesCommand(p) {
  if (!isPlainObject(p) || !FIND_FILES_COMMANDS.includes(p.cmd)) return null;
  return p;
}

/** Smallest Find files window (logical pixels); `open_find_files` uses the same. */
export const FIND_FILES_MIN = { width: 520, height: 380 };

/**
 * The kept Find files window geometry (`{x, y, width, height, maximized}` in logical pixels,
 * from browser storage) for `open_find_files`: null without a usable size; the position only
 * when both coordinates are numbers (Rust drops it when it is off every screen).
 */
export function findFilesGeometry(saved) {
  if (!isPlainObject(saved)) return null;
  const num = (v) => typeof v === "number" && Number.isFinite(v);
  const { width, height } = saved;
  if (!num(width) || !num(height) || width < FIND_FILES_MIN.width || height < FIND_FILES_MIN.height) return null;
  const g = { width: Math.round(width), height: Math.round(height), maximized: saved.maximized === true };
  if (num(saved.x) && num(saved.y)) Object.assign(g, { x: Math.round(saved.x), y: Math.round(saved.y) });
  return g;
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

/**
 * A dialog moved or resized inside the main window: `r` {x, y, w, h} kept inside a `vw` x `vh`
 * window and no smaller than `min` {w, h} (unless the window itself is smaller).
 */
export function fitWindow(r, vw, vh, min) {
  const w = Math.round(Math.min(vw, Math.max(min.w, r.w)));
  const h = Math.round(Math.min(vh, Math.max(min.h, r.h)));
  return {
    x: Math.round(Math.min(Math.max(0, r.x), vw - w)),
    y: Math.round(Math.min(Math.max(0, r.y), vh - h)),
    w,
    h,
  };
}

/** Connect to server: groups hold servers and groups, at most this many levels deep. */
export const GROUP_LEVELS = 3;

/** How many levels of groups `nodes` has (0: no group). */
export function groupLevels(nodes) {
  let max = 0;
  for (const n of nodes) if (isContainer(n)) max = Math.max(max, 1 + groupLevels(n.children));
  return max;
}

/**
 * A change from `before` to `after` that makes the groups deeper than GROUP_LEVELS (a list that
 * was already deeper may still be edited, as long as it does not get deeper).
 */
export function groupsTooDeep(before, after) {
  const n = groupLevels(after);
  return n > GROUP_LEVELS && n > groupLevels(before);
}

/** Alt+Left (Connect to server): the node at `path` leaves its group and follows it. New path, or null at the top. */
export function outdentNode(nodes, path) {
  if (path.length < 2) return null;
  const list = parentList(nodes, path);
  const [node] = list.splice(path[path.length - 1], 1);
  const groupPath = path.slice(0, -1);
  const outer = parentList(nodes, groupPath);
  const at = groupPath[groupPath.length - 1] + 1;
  outer.splice(at, 0, node);
  return [...groupPath.slice(0, -1), at];
}

/** Alt+Right (Connect to server): the node at `path` goes to the end of the group right above it. New path, or null. */
export function indentNode(nodes, path) {
  const i = path[path.length - 1];
  const list = parentList(nodes, path);
  const group = list[i - 1];
  if (!isContainer(group)) return null;
  const [node] = list.splice(i, 1);
  group.children.push(node);
  return [...path.slice(0, -1), i - 1, group.children.length - 1];
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

/** What macOS does with each reserved combo (keymap SYSTEM_RESERVED, physical keys: Ctrl = Control, Alt = Option). */
const SYSTEM_USES = {
  "Ctrl+Up": "Mission Control",
  "Ctrl+Down": "Application windows",
  "Ctrl+Left": "Move left a space",
  "Ctrl+Right": "Move right a space",
  "Ctrl+Space": "Select the previous input source",
  "Ctrl+Alt+Space": "Select the next input source",
};

/** Warning for a binding (TC combo) macOS takes before fxfm sees it, with the keys in use under `map`. */
export function conflictMessage(combo, map = DEFAULT_MODIFIERS) {
  const use = SYSTEM_USES[physicalCombo(combo, map)] ?? "a system shortcut";
  return `macOS uses ${comboLabel(combo, map)} for ${use}. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key.`;
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
  if (n.endsWith(".tar.bz2") || n.endsWith(".tbz2") || n.endsWith(".tbz")) return "tbz2";
  if (n.endsWith(".tar.xz") || n.endsWith(".txz")) return "txz";
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

/**
 * What the panel shows as path: inside a server archive's copy, the server location. The
 * results list shows "Search results: <query>"; its `{query}` is the third argument (or the
 * second, in place of `remoteArchives`).
 */
export function displayLocation(location, remoteArchives, results) {
  if (isSearchLocation(location)) {
    const r = results ?? remoteArchives;
    if (r?.title) return r.title;
    const query = r?.query ?? "";
    return query ? `Search results: ${query}` : "Search results";
  }
  const r = remoteArchiveOf(location, remoteArchives);
  if (!r) return location;
  const rest = location.slice(r.archive.length).replace(/\/+$/, "");
  return joinLocation(r.parent, lastComponent(r.archive)) + rest;
}

/**
 * The reverse of `displayLocation` for a server archive's copy: a location shown as
 * `<server folder>/<archive>[/inner]` becomes the local copy's path; anything else stays.
 */
export function realLocation(location, remoteArchives) {
  const loc = location.length > 1 ? location.replace(/\/+$/, "") : location;
  for (const [archive, parent] of remoteArchives) {
    const shown = joinLocation(parent, lastComponent(archive));
    if (loc === shown) return archive;
    if (loc.startsWith(`${shown}/`)) return archive + loc.slice(shown.length);
  }
  return location;
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
  return player.state === "loading" || player.state === "playing" || player.state === "paused";
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
  if (state !== "playing" && state !== "paused") return null;
  const total = duration ?? null;
  return {
    paused: state === "paused",
    text: name,
    clock: `${formatClock(position)} / ${total == null ? "-:--" : formatClock(total)}`,
    // A note from the player (the chosen output device is missing) takes the codec's place.
    codec: player.message || codecSummary(player.info),
    progress: total ? Math.min(1, Math.max(0, position / total)) : null,
  };
}

/**
 * Options → Audio → Output device: `System default` (value "") first, then each device in the
 * system's order. `devices`: `audio_outputs` items ({name, id}) or plain names. A device with a
 * unique name is saved by name; devices that share a name are saved by id and labeled
 * `<name> (1)`, `<name> (2)`… A saved device that is not connected stays as
 * `<name> (not connected)`.
 */
export function outputOptions(devices, saved) {
  const list = (devices ?? []).map((d) => (typeof d === "string" ? { name: d, id: null } : d)).filter((d) => d.name);
  const count = new Map();
  for (const d of list) count.set(d.name, (count.get(d.name) ?? 0) + 1);
  const out = [{ value: "", label: "System default" }];
  const seen = new Set([""]);
  const nth = new Map();
  for (const d of list) {
    const shared = count.get(d.name) > 1 && d.id;
    const value = shared ? d.id : d.name;
    if (seen.has(value)) continue;
    seen.add(value);
    const k = (nth.get(d.name) ?? 0) + 1;
    nth.set(d.name, k);
    out.push({ value, label: shared ? `${d.name} (${k})` : d.name });
  }
  if (saved && !seen.has(saved)) {
    // A name saved before its twin connected: the first device of that name.
    const twin = list.find((d) => d.name === saved);
    if (twin) out.push({ value: saved, label: `${saved} (first)` });
    else out.push({ value: saved, label: `${saved} (not connected)` });
  }
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

/**
 * The paths of fxfm's own native drag `drag` ({paths, promised}, or null) when a drop that
 * carries `dropPaths` is that drag, else null. File promises (archive and server entries)
 * carry no paths, so a drop without paths is the promised drag's.
 */
export function ownDropPaths(drag, dropPaths) {
  if (!drag) return null;
  if (drag.promised) return dropPaths.length === 0 ? drag.paths : null;
  return samePaths(dropPaths, drag.paths) ? drag.paths : null;
}

// ---------------------------------------------------------------- servers (mirrors crates/core servers.rs)

const SSH_PROTOCOLS = new Set(["sftp", "scp"]);
const DEFAULT_PORT = { sftp: 22, scp: 22, ftp: 21, ftps: 21, smb: 445 };

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
    share: "",
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
  if (entry.protocol === "smb") {
    if (!entry.share?.trim()) return "Share is required";
    if (entry.share.includes("/")) return "Share cannot contain '/'";
    return null;
  }
  if ((entry.auth === "key" || entry.auth === "keyPassphrase") && !entry.keyPath?.trim()) {
    return "Choose a key file";
  }
  return null;
}

/** The editor fields shown for `protocol`. */
export function fieldsFor(protocol) {
  // SMB: macOS mounts the share; the remote dir is a sub-folder inside it.
  if (protocol === "smb") {
    return new Set(["name", "protocol", "host", "port", "user", "share", "kerberos", "savePassword", "remoteDir", "autoConnect"]);
  }
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

/**
 * Browse… button label of a computer found on the network (`smb_browse`): its name, and the
 * host name or (NetBIOS) address when that says more.
 */
export function smbHostLabel(h) {
  const extra = h.address ?? (h.host !== h.name ? h.host : null);
  return extra ? `${h.name} (${extra})` : h.name;
}

/** Drive-list path of a saved SMB share that is not mounted (never a real folder). */
export function smbDrivePath(name) {
  return `smb://${name}/`;
}

/**
 * The drive list: `volumes` (`list_volumes`) followed by every saved SMB share (`smb_shares`,
 * in the connection manager's order) as a network drive {name, path, kind, total, free, smb,
 * mounted}. A mounted share takes its mount point and space, and the plain row of that mount
 * point leaves the volume list (shown once, as the share). An unmounted share has a
 * `smbDrivePath` and no space.
 */
export function mergeShares(volumes, shares) {
  const mounted = new Set(shares.filter((s) => s.mount).map((s) => s.mount));
  return [
    ...volumes.filter((v) => !mounted.has(v.path)),
    ...shares.map((s) => ({
      name: s.name,
      path: s.mount ?? smbDrivePath(s.name),
      kind: "network",
      total: s.mount ? s.total : 0,
      free: s.mount ? s.free : 0,
      smb: s.name,
      mounted: !!s.mount,
    })),
  ];
}

/**
 * Saved servers other than SMB shares (SFTP, SCP, FTP, FTPS) from `servers_load` nodes, in the
 * connection manager's order, as drive-menu rows. `conns` (`list_connections`) marks the
 * connected ones; `path` is the server root (`sftp://name/`).
 */
export function serverDrives(nodes, conns = []) {
  const out = [];
  const walk = (list) => {
    for (const n of list ?? []) {
      if (n.kind === "folder") walk(n.children);
      else if (n.kind === "server" && n.protocol !== "smb") {
        const c = conns.find((x) => x.name === n.name);
        out.push({ name: n.name, kind: "server", server: n.name, connected: !!c, path: c?.root ?? `${n.protocol}://${n.name}/` });
      }
    }
  };
  walk(nodes);
  return out;
}

/**
 * Where picking the saved server `drive` (`serverDrives`) goes, like `driveTarget`: the other
 * panel's folder when it is on that server, the root when this panel already is, else the last
 * folder visited there (`last`: root -> location), else the root. Null when not connected yet
 * (connecting opens the server's start folder).
 */
export function serverTarget(drive, { here, other, last }) {
  if (!drive.connected) return null;
  if (other !== here && remoteRoot(other) === drive.path) return other;
  if (remoteRoot(here) === drive.path) return drive.path;
  return last?.get(drive.path) ?? drive.path;
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

/** Backend refusal of `chmod` inside archives; same as `CHMOD_NOT_IN_ARCHIVE` in src-tauri/src/remote.rs. */
export const CHMOD_NOT_IN_ARCHIVE = "Change attributes does not work inside archives";

/** Lines of the chmod result box ([{name, error|null}]), or null when every entry changed. */
export function chmodSummary(results) {
  const failed = results.filter((r) => r.error != null);
  if (!failed.length) return null;
  return [`Changed ${results.length - failed.length} of ${results.length}`, ...failed.map((r) => `${r.name}: ${r.error}`)];
}

/**
 * F8 / Shift+F8 question: on a server always permanent, else the archive / Trash / permanent
 * text. `onNetwork`: how many of the items are on a network share, which has no Trash (F8
 * deletes those permanently).
 */
export function deleteConfirmText(location, n, { inArchive = false, toTrash = false, name = null, onNetwork = 0 } = {}) {
  // One item is named (as in TC); several are counted.
  const one = n === 1 && name ? `"${name}"` : null;
  if (parseRemote(location)) return `Delete ${one ?? `${n} file(s)`} from the server? This cannot be undone.`;
  if (inArchive) return `Delete ${one ?? `${n} file(s)`} from archive? This cannot be undone.`;
  const what = one ?? `${n} file(s)/dir(s)`;
  if (toTrash && onNetwork >= n) {
    return `Do you really want to delete ${what} permanently? ${n === 1 ? "It is" : "They are"} on a network share, which has no Trash. This cannot be undone.`;
  }
  if (toTrash && onNetwork > 0) {
    return `Do you really want to move ${what} to the Trash? ${onNetwork} of them ${onNetwork === 1 ? "is" : "are"} on a network share, which has no Trash: deleted permanently.`;
  }
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

// ---------------------------------------------------------------- pack files (Alt+F5)


/** A plain local folder: not on a server, not inside an archive (by name, like `archiveOf`), not search results. */
export function isLocalFolder(location) {
  return !parseRemote(location) && !archiveOf(location) && !isSearchLocation(location);
}

/**
 * Folder icons of macOS (`CoreTypes.bundle`, read from the Mac at run time) offered in
 * Options → View → Folder icon: [icon name, label]. The Sidebar ones are one-color glyphs drawn
 * in the panel's font color.
 */
export const FOLDER_ICONS = [
  ["GenericFolderIcon", "Folder (Finder)"],
  ["OpenFolderIcon", "Open folder"],
  ["SmartFolderIcon", "Smart folder"],
  ["BurnableFolderIcon", "Burn folder"],
  ["GroupFolder", "Group folder"],
  ["SystemFolderIcon", "System folder"],
  ["LibraryFolderIcon", "Library folder"],
  ["DeveloperFolderIcon", "Developer folder"],
  ["UsersFolderIcon", "Users folder"],
  ["PublicFolderIcon", "Public folder"],
  ["UtilitiesFolder", "Utilities folder"],
  ["SidebarGenericFolder", "Sidebar folder (one color)"],
  ["SidebarSmartFolder", "Sidebar smart folder (one color)"],
];

/** Finder's own icons of the special folders: [path below home (or absolute), full icon, sidebar glyph]. */
const SPECIAL_FOLDERS = [
  ["Desktop", "DesktopFolderIcon", "SidebarDesktopFolder"],
  ["Documents", "DocumentsFolderIcon", "SidebarDocumentsFolder"],
  ["Downloads", "DownloadsFolder", "SidebarDownloadsFolder"],
  ["Movies", "MovieFolderIcon", "SidebarMoviesFolder"],
  ["Music", "MusicFolderIcon", "SidebarMusicFolder"],
  ["Pictures", "PicturesFolderIcon", "SidebarPicturesFolder"],
  ["Public", "PublicFolderIcon", "SidebarGenericFolder"],
  ["Library", "LibraryFolderIcon", "SidebarGenericFolder"],
  ["Applications", "ApplicationsFolderIcon", "SidebarApplicationsFolder"],
  ["Sites", "SitesFolderIcon", "SidebarGenericFolder"],
  ["/Applications", "ApplicationsFolderIcon", "SidebarApplicationsFolder"],
  ["/Applications/Utilities", "UtilitiesFolder", "SidebarUtilitiesFolder"],
  ["/Library", "LibraryFolderIcon", "SidebarGenericFolder"],
  ["/System", "SystemFolderIcon", "SidebarGenericFolder"],
  ["/Users", "UsersFolderIcon", "SidebarGenericFolder"],
];

/** The system icon of special folder `path` (home's Desktop, /Applications, the home itself…), or null. */
export function specialFolderIcon(path, home, sidebar = false) {
  if (home && path === home) return sidebar ? "SidebarHomeFolder" : "HomeFolderIcon";
  for (const [p, full, glyph] of SPECIAL_FOLDERS) {
    const at = p.startsWith("/") ? p : home ? `${home}/${p}` : null;
    if (at === path) return sidebar ? glyph : full;
  }
  return null;
}

/** Row icon styles for Options → View: macOS's own icons (default) or emoji. */
export const ICON_STYLES = [
  ["finder", "Finder (macOS icons)"],
  ["classic", "Classic (yellow folders)"],
  ["outline", "Outline (line icons)"],
  ["emoji", "Emoji"],
];

/** The icon style of `settings`; unknown values give "finder". */
export function iconStyle(settings) {
  return ICON_STYLES.some(([id]) => id === settings?.icons) ? settings.icons : "finder";
}

/**
 * The `file_icons` key for a row in the folder `location`, or null for "..": an application
 * (or other bundle) in a local folder uses its own icon, other folders the folder icon, files
 * the icon of their extension.
 */
export function iconKey(entry, location, { home = "", folderIcon = "" } = {}) {
  if (entry.up) return null;
  if (entry.isDir) {
    const bundle = /\.(app|prefPane|bundle|framework)$/i.test(entry.name) && isLocalFolder(location);
    if (bundle) return `path:${joinLocation(location, entry.name)}`;
    const sidebar = folderIcon.startsWith("Sidebar");
    const special = isLocalFolder(location) && specialFolderIcon(joinLocation(location, entry.name), home, sidebar);
    if (special) return `sys:${special}`;
    return FOLDER_ICONS.some(([id]) => id === folderIcon) && folderIcon !== "GenericFolderIcon" ? `sys:${folderIcon}` : "dir";
  }
  const dot = entry.name.lastIndexOf(".");
  return dot > 0 && dot < entry.name.length - 1 ? `ext:${entry.name.slice(dot + 1).toLowerCase()}` : "file";
}

/**
 * Default archive name: one file -> its name without the last extension, one folder -> its
 * name (a leading dot is kept: ".config" -> ".config.zip"); several entries -> the name of
 * the folder they are in, "archive" at the root.
 */
export function packArchiveName(entries, folderPath, ext = ".zip") {
  if (entries.length === 1) {
    const { name, isDir } = entries[0];
    const dot = name.lastIndexOf(".");
    // A .gz keeps the whole file name (report.pdf.gz), as gzip does.
    if (ext === ".gz" && !isDir) return `${name}.gz`;
    return `${!isDir && dot > 0 ? name.slice(0, dot) : name}${ext}`;
  }
  return `${lastComponent(folderPath) || "archive"}${ext}`;
}

/** Pack formats: [id, label, extension]. Only ZIP can be encrypted; GZ holds one file. */
export const PACK_FORMATS = [
  ["zip", "ZIP", ".zip"],
  ["tar", "TAR", ".tar"],
  ["tgz", "TAR.GZ", ".tar.gz"],
  ["tbz2", "TAR.BZ2", ".tar.bz2"],
  ["txz", "TAR.XZ", ".tar.xz"],
  ["gz", "GZ (one file)", ".gz"],
  ["7z", "7Z", ".7z"],
];

const PACK_SUFFIXES = [
  [".tar.gz", "tgz"],
  [".tgz", "tgz"],
  [".tar.bz2", "tbz2"],
  [".tbz2", "tbz2"],
  [".tbz", "tbz2"],
  [".tar.xz", "txz"],
  [".txz", "txz"],
  [".tar", "tar"],
  [".zip", "zip"],
  [".gz", "gz"],
  [".7z", "7z"],
];

/** The pack format of an archive name by its suffix (case-insensitive), or null. */
export function packFormatOf(name) {
  const n = name.toLowerCase();
  return PACK_SUFFIXES.find(([sfx]) => n.endsWith(sfx))?.[1] ?? null;
}

/** `target` with its archive suffix replaced by the one of `format` (appended when it has none). */
export function withPackFormat(target, format) {
  const ext = PACK_FORMATS.find(([id]) => id === format)?.[2] ?? ".zip";
  const n = target.toLowerCase();
  const sfx = PACK_SUFFIXES.find(([x]) => n.endsWith(x))?.[0];
  return (sfx ? target.slice(0, target.length - sfx.length) : target) + ext;
}

/**
 * Folder the archive goes to (as in TC): the other panel's folder (also inside an archive or
 * on a server), unless it shows a results list; then the active panel's, and with results in
 * both, the folder of the first source (`fallback`).
 */
export function packTargetDir(activeLocation, otherLocation, fallback = "/") {
  if (!isSearchLocation(otherLocation)) return otherLocation;
  return isSearchLocation(activeLocation) ? fallback : activeLocation;
}

/** The Pack files dialog's first line. */
export function packDialogText(entries) {
  return entries.length === 1
    ? `Pack "${entries[0].name}" to the archive:`
    : `Pack ${entries.length} file(s) to the archive:`;
}

/** Text cursor of the target field: before ".zip" (at the end without one). */
export function packCursor(target) {
  const at = target.toLowerCase().endsWith(".zip") ? target.length - 4 : target.length;
  return { start: at, end: at };
}

/** Why the Pack files dialog cannot be accepted yet, or null. */
export function packFormError({ target, encrypt, password, repeat, sources = null }) {
  if (!target.trim()) return "Enter the archive name.";
  const format = packFormatOf(target.trim());
  if (!format) return "The name must end in .zip, .tar, .tar.gz, .tgz, .tar.bz2, .tar.xz, .gz or .7z.";
  if (encrypt && format !== "zip") return "Only ZIP archives can have a password.";
  if (format === "gz" && sources && (sources.length !== 1 || sources[0].isDir)) {
    return "A .gz file holds exactly one file; choose another format.";
  }
  if (encrypt && !password) return "Enter a password.";
  if (encrypt && password !== repeat) return "The passwords do not match.";
  return null;
}

/** Status line after packing. */
export function packedText(summary, target, add = false) {
  return add ? `Added ${summary.done} item(s) to ${lastComponent(target)}` : `Packed ${summary.done} file(s) into ${lastComponent(target)}`;
}

const TEXT_EDIT_CODES = new Set(["KeyA", "KeyC", "KeyV", "KeyX", "KeyZ"]);

/**
 * Command+A/C/V/X/Z in a text field while Command acts as Alt or Ctrl: left to the field
 * (select all, copy, paste, cut, undo) instead of becoming a TC command.
 */
export function textEditKey(e, map = DEFAULT_MODIFIERS) {
  const commandMapped = map.alt === "command" || map.ctrl === "command";
  return Boolean(e.metaKey) && commandMapped && TEXT_EDIT_CODES.has(e.code);
}

/**
 * The disk a local location is on: the longest volume path containing it, ignoring the home
 * shortcut (`kind: "home"`), so `~/Desktop` is on "Macintosh HD". Null on a server and for the search results list.
 */
export function diskFor(volumes, location) {
  if (!location || parseRemote(location) || isSearchLocation(location)) return null;
  let best = null;
  for (const v of volumes) {
    if (v.kind === "home") continue;
    const p = v.path;
    const inside = p === "/" || location === p || location.startsWith(`${p}/`);
    if (inside && (!best || p.length > best.path.length)) best = v;
  }
  return best;
}

/**
 * Where picking drive `vol` takes a panel (TC: each drive keeps a current folder):
 * - the home shortcut always goes home;
 * - the other panel's folder when it is on that drive (and not where this panel already is);
 * - else the drive's root when this panel is already on it;
 * - else the last folder visited on it (`last`: disk path -> location), else its root.
 */
export function driveTarget(vol, { volumes, here, other, last }) {
  if (vol.kind === "home") return vol.path;
  if (other !== here && diskFor(volumes, other)?.path === vol.path) return other;
  if (diskFor(volumes, here)?.path === vol.path) return vol.path;
  return last?.get(vol.path) ?? vol.path;
}

const SEARCH_DEFAULTS = { index: null, volumes: [], exclude: [], archives: false, gzLimitMb: 500, networkArchives: false, networkRefreshHours: 24 };

/** A whole number from an input's text (at least `min`), else null. */
function wholeNumber(text, min) {
  const t = String(text ?? "").trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number.parseInt(t, 10);
  return n >= min ? n : null;
}

/**
 * The `search` part of the settings after one Options → Search change (the input is not modified).
 * change: {index}, {volume: {path, enabled}}, {addExclude}, {removeExclude}, {archives}, {networkArchives},
 * {gzLimitMb: text (integer >= 1)}, {networkRefreshHours: text (integer >= 0, 0 = by hand)}.
 * Invalid number text keeps the previous value.
 */
export function searchSettingsPatch(settings, change) {
  const s = { ...SEARCH_DEFAULTS, ...(settings?.search ?? {}) };
  s.volumes = s.volumes.map((v) => ({ ...v }));
  s.exclude = [...s.exclude];
  if ("index" in change) s.index = change.index;
  if (change.volume) {
    const v = s.volumes.find((x) => x.path === change.volume.path);
    if (v) v.enabled = change.volume.enabled;
    else s.volumes.push({ path: change.volume.path, enabled: change.volume.enabled });
  }
  if (change.addExclude && !s.exclude.includes(change.addExclude)) s.exclude.push(change.addExclude);
  if (change.removeExclude) s.exclude = s.exclude.filter((x) => x !== change.removeExclude);
  if ("archives" in change) s.archives = Boolean(change.archives);
  if ("networkArchives" in change) s.networkArchives = Boolean(change.networkArchives);
  if ("gzLimitMb" in change) s.gzLimitMb = wholeNumber(change.gzLimitMb, 1) ?? s.gzLimitMb;
  if ("networkRefreshHours" in change) s.networkRefreshHours = wholeNumber(change.networkRefreshHours, 0) ?? s.networkRefreshHours;
  return s;
}

// ---------------------------------------------------------------- quick search

/**
 * Quick search (TC Alt+letters): which keys type into it. Settings `quickSearch`;
 * "command" is the default because Command sits where TC's Alt is on a PC keyboard.
 */
export const QUICK_SEARCH_KEYS = [
  ["command", "Command+letters"],
  ["controlCommand", "Control+Command+letters"],
  ["option", "Option+letters"],
  ["letters", "Letters alone"],
  ["off", "Off"],
];

/** The quick search key setting of `settings`; unknown values give "command". */
export function quickSearchMode(settings) {
  const v = settings?.quickSearch;
  return QUICK_SEARCH_KEYS.some(([id]) => id === v) ? v : "command";
}

/** Command shortcuts macOS keeps (Quit, Hide) never type into the quick search. */
const QUICK_SEARCH_RESERVED = new Set(["KeyQ", "KeyH"]);

const QUICK_SEARCH_CODES = { Period: ".", Minus: "-", Space: " " };

/**
 * The character a key event types into the quick search under `mode`, or null when the
 * event is not a quick search key. Letters and digits come from the physical key (Option
 * changes what `key` reports); other layout characters (č, ž …) from `key`.
 */
export function quickSearchChar(e, mode) {
  const held = { control: Boolean(e.ctrlKey), option: Boolean(e.altKey), command: Boolean(e.metaKey) };
  const want = {
    command: { control: false, option: false, command: true },
    controlCommand: { control: true, option: false, command: true },
    option: { control: false, option: true, command: false },
    letters: { control: false, option: false, command: false },
  }[mode];
  if (!want) return null;
  if (held.control !== want.control || held.option !== want.option || held.command !== want.command) return null;
  if (held.command && QUICK_SEARCH_RESERVED.has(e.code)) return null;
  let m;
  if ((m = /^Key([A-Z])$/.exec(e.code ?? ""))) {
    // Non-Latin layouts and Option report other characters; Latin layouts keep theirs (z/y).
    const k = e.key ?? "";
    return !held.option && k.length === 1 && /\p{L}/u.test(k) ? k.toLowerCase() : m[1].toLowerCase();
  }
  if ((m = /^Digit([0-9])$/.exec(e.code ?? ""))) return e.shiftKey ? null : m[1];
  if (e.code === "Minus" && e.shiftKey && !held.option) return "_";
  if (Object.hasOwn(QUICK_SEARCH_CODES, e.code) && !e.shiftKey) {
    // Space alone stays the mark key.
    if (e.code === "Space" && mode === "letters") return null;
    return QUICK_SEARCH_CODES[e.code];
  }
  const k = e.key ?? "";
  if (!held.option && k.length === 1 && /\p{L}/u.test(k)) return k.toLowerCase();
  return null;
}

const qsFold = (s) => s.normalize("NFC").toLowerCase();

/**
 * Index of an entry whose name starts with `text` (case-insensitive), or -1. `dir` 0: the
 * first from the top; 1 / -1: the next / previous after `from`, wrapping. The ".." entry
 * never matches.
 */
export function quickSearchFind(entries, text, from = 0, dir = 0) {
  const t = qsFold(text);
  const n = entries.length;
  const hit = (i) => !entries[i].up && qsFold(entries[i].name).startsWith(t);
  if (!n || !t) return -1;
  if (dir === 0) {
    for (let i = 0; i < n; i++) if (hit(i)) return i;
    return -1;
  }
  for (let s = 1; s <= n; s++) {
    const i = (((from + dir * s) % n) + n) % n;
    if (hit(i)) return i;
  }
  return -1;
}

// ---------------------------------------------------------------- multi-rename (Ctrl+M)

/** Multi-rename options and their defaults. */
export const MULTI_RENAME_DEFAULTS = Object.freeze({
  name: "[N]",
  ext: "[E]",
  search: "",
  replace: "",
  regex: false,
  matchCase: false,
  /** "same", "lower", "upper", "first" (first letter), "words" (each word's first letter). */
  case: "same",
  start: 1,
  step: 1,
  digits: 1,
  /** The counter starts again in each folder (a results list mixes folders). */
  perFolder: false,
});

/** Placeholders of the name and extension masks, for the dialog's help line. */
export const MULTI_RENAME_HELP =
  "[N] name  [N2-5] chars 2–5  [N3-] from 3  [E] extension  [C] counter  [P] folder  [Y][M][D] date  [h][m][s] time  [[ = [";

/** {base, ext} of a name: the last dot splits it; folders and dot-files (".bashrc") have no ext. */
export function splitExtension(name, isDir = false) {
  const dot = name.lastIndexOf(".");
  if (isDir || dot <= 0 || dot === name.length - 1) return { base: name, ext: "" };
  return { base: name.slice(0, dot), ext: name.slice(dot + 1) };
}

const pad2 = (n) => String(n).padStart(2, "0");

/** Characters `from`..`to` (1-based, inclusive; `to` null = to the end) of `s`. */
function charRange(s, from, to) {
  const cs = Array.from(s);
  return cs.slice(Math.max(0, from - 1), to == null ? cs.length : Math.max(0, to)).join("");
}

/**
 * Expands a multi-rename mask. v: {base, ext, counter, parent, mtime (s)}. Unknown
 * placeholders stay as written; "[[" is a literal "[".
 */
export function expandRenameMask(mask, v) {
  const d = new Date((v.mtime ?? 0) * 1000);
  const simple = {
    C: v.counter ?? "",
    P: v.parent ?? "",
    Y: String(d.getFullYear()),
    M: pad2(d.getMonth() + 1),
    D: pad2(d.getDate()),
    h: pad2(d.getHours()),
    m: pad2(d.getMinutes()),
    s: pad2(d.getSeconds()),
  };
  return mask.replace(/\[\[|\[([NE])(?:(\d+)(?:(-)(\d+)?)?)?\]|\[([CPYMDhms])\]/g, (all, ne, from, dash, to, other) => {
    if (all === "[[") return "[";
    if (other) return simple[other];
    const src = ne === "N" ? v.base : v.ext;
    if (from == null) return src;
    const f = Number(from);
    if (!dash) return charRange(src, f, f);
    return charRange(src, f, to == null ? null : Number(to));
  });
}

function applyCase(name, mode) {
  switch (mode) {
    case "lower": return name.toLowerCase();
    case "upper": return name.toUpperCase();
    case "first": return name.charAt(0).toUpperCase() + name.slice(1).toLowerCase();
    case "words": return name.toLowerCase().replace(/(^|[\s_\-.([])(\p{L})/gu, (_, sep, c) => sep + c.toUpperCase());
    default: return name;
  }
}

const escapeRegExp = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The search-and-replace step as a function, or {error} for a bad regular expression. */
export function renameReplacer(opts) {
  if (!opts.search) return (s) => s;
  try {
    const re = new RegExp(opts.regex ? opts.search : escapeRegExp(opts.search), opts.matchCase ? "g" : "gi");
    // Plain text: "$" in the replacement is literal.
    const rep = opts.regex ? opts.replace : opts.replace.replace(/\$/g, "$$$$");
    return (s) => s.replace(re, rep);
  } catch (e) {
    return { error: `Search: ${e.message}` };
  }
}

/**
 * The new name of `entry` ({name, isDir, mtime}) as item `index` of the batch, in folder
 * `parent` (its name only). `replacer` comes from renameReplacer.
 */
export function multiRenameName(entry, index, opts, parent, replacer = renameReplacer(opts)) {
  // A names list (from a file) gives the whole new name; the rest of the list's rows keep theirs.
  if (Array.isArray(opts.names)) {
    let name = index < opts.names.length ? opts.names[index] : entry.name;
    if (typeof replacer === "function") name = replacer(name);
    return applyCase(name, opts.case);
  }
  const { base, ext } = splitExtension(entry.name, entry.isDir);
  const n = Math.trunc(Number(opts.start) || 0) + index * Math.trunc(Number(opts.step) || 0);
  const digits = Math.max(1, Math.min(10, Math.trunc(Number(opts.digits) || 1)));
  const counter = n < 0 ? `-${String(-n).padStart(digits, "0")}` : String(n).padStart(digits, "0");
  const v = { base, ext, counter, parent, mtime: entry.mtime };
  const newBase = expandRenameMask(opts.name, v);
  const newExt = expandRenameMask(opts.ext, v);
  let name = newExt ? `${newBase}.${newExt}` : newBase;
  if (typeof replacer === "function") name = replacer(name);
  return applyCase(name, opts.case);
}

const nameKey = (s) => s.normalize("NFC").toLowerCase();

/** New names from a text file: one per line (CR LF or LF), trailing empty lines dropped. */
export function namesFromText(text) {
  const lines = String(text).replace(/^\uFEFF/, "").split(/\r?\n/);
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop();
  return lines;
}

/**
 * Preview of a multi-rename: {rows: [{entry, from, to, error}], ok, error}. `ok`: something
 * changes and no row has an error. entries: [{name, isDir, mtime, folder}] — `folder` is the
 * path of the item's folder (a results list mixes folders). `others(folder)` gives the names
 * in that folder (null when unknown; the backend checks again anyway).
 */
export function multiRenamePlan(entries, opts, others = () => null) {
  const replacer = renameReplacer(opts);
  if (replacer.error) {
    return { rows: entries.map((e) => ({ entry: e, from: e.name, to: e.name, error: null })), ok: false, error: replacer.error };
  }
  const inFolder = new Map();
  const rows = entries.map((e, i) => {
    const parent = lastComponent(e.folder) ?? "";
    const k = inFolder.get(e.folder) ?? 0;
    inFolder.set(e.folder, k + 1);
    const to = multiRenameName(e, opts.perFolder ? k : i, opts, parent, replacer);
    let error = null;
    if (!to || to === "." || to === ".." || to.includes("/") || to.includes("\0")) error = "Invalid name";
    return { entry: e, from: e.name, to, error };
  });
  const seen = new Map();
  for (const r of rows) {
    if (r.error) continue;
    const k = `${r.entry.folder}\0${nameKey(r.to)}`;
    if (seen.has(k)) {
      r.error = "Same name twice";
      seen.get(k).error = "Same name twice";
    } else seen.set(k, r);
  }
  const leaving = new Set(rows.filter((r) => r.from !== r.to).map((r) => `${r.entry.folder}\0${nameKey(r.from)}`));
  // Each folder's other names once as a Set of keys (a big folder would otherwise be scanned
  // per row on every keystroke).
  const keyCache = new Map();
  const otherKeys = (folder) => {
    if (!keyCache.has(folder)) keyCache.set(folder, new Set((others(folder) ?? []).map(nameKey)));
    return keyCache.get(folder);
  };
  for (const r of rows) {
    if (r.error || r.from === r.to) continue;
    const k = nameKey(r.to);
    // A case-only change of the item itself is fine.
    if (k === nameKey(r.from)) continue;
    if (otherKeys(r.entry.folder).has(k) && !leaving.has(`${r.entry.folder}\0${k}`)) r.error = "Already exists";
  }
  const changed = rows.some((r) => r.from !== r.to);
  return { rows, ok: changed && rows.every((r) => !r.error), error: null };
}

// ---------------------------------------------------------------- clipboard and context menu

/** Status text when Paste finds neither fxfm's own Copy/Cut nor files copied in Finder. */
export const NOTHING_TO_PASTE = "Nothing to paste";
/** Paste into a search results list is refused: it is no folder. */
export const PASTE_NEEDS_FOLDER = "Paste needs a folder, not a search results list.";

/**
 * What Paste does: fxfm's own Copy/Cut (`clip`: {kind, paths, cwd, change}) while the macOS
 * clipboard still holds it (same change count), else a copy of the files on the clipboard
 * (`board`: {change, files}, e.g. copied in Finder). Returns {kind, paths, cwd} or null.
 */
export function pasteSource(clip, board) {
  if (clip && clip.change === board.change) return { kind: clip.kind, paths: clip.paths, cwd: clip.cwd };
  if (!board.files.length) return null;
  return { kind: "copy", paths: board.files, cwd: parentLocation(board.files[0]) };
}

/** Every right-click item in menu order ([id, label] or "-"); ids are command ids or own actions. */
export const CONTEXT_ITEMS = [
  ["open", "Open"],
  ["cm_view", "View"],
  ["open_with", "Open with…"],
  ["quick_look", "Quick Look"],
  ["show_in_finder", "Show in Finder"],
  "-",
  ["cm_copy_names_to_clip", "Copy name"],
  ["cm_copy_full_names_to_clip", "Copy path"],
  ["cm_copy_share_links", "Copy network path…"],
  "-",
  ["cm_cut_to_clipboard", "Cut"],
  ["cm_copy_to_clipboard", "Copy"],
  ["cm_paste_from_clipboard", "Paste"],
  "-",
  ["cm_packfiles", "Pack files…"],
  ["cm_delete", "Delete"],
  ["cm_rename", "Rename"],
  "-",
  ["cm_chmod", "Change attributes…"],
];

/** Items that need a local file (macOS opens it itself). */
const LOCAL_ONLY = new Set(["open_with", "quick_look", "show_in_finder"]);
/** Items only for files on a network volume. */
const NETWORK_ONLY = new Set(["cm_copy_share_links"]);

/**
 * Right-click menu of a panel as [id, label] pairs and "-" separators. `entry`: the row under
 * the pointer (null or `..`: only Paste); `n`: how many items the commands act on; `off`: ids
 * switched off in Options; `local`: the items are local files (not in an archive or on a server);
 * `network`: they are on a network volume. Separators never lead, trail or repeat.
 */
export function contextMenuItems(entry, n, off = [], local = true, network = false) {
  const skip = new Set(off);
  const want = !entry || entry.up ? ["cm_paste_from_clipboard"] : null;
  const s = n > 1 ? "s" : "";
  const out = [];
  for (const it of CONTEXT_ITEMS) {
    if (it === "-") {
      if (out.length && out[out.length - 1] !== "-") out.push("-");
      continue;
    }
    const [id, label] = it;
    if (skip.has(id) || (want && !want.includes(id)) || (!local && LOCAL_ONLY.has(id)) || (!network && NETWORK_ONLY.has(id))) continue;
    out.push([id, id === "cm_copy_names_to_clip" || id === "cm_copy_full_names_to_clip" ? label + s : label]);
  }
  while (out[out.length - 1] === "-") out.pop();
  return out;
}

/**
 * Quick filter (Ctrl+S): `pattern` with `*` or `?` is a mask (several split by ";"),
 * anything else must appear in the name; case does not matter.
 */
export function filterMatch(name, pattern) {
  const p = pattern.trim();
  if (!p) return true;
  if (/[*?]/.test(p)) return matchMask(name, p);
  return name.normalize("NFC").toLowerCase().includes(p.normalize("NFC").toLowerCase());
}

/** Folder history: at most this many places back (and forward). */
export const HISTORY_MAX = 50;

/** `list` with `item` added at the end (an equal last location is replaced), capped at HISTORY_MAX. */
export function pushHistory(list, item) {
  const out = list.length && list[list.length - 1].location === item.location ? list.slice(0, -1) : [...list];
  out.push(item);
  return out.slice(-HISTORY_MAX);
}

/**
 * Help → Keyboard shortcuts: [menu, title, keys] per command in registry order; `keys` are the
 * combos bound to it in `keymap` (combo → id), labelled with `label`, or "—".
 */
export function keyTableRows(commands, keymap, label) {
  return commands.map((c) => {
    const keys = Object.keys(keymap).filter((k) => keymap[k] === c.id);
    return [c.menu, c.title, keys.length ? keys.map(label).join(", ") : "—"];
  });
}

// ---------------------------------------------------------------- synchronize dirs

/**
 * Synchronize dirs modes: [id, label]. Each proposes a direction per row (`syncModeAction`);
 * the user can still change every row before Synchronize.
 */
export const SYNC_MODES = [
  ["twoWay", "Two-way (newer and missing go both ways)"],
  ["mirrorRight", "Mirror → (make the right side like the left)"],
  ["mirrorLeft", "Mirror ← (make the left side like the right)"],
  ["updateRight", "Update → (new and newer files to the right)"],
  ["updateLeft", "Update ← (new and newer files to the left)"],
  ["deleteRight", "Delete on the right what the left lacks"],
  ["deleteLeft", "Delete on the left what the right lacks"],
];

/** Default direction of a compared row (`state` from `sync_compare`): newer / one-sided goes over. */
export function syncDefaultAction(state) {
  return syncModeAction("twoWay", { state });
}

/**
 * Direction `mode` proposes for `row`: "right" / "left" (copy that way), "delRight" / "delLeft"
 * (move the item on that side to the Trash) or "none". Mirror copies every difference one way
 * and deletes what the source lacks; Update only copies new and newer items one way.
 */
export function syncModeAction(mode, row) {
  const s = row.state;
  const pick = (a) => (syncActionAllowed(row, a) ? a : "none");
  switch (mode) {
    case "mirrorRight":
      if (s === "rightOnly") return pick("delRight");
      return ["leftOnly", "leftNewer", "rightNewer", "differs"].includes(s) ? pick("right") : "none";
    case "mirrorLeft":
      if (s === "leftOnly") return pick("delLeft");
      return ["rightOnly", "rightNewer", "leftNewer", "differs"].includes(s) ? pick("left") : "none";
    case "updateRight":
      return s === "leftOnly" || s === "leftNewer" ? pick("right") : "none";
    case "updateLeft":
      return s === "rightOnly" || s === "rightNewer" ? pick("left") : "none";
    case "deleteRight":
      return s === "rightOnly" ? pick("delRight") : "none";
    case "deleteLeft":
      return s === "leftOnly" ? pick("delLeft") : "none";
    default:
      if (s === "leftOnly" || s === "leftNewer") return "right";
      if (s === "rightOnly" || s === "rightNewer") return "left";
      return "none";
  }
}

/** May `row` get `action`? Copying needs the source item, deleting the item on that side. */
export function syncActionAllowed(row, action) {
  if (action === "none") return true;
  if (row.state === "typeMismatch" || row.state === "nameClash") return false;
  // A folder on both sides is compared inside; copying or deleting it whole would hit every file in it.
  if (row.left?.isDir && row.right?.isDir) return false;
  if (action === "right" || action === "delLeft") return Boolean(row.left);
  if (action === "left" || action === "delRight") return Boolean(row.right);
  return false;
}

/** The symbol of an action in the list. */
export function syncSymbol(action, state) {
  const sym = { right: "→", left: "←", delRight: "→✕", delLeft: "✕←" }[action];
  if (sym) return sym;
  return state === "equal" ? "=" : "≠";
}

const syncJoin = (dir, name) => `${dir.replace(/\/+$/, "")}/${name}`;

/**
 * Jobs for the rows with an action (`actions[i]` for `rows[i]`), copies first, then deletes:
 * sources from one folder into the same folder on the other side are one copy job (dest ends
 * with "/": into that folder); a file spelled differently on the target (NFC/NFD) is copied
 * onto that exact path instead, so no second file appears. Deletes go to the Trash (servers,
 * network shares and archives: permanently, the delete job asks).
 */
export function syncPlan(rows, actions) {
  const groups = new Map();
  const jobs = [];
  const del = { delLeft: [], delRight: [] };
  rows.forEach((row, i) => {
    const a = actions[i];
    if (a === "none" || !syncActionAllowed(row, a)) return;
    if (a === "delLeft" || a === "delRight") {
      del[a].push(a === "delLeft" ? syncJoin(row.leftDir, row.left.name) : syncJoin(row.rightDir, row.right.name));
      return;
    }
    const [src, srcDir, dst, dstDir] = a === "right" ? [row.left, row.leftDir, row.right, row.rightDir] : [row.right, row.rightDir, row.left, row.leftDir];
    const from = syncJoin(srcDir, src.name);
    if (dst && dst.name !== src.name) {
      jobs.push({ kind: "copy", sources: [from], dest: syncJoin(dstDir, dst.name), cwd: srcDir });
      return;
    }
    const key = `${a}\n${srcDir}\n${dstDir}`;
    if (!groups.has(key)) {
      const job = { kind: "copy", sources: [], dest: `${dstDir.replace(/\/+$/, "")}/`, cwd: srcDir };
      groups.set(key, job);
      jobs.push(job);
    }
    groups.get(key).sources.push(from);
  });
  for (const paths of [del.delRight, del.delLeft]) if (paths.length) jobs.push({ kind: "delete", paths, toTrash: true });
  return jobs;
}

/** Counts for the summary line: items and bytes copied each way, items deleted on each side. */
export function syncCounts(rows, actions) {
  const c = { right: 0, left: 0, rightBytes: 0, leftBytes: 0, delRight: 0, delLeft: 0 };
  rows.forEach((row, i) => {
    const a = actions[i];
    if (a === "none" || !syncActionAllowed(row, a)) return;
    c[a] += 1;
    if (a === "right" || a === "left") {
      const src = a === "right" ? row.left : row.right;
      c[`${a}Bytes`] += src.isDir ? 0 : src.size;
    }
  });
  return c;
}

/** Synchronize dirs window ↔ main window: the window asks, the main window answers. */
export const SYNC_ACTION = "syncdirs://action";
export const SYNC_REPLY = "syncdirs://reply";

/** Sort order of the direction column: → first (with deletes on the right), then ←, unchanged, ignored. */
const SYNC_ACTION_RANK = { right: 0, delRight: 1, left: 2, delLeft: 3 };

/** Columns of the Synchronize dirs list, left to right (the header's sort keys). */
export const SYNC_COLUMNS = ["leftName", "leftSize", "leftDate", "action", "rightDate", "rightSize", "rightName"];

/**
 * `indexes` (into `rows`) sorted by `column` (one of SYNC_COLUMNS; null keeps the compare order),
 * descending when `desc`. Rows missing the item of that side sort after the others; ties go by
 * path. By direction: copies → (and deletes on the right), copies ← (and deletes on the left),
 * equal, then the not copied differences.
 */
export function syncSort(rows, actions, indexes, column, desc = false) {
  if (!column) return [...indexes];
  const side = column.startsWith("left") ? "left" : "right";
  const key = (i) => {
    const row = rows[i];
    if (column === "action") {
      const a = actions[i];
      return a in SYNC_ACTION_RANK ? SYNC_ACTION_RANK[a] : row.state === "equal" ? 4 : 5;
    }
    const item = row[side];
    if (!item) return null;
    if (column.endsWith("Size")) return item.isDir ? -1 : item.size;
    if (column.endsWith("Date")) return item.mtime;
    return row.rel.toLowerCase();
  };
  // Keys once per row, one collator: big lists sort in one pass of cheap comparisons.
  const keys = new Map(indexes.map((i) => [i, key(i)]));
  const text = new Intl.Collator(undefined, { numeric: true });
  const byPath = (a, b) => text.compare(rows[a].rel, rows[b].rel);
  return [...indexes].sort((a, b) => {
    const ka = keys.get(a);
    const kb = keys.get(b);
    if (ka === null || kb === null) return ka === kb ? byPath(a, b) : ka === null ? 1 : -1;
    let c = typeof ka === "string" ? text.compare(ka, kb) : ka - kb;
    if (desc) c = -c;
    return c || byPath(a, b);
  });
}

// ---------------------------------------------------------------- branch view (Ctrl+B)

/** Results-list entries of a `branch_list` answer: named by their path below the folder. */
export function branchEntries(items) {
  return items.map((it) => ({
    name: it.rel,
    path: it.path,
    isDir: false,
    size: it.size,
    mtime: it.mtime,
    hidden: false,
    inArchive: Boolean(it.inArchive),
    mounted: true,
    mode: 0,
    isSymlink: false,
  }));
}

/** Status line after Ctrl+B: how many files, and what was left out. */
export function branchStatus({ items, truncated, unreadable }) {
  const parts = [`Branch view: ${groupThousands(items.length)} file(s)`];
  if (truncated) parts.push(`only the first ${groupThousands(items.length)}`);
  if (unreadable) parts.push(`${unreadable} folder(s) could not be read`);
  return parts.join(" · ");
}

/**
 * The Show filter group of a Synchronize dirs row: "right" (copied or deleted on the right),
 * "left" (copied or deleted on the left), "equal", or "diff" (a difference that is not copied).
 */
export function syncShowKind(row, action) {
  if (action === "right" || action === "delRight") return "right";
  if (action === "left" || action === "delLeft") return "left";
  return row.state === "equal" ? "equal" : "diff";
}

/** The Show filter (`{right, left, diff, equal}` booleans) from a kept layout; older layouts had `showEqual` only. */
export function syncShowFilter(saved) {
  const s = saved?.show ?? {};
  const on = (k, dflt) => (typeof s[k] === "boolean" ? s[k] : dflt);
  return { right: on("right", true), left: on("left", true), diff: on("diff", true), equal: on("equal", saved?.showEqual === true) };
}
