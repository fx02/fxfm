import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import * as lib from "./lib.mjs";

test("smoke", () => {
  assert.equal(typeof lib, "object");
});

test("comboFromEvent matches fixture", async () => {
  const { readFile } = await import("node:fs/promises");
  const cases = JSON.parse(
    await readFile(new URL("../tests/fixtures/keycombos.json", import.meta.url), "utf8"),
  );
  assert.ok(cases.length >= 20);
  for (const { event, combo } of cases) {
    assert.equal(lib.comboFromEvent(event), combo, JSON.stringify(event));
  }
});

test("comboFromEvent ignores pure modifiers and unknown codes", () => {
  const e = (code, extra = {}) => ({ code, key: "", ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...extra });
  assert.equal(lib.comboFromEvent(e("ShiftLeft", { shiftKey: true })), null);
  assert.equal(lib.comboFromEvent(e("MetaRight", { metaKey: true })), null);
  assert.equal(lib.comboFromEvent(e("IntlRo")), null);
});

test("formatK floors to KiB with comma separators", () => {
  assert.equal(lib.formatK(295520305152), "288,594,048 k");
  assert.equal(lib.formatK(0), "0 k");
  assert.equal(lib.formatK(1023), "0 k");
  assert.equal(lib.formatK(1024 * 1000), "1,000 k");
});

test("freeLine", () => {
  assert.equal(
    lib.freeLine("Macintosh HD", 295520305152, 994662584320),
    "[Macintosh HD]  288,594,048 k of 971,350,180 k free",
  );
});

test("formatDate dd.MM.yyyy HH:mm in a fixed zone", () => {
  assert.equal(lib.formatDate(1793890560, "Europe/Zagreb"), "05.11.2026 15:56");
  // summer time (UTC+2), single-digit day/month padded
  assert.equal(lib.formatDate(Date.UTC(2026, 6, 4, 7, 5) / 1000, "Europe/Zagreb"), "04.07.2026 09:05");
  assert.equal(lib.formatDate(Date.UTC(2026, 0, 1, 23, 30) / 1000, "Europe/Zagreb"), "02.01.2026 00:30");
});

test("formatSize", () => {
  assert.equal(lib.formatSize({ name: "a", isDir: true, size: 96 }), "<DIR>");
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 4218 }), "4,218");
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 7 }), "7");
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 1234567 }), "1,234,567");
});

test("formatMode", () => {
  assert.equal(lib.formatMode(0o40755, true, false), "drwxr-xr-x");
  assert.equal(lib.formatMode(0o40755, true, true), "lrwxr-xr-x");
  assert.equal(lib.formatMode(0o100644, false, false), "-rw-r--r--");
  assert.equal(lib.formatMode(0, false, false), "----------");
});

test("displayName", () => {
  assert.equal(lib.displayName({ name: "DEBIAN", isDir: true }), "[DEBIAN]");
  assert.equal(lib.displayName({ name: "a.txt", isDir: false }), "a.txt");
});

test("iconKind", () => {
  const f = (name) => lib.iconKind({ name, isDir: false, hidden: name.startsWith(".") });
  assert.equal(f("a.tgz"), "arc");
  assert.equal(f(".x.zip"), "arc");
  assert.equal(f("B.TAR"), "arc");
  assert.equal(f("film.MKV"), "med");
  assert.equal(f("song.mp3"), "med");
  assert.equal(f("notes.txt"), "");
  assert.equal(f("zip"), "");
  assert.equal(lib.iconKind({ name: "x.zip", isDir: true, hidden: false }), "");
});

const DEFAULT_COLORS = JSON.parse(
  readFileSync(new URL("../tests/fixtures/default_colors.json", import.meta.url), "utf8"),
);
const file = (name, extra = {}) => ({ name, isDir: false, hidden: false, ...extra });
const dir = (name, extra = {}) => ({ name, isDir: true, hidden: false, ...extra });

test("compileTypeRules: ';' and spaces, case, NFD names, first match wins", () => {
  const m = lib.compileTypeRules([
    { masks: "*.txt  *.MD;;", color: "#111111" },
    { masks: "č*", color: "#222222" },
    { masks: "*.txt", color: "#333333" },
    { masks: "", color: "#444444" },
  ]);
  assert.equal(m("a.txt"), "#111111");
  assert.equal(m("README.md"), "#111111");
  assert.equal(m("čapek.doc"), "#222222");
  assert.equal(m("čapek.doc".normalize("NFD")), "#222222");
  assert.equal(m("čapek.txt"), "#111111");
  assert.equal(m("x.bin"), null);
  assert.equal(lib.compileTypeRules([])("a.txt"), null);
  assert.equal(lib.compileTypeRules(undefined)("a.txt"), null);
});

test("entryColor: each default group, folders, hidden fallback, byType off", () => {
  const c = DEFAULT_COLORS;
  const m = lib.compileTypeRules(c.typeRules);
  const e = (entry, colors = c) => lib.entryColor(entry, colors, m);
  assert.equal(e(file("notes.TXT")), "#c8c8c8");
  assert.equal(e(file("doc.pdf")), "#c8c8c8");
  assert.equal(e(file("Install.pkg")), "#5fd35f");
  assert.equal(e(file("run.sh")), "#5fd35f");
  assert.equal(e(file("photo.JPG")), "#ffff4d");
  assert.equal(e(file("a.7z")), "#ff6a4d");
  assert.equal(e(file("disk.dmg")), "#ff6a4d");
  assert.equal(e(file("film.mkv")), "#8a8aff");
  assert.equal(e(file("song.flac")), "#8a8aff");
  assert.equal(e(file("Makefile")), null);
  // Folders are matched too (TC): an app bundle is green.
  assert.equal(e(dir("Safari.app")), "#5fd35f");
  // A matching rule beats hidden; hidden is the fallback.
  assert.equal(e(file(".notes.txt", { hidden: true })), "#c8c8c8");
  assert.equal(e(file(".DS_Store", { hidden: true })), c.hidden);
  assert.equal(e(dir(".git", { hidden: true })), c.hidden);
  // The ".." row is never colored by a rule.
  assert.equal(e({ name: "..", isDir: true, up: true, hidden: false }), null);
  const off = { ...c, byType: false };
  assert.equal(e(file("a.7z"), off), null);
  assert.equal(e(file(".a.7z", { hidden: true }), off), c.hidden);
});

test("entryColor: a folder mask without an extension", () => {
  const m = lib.compileTypeRules([{ masks: "www*", color: "#00ffff" }]);
  const c = { ...DEFAULT_COLORS, typeRules: [{ masks: "www*", color: "#00ffff" }] };
  assert.equal(lib.entryColor(dir("www-root"), c, m), "#00ffff");
  assert.equal(lib.entryColor(dir("html"), c, m), null);
});

test("rowStyle precedence: cursor > marked > rule > hidden > font", () => {
  const c = { ...DEFAULT_COLORS, background2: "#0a0a0a" };
  const m = lib.compileTypeRules(c.typeRules);
  const st = (entry, state, colors = c) =>
    lib.rowStyle(entry, { marked: false, cursor: false, active: true, index: 0, ...state }, colors, m);
  // Plain rows alternate the backgrounds and use rule / hidden / font colors.
  assert.deepEqual(st(file("a.zip"), {}), { color: "#ff6a4d", background: "#000000", outline: "" });
  assert.deepEqual(st(file("Makefile"), { index: 1 }), { color: "#ffffff", background: "#0a0a0a", outline: "" });
  assert.equal(st(file(".DS_Store", { hidden: true }), {}).color, c.hidden);
  // Marked beats the rule.
  assert.equal(st(file("a.zip"), { marked: true }).color, c.mark);
  // Cursor in the active panel with an inverted cursor: a bar with cursorFont.
  assert.deepEqual(st(file("a.zip"), { cursor: true, index: 1 }), {
    color: c.cursorFont,
    background: c.cursor,
    outline: "",
  });
  // Selected+Cursor: mark text on the cursor bar.
  assert.deepEqual(st(file("a.zip"), { cursor: true, marked: true }), {
    color: c.mark,
    background: c.cursor,
    outline: "",
  });
  // Empty cursorFont: the entry keeps its own color on the bar.
  const keep = { ...c, cursorFont: "" };
  assert.equal(st(file("a.zip"), { cursor: true }, keep).color, "#ff6a4d");
  assert.equal(st(file("Makefile"), { cursor: true }, keep).color, c.font);
  // Not inverted: no fill, a frame in the cursor color, the text keeps its color.
  const frame = { ...c, invertedCursor: false };
  assert.deepEqual(st(file("a.zip"), { cursor: true, index: 1 }, frame), {
    color: "#ff6a4d",
    background: "#0a0a0a",
    outline: `1px solid ${c.cursor}`,
  });
  assert.equal(st(file("a.zip"), { cursor: true, marked: true }, frame).color, c.mark);
  // Inactive panel: only a dashed frame, whatever invertedCursor says.
  for (const colors of [c, frame]) {
    assert.deepEqual(st(file("a.zip"), { cursor: true, active: false }, colors), {
      color: "#ff6a4d",
      background: "#000000",
      outline: lib.INACTIVE_CURSOR_OUTLINE,
    });
  }
  assert.equal(st(file("a.zip"), { cursor: true, active: false, marked: true }).color, c.mark);
});

test("matchMask", () => {
  assert.equal(lib.matchMask("index.PHP", "*.php"), true);
  assert.equal(lib.matchMask("čćžšđ.txt", "*.txt"), true);
  // NFD file name (as macOS may store it) against an NFC mask
  assert.equal(lib.matchMask("čapek.txt", "č*.txt"), true);
  assert.equal(lib.matchMask("Makefile", "*.*"), true);
  assert.equal(lib.matchMask("a.sh", "*.php;*.sh"), true);
  assert.equal(lib.matchMask("a.sh", "*.php *.sh"), true);
  assert.equal(lib.matchMask("a.txt", "*.php;*.sh"), false);
  assert.equal(lib.matchMask("ab.md", "a?.md"), true);
  assert.equal(lib.matchMask("abc.md", "a?.md"), false);
  assert.equal(lib.matchMask("a+b(1).md", "a+b(?).md"), true);
  assert.equal(lib.matchMask("x.md", ""), false);
});

test("visibleRange", () => {
  assert.deepEqual(lib.visibleRange(0, 25, 500, 50000), { start: 0, end: 30 });
  assert.deepEqual(lib.visibleRange(25 * 1000, 25, 500, 50000), { start: 990, end: 1030 });
  assert.deepEqual(lib.visibleRange(0, 25, 500, 5), { start: 0, end: 5 });
  assert.deepEqual(lib.visibleRange(0, 25, 0, 0), { start: 0, end: 0 });
});

test("parentLocation / joinLocation / lastComponent", () => {
  assert.equal(lib.parentLocation("/"), "/");
  assert.equal(lib.parentLocation("/a/b"), "/a");
  assert.equal(lib.parentLocation("/a"), "/");
  assert.equal(lib.parentLocation("/a/b/"), "/a");
  assert.equal(lib.joinLocation("/", "x"), "/x");
  assert.equal(lib.joinLocation("/a", "x"), "/a/x");
  assert.equal(lib.lastComponent("/a/b"), "b");
  assert.equal(lib.lastComponent("/a/b/"), "b");
  assert.equal(lib.lastComponent("/"), "");
});

test("markSummary", () => {
  const entries = [{ name: "..", isDir: true, up: true, size: 0 }];
  for (let i = 0; i < 29; i++) entries.push({ name: `f${i}`, isDir: false, size: 0 });
  entries.push({ name: "a", isDir: false, size: 2000 });
  entries.push({ name: "b", isDir: false, size: 2319 });
  entries[1].size = 295520305152 - 4319;
  assert.equal(lib.markSummary(entries, new Set(["a", "b"])), "4 k / 288,594,048 k in 2 / 31 file(s)");
  const small = entries.map((e) => ({ ...e }));
  small[1].size = 295520300 - 4319359;
  small[30].size = 4319359 - 2319;
  small[31].size = 2319;
  assert.equal(lib.markSummary(small, new Set(["a", "b"])), "4,218 k / 288,594 k in 2 / 31 file(s)");
  assert.equal(lib.markSummary(small, new Set()), "0 k / 288,594 k in 0 / 31 file(s)");
});

test("markSummary counts dirs separately and uses computed dir sizes", () => {
  const entries = [
    { name: "..", isDir: true, up: true, size: 0 },
    { name: "d1", isDir: true, size: 96 },
    { name: "d2", isDir: true, size: 96 },
    { name: "f", isDir: false, size: 2048 },
  ];
  const sizes = new Map([["d1", 10240]]);
  assert.equal(lib.markSummary(entries, new Set(["d1"]), sizes), "10 k / 12 k in 0 / 1 file(s), 1 / 2 dir(s)");
});

test("navigate returns the new listing and suggests the cursor", async () => {
  const state = { location: "/a/b" };
  const r = await lib.navigate(state, "/a", async (loc) => {
    assert.equal(loc, "/a");
    return [{ name: "b", isDir: true }];
  });
  assert.equal(r.ok, true);
  assert.equal(r.location, "/a");
  assert.equal(r.cursorName, "b");
  assert.deepEqual(r.entries, [{ name: "b", isDir: true }]);
  assert.equal(state.location, "/a/b");
  const r2 = await lib.navigate(state, "/x", async () => [], { cursorName: "y" });
  assert.equal(r2.cursorName, "y");
  const r3 = await lib.navigate(state, "/x", async () => []);
  assert.equal(r3.cursorName, null);
});

test("navigateKeepsOldPathOnError", async () => {
  const state = { location: "/a" };
  const r = await lib.navigate(state, "/a/secret", async () => {
    throw { kind: "permissionDenied", path: "/a/secret" };
  }, { nearest: async () => "/a" });
  assert.equal(r.ok, false);
  assert.equal(r.error.kind, "permissionDenied");
  assert.equal(state.location, "/a");
});

test("refreshFallsBackOnMissingDir", async () => {
  const state = { location: "/Volumes/USB/x" };
  const listed = [];
  const r = await lib.navigate(
    state,
    "/Volumes/USB/x",
    async (loc) => {
      listed.push(loc);
      if (loc.startsWith("/Volumes/USB")) throw { kind: "notFound", path: loc };
      return [{ name: "Other", isDir: true }];
    },
    { nearest: async (loc) => lib.parentLocation(lib.parentLocation(loc)) },
  );
  assert.equal(r.ok, true);
  assert.equal(r.location, "/Volumes");
  assert.deepEqual(listed, ["/Volumes/USB/x", "/Volumes"]);
  assert.equal(r.cursorName, "USB");
  // without a nearest function the error is returned unchanged
  const r2 = await lib.navigate(state, "/gone", async () => {
    throw { kind: "notFound", path: "/gone" };
  });
  assert.equal(r2.ok, false);
  assert.equal(r2.error.kind, "notFound");
});

test("refreshTarget skips empty panels and pending user navigation", () => {
  assert.equal(lib.refreshTarget({ location: "/a", pendingTarget: null }), "/a");
  assert.equal(lib.refreshTarget({ location: "", pendingTarget: null }), null);
  assert.equal(lib.refreshTarget({ location: "/a", pendingTarget: "/a/slow" }), null);
});

test("rectOnScreen needs a usable overlap with a monitor", () => {
  const mons = [
    { x: 0, y: 0, w: 1440, h: 900 },
    { x: 1440, y: -200, w: 1920, h: 1080 },
  ];
  assert.equal(lib.rectOnScreen({ x: 100, y: 80, w: 1300, h: 820 }, mons), true);
  assert.equal(lib.rectOnScreen({ x: 2000, y: 0, w: 800, h: 600 }, mons), true);
  // monitor that used to be on the right is gone
  assert.equal(lib.rectOnScreen({ x: 2000, y: 0, w: 800, h: 600 }, mons.slice(0, 1)), false);
  // only a 20 px sliver visible
  assert.equal(lib.rectOnScreen({ x: 1420, y: 0, w: 800, h: 600 }, mons.slice(0, 1)), false);
  assert.equal(lib.rectOnScreen({ x: -5000, y: -5000, w: 800, h: 600 }, mons), false);
  assert.equal(lib.rectOnScreen({ x: 0, y: 0, w: 800, h: 600 }, []), false);
});

test("suggestRename adds the first free (n) before the extension", () => {
  assert.equal(lib.suggestRename("a.txt", new Set(["a.txt"])), "a (2).txt");
  assert.equal(lib.suggestRename("a.txt", ["a.txt", "a (2).txt"]), "a (3).txt");
  assert.equal(lib.suggestRename("README", ["README"]), "README (2)");
  assert.equal(lib.suggestRename(".bashrc", [".bashrc"]), ".bashrc (2)");
  assert.equal(lib.suggestRename("x.tar.gz", ["x.tar.gz"]), "x (2).tar.gz");
});

test("renameSelection selects the name without the extension", () => {
  assert.deepEqual(lib.renameSelection("archive.tar.gz"), { start: 0, end: 7 });
  assert.deepEqual(lib.renameSelection("my.backup.tar.bz2"), { start: 0, end: 9 });
  assert.deepEqual(lib.renameSelection("a.b.txt"), { start: 0, end: 3 });
  assert.deepEqual(lib.renameSelection("README"), { start: 0, end: 6 });
  assert.deepEqual(lib.renameSelection(".bashrc"), { start: 0, end: 7 });
  assert.deepEqual(lib.renameSelection("photos.2026", true), { start: 0, end: 11 });
});

test("copyTargetDefault: other panel folder, plus the name of a single file", () => {
  const f = { name: "a.txt", isDir: false };
  const d = { name: "dir", isDir: true };
  assert.equal(lib.copyTargetDefault("/x", [f]), "/x/a.txt");
  assert.equal(lib.copyTargetDefault("/x", [f, d]), "/x/");
  assert.equal(lib.copyTargetDefault("/x", [d]), "/x/");
  assert.equal(lib.copyTargetDefault("/", [f]), "/a.txt");
  assert.equal(lib.copyTargetDefault("/", [f, d]), "/");
});

test("operationSources: marked entries, else the cursor entry, never [..]", () => {
  const up = { name: "..", isDir: true, up: true };
  const a = { name: "a", isDir: false };
  const b = { name: "b", isDir: true };
  const entries = [up, a, b];
  assert.deepEqual(lib.operationSources(entries, new Set(["b", "a"]), 0), [a, b]);
  assert.deepEqual(lib.operationSources(entries, new Set(), 2), [b]);
  assert.deepEqual(lib.operationSources(entries, new Set(), 0), []);
  assert.deepEqual(lib.operationSources(entries, new Set([".."]), 1), [a]);
  assert.deepEqual(lib.operationSources([], new Set(), 0), []);
});

test("maskSelects: files by default, folders only with a trailing slash", () => {
  const f = { name: "a.txt", isDir: false };
  const d = { name: "src", isDir: true };
  assert.equal(lib.maskSelects(f, "*.*"), true);
  assert.equal(lib.maskSelects(d, "*.*"), false);
  assert.equal(lib.maskSelects(d, "*/"), true);
  assert.equal(lib.maskSelects(f, "*/"), false);
  assert.equal(lib.maskSelects(d, "*.txt;s*/"), true);
  assert.equal(lib.maskSelects(f, "*.txt;s*/"), true);
  assert.equal(lib.maskSelects({ name: "doc", isDir: true }, "*.txt s*/"), false);
});

test("operationPaths snapshots the folder and full paths", () => {
  const s = [{ name: "a" }, { name: "b c" }];
  assert.deepEqual(lib.operationPaths("/x", s), { cwd: "/x", paths: ["/x/a", "/x/b c"] });
  assert.deepEqual(lib.operationPaths("/", s.slice(0, 1)), { cwd: "/", paths: ["/a"] });
});

test("modalKeyBlocked: early keys and auto-repeated Enter/Space", () => {
  const k = (key, repeat = false, onButton = false) => ({ key, repeat, onButton });
  assert.equal(lib.modalKeyBlocked(k("Enter"), 50), true);
  assert.equal(lib.modalKeyBlocked(k("Escape"), 149), true);
  assert.equal(lib.modalKeyBlocked(k("Enter"), 150), false);
  assert.equal(lib.modalKeyBlocked(k("Enter", true), 5000), true);
  assert.equal(lib.modalKeyBlocked(k(" ", true, true), 5000), true);
  assert.equal(lib.modalKeyBlocked(k(" ", true, false), 5000), false);
  assert.equal(lib.modalKeyBlocked(k("a", true), 5000), false);
  assert.equal(lib.modalKeyBlocked(k("Escape"), 5000), false);
});

test("modalKeyBlocked: typing passes at once, only activation/dismissal keys wait", () => {
  const k = (key, repeat = false, onButton = false) => ({ key, repeat, onButton });
  for (const key of ["a", "Z", "1", "Backspace", "Delete", "ArrowLeft", "Home", "Tab"]) {
    assert.equal(lib.modalKeyBlocked(k(key), 0), false, key);
  }
  assert.equal(lib.modalKeyBlocked(k(" "), 10), false); // a space typed into a text field
  assert.equal(lib.modalKeyBlocked(k(" ", false, true), 10), true); // would press the focused button
  assert.equal(lib.modalKeyBlocked(k(" ", false, true), 150), false);
  assert.equal(lib.modalKeyBlocked(k("Enter"), 10), true);
  assert.equal(lib.modalKeyBlocked(k("Escape"), 10), true);
});

test("renameDecision compares the untrimmed name", () => {
  assert.deepEqual(lib.renameDecision(" a.txt ", " a.txt "), { kind: "same" });
  assert.deepEqual(lib.renameDecision(" a.txt ", "a.txt"), { kind: "rename", name: "a.txt" });
  assert.deepEqual(lib.renameDecision("a", "b "), { kind: "rename", name: "b " });
  assert.equal(lib.renameDecision("a", "   ").kind, "invalid");
  assert.equal(lib.renameDecision("a", "").kind, "invalid");
  assert.equal(lib.renameDecision("a", "x/y").kind, "invalid");
});

test("newNameError: F7 allows nesting, never absolute or '..'", () => {
  assert.equal(lib.newNameError("a/b/c", true), null);
  assert.equal(lib.newNameError("new folder", true), null);
  assert.notEqual(lib.newNameError("/abs", true), null);
  assert.notEqual(lib.newNameError("a/../b", true), null);
  assert.notEqual(lib.newNameError("..", true), null);
  assert.notEqual(lib.newNameError(".", true), null);
  assert.notEqual(lib.newNameError("./", true), null);
  assert.equal(lib.newNameError("x.txt", false), null);
  assert.notEqual(lib.newNameError("a/b", false), null);
  assert.notEqual(lib.newNameError("..", false), null);
  assert.equal(lib.firstComponent("./a/b"), "a");
  assert.equal(lib.firstComponent("a//b"), "a");
});

test("permanentConfirmText names a single item", () => {
  assert.equal(lib.permanentConfirmText(["/x/a.txt"]), 'Delete "a.txt" permanently?');
  assert.equal(lib.permanentConfirmText(["/x/a", "/x/b"]), "Delete 2 remaining item(s) permanently?");
});

test("jobSucceeded: only a finished, uncancelled job without error", () => {
  assert.equal(lib.jobSucceeded({ summary: { done: 2, skipped: 1, cancelled: false } }), true);
  assert.equal(lib.jobSucceeded({ summary: { done: 0, skipped: 0, cancelled: true } }), false);
  assert.equal(lib.jobSucceeded({ error: { kind: "intoItself", message: "x" } }), false);
});

test("hexLines: 16 bytes per row with offset and ASCII column", () => {
  assert.deepEqual(lib.hexLines([72, 101, 108, 108, 111], 0), ["00000000  48 65 6c 6c 6f" + " ".repeat(34) + "  |Hello|"]);
  const rows = lib.hexLines(Array.from({ length: 17 }, (_, i) => i + 0x41), 0x10);
  assert.equal(rows.length, 2);
  assert.equal(rows[0], "00000010  41 42 43 44 45 46 47 48  49 4a 4b 4c 4d 4e 4f 50  |ABCDEFGHIJKLMNOP|");
  assert.equal(rows[1].slice(0, 12), "00000020  51");
  assert.equal(lib.hexLines([0, 0x7f, 0x20], 0)[0].endsWith("|.. |"), true);
});

test("formatViewStatus: encoding, grouped byte count, truncated mark", () => {
  assert.equal(lib.formatViewStatus("UTF-8", 12345, false), "UTF-8 · 12,345 bytes");
  assert.equal(lib.formatViewStatus("windows-1250", 5, true), "windows-1250 · 5 bytes (truncated)");
});

// ---- hotlist helpers (same examples as crates/core hotlist.rs tests)

test("parseCd mirrors the core parse_cd", () => {
  const home = "/Users/x";
  assert.equal(lib.parseCd("cd ~/Downloads", home), "/Users/x/Downloads");
  assert.equal(lib.parseCd("CD /tmp ", home), "/tmp");
  assert.equal(lib.parseCd("cd sftp://web-1/var/www", home), "sftp://web-1/var/www");
  assert.equal(lib.parseCd("ls", home), null);
  assert.equal(lib.parseCd("cdx /tmp", home), null);
  assert.equal(lib.parseCd("cd ~", home), "/Users/x");
  assert.equal(lib.parseCd("cd", home), null);
  assert.equal(lib.parseCd("cd   ", home), null);
  assert.equal(lib.parseCd("  cd\t/a b", home), "/a b");
});

test("menuLabel: first & is the accelerator, && is a literal ampersand", () => {
  assert.deepEqual(lib.menuLabel("&Downloads"), { label: "Downloads", accel: "d", accelIndex: 0 });
  assert.deepEqual(lib.menuLabel("A&&B"), { label: "A&B", accel: null, accelIndex: -1 });
  assert.deepEqual(lib.menuLabel("Pro&jekti &x"), { label: "Projekti x", accel: "j", accelIndex: 3 });
  assert.equal(lib.menuLabel("Plain").accel, null);
});

test("addCurrent: last component, ~ for home, full location for /", () => {
  const home = "/Users/x";
  const n = [];
  lib.addCurrent(n, "/Users/x/Documents/PVT", home);
  lib.addCurrent(n, "/Users/x", home);
  lib.addCurrent(n, "/", home);
  lib.addCurrent(n, "sftp://h/var/www", home);
  assert.deepEqual(n[0], { kind: "item", title: "PVT", command: "cd ~/Documents/PVT", target: "" });
  assert.equal(n[1].command, "cd ~");
  assert.equal(n[2].title, "/");
  assert.equal(n[3].command, "cd sftp://h/var/www");
});

test("visibleNodes drops hidden titles recursively", () => {
  const nodes = [
    { kind: "item", title: "?old", command: "cd /a", target: "" },
    { kind: "submenu", title: "S", children: [{ kind: "item", title: "?x", command: "", target: "" }, { kind: "item", title: "y", command: "", target: "" }] },
  ];
  const v = lib.visibleNodes(nodes, "?");
  assert.equal(v.length, 1);
  assert.deepEqual(v[0].children.map((c) => c.title), ["y"]);
  assert.equal(lib.visibleNodes(nodes, null).length, 2);
});

test("moveNode swaps within the parent and stops at the bounds", () => {
  const mk = () => [
    { kind: "item", title: "a" },
    { kind: "submenu", title: "S", children: [{ kind: "item", title: "b" }, { kind: "item", title: "c" }] },
    { kind: "item", title: "d" },
  ];
  let n = mk();
  assert.deepEqual(lib.moveNode(n, [0], 1), [1]);
  assert.deepEqual(n.map((x) => x.title), ["S", "a", "d"]);
  n = mk();
  assert.deepEqual(lib.moveNode(n, [0], -1), [0]);
  assert.deepEqual(lib.moveNode(n, [2], 1), [2]);
  assert.deepEqual(n.map((x) => x.title), ["a", "S", "d"]);
  assert.deepEqual(lib.moveNode(n, [1, 1], -1), [1, 0]);
  assert.deepEqual(n[1].children.map((x) => x.title), ["c", "b"]);
  assert.deepEqual(lib.moveNode(n, [1, 0], -1), [1, 0]);
});

test("relocateNode: into a submenu, to a slot, never into itself", () => {
  const mk = () => [
    { kind: "item", title: "a" },
    { kind: "submenu", title: "S", children: [{ kind: "item", title: "b" }] },
    { kind: "item", title: "d" },
  ];
  let n = mk();
  assert.deepEqual(lib.relocateNode(n, [0], [1]), [0, 1]);
  assert.deepEqual(n.map((x) => x.title), ["S", "d"]);
  assert.deepEqual(n[0].children.map((x) => x.title), ["b", "a"]);
  n = mk();
  assert.deepEqual(lib.relocateNode(n, [0], [2]), [2]);
  assert.deepEqual(n.map((x) => x.title), ["S", "d", "a"]);
  n = mk();
  assert.deepEqual(lib.relocateNode(n, [2], [0]), [0]);
  assert.deepEqual(n.map((x) => x.title), ["d", "a", "S"]);
  n = mk();
  assert.deepEqual(lib.relocateNode(n, [1, 0], [0]), [0]);
  assert.deepEqual(n.map((x) => x.title), ["b", "a", "S", "d"]);
  n = mk();
  assert.equal(lib.relocateNode(n, [1], [1, 0]), null);
  assert.equal(lib.relocateNode(n, [1], [1]), null);
});

test("toggleHidden adds or removes the hide character", () => {
  assert.equal(lib.toggleHidden("Downloads", "?"), "?Downloads");
  assert.equal(lib.toggleHidden("?Downloads", "?"), "Downloads");
});

test("flattenNodes lists submenu children after their parent with depth", () => {
  const rows = lib.flattenNodes([
    { kind: "submenu", title: "S", children: [{ kind: "item", title: "b" }] },
    { kind: "item", title: "d" },
  ]);
  assert.deepEqual(rows.map((r) => [r.path.join("."), r.depth]), [["0", 0], ["0.0", 1], ["1", 0]]);
});

test("selectedPosition is the 1-based flat index, null without selection", () => {
  const n = [
    { kind: "submenu", title: "S", children: [{ kind: "item", title: "b" }] },
    { kind: "item", title: "d" },
  ];
  assert.equal(lib.selectedPosition(n, [0]), 1);
  assert.equal(lib.selectedPosition(n, [0, 0]), 2);
  assert.equal(lib.selectedPosition(n, [1]), 3);
  assert.equal(lib.selectedPosition(n, null), null);
});

test("pickerStart: local cd target, else the active location", () => {
  assert.equal(lib.pickerStart("cd ~/Downloads", "/Users/x", "/tmp"), "/Users/x/Downloads");
  assert.equal(lib.pickerStart("cd sftp://h/x", "/Users/x", "/tmp"), "/tmp");
  assert.equal(lib.pickerStart("ls", "/Users/x", "/tmp"), "/tmp");
  assert.equal(lib.pickerStart("", "/Users/x", "/tmp"), "/tmp");
});

test("conflictMessage names the macOS feature with the physical keys", () => {
  assert.equal(
    lib.conflictMessage("Ctrl+Up"),
    "macOS uses ⌃↑ for Mission Control. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key.",
  );
  // Control as TC Alt: TC Alt+Up is the reserved Control+Up.
  assert.match(lib.conflictMessage("Alt+Up", { alt: "control", ctrl: "option" }), /^macOS uses ⌃↑ for Mission Control\./);
});

test("conflictMessage falls back for unknown combos", () => {
  assert.match(lib.conflictMessage("Ctrl+Space"), /^macOS uses ⌃Space for Select the previous input source\./);
  assert.match(lib.conflictMessage("Ctrl+F12"), /^macOS uses ⌃F12 for a system shortcut\./);
});

test("keyRows: registry order, menu › title, sorted keys, unknown ids ignored", () => {
  const commands = [
    { id: "cm_copy", menu: "Files", title: "Copy" },
    { id: "cm_rename", menu: "Files", title: "Rename" },
    { id: "cm_root", menu: "Commands", title: "Root directory" },
  ];
  const rows = lib.keyRows(commands, { "Shift+F6": "cm_rename", F2: "cm_rename", F5: "cm_copy", F9: "cm_gone" });
  assert.deepEqual(rows, [
    { id: "cm_copy", label: "Files › Copy", keys: ["F5"] },
    { id: "cm_rename", label: "Files › Rename", keys: ["F2", "Shift+F6"] },
    { id: "cm_root", label: "Commands › Root directory", keys: [] },
  ]);
});

test("cmdlineKeyGoesToPanel: F-keys, Ctrl/Alt and Up/Down go to the panel; editing keys stay", () => {
  for (const k of ["F3", "F5", "Shift+F6", "F12", "Ctrl+D", "Ctrl+R", "Alt+F1", "Up", "Down", "PageDown", "Tab"]) {
    assert.equal(lib.cmdlineKeyGoesToPanel(k), true, k);
  }
  for (const k of ["A", "Shift+A", "1", "Space", "Backspace", "Delete", "Left", "Right", "Home", "End", "Enter", "Ctrl+Enter", "Escape", null]) {
    assert.equal(lib.cmdlineKeyGoesToPanel(k), false, String(k));
  }
});

test("fullyVisibleRows: first/last row fully in view", () => {
  // 20 rows of 25 px fit in 500 px
  assert.deepEqual(lib.fullyVisibleRows(0, 25, 500, 1000), { first: 0, last: 19 });
  // scrolled by half a row: row 0 and row 20 are cut, so 1..19
  assert.deepEqual(lib.fullyVisibleRows(12, 25, 500, 1000), { first: 1, last: 19 });
  // short list: last is the last entry
  assert.deepEqual(lib.fullyVisibleRows(0, 25, 500, 5), { first: 0, last: 4 });
  // scrolled to the end
  assert.deepEqual(lib.fullyVisibleRows(25 * 980, 25, 500, 1000), { first: 980, last: 999 });
  assert.deepEqual(lib.fullyVisibleRows(0, 25, 500, 0), { first: 0, last: 0 });
});

test("sideJump: to the window edge, then a window further on each press", () => {
  // window shows rows 0..19
  assert.equal(lib.sideJump(5, 0, 19, +1), 19);
  assert.equal(lib.sideJump(19, 0, 19, +1), 39);
  assert.equal(lib.sideJump(5, 0, 19, -1), 0);
  // window shows rows 20..39 (cursor at its bottom after scrolling)
  assert.equal(lib.sideJump(39, 20, 39, +1), 59);
  assert.equal(lib.sideJump(30, 20, 39, -1), 20);
  assert.equal(lib.sideJump(20, 20, 39, -1), 0);
});

test("archiveKindOf follows the Rust from_name rules", () => {
  assert.equal(lib.archiveKindOf("A.ZIP"), "zip");
  assert.equal(lib.archiveKindOf("x.tar"), "tar");
  assert.equal(lib.archiveKindOf("x.tar.gz"), "tgz");
  assert.equal(lib.archiveKindOf("x.TGZ"), "tgz");
  assert.equal(lib.archiveKindOf("x.gz"), "gz");
  assert.equal(lib.archiveKindOf("x.7z"), "7z");
  assert.equal(lib.archiveKindOf("x.rar"), null);
  assert.equal(lib.archiveKindOf("x.bz2"), null);
  assert.equal(lib.archiveKindOf("zip"), null);
});

test("archiveOf: location prefix up to the first archive component", () => {
  assert.equal(lib.archiveOf("/u/a.zip/src"), "/u/a.zip");
  assert.equal(lib.archiveOf("/u/a.zip"), "/u/a.zip");
  assert.equal(lib.archiveOf("/u/a.zip/b.zip/x"), "/u/a.zip");
  assert.equal(lib.archiveOf("/u/src"), null);
  assert.equal(lib.archiveOf("/"), null);
});

test("passwordArchive extracts the archive path from password errors", () => {
  assert.equal(lib.passwordArchive("password required: /x/a.zip"), "/x/a.zip");
  assert.equal(lib.passwordArchive("wrong password: /x/my a.zip"), "/x/my a.zip");
  assert.equal(lib.passwordArchive("corrupt archive: x"), null);
  assert.equal(lib.passwordArchive(undefined), null);
});

test("uploadSucceeded: the one file really went into the archive", () => {
  assert.equal(lib.uploadSucceeded({ summary: { done: 1, skipped: 0, cancelled: false } }), true);
  assert.equal(lib.uploadSucceeded({ summary: { done: 0, skipped: 1, cancelled: false } }), false);
  assert.equal(lib.uploadSucceeded({ summary: { done: 0, skipped: 0, cancelled: false } }), false);
  assert.equal(lib.uploadSucceeded({ summary: { done: 1, skipped: 0, cancelled: true } }), false);
  assert.equal(lib.uploadSucceeded({ error: { kind: "io", message: "x" } }), false);
});

test("pending edits dialog keeps the copies unless Discard is confirmed", () => {
  const edits = [{ location: "/t/a.zip/x.txt", local: "/c/1/edit-0/x.txt" }];
  const d = lib.pendingEditsDialog(edits);
  assert.deepEqual(d.text, [
    "Edited copies from a previous session were not uploaded:",
    "/t/a.zip/x.txt → /c/1/edit-0/x.txt",
  ]);
  assert.deepEqual(
    d.buttons.map((b) => b.label),
    ["Show in Finder", "Keep", "Discard"],
  );
  const def = d.buttons.filter((b) => b.default);
  assert.deepEqual(def.map((b) => b.value), ["keep"]);
  const c = lib.discardPendingConfirm();
  assert.equal(c.text, "Move these copies to the Trash?");
  assert.deepEqual(c.buttons.map((b) => b.label), ["Yes", "No"]);
  // Enter (default) and Escape (null) both mean No.
  assert.deepEqual(c.buttons.filter((b) => b.default).map((b) => b.value), [null]);
});

test("isAudioName by extension, any case", () => {
  for (const n of ["a.wav", "CALL.WAV", "x.Mp3", "r.flac", "v.ogg", "v.oga", "o.opus", "m.m4a", "m.aac", "s.aif", "s.AIFF", "s.au", "t.gsm", "a.b.wav"]) {
    assert.equal(lib.isAudioName(n), true, n);
  }
  for (const n of ["wav", "noext", "a.wav.txt", "a.wma", ".wav", "", undefined, "a."]) {
    assert.equal(lib.isAudioName(n), false, String(n));
  }
});

test("formatClock is m:ss", () => {
  assert.equal(lib.formatClock(0), "0:00");
  assert.equal(lib.formatClock(59.4), "0:59");
  assert.equal(lib.formatClock(61), "1:01");
  assert.equal(lib.formatClock(3600), "60:00");
  assert.equal(lib.formatClock(NaN), "0:00");
  assert.equal(lib.formatClock(-3), "0:00");
});

test("codecSummary", () => {
  assert.equal(lib.codecSummary({ codec: "GSM 6.10", sampleRate: 8000, channels: 1 }), "GSM 6.10, 8000 Hz, mono");
  assert.equal(lib.codecSummary({ codec: "MP3", sampleRate: 44100, channels: 2 }), "MP3, 44100 Hz, stereo");
  assert.equal(lib.codecSummary({ codec: "FLAC", sampleRate: 48000, channels: 6 }), "FLAC, 48000 Hz, 6 channels");
  assert.equal(lib.codecSummary(null), "");
});

test("F3 on the playing file stops, on another file plays", () => {
  const playing = { state: "playing", path: "/r/a.wav" };
  assert.equal(lib.audioF3Action(playing, "/r/a.wav"), "stop");
  assert.equal(lib.audioF3Action(playing, "/r/b.wav"), "play");
  assert.equal(lib.audioF3Action({ state: "loading", path: "/r/a.wav" }, "/r/a.wav"), "stop");
  assert.equal(lib.audioF3Action({ state: "error", path: "/r/a.wav" }, "/r/a.wav"), "play");
  assert.equal(lib.audioF3Action({ state: null, path: null }, "/r/a.wav"), "play");
});

test("Escape stops audio only when nothing else uses it", () => {
  const player = { state: "playing" };
  assert.equal(lib.escapeStopsAudio({ player, escapeBound: false, commandText: "" }), true);
  assert.equal(lib.escapeStopsAudio({ player, escapeBound: false, commandText: "ls -l" }), false);
  assert.equal(lib.escapeStopsAudio({ player, escapeBound: true, commandText: "" }), false);
  assert.equal(lib.escapeStopsAudio({ player: { state: "ended" }, escapeBound: false, commandText: "" }), false);
  assert.equal(lib.escapeStopsAudio({ player: { state: "loading" }, escapeBound: false, commandText: "  " }), true);
});

test("playerStrip texts", () => {
  assert.equal(lib.playerStrip({ state: null }), null);
  assert.equal(lib.playerStrip({ state: "ended" }), null);
  assert.equal(lib.playerStrip({ state: "loading", name: "x.wav" }).text, "Loading x.wav…");
  const e = lib.playerStrip({ state: "error", message: "Cannot play: no audio output device" });
  assert.equal(e.text, "Cannot play: no audio output device");
  assert.equal(e.error, true);
  const s = lib.playerStrip({
    state: "playing", name: "c.wav", position: 61.5, duration: 123,
    info: { codec: "GSM 6.10", sampleRate: 8000, channels: 1 },
  });
  assert.deepEqual(s, { paused: false, text: "c.wav", clock: "1:01 / 2:03", codec: "GSM 6.10, 8000 Hz, mono", progress: 0.5 });
  assert.equal(lib.playerStrip({ state: "playing", name: "s", position: 3, duration: null }).clock, "0:03 / -:--");
});

test("seekSeconds clamps to the bar", () => {
  assert.equal(lib.seekSeconds(150, 100, 200, 60), 15);
  assert.equal(lib.seekSeconds(50, 100, 200, 60), 0);
  assert.equal(lib.seekSeconds(400, 100, 200, 60), 60);
  assert.equal(lib.seekSeconds(150, 100, 200, null), null);
});

test("late events of an older request are dropped", () => {
  const player = { minId: 7 };
  assert.equal(lib.acceptAudioEvent(player, { id: 6, state: "playing" }), false);
  assert.equal(lib.acceptAudioEvent(player, { id: 7, state: "playing" }), true);
  assert.equal(lib.acceptAudioEvent(player, { id: 9, state: "stopped" }), true);
  assert.equal(lib.acceptAudioEvent(player, { state: null }), true);
  assert.equal(lib.acceptAudioEvent({}, { id: 1 }), true);
});

// ---------------------------------------------------------------- drag & drop

const ddEntries = [
  { name: "..", isDir: true, up: true },
  { name: "docs", isDir: true },
  { name: "a.txt", isDir: false },
  { name: "b.txt", isDir: false },
];

test("dragSelection: a marked pressed row drags all marks, in listing order", () => {
  const marks = new Set(["b.txt", "docs"]);
  assert.deepEqual(lib.dragSelection(ddEntries, marks, "b.txt"), ["docs", "b.txt"]);
});

test("dragSelection: an unmarked pressed row drags only itself", () => {
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(["b.txt"]), "a.txt"), ["a.txt"]);
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(), "docs"), ["docs"]);
});

test("dragSelection: '..' is never dragged", () => {
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(["a.txt"]), ".."), []);
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(), null), []);
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(), "missing"), []);
  // A stale mark named ".." (never set by the panel) is still not dragged.
  assert.deepEqual(lib.dragSelection(ddEntries, new Set(["..", "a.txt"]), "a.txt"), ["a.txt"]);
});

test("dropTarget: the other panel's folder", () => {
  const r = lib.dropTarget({ panelLocation: "/b", rowEntry: null, draggedLocations: ["/a/x"], sourcePanelLocation: "/a" });
  assert.equal(r, "/b");
});

test("dropTarget: a folder row under the pointer", () => {
  const args = { panelLocation: "/b", draggedLocations: ["/a/x"], sourcePanelLocation: "/a" };
  assert.equal(lib.dropTarget({ ...args, rowEntry: { name: "sub", isDir: true } }), "/b/sub");
  assert.equal(lib.dropTarget({ ...args, panelLocation: "/", rowEntry: { name: "tmp", isDir: true } }), "/tmp");
});

test("dropTarget: a file row or '..' targets the panel's folder", () => {
  const args = { panelLocation: "/b", draggedLocations: ["/a/x"], sourcePanelLocation: "/a" };
  assert.equal(lib.dropTarget({ ...args, rowEntry: { name: "f.txt", isDir: false } }), "/b");
  assert.equal(lib.dropTarget({ ...args, rowEntry: { name: "..", isDir: true, up: true } }), "/b");
  assert.equal(lib.dropTarget({ ...args, rowEntry: { name: "x.zip", isDir: false } }), "/b");
});

test("dropTarget: same folder as the sources does nothing", () => {
  const args = { panelLocation: "/a", rowEntry: null, draggedLocations: ["/a/x", "/a/y"] };
  assert.equal(lib.dropTarget({ ...args, sourcePanelLocation: "/a" }), null);
  assert.equal(lib.dropTarget({ ...args, sourcePanelLocation: "/a/" }), null);
  // From another app (no source panel): the dragged files' own folder.
  assert.equal(lib.dropTarget({ ...args, sourcePanelLocation: null }), null);
  assert.equal(lib.dropTarget({ ...args, rowEntry: { name: "..", isDir: true, up: true }, sourcePanelLocation: "/a" }), null);
  assert.equal(
    lib.dropTarget({ panelLocation: "/", rowEntry: null, draggedLocations: ["/x"], sourcePanelLocation: "/" }),
    null,
  );
});

test("dropTarget: the same panel's folder row copies into that folder", () => {
  const r = lib.dropTarget({
    panelLocation: "/a", rowEntry: { name: "d", isDir: true }, draggedLocations: ["/a/x"], sourcePanelLocation: "/a",
  });
  assert.equal(r, "/a/d");
});

test("dropTarget: onto a dragged entry itself does nothing", () => {
  const r = lib.dropTarget({
    panelLocation: "/a", rowEntry: { name: "x", isDir: true }, draggedLocations: ["/a/x", "/a/y"], sourcePanelLocation: "/a",
  });
  assert.equal(r, null);
  // The other panel shows a dragged folder itself.
  assert.equal(lib.dropTarget({ panelLocation: "/a/x/", rowEntry: null, draggedLocations: ["/a/x"], sourcePanelLocation: "/a" }), null);
});

test("dropTarget: into a folder inside a dragged folder does nothing", () => {
  const args = { draggedLocations: ["/a/x"], sourcePanelLocation: "/a" };
  assert.equal(lib.dropTarget({ ...args, panelLocation: "/a/x/sub", rowEntry: null }), null);
  assert.equal(lib.dropTarget({ ...args, panelLocation: "/a/x", rowEntry: { name: "sub", isDir: true } }), null);
  // A sibling with the same prefix is not inside.
  assert.equal(lib.dropTarget({ ...args, panelLocation: "/a/xy", rowEntry: null }), "/a/xy");
});

test("dropTarget: archive and server locations", () => {
  assert.equal(
    lib.dropTarget({ panelLocation: "sftp://u@h/home", rowEntry: null, draggedLocations: ["/a/x"], sourcePanelLocation: "/a" }),
    "sftp://u@h/home",
  );
  assert.equal(
    lib.dropTarget({ panelLocation: "/b", rowEntry: null, draggedLocations: ["/z/p.zip/in/f"], sourcePanelLocation: "/z/p.zip/in" }),
    "/b",
  );
  assert.equal(
    lib.dropTarget({ panelLocation: "sftp://u@h/", rowEntry: null, draggedLocations: ["sftp://u@h/f"], sourcePanelLocation: "sftp://u@h/" }),
    null,
  );
});

test("commonParent: the dragged files' shared folder, else null", () => {
  assert.equal(lib.commonParent(["/a/x", "/a/y/"]), "/a");
  assert.equal(lib.commonParent(["/x"]), "/");
  assert.equal(lib.commonParent(["/a/x", "/b/y"]), null);
  assert.equal(lib.commonParent([]), null);
});

test("dragStarted: more than 5 px from the press", () => {
  assert.equal(lib.dragStarted({ x: 10, y: 10 }, { x: 15, y: 10 }), false);
  assert.equal(lib.dragStarted({ x: 10, y: 10 }, { x: 13, y: 14 }), false);
  assert.equal(lib.dragStarted({ x: 10, y: 10 }, { x: 16, y: 10 }), true);
  assert.equal(lib.dragStarted({ x: 10, y: 10 }, { x: 14, y: 14 }), true);
});

test("dragDropPoint: macOS reports points (CSS px), labelled physical", () => {
  assert.deepEqual(lib.dragDropPoint({ x: 300, y: 120 }, 2, true), { x: 300, y: 120 });
  assert.deepEqual(lib.dragDropPoint({ x: 600, y: 240 }, 2, false), { x: 300, y: 120 });
  assert.deepEqual(lib.dragDropPoint({ x: 600, y: 240 }, 0, false), { x: 600, y: 240 });
});

test("samePaths: order-insensitive set equality", () => {
  assert.equal(lib.samePaths(["/a", "/b"], ["/b", "/a"]), true);
  assert.equal(lib.samePaths(["/a"], ["/a", "/b"]), false);
  assert.equal(lib.samePaths([], []), false);
});

test("ownDropPaths: local drags by their paths, promised drags by carrying none", () => {
  const local = { paths: ["/a", "/b"], promised: false };
  assert.deepEqual(lib.ownDropPaths(local, ["/b", "/a"]), ["/a", "/b"]);
  assert.equal(lib.ownDropPaths(local, ["/a"]), null);
  assert.equal(lib.ownDropPaths(local, []), null);
  const promised = { paths: ["sftp://h/x", "sftp://h/y"], promised: true };
  assert.deepEqual(lib.ownDropPaths(promised, []), promised.paths);
  assert.equal(lib.ownDropPaths(promised, ["/Users/me/x"]), null);
  assert.equal(lib.ownDropPaths(null, []), null);
});

// ---------------------------------------------------------------- servers (Task 11)

test("serverDefaults: SSH 22, FTP 21, password login", () => {
  for (const p of ["sftp", "scp"]) assert.deepEqual(lib.serverDefaults(p), { port: 22, auth: "password" });
  for (const p of ["ftp", "ftps"]) assert.deepEqual(lib.serverDefaults(p), { port: 21, auth: "password" });
});

test("validateServer: the five messages of servers.rs validate", () => {
  const ok = { name: "web-1", host: "10.0.0.1", auth: "password", keyPath: null };
  assert.equal(lib.validateServer(ok, ["other"]), null);
  assert.equal(lib.validateServer({ ...ok, name: "  " }, []), "Name is required");
  assert.equal(lib.validateServer({ ...ok, name: "a/b" }, []), "Name cannot contain '/'");
  assert.equal(lib.validateServer(ok, ["x", "web-1"]), "Another server is already named 'web-1'");
  assert.equal(lib.validateServer({ ...ok, host: " " }, []), "Host is required");
  assert.equal(lib.validateServer({ ...ok, auth: "key" }, []), "Choose a key file");
  assert.equal(lib.validateServer({ ...ok, auth: "keyPassphrase", keyPath: " " }, []), "Choose a key file");
  assert.equal(lib.validateServer({ ...ok, auth: "key", keyPath: "keys/id" }, []), null);
});

test("fieldsFor: SSH has auth/key/legacy, FTP has passive/encoding", () => {
  const common = ["name", "protocol", "host", "port", "user", "savePassword", "remoteDir", "localDir"];
  for (const p of ["sftp", "scp"]) {
    const f = lib.fieldsFor(p);
    for (const k of [...common, "auth", "keyPath", "legacyAlgorithms"]) assert.ok(f.has(k), `${p} ${k}`);
    assert.ok(!f.has("ftpPassive") && !f.has("ftpEncoding"), p);
  }
  for (const p of ["ftp", "ftps"]) {
    const f = lib.fieldsFor(p);
    for (const k of [...common, "ftpPassive", "ftpEncoding"]) assert.ok(f.has(k), `${p} ${k}`);
    assert.ok(!f.has("auth") && !f.has("keyPath") && !f.has("legacyAlgorithms"), p);
  }
});

test("connectionLabel: cloud and name", () => {
  assert.equal(lib.connectionLabel({ name: "web-1", display: "sftp://zb@h", root: "sftp://web-1/" }), "☁️ web-1");
});

test("hostKeyText: new and changed key, exact text", () => {
  assert.equal(
    lib.hostKeyText({ kind: "hostKey", host: "h", port: 22, keyType: "ssh-ed25519", fingerprint: "SHA256:abc", changedFrom: null }),
    "The authenticity of host 'h:22' can't be established.\nssh-ed25519 key fingerprint is SHA256:abc.\nConnect and remember this key?",
  );
  assert.equal(
    lib.hostKeyText({ kind: "hostKey", host: "h", port: 2222, keyType: "ssh-rsa", fingerprint: "SHA256:new", changedFrom: "SHA256:old" }),
    "WARNING: THE HOST KEY FOR 'h:2222' HAS CHANGED!\nSomeone may be intercepting the connection, or the server was reinstalled.\nOld: SHA256:old\nNew: SHA256:new\nReplace the stored key and connect?",
  );
});

test("portAfterProtocolChange: only a default port follows the protocol", () => {
  assert.equal(lib.portAfterProtocolChange("sftp", "ftp", 22), 21);
  assert.equal(lib.portAfterProtocolChange("ftp", "ftps", 21), 21);
  assert.equal(lib.portAfterProtocolChange("ftps", "scp", 21), 22);
  assert.equal(lib.portAfterProtocolChange("sftp", "ftp", 2222), 2222);
  assert.equal(lib.portAfterProtocolChange("ftp", "sftp", "22"), 22);
});

test("parseRemote: same rules as parse_remote", () => {
  assert.deepEqual(lib.parseRemote("sftp://m/var/www"), { scheme: "sftp", server: "m", path: "/var/www" });
  assert.deepEqual(lib.parseRemote("sftp://m"), { scheme: "sftp", server: "m", path: "/" });
  assert.deepEqual(lib.parseRemote("ftp://a b/x/"), { scheme: "ftp", server: "a b", path: "/x" });
  assert.equal(lib.parseRemote("http://x/y"), null);
  assert.equal(lib.parseRemote("/Users/x"), null);
  assert.equal(lib.parseRemote("sftp:///x"), null);
});

test("duplicateName: '<name> (2)', next free number", () => {
  assert.equal(lib.duplicateName("web-1", ["web-1"]), "web-1 (2)");
  assert.equal(lib.duplicateName("web-1", ["web-1", "web-1 (2)", "web-1 (3)"]), "web-1 (4)");
});

test("serverNames lists every server in every folder", () => {
  const nodes = [
    { kind: "server", name: "a" },
    { kind: "folder", name: "F", children: [{ kind: "server", name: "b" }, { kind: "folder", name: "G", children: [] }] },
  ];
  assert.deepEqual(lib.serverNames(nodes), ["a", "b"]);
});

test("tree helpers treat server folders like hotlist submenus", () => {
  const mk = () => [
    { kind: "server", name: "a" },
    { kind: "folder", name: "F", children: [{ kind: "server", name: "b" }] },
    { kind: "server", name: "d" },
  ];
  const n = mk();
  assert.deepEqual(lib.relocateNode(n, [0], [1]), [0, 1]);
  assert.deepEqual(n[0].children.map((x) => x.name), ["b", "a"]);
  assert.deepEqual(lib.flattenNodes(mk()).map((r) => [r.path.join("."), r.depth]), [["0", 0], ["1", 0], ["1.0", 1], ["2", 0]]);
});

test("treeRows hides the children of closed folders", () => {
  const nodes = [
    { kind: "folder", name: "F", children: [{ kind: "server", name: "b" }, { kind: "folder", name: "G", children: [{ kind: "server", name: "c" }] }] },
    { kind: "server", name: "d" },
  ];
  const all = lib.treeRows(nodes, () => true).map((r) => r.node.name);
  assert.deepEqual(all, ["F", "b", "G", "c", "d"]);
  const shut = lib.treeRows(nodes, (node) => node.name !== "G").map((r) => r.node.name);
  assert.deepEqual(shut, ["F", "b", "G", "d"]);
  assert.deepEqual(lib.treeRows(nodes, () => false).map((r) => r.node.name), ["F", "d"]);
});

test("folderKey: folder names from the root, stable across reloads", () => {
  const nodes = [{ kind: "folder", name: "F", children: [{ kind: "folder", name: "G", children: [] }] }];
  assert.equal(lib.folderKey(nodes, [0, 0]), "F/G");
  assert.equal(lib.folderKey(nodes, [0]), "F");
});

test("storageMasterPrompt: which master password a storage switch needs", () => {
  const st = (exists, unlocked) => ({ mode: "x", exists, unlocked });
  // To Portable: always a new master password (a new password file is created).
  assert.equal(lib.storageMasterPrompt("system", "portable", st(false, false)), "new");
  assert.equal(lib.storageMasterPrompt("system", "portable", st(true, true)), "new");
  // To System: the current one only when the portable password file is locked.
  assert.equal(lib.storageMasterPrompt("portable", "system", st(true, false)), "current");
  assert.equal(lib.storageMasterPrompt("portable", "system", st(true, true)), "none");
  assert.equal(lib.storageMasterPrompt("portable", "system", st(false, false)), "none");
  // No switch, no prompt.
  assert.equal(lib.storageMasterPrompt("system", "system", st(true, false)), "none");
  assert.equal(lib.storageMasterPrompt("portable", "portable", st(false, false)), "none");
});

test("remoteLinkIsFolder: only the server folder refusal", () => {
  assert.equal(lib.remoteLinkIsFolder("Only files can be opened from a server"), true);
  assert.equal(lib.remoteLinkIsFolder(lib.REMOTE_FOLDER_REFUSED), true);
  assert.equal(lib.remoteLinkIsFolder("Only files can be opened from an archive"), false);
  assert.equal(lib.remoteLinkIsFolder("not found: /x"), false);
  assert.equal(lib.remoteLinkIsFolder("connection lost"), false);
  assert.equal(lib.remoteLinkIsFolder(undefined), false);
});

test("masterPartial: unlocked, but some passwords not saved", () => {
  assert.equal(lib.masterPartial("Unlocked, but 2 password(s) could not be saved"), true);
  assert.equal(lib.masterPartial("Master password set, but 1 password(s) could not be saved"), true);
  assert.equal(lib.masterPartial("Wrong master password"), false);
});

test("newServerEntry: SFTP defaults with the given id", () => {
  assert.deepEqual(lib.newServerEntry("id1"), {
    kind: "server", id: "id1", name: "", protocol: "sftp", host: "", port: 22, user: "",
    auth: "password", keyPath: null, savePassword: false, remoteDir: null, localDir: null,
    legacyAlgorithms: false, ftpPassive: true, ftpEncoding: "utf8", share: "",
  });
});

test("SMB: port 445, share required, only the SMB fields", () => {
  assert.deepEqual(lib.serverDefaults("smb"), { port: 445, auth: "password" });
  assert.equal(lib.portAfterProtocolChange("sftp", "smb", "22"), 445);
  assert.equal(lib.portAfterProtocolChange("smb", "ftp", "445"), 21);
  const ok = { name: "nas", host: "nas.local", protocol: "smb", share: "data", auth: "password", keyPath: null };
  assert.equal(lib.validateServer(ok, []), null);
  assert.equal(lib.validateServer({ ...ok, share: " " }, []), "Share is required");
  assert.equal(lib.validateServer({ ...ok, share: undefined }, []), "Share is required");
  assert.equal(lib.validateServer({ ...ok, share: "a/b" }, []), "Share cannot contain '/'");
  // A key login left over from SFTP does not matter for SMB.
  assert.equal(lib.validateServer({ ...ok, auth: "key" }, []), null);
  const f = lib.fieldsFor("smb");
  for (const k of ["name", "protocol", "host", "port", "user", "share", "kerberos", "savePassword", "remoteDir", "autoConnect"]) assert.ok(f.has(k), k);
  assert.ok(!lib.fieldsFor("sftp").has("kerberos"));
  for (const k of ["auth", "keyPath", "legacyAlgorithms", "ftpPassive", "ftpEncoding", "localDir"]) assert.ok(!f.has(k), k);
  assert.ok(!lib.fieldsFor("sftp").has("share"));
  assert.ok(!lib.fieldsFor("sftp").has("autoConnect"));
});

test("mergeShares: a mounted share replaces its /Volumes row, in the manager's order", () => {
  const vols = [
    { name: "Macintosh HD", path: "/", kind: "root", total: 100, free: 50 },
    { name: "~", path: "/Users/zb", kind: "home", total: 100, free: 50 },
    { name: "Data", path: "/Volumes/Data", kind: "network", total: 9, free: 3 },
    { name: "USB", path: "/Volumes/USB", kind: "volume", total: 8, free: 1 },
  ];
  const shares = [
    { name: "office", mount: null, total: 0, free: 0 },
    { name: "nas data", mount: "/Volumes/Data", total: 9, free: 3 },
  ];
  const m = lib.mergeShares(vols, shares);
  assert.deepEqual(
    m.map((v) => v.name),
    ["Macintosh HD", "~", "USB", "office", "nas data"],
  );
  assert.deepEqual(m[3], {
    name: "office", path: "smb://office/", kind: "network", total: 0, free: 0, smb: "office", mounted: false,
  });
  assert.deepEqual(m[4], {
    name: "nas data", path: "/Volumes/Data", kind: "network", total: 9, free: 3, smb: "nas data", mounted: true,
  });
  // No shares (no SMB entries, as SFTP/FTP servers are never in `smb_shares`): volumes as they are.
  assert.deepEqual(lib.mergeShares(vols, []), vols);
  // An unmounted share is never a disk of a local folder.
  assert.equal(lib.diskFor(m, "/Users/zb/x").path, "/");
  // A mounted share is the disk of its folders, so picking it again goes where it was.
  assert.equal(lib.diskFor(m, "/Volumes/Data/docs").smb, "nas data");
});

test("urlDisplayName: userinfo ends at the last @ as in parse_url", () => {
  assert.equal(lib.urlDisplayName("sftp://zb:2024/x@h"), "zb@h");
  assert.equal(lib.urlDisplayName("sftp://zb:pa/55@host"), "zb@host");
  assert.equal(lib.urlDisplayName("sftp://zb:p@ss/w0rd@h:2222/srv"), "zb@h:2222");
  assert.equal(lib.urlDisplayName("sftp://zb:22@h"), "zb@h");
  // No possible password: an @ may stay in the path.
  assert.equal(lib.urlDisplayName("sftp://zb@h/home/a@b"), "zb@h");
  assert.equal(lib.urlDisplayName("sftp://zb@h:2222/a@b"), "zb@h:2222");
  assert.equal(lib.urlDisplayName("sftp://h/a@b"), "anonymous@h");
  assert.equal(lib.urlDisplayName("sftp://[::1]:2222/a@b"), "anonymous@::1:2222");
});

test("urlDisplayName: user@host[:port], never the URL password or path", () => {
  assert.equal(lib.urlDisplayName("sftp://zb:secret@web-1/var/www"), "zb@web-1");
  assert.equal(lib.urlDisplayName("sftp://zb@web-1:2222/var/www"), "zb@web-1:2222");
  assert.equal(lib.urlDisplayName("sftp://zb@web-1:22"), "zb@web-1");
  assert.equal(lib.urlDisplayName("ftp://h"), "anonymous@h");
  assert.equal(lib.urlDisplayName("FTPS://u:p@h:21/x"), "u@h");
  assert.equal(lib.urlDisplayName("ftp://u:pw@h:990"), "u@h:990");
  assert.equal(lib.urlDisplayName("sftp://u@[::1]:2222/x"), "u@::1:2222");
  assert.equal(lib.urlDisplayName("sftp://u:pw@[fe80::1]"), "u@fe80::1");
  assert.equal(lib.urlDisplayName("sftp://j%20d@h"), "j d@h");
  for (const bad of ["", "garbage", "http://u:pw@h", "sftp://u:pw@", "sftp://u:pw@h:notaport", "sftp://u:pw@h:99999", "u:pw@h"]) {
    const shown = lib.urlDisplayName(bad);
    assert.equal(typeof shown, "string", bad);
    assert.ok(!shown.includes("pw"), `${bad} -> ${shown}`);
  }
});

test("secretChanges: what the editor stores or deletes after OK", () => {
  const e = (auth, savePassword) => ({ id: "i", auth, savePassword });
  // Typed secret with Save password on.
  assert.deepEqual(lib.secretChanges(null, e("password", true), "s"), [["password", "s"]]);
  assert.deepEqual(lib.secretChanges(null, e("keyPassphrase", true), "s"), [["passphrase", "s"]]);
  // Empty field keeps the stored one; nothing stored before and nothing typed: no calls.
  assert.deepEqual(lib.secretChanges(e("password", true), e("password", true), ""), []);
  assert.deepEqual(lib.secretChanges(e("password", false), e("password", false), ""), []);
  // Save password turned off: both kinds go.
  assert.deepEqual(lib.secretChanges(e("password", true), e("password", false), ""), [["password", null], ["passphrase", null]]);
  assert.deepEqual(lib.secretChanges(e("keyPassphrase", true), e("key", false), ""), [["password", null], ["passphrase", null]]);
  // Auth switched with Save password on: the kind that no longer applies goes.
  assert.deepEqual(lib.secretChanges(e("password", true), e("keyPassphrase", true), "s"), [["passphrase", "s"], ["password", null]]);
  assert.deepEqual(lib.secretChanges(e("keyPassphrase", true), e("password", true), ""), [["passphrase", null]]);
});

// ---------------------------------------------------------------- servers in the panels (Task 10)

test("parseRemote: root, nested, ftps and scp", () => {
  assert.deepEqual(lib.parseRemote("sftp://m/"), { scheme: "sftp", server: "m", path: "/" });
  assert.deepEqual(lib.parseRemote("ftps://u@h:2121/a//"), { scheme: "ftps", server: "u@h:2121", path: "/a" });
  assert.deepEqual(lib.parseRemote("scp://m/a/b c"), { scheme: "scp", server: "m", path: "/a/b c" });
  assert.equal(lib.parseRemote("SFTP://m/"), null);
  assert.equal(lib.parseRemote("sftp:/m/"), null);
});

test("remoteRoot: the server's root, null for local paths", () => {
  assert.equal(lib.remoteRoot("sftp://m/var/www"), "sftp://m/");
  assert.equal(lib.remoteRoot("sftp://m"), "sftp://m/");
  assert.equal(lib.remoteRoot("ftp://u@h:2121/x"), "ftp://u@h:2121/");
  assert.equal(lib.remoteRoot("/Users/x"), null);
});

test("parentLocation on a server: up to the root, then null", () => {
  assert.equal(lib.parentLocation("sftp://m/var/www"), "sftp://m/var");
  assert.equal(lib.parentLocation("sftp://m/var"), "sftp://m/");
  assert.equal(lib.parentLocation("sftp://m/var/"), "sftp://m/");
  assert.equal(lib.parentLocation("sftp://m/"), null);
  assert.equal(lib.parentLocation("sftp://m"), null);
  // Repeated slashes collapse, so the result is canonical.
  assert.equal(lib.parentLocation("sftp://m/a//b"), "sftp://m/a");
  assert.equal(lib.parentLocation("sftp://m//a"), "sftp://m/");
  // Local paths are unchanged.
  assert.equal(lib.parentLocation("/a"), "/");
  assert.equal(lib.parentLocation("/"), "/");
});

test("joinLocation / lastComponent on a server", () => {
  assert.equal(lib.joinLocation("sftp://m/", "x"), "sftp://m/x");
  assert.equal(lib.joinLocation("sftp://m/var", "x"), "sftp://m/var/x");
  assert.equal(lib.lastComponent("sftp://m/var/x"), "x");
  assert.equal(lib.lastComponent("sftp://m/"), "");
});

test("navigate keeps a server root as 'scheme://name/'", async () => {
  const r = await lib.navigate({ location: "sftp://m/var" }, "sftp://m/", async () => []);
  assert.equal(r.location, "sftp://m/");
  assert.equal(r.cursorName, "var");
  const r2 = await lib.navigate({ location: "" }, "sftp://m/var/", async () => []);
  assert.equal(r2.location, "sftp://m/var");
});

test("connectionLost: the two prefixes, also after a location", () => {
  assert.equal(lib.connectionLost("connection lost"), true);
  assert.equal(lib.connectionLost("server not responding"), true);
  assert.equal(lib.connectionLost("sftp://m/var: connection lost"), true);
  assert.equal(lib.connectionLost("sftp://m/var: server not responding"), true);
  assert.equal(lib.connectionLost("Connection refused"), false);
  assert.equal(lib.connectionLost("not found: /x"), false);
  assert.equal(lib.connectionLost("/a/connection lost.txt: permission denied"), false);
  assert.equal(lib.connectionLost(""), false);
  assert.equal(lib.connectionLost(undefined), false);
});

test("deleteConfirmText: server text, else the existing texts", () => {
  assert.equal(lib.deleteConfirmText("sftp://m/var", 3), "Delete 3 file(s) from the server? This cannot be undone.");
  assert.equal(
    lib.deleteConfirmText("ftp://m/", 1, { toTrash: true }),
    "Delete 1 file(s) from the server? This cannot be undone.",
  );
  assert.equal(
    lib.deleteConfirmText("/a/x.zip", 2, { inArchive: true, toTrash: true }),
    "Delete 2 file(s) from archive? This cannot be undone.",
  );
  assert.equal(lib.deleteConfirmText("/a", 2, { toTrash: true }), "Do you really want to move 2 file(s)/dir(s) to the Trash?");
  assert.equal(lib.deleteConfirmText("/a", 1, { toTrash: false }), "Do you really want to delete 1 file(s)/dir(s) permanently?");
});

test("deleteConfirmText: a single item is named", () => {
  assert.equal(lib.deleteConfirmText("/a", 1, { toTrash: true, name: "b.txt" }), 'Do you really want to move "b.txt" to the Trash?');
  assert.equal(lib.deleteConfirmText("sftp://m/", 1, { name: "x" }), 'Delete "x" from the server? This cannot be undone.');
  assert.equal(lib.deleteConfirmText("/a/x.zip", 1, { inArchive: true, name: "y" }), 'Delete "y" from archive? This cannot be undone.');
  assert.equal(lib.deleteConfirmText("/a", 2, { toTrash: true, name: "ignored" }), "Do you really want to move 2 file(s)/dir(s) to the Trash?");
});

test("lostToShow: drops names closed on purpose (until the next connect), and duplicates", () => {
  const closed = new Set(["a"]);
  assert.deepEqual(lib.lostToShow(["a", "b", "c", "c"], closed), ["b", "c"]);
  assert.deepEqual(lib.lostToShow(["a"], new Set()), ["a"]);
  assert.deepEqual(lib.lostToShow([], closed), []);
});

test("editTexts: server or archive wording of the F4 upload questions", () => {
  assert.deepEqual(lib.editTexts("sftp://m/etc/hosts", "hosts"), {
    upload: '"hosts" was changed. Upload it back to the server?',
    conflict: '"hosts" was changed on the server since you opened it. Overwrite it with your edited copy?',
  });
  assert.deepEqual(lib.editTexts("/a/x.zip/f.txt", "f.txt"), {
    upload: '"f.txt" was changed. Upload it back into the archive?',
    conflict: '"f.txt" was changed in the archive since you opened it. Overwrite it with your edited copy?',
  });
});

test("modeOctal / parseModeOctal: 3 digits, 4 with special bits", () => {
  assert.equal(lib.modeOctal(0o644), "644");
  assert.equal(lib.modeOctal(0o100755), "755");
  assert.equal(lib.modeOctal(0o4755), "4755");
  assert.equal(lib.modeOctal(0o7), "007");
  assert.equal(lib.parseModeOctal("644"), 0o644);
  assert.equal(lib.parseModeOctal(" 0755 "), 0o755);
  assert.equal(lib.parseModeOctal("4755"), 0o4755);
  assert.equal(lib.parseModeOctal("7"), 0o7);
  assert.equal(lib.parseModeOctal(""), null);
  assert.equal(lib.parseModeOctal("8"), null);
  assert.equal(lib.parseModeOctal("12345"), null);
  assert.equal(lib.parseModeOctal("rwx"), null);
});

test("modeBits / withModeBit: the nine rwx boxes, special bits kept", () => {
  assert.deepEqual(lib.modeBits(0o750), [true, true, true, true, false, true, false, false, false]);
  assert.equal(lib.withModeBit(0o644, 2, true), 0o744);
  assert.equal(lib.withModeBit(0o644, 1, false), 0o444);
  assert.equal(lib.withModeBit(0o4644, 8, true), 0o4645);
  assert.equal(lib.withModeBit(0o644, 3, false), 0o604);
});

test("chmodStart: the entry's mode, else 644 for files and 755 for folders", () => {
  assert.equal(lib.chmodStart({ name: "a", isDir: false, mode: 0o100600 }), 0o600);
  assert.equal(lib.chmodStart({ name: "d", isDir: true, mode: 0o40750 }), 0o750);
  assert.equal(lib.chmodStart({ name: "a", isDir: false, mode: 0 }), 0o644);
  assert.equal(lib.chmodStart({ name: "d", isDir: true, mode: 0 }), 0o755);
  assert.equal(lib.chmodStart({ name: "d", isDir: true }), 0o755);
});

test("onRoot: path on the connection's root (its scheme wins)", () => {
  assert.equal(lib.onRoot("ftp://m/", "sftp://m/var/www"), "ftp://m/var/www");
  assert.equal(lib.onRoot("sftp://m/", "sftp://m"), "sftp://m/");
  assert.equal(lib.onRoot("sftp://m/", "sftp://m/a/"), "sftp://m/a");
});

test("jobServers: unique server names of a job's locations", () => {
  assert.deepEqual(lib.jobServers(["sftp://a/x", "sftp://a/y", "/local", null, undefined, "ftp://b/", "sub"]), ["a", "b"]);
  assert.deepEqual(lib.jobServers(["/a", "/b"]), []);
});

test("jobErrorBox: a lost connection gets no box only when every server was connected at start", () => {
  const lost = { kind: "io", message: "connection lost" };
  // All connected at start: the backend's lost event names the server, so no box.
  assert.equal(lib.jobErrorBox(lost, ["a", "b"], ["a", "b", "c"]), false);
  assert.equal(lib.jobErrorBox({ kind: "io", message: "server not responding" }, ["a"], ["a"]), false);
  // One not connected: no event will come, so the box shows.
  assert.equal(lib.jobErrorBox(lost, ["a", "b"], ["a"]), true);
  // No servers at all and a lost-looking message: the box shows.
  assert.equal(lib.jobErrorBox(lost, [], []), true);
  assert.equal(lib.jobErrorBox(null, [], []), false);
  assert.equal(lib.jobErrorBox({ kind: "cancelled", message: "cancelled" }, [], []), false);
  assert.equal(lib.jobErrorBox({ kind: "io", message: "permission denied: /x" }, ["a"], ["a"]), true);
});

test("chmodSummary: null when all changed, else 'Changed N of M' and one line per failure", () => {
  assert.equal(lib.chmodSummary([{ name: "a", error: null }, { name: "b", error: null }]), null);
  assert.deepEqual(
    lib.chmodSummary([
      { name: "a", error: null },
      { name: "b", error: "permission denied: /b" },
      { name: "c", error: "This server does not support changing permissions" },
    ]),
    ["Changed 1 of 3", "b: permission denied: /b", "c: This server does not support changing permissions"],
  );
});

test("CHMOD_NOT_IN_ARCHIVE matches the backend text", () => {
  // Same as `CHMOD_NOT_IN_ARCHIVE` in src-tauri/src/remote.rs; change both together.
  assert.equal(lib.CHMOD_NOT_IN_ARCHIVE, "Change attributes does not work inside archives");
});

test("remote_archive_parent_returns_to_server", () => {
  const m = new Map([["/tmp/edit-1/a.zip", "sftp://srv/var/www"]]);
  // Root of the temp copy: back to the folder on the server.
  assert.equal(lib.archiveParent("/tmp/edit-1/a.zip", m), "sftp://srv/var/www");
  assert.equal(lib.archiveParent("/tmp/edit-1/a.zip/", m), "sftp://srv/var/www");
  // Inside the archive and elsewhere: the normal parent applies.
  assert.equal(lib.archiveParent("/tmp/edit-1/a.zip/src", m), null);
  assert.equal(lib.archiveParent("/tmp/edit-1", m), null);
  assert.equal(lib.archiveParent("/tmp/edit-1/a.zip.bak", m), null);
  assert.equal(lib.archiveParent("/u/b.zip", m), null);
  assert.equal(lib.archiveParent("/tmp/edit-1/a.zip", new Map()), null);
});

test("remoteArchiveOf and displayLocation: temp copy of a server archive", () => {
  const m = new Map([["/tmp/edit-1/a.zip", "sftp://srv/var/www"]]);
  assert.deepEqual(lib.remoteArchiveOf("/tmp/edit-1/a.zip/src/x", m), {
    archive: "/tmp/edit-1/a.zip",
    parent: "sftp://srv/var/www",
  });
  assert.equal(lib.remoteArchiveOf("/tmp/edit-1/a.zip.bak", m), null);
  assert.equal(lib.displayLocation("/tmp/edit-1/a.zip/src/x", m), "sftp://srv/var/www/a.zip/src/x");
  assert.equal(lib.displayLocation("/tmp/edit-1/a.zip", m), "sftp://srv/var/www/a.zip");
  assert.equal(lib.displayLocation("/u/b", m), "/u/b");
});

test("archiveLeave: server folder with the cursor on the archive", () => {
  const m = new Map([["/tmp/edit-1/a.zip", "sftp://srv/var/www"]]);
  assert.deepEqual(lib.archiveLeave("/tmp/edit-1/a.zip", m), { location: "sftp://srv/var/www", cursor: "a.zip" });
  assert.equal(lib.archiveLeave("/tmp/edit-1/a.zip/src", m), null);
});

test("outputOptions: system default first, then the devices in order", () => {
  const def = { value: "", label: "System default" };
  assert.deepEqual(lib.outputOptions([], null), [def]);
  assert.deepEqual(lib.outputOptions(undefined, ""), [def]);
  assert.deepEqual(lib.outputOptions(["Speakers", "USB DAC"], "USB DAC"), [
    def,
    { value: "Speakers", label: "Speakers" },
    { value: "USB DAC", label: "USB DAC" },
  ]);
  // Saved empty = system default, nothing extra.
  assert.deepEqual(lib.outputOptions(["Speakers"], ""), [def, { value: "Speakers", label: "Speakers" }]);
});

test("outputOptions tells devices with one name apart by id", () => {
  const devs = [
    { name: "DAC", id: "coreaudio:a" },
    { name: "Speakers", id: "coreaudio:s" },
    { name: "DAC", id: "coreaudio:b" },
  ];
  assert.deepEqual(lib.outputOptions(devs, "coreaudio:b"), [
    { value: "", label: "System default" },
    { value: "coreaudio:a", label: "DAC (1)" },
    { value: "Speakers", label: "Speakers" },
    { value: "coreaudio:b", label: "DAC (2)" },
  ]);
  // Saved by name before the second one connected: kept, it means the first.
  assert.deepEqual(lib.outputOptions(devs, "DAC").at(-1), { value: "DAC", label: "DAC (first)" });
});
test("outputOptions keeps a saved device that is not connected", () => {
  assert.deepEqual(lib.outputOptions(["Speakers"], "Headphones"), [
    { value: "", label: "System default" },
    { value: "Speakers", label: "Speakers" },
    { value: "Headphones", label: "Headphones (not connected)" },
  ]);
  assert.deepEqual(lib.outputOptions([], "Headphones"), [
    { value: "", label: "System default" },
    { value: "Headphones", label: "Headphones (not connected)" },
  ]);
});

test("outputOptions lists a name once (and never an empty one)", () => {
  assert.deepEqual(lib.outputOptions(["DAC", "Speakers", "DAC", ""], "DAC"), [
    { value: "", label: "System default" },
    { value: "DAC", label: "DAC" },
    { value: "Speakers", label: "Speakers" },
  ]);
});

test("playerStrip shows a playing note in the codec line", () => {
  const info = { codec: "PCM 16-bit", sampleRate: 8000, channels: 1 };
  const note = "Output: Headphones is not connected, using the system default";
  const p = { state: "playing", name: "a.wav", position: 1, duration: 2, info };
  assert.equal(lib.playerStrip(p).codec, "PCM 16-bit, 8000 Hz, mono");
  assert.equal(lib.playerStrip({ ...p, message: note }).codec, note);
});

test("packArchiveName: one entry, several entries, root", () => {
  const f = (name) => ({ name, isDir: false });
  const d = (name) => ({ name, isDir: true });
  assert.equal(lib.packArchiveName([f("report.pdf")], "/Users/x"), "report.zip");
  assert.equal(lib.packArchiveName([f("archive.tar.gz")], "/Users/x"), "archive.tar.zip");
  assert.equal(lib.packArchiveName([f("README")], "/Users/x"), "README.zip");
  assert.equal(lib.packArchiveName([f(".config")], "/Users/x"), ".config.zip");
  assert.equal(lib.packArchiveName([f(".bash.rc")], "/Users/x"), ".bash.zip");
  assert.equal(lib.packArchiveName([d("site")], "/Users/x"), "site.zip");
  assert.equal(lib.packArchiveName([d("v1.2")], "/Users/x"), "v1.2.zip");
  const three = [f("a.txt"), d("b"), f("c.md")];
  assert.equal(lib.packArchiveName(three, "/Users/x/Projects/web"), "web.zip");
  assert.equal(lib.packArchiveName(three, "/Users/x/Projects/web/"), "web.zip");
  assert.equal(lib.packArchiveName(three, "/"), "archive.zip");
});

test("packTargetDir: the other panel, also on a server or in an archive", () => {
  assert.equal(lib.packTargetDir("/a", "/b"), "/b");
  assert.equal(lib.packTargetDir("/a", "sftp://web/var"), "sftp://web/var");
  assert.equal(lib.packTargetDir("/a", "/b/x.zip/inner"), "/b/x.zip/inner");
  assert.equal(lib.packTargetDir(lib.SEARCH_LOCATION, lib.SEARCH_LOCATION, "/c"), "/c");
  assert.equal(lib.isLocalFolder("/b"), true);
  assert.equal(lib.isLocalFolder("/b/x.tar"), false);
  assert.equal(lib.isLocalFolder("ftp://h/"), false);
});

test("packDialogText and the cursor before .zip", () => {
  assert.equal(lib.packDialogText([{ name: "site" }]), 'Pack "site" to the archive:');
  assert.equal(lib.packDialogText([{ name: "a" }, { name: "b" }]), "Pack 2 file(s) to the archive:");
  assert.deepEqual(lib.packCursor("/b/site.zip"), { start: 7, end: 7 });
  assert.deepEqual(lib.packCursor("/b/site"), { start: 7, end: 7 });
});

test("packFormError", () => {
  const ok = { target: "/b/a.zip", encrypt: false, password: "", repeat: "" };
  assert.equal(lib.packFormError(ok), null);
  assert.equal(lib.packFormError({ ...ok, target: "  " }), "Enter the archive name.");
  assert.equal(lib.packFormError({ ...ok, encrypt: true }), "Enter a password.");
  assert.equal(lib.packFormError({ ...ok, encrypt: true, password: "x", repeat: "y" }), "The passwords do not match.");
  assert.equal(lib.packFormError({ ...ok, encrypt: true, password: "x", repeat: "x" }), null);
});

test("packedText", () => {
  assert.equal(lib.packedText({ done: 3 }, "/b/web.zip"), "Packed 3 file(s) into web.zip");
});

// ---------------------------------------------------------------- modifier mapping

const ev = (code, mods = {}) => ({ code, key: "", ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });
const CMD_CTRL = { alt: "option", ctrl: "command" };
const SWAPPED = { alt: "control", ctrl: "option" };

test("modifierMap: defaults, saved choice, unusable values", () => {
  assert.deepEqual(lib.modifierMap({}), { alt: "option", ctrl: "control" });
  assert.deepEqual(lib.modifierMap(null), { alt: "option", ctrl: "control" });
  assert.deepEqual(lib.modifierMap({ modifiers: { alt: "option", ctrl: "command" } }), CMD_CTRL);
  assert.deepEqual(lib.modifierMap({ modifiers: { alt: "hyper", ctrl: "command" } }), CMD_CTRL);
  assert.deepEqual(lib.modifierMap({ modifiers: { alt: "command", ctrl: "command" } }), { alt: "option", ctrl: "control" });
});

test("comboFromEvent with the default map is the physical TC mapping", () => {
  assert.equal(lib.comboFromEvent(ev("F5", { altKey: true })), "Alt+F5");
  assert.equal(lib.comboFromEvent(ev("KeyD", { ctrlKey: true }), lib.DEFAULT_MODIFIERS), "Ctrl+D");
  assert.equal(lib.comboFromEvent(ev("KeyD", { metaKey: true })), "Cmd+D");
  assert.equal(lib.comboFromEvent(ev("KeyQ", { metaKey: true })), "Cmd+Q");
});

test("comboFromEvent: Command as Ctrl", () => {
  assert.equal(lib.comboFromEvent(ev("KeyD", { metaKey: true }), CMD_CTRL), "Ctrl+D");
  assert.equal(lib.comboFromEvent(ev("F5", { altKey: true }), CMD_CTRL), "Alt+F5");
  // Command no longer gives Cmd+, and the unmapped Control gives no command at all.
  assert.equal(lib.comboFromEvent(ev("KeyD", { ctrlKey: true }), CMD_CTRL), null);
  assert.equal(lib.comboFromEvent(ev("F5"), CMD_CTRL), "F5");
  assert.equal(lib.comboFromEvent(ev("F6", { shiftKey: true }), CMD_CTRL), "Shift+F6");
  // Keys the native menu owns stay with it (Quit, Hide, Hide Others) ...
  for (const code of ["KeyQ", "KeyH"]) {
    assert.equal(lib.comboFromEvent(ev(code, { metaKey: true }), CMD_CTRL), null, code);
  }
  assert.equal(lib.comboFromEvent(ev("KeyH", { metaKey: true, altKey: true }), CMD_CTRL), null);
  // ... the rest are TC keys (the menu has no Close or Minimize).
  assert.equal(lib.comboFromEvent(ev("KeyW", { metaKey: true }), CMD_CTRL), "Ctrl+W");
  assert.equal(lib.comboFromEvent(ev("KeyM", { metaKey: true }), CMD_CTRL), "Ctrl+M");
  assert.equal(lib.comboFromEvent(ev("KeyW", { metaKey: true }), { alt: "command", ctrl: "control" }), "Alt+W");
});

test("captureRefusal explains keys that give no combo", () => {
  assert.equal(lib.captureRefusal(ev("KeyQ", { metaKey: true }), CMD_CTRL), "⌘Q is reserved by macOS.");
  assert.equal(lib.captureRefusal(ev("KeyH", { metaKey: true, altKey: true }), CMD_CTRL), "⌥⌘H is reserved by macOS.");
  assert.match(lib.captureRefusal(ev("KeyD", { ctrlKey: true }), CMD_CTRL), /^Control is neither Alt nor Ctrl/);
  assert.match(lib.captureRefusal(ev("KeyD", { altKey: true }), { alt: "command", ctrl: "control" }), /^Option is neither/);
  assert.equal(lib.captureRefusal(ev("KeyD", { metaKey: true }), CMD_CTRL), null);
  assert.equal(lib.captureRefusal(ev("ShiftLeft", { shiftKey: true }), CMD_CTRL), null);
  assert.equal(lib.captureRefusal(ev("KeyQ", { metaKey: true })), null);
});

test("comboFromEvent: Control as Alt and Option as Ctrl (swapped)", () => {
  assert.equal(lib.comboFromEvent(ev("F5", { ctrlKey: true }), SWAPPED), "Alt+F5");
  assert.equal(lib.comboFromEvent(ev("KeyD", { altKey: true }), SWAPPED), "Ctrl+D");
  assert.equal(lib.comboFromEvent(ev("Space", { altKey: true, ctrlKey: true }), SWAPPED), "Ctrl+Alt+Space");
  // The unmapped Command still gives Cmd+.
  assert.equal(lib.comboFromEvent(ev("KeyD", { metaKey: true }), SWAPPED), "Cmd+D");
  assert.equal(lib.comboFromEvent(ev("KeyQ", { metaKey: true }), SWAPPED), "Cmd+Q");
});

test("comboFromEvent: Command as Alt", () => {
  const cmdAlt = { alt: "command", ctrl: "control" };
  assert.equal(lib.comboFromEvent(ev("F5", { metaKey: true }), cmdAlt), "Alt+F5");
  assert.equal(lib.comboFromEvent(ev("F5", { altKey: true }), cmdAlt), null);
  assert.equal(lib.comboFromEvent(ev("KeyD", { ctrlKey: true }), cmdAlt), "Ctrl+D");
});

test("tcModifiers", () => {
  assert.deepEqual(lib.tcModifiers(ev("KeyW", { metaKey: true }), CMD_CTRL), {
    ctrl: true, alt: false, shift: false, cmd: false, stray: false, any: true,
  });
  assert.deepEqual(lib.tcModifiers(ev("KeyW", { ctrlKey: true }), CMD_CTRL), {
    ctrl: false, alt: false, shift: false, cmd: false, stray: true, any: true,
  });
  assert.equal(lib.tcModifiers(ev("KeyW"), lib.DEFAULT_MODIFIERS).any, false);
  assert.equal(lib.tcModifiers(ev("KeyW", { shiftKey: true }), lib.DEFAULT_MODIFIERS).any, false);
});

test("Fn as Alt: the native Fn state makes the combo", () => {
  const FN_ALT = { alt: "fn", ctrl: "control" };
  assert.deepEqual(lib.modifierMap({ modifiers: FN_ALT }), FN_ALT);
  const f5 = { code: "F5", ctrlKey: false, altKey: false, metaKey: false, shiftKey: false };
  assert.equal(lib.comboFromEvent(f5, FN_ALT), "F5");
  lib.setFnHeld(true);
  try {
    assert.equal(lib.comboFromEvent(f5, FN_ALT), "Alt+F5");
    // Option is neither Alt nor Ctrl now: a stray key.
    assert.equal(lib.comboFromEvent({ ...f5, altKey: true }, FN_ALT), null);
    // Fn held while Fn is not mapped changes nothing.
    assert.equal(lib.comboFromEvent(f5), "F5");
  } finally {
    lib.setFnHeld(false);
  }
  assert.equal(lib.comboFromEvent({ ...f5, fnKey: true }, FN_ALT), "Alt+F5");
  assert.equal(lib.comboLabel("Alt+F5", FN_ALT), "fn F5");
  assert.equal(lib.physicalCombo("Ctrl+Alt+X", FN_ALT), "Fn+Ctrl+X");
  assert.equal(lib.comboLabel("Ctrl+Alt+X", { alt: "option", ctrl: "fn" }), "fn ⌥X");
});
test("comboLabel and physicalCombo show the keys in use", () => {
  assert.equal(lib.comboLabel("Alt+F5"), "⌥F5");
  assert.equal(lib.comboLabel("Alt+F5", { alt: "command", ctrl: "control" }), "⌘F5");
  assert.equal(lib.comboLabel("Ctrl+Shift+F"), "⌃⇧F");
  assert.equal(lib.comboLabel("Ctrl+D", CMD_CTRL), "⌘D");
  assert.equal(lib.comboLabel("Ctrl+Alt+Space", SWAPPED), "⌃⌥Space");
  assert.equal(lib.comboLabel("Ctrl+Down"), "⌃↓");
  assert.equal(lib.comboLabel("F3"), "F3");
  assert.equal(lib.comboLabel("Cmd+Q"), "⌘Q");
  assert.equal(lib.physicalCombo("Ctrl+D", SWAPPED), "Alt+D");
  assert.equal(lib.physicalCombo("Alt+Up", SWAPPED), "Ctrl+Up");
  assert.equal(lib.physicalCombo("Ctrl+Shift+F", CMD_CTRL), "Shift+Cmd+F");
});

test("modifierChoiceError refuses one key for both", () => {
  assert.equal(lib.modifierChoiceError("option", "control"), null);
  assert.equal(lib.modifierChoiceError("command", "command"), "Alt and Ctrl need different keys: Command is chosen for both.");
});

test("textEditKey: Command+A/C/V/X/Z stay with a text field when Command is Alt or Ctrl", () => {
  assert.equal(lib.textEditKey(ev("KeyA", { metaKey: true }), CMD_CTRL), true);
  assert.equal(lib.textEditKey(ev("KeyZ", { metaKey: true, shiftKey: true }), CMD_CTRL), true);
  assert.equal(lib.textEditKey(ev("KeyD", { metaKey: true }), CMD_CTRL), false);
  assert.equal(lib.textEditKey(ev("KeyA", { ctrlKey: true }), CMD_CTRL), false);
  // With Command unmapped Cmd+A is not a TC combo anyway (Cmd+A).
  assert.equal(lib.textEditKey(ev("KeyA", { metaKey: true }), lib.DEFAULT_MODIFIERS), false);
});

test("driveTarget follows the other panel and remembers each drive", () => {
  const vols = [
    { name: "Macintosh HD", path: "/", kind: "root" },
    { name: "~", path: "/Users/x", kind: "home" },
    { name: "USB", path: "/Volumes/USB", kind: "volume" },
  ];
  const [hd, home, usb] = vols;
  assert.equal(lib.diskFor(vols, "/Users/x/Desktop").path, "/");
  assert.equal(lib.diskFor(vols, "/Volumes/USB/a").path, "/Volumes/USB");
  assert.equal(lib.diskFor(vols, "sftp://m/var"), null);
  const last = new Map();
  // Panel on USB, other panel on the Desktop: Macintosh HD goes to the Desktop.
  assert.equal(lib.driveTarget(hd, { volumes: vols, here: "/Volumes/USB/a", other: "/Users/x/Desktop", last }), "/Users/x/Desktop");
  // Other panel elsewhere: the last folder visited on that drive, else its root.
  last.set("/", "/Users/x/Projects");
  assert.equal(lib.driveTarget(hd, { volumes: vols, here: "/Volumes/USB/a", other: "/Volumes/USB/b", last }), "/Users/x/Projects");
  assert.equal(lib.driveTarget(usb, { volumes: vols, here: "/Users/x", other: "/Users/x/Desktop", last }), "/Volumes/USB");
  // Already on that drive (e.g. in the home folder): still the other panel's folder there.
  assert.equal(lib.driveTarget(hd, { volumes: vols, here: "/Users/x", other: "/Users/x/Desktop", last }), "/Users/x/Desktop");
  // Already on that drive, other panel elsewhere or in the same folder: the drive's root.
  assert.equal(lib.driveTarget(hd, { volumes: vols, here: "/Users/x/Desktop", other: "/Volumes/USB", last }), "/");
  assert.equal(lib.driveTarget(hd, { volumes: vols, here: "/Users/x/Desktop", other: "/Users/x/Desktop", last }), "/");
  // The home shortcut always goes home.
  assert.equal(lib.driveTarget(home, { volumes: vols, here: "/Volumes/USB", other: "/Users/x/Desktop", last }), "/Users/x");
  // The other panel on a server does not count.
  assert.equal(lib.driveTarget(usb, { volumes: vols, here: "/", other: "sftp://m/Volumes/USB", last: new Map() }), "/Volumes/USB");
});

test("serverDrives lists saved non-SMB servers in tree order and marks connected ones", () => {
  const nodes = [
    { kind: "server", name: "web", protocol: "sftp" },
    { kind: "folder", name: "Work", children: [
      { kind: "server", name: "nas", protocol: "smb" },
      { kind: "server", name: "old", protocol: "scp" },
      { kind: "server", name: "files", protocol: "ftps" },
    ] },
  ];
  const d = lib.serverDrives(nodes, [{ name: "old", root: "scp://old/" }]);
  assert.deepEqual(d.map((x) => [x.name, x.path, x.connected]), [
    ["web", "sftp://web/", false],
    ["old", "scp://old/", true],
    ["files", "ftps://files/", false],
  ]);
  assert.ok(d.every((x) => x.kind === "server" && x.server === x.name));
});

test("serverTarget follows the other panel and remembers each server", () => {
  const web = { path: "sftp://web/", connected: true };
  const last = new Map([["sftp://web/", "sftp://web/var/log"]]);
  assert.equal(lib.serverTarget({ ...web, connected: false }, { here: "/Users/x", other: "sftp://web/etc", last }), null);
  assert.equal(lib.serverTarget(web, { here: "/Users/x", other: "sftp://web/etc", last }), "sftp://web/etc");
  assert.equal(lib.serverTarget(web, { here: "sftp://web/tmp", other: "/Users/x", last }), "sftp://web/");
  assert.equal(lib.serverTarget(web, { here: "/Users/x", other: "/tmp", last }), "sftp://web/var/log");
  assert.equal(lib.serverTarget(web, { here: "/Users/x", other: "/tmp", last: new Map() }), "sftp://web/");
});

test("search location is not remote", () => { assert.equal(lib.parseRemote(lib.SEARCH_LOCATION), null); assert.ok(lib.isSearchLocation(lib.SEARCH_LOCATION)); assert.ok(!lib.isSearchLocation("/d")); });
test("equal names stay separate rows", () => { const e = lib.resultEntries([{path:"/Users/x/a/notes.txt",name:"notes.txt"},{path:"/Users/x/b/notes.txt",name:"notes.txt"}], "/Users/x"); assert.deepEqual(e.map(x=>x.name), ["~/a/notes.txt","~/b/notes.txt"]); });
test("resultEntries maps item fields", () => { const [e] = lib.resultEntries([{path:"/p/a.tar/i.txt",isDir:false,size:5,mtime:7,inArchive:true,mounted:true}], "/h"); assert.deepEqual(e, {name:"/p/a.tar/i.txt",path:"/p/a.tar/i.txt",isDir:false,size:5,mtime:7,hidden:false,inArchive:true,mounted:true,mode:0,isSymlink:false}); });
test("operationPaths uses entry path", () => { const e = lib.resultEntries([{path:"/v/a/x.txt"}], "/h"); assert.deepEqual(lib.operationPaths(lib.SEARCH_LOCATION, e), {cwd:"/v/a", paths:["/v/a/x.txt"]}); });
test("operationPaths unchanged for folders", () => { assert.deepEqual(lib.operationPaths("/d", [{name:"f"}]), {cwd:"/d", paths:["/d/f"]}); });
test("archive entries allow view, copy and drag only", () => { assert.deepEqual(lib.resultOps([{inArchive:true}]), {view:true, edit:false, copy:true, move:false, del:false, drag:true}); assert.equal(lib.resultOps([{inArchive:true},{}]).view, false); assert.equal(lib.resultOps([{inArchive:true},{}]).drag, false); });
test("unmounted entries allow nothing", () => { assert.equal(lib.resultOps([{mounted:false}]).copy, false); assert.equal(lib.resultOps([{mounted:false,inArchive:true}]).view, false); });
test("normal entries allow everything", () => { assert.deepEqual(lib.resultOps([{mounted:true},{}]), {view:true,edit:true,copy:true,move:true,del:true,drag:true}); });
test("dropMissing", () => { assert.deepEqual(lib.dropMissing([{path:"/a"},{path:"/b"}], new Set(["/b"])).map(e=>e.path), ["/b"]); });
test("homeDisplay", () => { assert.equal(lib.homeDisplay("/Users/x/D/a", "/Users/x"), "~/D/a"); assert.equal(lib.homeDisplay("/Users/x", "/Users/x"), "~"); assert.equal(lib.homeDisplay("/Users/xy/a", "/Users/x"), "/Users/xy/a"); assert.equal(lib.homeDisplay("/Volumes/M/a", "/Users/x"), "/Volumes/M/a"); assert.equal(lib.homeDisplay("/a", ""), "/a"); });
test("resultRow", () => {
  assert.deepEqual(lib.resultRow({ name: "a.txt", folder: "/Users/x/D", mtime: 0, size: 1234 }, "/Users/x"), {
    name: "a.txt", folder: "~/D", size: "1,234", date: lib.formatDate(0),
  });
  assert.equal(lib.resultRow({ name: "D", folder: "/", mtime: 0, isDir: true }, "/Users/x").size, "<DIR>");
  assert.equal(lib.resultRow({ name: "x", folder: "/", mtime: 0 }, "/Users/x").size, "");
});

test("result columns: defaults, saved widths and hidden columns; Name always shown", () => {
  const d = lib.resultColumns(undefined);
  assert.deepEqual(d.map((c) => c.id), ["name", "folder", "size", "date"]);
  assert.ok(d.every((c) => !c.hidden));
  const s = lib.resultColumns([{ id: "name", width: 5, hidden: true }, { id: "size", hidden: true }, { id: "date", width: 99.6 }, null, { id: "x" }]);
  assert.equal(s[0].width, lib.RESULT_COLUMN_MIN);
  assert.equal(s[0].hidden, false);
  assert.equal(s[2].hidden, true);
  assert.equal(s[3].width, 100);
  assert.equal(lib.resultGridTemplate(s), `${lib.RESULT_COLUMN_MIN}px 280px 100px 1fr`);
});

test("result clicks: plain, Cmd/Ctrl toggle, Shift range", () => {
  const rows = ["a", "b", "c", "d"].map((p) => ({ path: p }));
  let r = lib.resultClick(rows, new Set(), null, 1);
  assert.deepEqual([...r.marked], ["b"]);
  r = lib.resultClick(rows, r.marked, r.anchor, 3, { toggle: true });
  assert.deepEqual([...r.marked].sort(), ["b", "d"]);
  r = lib.resultClick(rows, r.marked, r.anchor, 1, { toggle: true });
  assert.deepEqual([...r.marked], ["d"]);
  // The last toggled row (b) is the anchor of a Shift range.
  r = lib.resultClick(rows, r.marked, r.anchor, 3, { range: true });
  assert.deepEqual([...r.marked].sort(), ["b", "c", "d"]);
  assert.equal(r.anchor, 1);
  r = lib.resultClick(rows, new Set(["a"]), 0, 2, { range: true, toggle: true });
  assert.deepEqual([...r.marked].sort(), ["a", "b", "c"]);
  assert.deepEqual(lib.feedItems(rows, new Set(["d", "b"])).map((x) => x.path), ["b", "d"]);
  assert.equal(lib.feedItems(rows, new Set(["b"])).length, 4);
  assert.equal(lib.feedItems(rows, new Set()).length, 4);
});

// ---------------------------------------------------------------- Find files window (Task 10)

const item = (path, extra = {}) => ({ path, name: path.slice(path.lastIndexOf("/") + 1), folder: path.slice(0, path.lastIndexOf("/")) || "/", mtime: 0, size: 1, isDir: false, inArchive: false, mounted: true, ...extra });
const started = (id, prev = lib.searchWindowState(undefined, { type: "clear" })) =>
  lib.searchWindowState(lib.searchWindowState(prev, { type: "start" }), { type: "started", id });

test("batch of old id ignored", () => {
  const s = { rows: [item("/a")], total: 1, done: false, id: 2 };
  assert.equal(lib.searchWindowState(s, { id: 1, items: [item("/b")], total: 1, done: true }), s);
});
test("walk batches append", () => {
  let s = started(2);
  s = lib.searchWindowState(s, { id: 2, items: [item("/a")], total: 1, done: false });
  s = lib.searchWindowState(s, { id: 2, items: [item("/b")], total: 2, done: false });
  s = lib.searchWindowState(s, { id: 2, items: [], total: 2, done: true });
  assert.deepEqual(s.rows.map((r) => r.path), ["/a", "/b"]);
  assert.equal(s.total, 2);
  assert.equal(s.done, true);
});
test("index batch replaces", () => {
  let s = started(1);
  s = lib.searchWindowState(s, { id: 1, items: [item("/old")], total: 1, done: true });
  s = started(2, s);
  assert.deepEqual(s.rows.map((r) => r.path), ["/old"], "old rows stay until the first batch");
  s = lib.searchWindowState(s, { id: 2, items: [item("/new")], total: 1, done: true });
  assert.deepEqual(s.rows.map((r) => r.path), ["/new"]);
  assert.equal(s.done, true);
});
test("batches that arrive before search_start returns are kept", () => {
  let s = lib.searchWindowState(lib.searchWindowState(undefined, { type: "clear" }), { type: "start" });
  s = lib.searchWindowState(s, { id: 5, items: [item("/early")], total: 1, done: true });
  assert.deepEqual(s.rows, []);
  s = lib.searchWindowState(s, { type: "started", id: 5 });
  assert.deepEqual(s.rows.map((r) => r.path), ["/early"]);
  assert.equal(s.done, true);
});
test("early batches of a superseded start are dropped", () => {
  let s = lib.searchWindowState(lib.searchWindowState(undefined, { type: "clear" }), { type: "start" });
  s = lib.searchWindowState(s, { id: 5, items: [item("/stale")], total: 1, done: true });
  s = lib.searchWindowState(s, { type: "started", id: 6 });
  s = lib.searchWindowState(s, { id: 6, items: [item("/x")], total: 1, done: true });
  assert.deepEqual(s.rows.map((r) => r.path), ["/x"]);
});
test("clear empties rows and ignores the old search's batches", () => {
  let s = lib.searchWindowState(started(3), { id: 3, items: [item("/a")], total: 1, done: false });
  s = lib.searchWindowState(s, { type: "clear" });
  assert.deepEqual(s.rows, []);
  assert.equal(lib.searchWindowState(s, { id: 3, items: [item("/b")], total: 2, done: false }), s);
});
test("failed start stops waiting", () => {
  let s = lib.searchWindowState(started(1), { id: 1, items: [item("/old")], total: 1, done: true });
  s = lib.searchWindowState(s, { type: "start" });
  s = lib.searchWindowState(s, { type: "failed" });
  assert.equal(s.pending, false);
  assert.deepEqual(s.rows, []);
  assert.equal(s.done, true);
  assert.equal(lib.searchWindowState(s, { id: 9, items: [item("/b")], total: 1, done: true }), s);
});
test("remove drops one row and lowers the total", () => {
  let s = lib.searchWindowState(started(1), { id: 1, items: [item("/a"), item("/b")], total: 2, done: true });
  s = lib.searchWindowState(s, { type: "remove", path: "/a" });
  assert.deepEqual(s.rows.map((r) => r.path), ["/b"]);
  assert.equal(s.total, 1);
});
test("rows are capped at the display limit", () => {
  const many = Array.from({ length: 1200 }, (_, i) => item(`/f${i}`));
  const s = lib.searchWindowState(started(1), { id: 1, items: many, total: 1200, done: true });
  assert.equal(s.rows.length, lib.SEARCH_LIMIT);
  assert.equal(lib.SEARCH_LIMIT, 1000);
});

test("cap note", () => {
  assert.equal(lib.statusText({ total: 1500, shown: 1000 }), "1,500 found · showing first 1,000");
});
test("statusText index states", () => {
  assert.equal(lib.statusText({ total: 3, shown: 3, state: "ready", entries: 1842310 }), "3 found · index: 1,842,310 files, up to date");
  assert.equal(lib.statusText({ total: 0, shown: 0, state: "building" }), "0 found · building index…");
  assert.equal(lib.statusText({ total: 2, shown: 2, state: "ready", walking: true }), "2 found · searching folder…");
  assert.equal(
    lib.statusText({ total: 1, shown: 1, state: "ready", entries: 5, stale: { name: "Data", date: "01.11.2026 10:00" } }),
    "1 found · volume Data last refreshed 01.11.2026 10:00",
  );
  assert.equal(lib.statusText({ state: "ready", entries: 10 }), "index: 10 files, up to date");
  assert.equal(lib.statusText({ state: "off" }), "", "the window shows the off note itself");
  assert.equal(lib.statusText({ state: "notAsked" }), "index not built");
  assert.equal(lib.statusText({}), "");
});
test("staleVolume picks an enabled unmounted volume", () => {
  const status = { volumes: [
    { path: "/", name: "Macintosh HD", kind: "startup", enabled: true, mounted: true, refreshed: 1, state: "ready" },
    { path: "/Volumes/Off", name: "Off", kind: "network", enabled: false, mounted: false, refreshed: 1, state: "off" },
    { path: "/Volumes/Data", name: "Data", kind: "network", enabled: true, mounted: false, refreshed: 0, state: "stale" },
  ] };
  assert.deepEqual(lib.staleVolume(status, "UTC"), { name: "Data", date: "never" });
  status.volumes[2].refreshed = 1762000000;
  assert.deepEqual(lib.staleVolume(status, "UTC"), { name: "Data", date: lib.formatDate(1762000000, "UTC") });
  assert.equal(lib.staleVolume({ volumes: [status.volumes[0]] }), null);
  assert.equal(lib.staleVolume(null), null);
});
test("searchScopes follow the index state", () => {
  assert.deepEqual(lib.searchScopes("ready"), { everywhere: true, note: null });
  assert.deepEqual(lib.searchScopes("building"), { everywhere: false, note: null });
  assert.deepEqual(lib.searchScopes("off"), { everywhere: false, note: "Index is off (Options → Search)" });
  assert.deepEqual(lib.searchScopes("notAsked"), { everywhere: false, note: null });
});
test("searchRequest", () => {
  assert.equal(lib.searchRequest({ query: "   ", scope: "everywhere", location: "/x", archives: false }), null);
  assert.deepEqual(lib.searchRequest({ query: " scope work ", scope: "everywhere", location: "/x", archives: true }), { query: "scope work", under: null, archives: true, limit: 1000 });
  assert.deepEqual(lib.searchRequest({ query: "a", scope: "folder", location: "/x/y", archives: false }), { query: "a", under: "/x/y", archives: false, limit: 1000 });
});
test("searchUnder: servers and the results list cannot be searched", () => {
  assert.equal(lib.searchUnder("/Users/x"), "/Users/x");
  assert.equal(lib.searchUnder("sftp://web/var"), null);
  assert.equal(lib.searchUnder(lib.SEARCH_LOCATION), null);
});
test("goTarget: folder and cursor name of a result", () => {
  assert.deepEqual(lib.goTarget(item("/x/a.txt")), { folder: "/x", name: "a.txt" });
  assert.deepEqual(lib.goTarget({ path: "/x/pack.tar/docs/a.txt", name: "pack.tar ▸ docs/a.txt", folder: "/x/pack.tar/docs", inArchive: true }), { folder: "/x/pack.tar/docs", name: "a.txt" });
});
test("volumeOf picks the longest volume prefix", () => {
  const vols = [{ path: "/", name: "HD" }, { path: "/Volumes/Data", name: "Data" }, { path: "/Volumes/Data2", name: "Data2" }];
  assert.equal(lib.volumeOf("/Volumes/Data/a/b", vols).name, "Data");
  assert.equal(lib.volumeOf("/Volumes/Data2/a", vols).name, "Data2");
  assert.equal(lib.volumeOf("/Users/x/a", vols).name, "HD");
  assert.equal(lib.volumeOf("/Volumes/Gone/a", [{ path: "/Volumes/Data", name: "Data" }]), null);
  assert.equal(lib.volumeOf("/Volumes/Gone/a", vols), null, "the startup disk does not hold /Volumes");
});
test("volumeName falls back to the /Volumes component", () => {
  assert.equal(lib.volumeName("/Volumes/Gone/a/b", null), "Gone");
  assert.equal(lib.volumeName("/x/a", { name: "HD" }), "HD");
  assert.equal(lib.volumeName("/x/a", null), "/");
});
test("smbServerForVolume matches a saved SMB share by mount name", () => {
  const nodes = [
    { kind: "server", name: "web", protocol: "sftp", share: "" },
    { kind: "folder", name: "Home", children: [
      { kind: "server", name: "NAS data", protocol: "smb", share: "Data" },
      { kind: "server", name: "NAS media", protocol: "smb", share: "media" },
    ] },
  ];
  assert.equal(lib.smbServerForVolume({ path: "/Volumes/Data", kind: "network" }, nodes), "NAS data");
  assert.equal(lib.smbServerForVolume({ path: "/Volumes/MEDIA", kind: "network" }, nodes), "NAS media");
  assert.equal(lib.smbServerForVolume({ path: "/Volumes/Data-1", kind: "network" }, nodes), "NAS data");
  assert.equal(lib.smbServerForVolume({ path: "/Volumes/web", kind: "network" }, nodes), null);
  assert.equal(lib.smbServerForVolume({ path: "/Volumes/Data", kind: "local" }, nodes), null);
  assert.equal(lib.smbServerForVolume(null, nodes), null);
});
test("smbServerForVolume prefers the mount source's host and share", () => {
  const nodes = [
    { kind: "server", name: "Old NAS", protocol: "smb", host: "old.example.com", share: "Data" },
    { kind: "server", name: "NAS", protocol: "smb", host: "nas.example.com", share: "Data" },
    { kind: "server", name: "Media", protocol: "smb", host: "nas.example.com", share: "My Media" },
  ];
  const net = (path, source) => ({ path, kind: "network", source });
  assert.equal(lib.smbServerForVolume(net("/Volumes/Data", "//joe@NAS.example.com/Data"), nodes), "NAS");
  // Mounted under another name: the source still finds it.
  assert.equal(lib.smbServerForVolume(net("/Volumes/x", "//DOMAIN;joe@nas.example.com:445/My%20Media"), nodes), "Media");
  // Unknown host: the share name decides (first in order).
  assert.equal(lib.smbServerForVolume(net("/Volumes/y", "//other.example.com/data"), nodes), "Old NAS");
  assert.equal(lib.smbServerForVolume(net("/Volumes/Data", null), nodes), "Old NAS");
  assert.deepEqual(lib.parseMountSource("//h/s"), { host: "h", share: "s" });
  assert.equal(lib.parseMountSource("/dev/disk3s1"), null);
  assert.equal(lib.parseMountSource("//h/s/sub"), null);
});
test("mountedAt: the share must come back where the index has it", () => {
  assert.ok(lib.mountedAt("/Volumes/Data", "/Volumes/Data"));
  assert.ok(lib.mountedAt("/Volumes/Data/sub", "/Volumes/Data"));
  assert.ok(!lib.mountedAt("/Volumes/Data-1", "/Volumes/Data"));
});
test("moveIndex for Up/Down/PgUp/PgDn", () => {
  assert.equal(lib.moveIndex(0, "ArrowDown", 5, 3), 1);
  assert.equal(lib.moveIndex(4, "ArrowDown", 5, 3), 4);
  assert.equal(lib.moveIndex(0, "ArrowUp", 5, 3), 0);
  assert.equal(lib.moveIndex(1, "PageDown", 5, 3), 4);
  assert.equal(lib.moveIndex(4, "PageUp", 5, 3), 1);
  assert.equal(lib.moveIndex(0, "PageUp", 5, 3), 0);
  assert.equal(lib.moveIndex(0, "ArrowDown", 0, 3), -1);
  assert.equal(lib.moveIndex(-1, "ArrowDown", 3, 3), 0);
});

test("missingAction: a missing result is forgotten only on a mounted volume", () => {
  assert.equal(lib.missingAction({ listOk: true, found: true, volumeMounted: true }), "go");
  assert.equal(lib.missingAction({ listOk: true, found: false, volumeMounted: true }), "forget");
  assert.equal(lib.missingAction({ listOk: false, found: false, volumeMounted: true }), "forget");
  // The volume was ejected after the search: never forget, offer the mount instead.
  assert.equal(lib.missingAction({ listOk: false, found: false, volumeMounted: false }), "mount");
  assert.equal(lib.missingAction({ listOk: true, found: false, volumeMounted: false }), "mount");
});
test("volumeMounted: a /Volumes path the status does not list is not mounted", () => {
  const vols = [{ path: "/", mounted: true }, { path: "/Volumes/Data", mounted: true }, { path: "/Volumes/Net", mounted: false }];
  assert.equal(lib.volumeMounted("/Users/x/a", vols), true);
  assert.equal(lib.volumeMounted("/Volumes/Data/a", vols), true);
  assert.equal(lib.volumeMounted("/Volumes/Net/a", vols), false);
  assert.equal(lib.volumeMounted("/Volumes/Ejected/a", vols), false);
  assert.equal(lib.volumeMounted("/Volumes/Ejected/a", []), false);
});

// ---------------------------------------------------------------- Feed to listbox (results panel)

test("displayLocation: the results panel shows its query", () => {
  assert.equal(lib.displayLocation(lib.SEARCH_LOCATION, { query: "fxfm" }), "Search results: fxfm");
  assert.equal(lib.displayLocation(lib.SEARCH_LOCATION, new Map(), { query: "a b" }), "Search results: a b");
  assert.equal(lib.displayLocation(lib.SEARCH_LOCATION, new Map()), "Search results");
  assert.equal(lib.displayLocation("/a/b", new Map(), { query: "x" }), "/a/b");
});

test("parentLocation: the results location has no parent", () => {
  assert.equal(lib.parentLocation(lib.SEARCH_LOCATION), null);
});

test("diskFor: the results location is on no disk", () => {
  const vols = [{ path: "/", kind: "root" }, { path: "/Users/x", kind: "home" }];
  assert.equal(lib.diskFor(vols, lib.SEARCH_LOCATION), null);
  assert.equal(lib.diskFor(vols, "/Users/x/a").path, "/");
});

test("sessionLocation: a results panel saves the folder it came from", () => {
  const results = { query: "q", entries: [], back: { location: "/Users/x/Docs", cursorName: "a.txt" } };
  assert.deepEqual(lib.sessionLocation({ location: lib.SEARCH_LOCATION, cursorName: "~/b/c.txt" }, results, new Map()), {
    location: "/Users/x/Docs",
    cursorName: "a.txt",
  });
  assert.equal(lib.sessionLocation({ location: lib.SEARCH_LOCATION, cursorName: null }, null, new Map()), null);
  assert.deepEqual(lib.sessionLocation({ location: "/a", cursorName: "b" }, null, new Map()), { location: "/a", cursorName: "b" });
  assert.equal(lib.sessionLocation({ location: "", cursorName: null }, null, new Map()), null);
  // A server archive's temp copy saves the server folder (also as the results' way back).
  const arcs = new Map([["/tmp/fxfm/1/p.zip", "sftp://m/www"]]);
  assert.deepEqual(lib.sessionLocation({ location: "/tmp/fxfm/1/p.zip/d", cursorName: "x" }, null, arcs), {
    location: "sftp://m/www",
    cursorName: "p.zip",
  });
  const back = { query: "q", entries: [], back: { location: "/tmp/fxfm/1/p.zip", cursorName: "x" } };
  assert.deepEqual(lib.sessionLocation({ location: lib.SEARCH_LOCATION, cursorName: null }, back, arcs), {
    location: "sftp://m/www",
    cursorName: "p.zip",
  });
});

test("resultFolder: the item's folder and name (inside an archive: the inner folder)", () => {
  assert.deepEqual(lib.resultFolder({ path: "/Users/x/a/notes.txt" }), { folder: "/Users/x/a", name: "notes.txt" });
  assert.deepEqual(lib.resultFolder({ path: "/Users/x/a", isDir: true }), { folder: "/Users/x", name: "a" });
  assert.deepEqual(lib.resultFolder({ path: "/x/pack.tar/docs/a.txt", inArchive: true }), { folder: "/x/pack.tar/docs", name: "a.txt" });
  assert.deepEqual(lib.resultFolder({ path: "/x/pack.zip/a.txt", inArchive: true }), { folder: "/x/pack.zip", name: "a.txt" });
  assert.deepEqual(lib.resultFolder({ path: "/a.txt" }), { folder: "/", name: "a.txt" });
});

test("resultOpRefusal: archive entries and unmounted volumes", () => {
  const [plain, arc, gone] = [
    { path: "/a", inArchive: false, mounted: true },
    { path: "/p.zip/b", inArchive: true, mounted: true },
    { path: "/Volumes/N/c", inArchive: false, mounted: false },
  ];
  assert.equal(lib.resultOpRefusal([plain], "move"), null);
  assert.equal(lib.resultOpRefusal([], "del"), null);
  assert.equal(lib.resultOpRefusal([plain, arc], "move"), "Not available for items inside archives");
  assert.equal(lib.resultOpRefusal([arc], "edit"), "Not available for items inside archives");
  assert.equal(lib.resultOpRefusal([arc], "copy"), null);
  assert.equal(lib.resultOpRefusal([arc], "view"), null);
  assert.equal(lib.resultOpRefusal([gone], "view"), "Not available for items on a volume that is not mounted");
  assert.equal(lib.resultOpRefusal([arc, gone], "copy"), "Not available for items on a volume that is not mounted");
});

test("results recheck: unmounted entries are never checked nor dropped", () => {
  const entries = [
    { path: "/a", mounted: true },
    { path: "/b", mounted: true },
    { path: "/Volumes/N/c", mounted: false },
  ];
  assert.deepEqual(lib.pathsToCheck(entries), ["/a", "/b"]);
  assert.deepEqual(lib.keepExisting(entries, ["/b"]).map((e) => e.path), ["/b", "/Volumes/N/c"]);
});

test("copyTargetDefault: a result entry targets its file name, not the shown path", () => {
  const [e] = lib.resultEntries([{ path: "/Users/x/a/notes.txt" }], "/Users/x");
  assert.equal(lib.copyTargetDefault("/dst", [e]), "/dst/notes.txt");
  assert.equal(lib.copyTargetDefault("/dst", [{ name: "n.txt" }]), "/dst/n.txt");
});

test("maskSelects: a result entry matches by its file name", () => {
  const [f, d] = lib.resultEntries([{ path: "/Users/x/a/notes.txt" }, { path: "/Users/x/abc", isDir: true }], "/Users/x");
  assert.ok(lib.maskSelects(f, "notes*"));
  assert.ok(!lib.maskSelects(f, "a*"));
  assert.ok(lib.maskSelects(d, "ab*/"));
});

test("marks of equal file names from different folders stay separate", () => {
  const e = lib.resultEntries([{ path: "/Users/x/a/notes.txt" }, { path: "/Users/x/b/notes.txt" }], "/Users/x");
  const marked = new Set([e[1].name]);
  assert.deepEqual(lib.operationSources(e, marked, 0).map((s) => s.path), ["/Users/x/b/notes.txt"]);
  assert.deepEqual(lib.dragSelection(e, marked, e[1].name), ["~/b/notes.txt"]);
});

test("packTargetDir: a results list in the other panel is no target folder", () => {
  assert.equal(lib.isLocalFolder(lib.SEARCH_LOCATION), false);
  assert.equal(lib.packTargetDir("/a", lib.SEARCH_LOCATION), "/a");
});

test("groupSources: local items in one group, each archive's entries in their own", () => {
  assert.deepEqual(
    lib.groupSources(["/a/x.txt", "/p/one.zip/d/i.txt", "/b/y.txt", "/q/two.tar/j.txt", "/p/one.zip/k.txt", "/c/three.zip"], "/a"),
    [
      { paths: ["/a/x.txt", "/b/y.txt", "/c/three.zip"], cwd: "/a" },
      { paths: ["/p/one.zip/d/i.txt", "/p/one.zip/k.txt"], cwd: "/a" },
      { paths: ["/q/two.tar/j.txt"], cwd: "/a" },
    ],
  );
  // A normal panel's sources share a folder: always one group, unchanged.
  assert.deepEqual(lib.groupSources(["/p/one.zip/d/a", "/p/one.zip/d/b"], "/p/one.zip/d"), [
    { paths: ["/p/one.zip/d/a", "/p/one.zip/d/b"], cwd: "/p/one.zip/d" },
  ]);
  assert.deepEqual(lib.groupSources(["sftp://m/a", "sftp://m/b"], "sftp://m/"), [{ paths: ["sftp://m/a", "sftp://m/b"], cwd: "sftp://m/" }]);
  assert.deepEqual(lib.groupSources([], "/a"), []);
});

test("panelFolder: a results list uses the folder it came from (hotlist, command line)", () => {
  const results = { query: "q", entries: [], back: { location: "/Users/x/Docs", cursorName: null } };
  assert.equal(lib.panelFolder(lib.SEARCH_LOCATION, results, "/Users/x"), "/Users/x/Docs");
  assert.equal(lib.panelFolder(lib.SEARCH_LOCATION, null, "/Users/x"), "/Users/x");
  assert.equal(lib.panelFolder("/tmp/a", results, "/Users/x"), "/tmp/a");
  assert.equal(lib.pickerStart("", "/Users/x", lib.panelFolder(lib.SEARCH_LOCATION, results, "/Users/x")), "/Users/x/Docs");
});

test("searchSettingsPatch: a volume is added, then updated in place", () => {
  const s = { search: { volumes: [], gzLimitMb: 500, networkRefreshHours: 24 } };
  const a = lib.searchSettingsPatch(s, { volume: { path: "/Volumes/A", enabled: true } });
  assert.deepEqual(a.volumes, [{ path: "/Volumes/A", enabled: true }]);
  assert.deepEqual(s.search.volumes, []); // the input is not changed
  const b = lib.searchSettingsPatch({ search: a }, { volume: { path: "/Volumes/A", enabled: false } });
  assert.deepEqual(b.volumes, [{ path: "/Volumes/A", enabled: false }]);
});

test("searchSettingsPatch: gz limit must be an integer MB >= 1, else the old value stays", () => {
  const s = { search: { gzLimitMb: 500 } };
  assert.equal(lib.searchSettingsPatch(s, { gzLimitMb: "abc" }).gzLimitMb, 500);
  assert.equal(lib.searchSettingsPatch(s, { gzLimitMb: "0" }).gzLimitMb, 500);
  assert.equal(lib.searchSettingsPatch(s, { gzLimitMb: "" }).gzLimitMb, 500);
  assert.equal(lib.searchSettingsPatch(s, { gzLimitMb: "250" }).gzLimitMb, 250);
});

test("searchSettingsPatch: 0 hours is kept (manual only); junk keeps the old value", () => {
  const s = { search: { networkRefreshHours: 24 } };
  assert.equal(lib.searchSettingsPatch(s, { networkRefreshHours: "0" }).networkRefreshHours, 0);
  assert.equal(lib.searchSettingsPatch(s, { networkRefreshHours: "6" }).networkRefreshHours, 6);
  assert.equal(lib.searchSettingsPatch(s, { networkRefreshHours: "-1" }).networkRefreshHours, 24);
  assert.equal(lib.searchSettingsPatch(s, { networkRefreshHours: "x" }).networkRefreshHours, 24);
});

test("searchSettingsPatch: excludes are added once and removed; flags and index are set", () => {
  let s = { search: { exclude: ["/a"] } };
  s = { search: lib.searchSettingsPatch(s, { addExclude: "/b" }) };
  s = { search: lib.searchSettingsPatch(s, { addExclude: "/a" }) };
  assert.deepEqual(s.search.exclude, ["/a", "/b"]);
  s = { search: lib.searchSettingsPatch(s, { removeExclude: "/a" }) };
  assert.deepEqual(s.search.exclude, ["/b"]);
  assert.equal(lib.searchSettingsPatch(s, { archives: true }).archives, true);
  assert.equal(lib.searchSettingsPatch(s, { networkArchives: true }).networkArchives, true);
  assert.equal(lib.searchSettingsPatch(s, { index: false }).index, false);
});

test("index button follows the index state", () => {
  assert.deepEqual(lib.indexButton("notAsked"), { label: "Build index", action: "build", disabled: false });
  assert.deepEqual(lib.indexButton("off"), { label: "Build index", action: "build", disabled: false });
  assert.deepEqual(lib.indexButton("ready"), { label: "Rebuild index", action: "rebuild", disabled: false });
  assert.equal(lib.indexButton("building").disabled, true);
});

test("quick search mode defaults to Command and rejects unknown values", () => {
  assert.equal(lib.quickSearchMode({}), "command");
  assert.equal(lib.quickSearchMode({ quickSearch: "bogus" }), "command");
  assert.equal(lib.quickSearchMode({ quickSearch: "off" }), "off");
  assert.equal(lib.quickSearchMode({ quickSearch: "letters" }), "letters");
});

test("quick search chars follow the chosen keys", () => {
  const k = (code, key, mods = {}) => ({ code, key, ...mods });
  assert.equal(lib.quickSearchChar(k("KeyL", "l", { metaKey: true }), "command"), "l");
  assert.equal(lib.quickSearchChar(k("KeyL", "L", { metaKey: true, shiftKey: true }), "command"), "l");
  assert.equal(lib.quickSearchChar(k("KeyL", "l"), "command"), null);
  assert.equal(lib.quickSearchChar(k("KeyL", "l", { metaKey: true, altKey: true }), "command"), null);
  assert.equal(lib.quickSearchChar(k("KeyQ", "q", { metaKey: true }), "command"), null);
  assert.equal(lib.quickSearchChar(k("KeyH", "h", { metaKey: true }), "command"), null);
  assert.equal(lib.quickSearchChar(k("KeyL", "l", { metaKey: true, ctrlKey: true }), "controlCommand"), "l");
  assert.equal(lib.quickSearchChar(k("KeyL", "¬", { altKey: true }), "option"), "l");
  assert.equal(lib.quickSearchChar(k("KeyL", "l"), "letters"), "l");
  assert.equal(lib.quickSearchChar(k("Space", " "), "letters"), null);
  assert.equal(lib.quickSearchChar(k("KeyL", "l", { metaKey: true }), "off"), null);
  // Layout characters: z/y swap on a Croatian layout, č from its own key.
  assert.equal(lib.quickSearchChar(k("KeyY", "z", { metaKey: true }), "command"), "z");
  assert.equal(lib.quickSearchChar(k("Semicolon", "č", { metaKey: true }), "command"), "č");
  assert.equal(lib.quickSearchChar(k("Digit2", "2", { metaKey: true }), "command"), "2");
  assert.equal(lib.quickSearchChar(k("Period", ".", { metaKey: true }), "command"), ".");
  assert.equal(lib.quickSearchChar(k("Minus", "_", { metaKey: true, shiftKey: true }), "command"), "_");
  assert.equal(lib.quickSearchChar(k("F5", "F5", { metaKey: true }), "command"), null);
});

test("quick search finds name prefixes, skips .. and wraps", () => {
  const es = [{ up: true, name: ".." }, { name: "Apps" }, { name: "lib" }, { name: "Logs" }, { name: "Čitaj" }, { name: "lzma" }];
  assert.equal(lib.quickSearchFind(es, "l"), 2);
  assert.equal(lib.quickSearchFind(es, "LO"), 3);
  assert.equal(lib.quickSearchFind(es, "."), -1);
  assert.equal(lib.quickSearchFind(es, "č"), 4);
  assert.equal(lib.quickSearchFind(es, "x"), -1);
  assert.equal(lib.quickSearchFind(es, "l", 2, 1), 3);
  assert.equal(lib.quickSearchFind(es, "l", 5, 1), 2);
  assert.equal(lib.quickSearchFind(es, "l", 2, -1), 5);
  assert.equal(lib.quickSearchFind([], "l"), -1);
});

test("multi-rename: extension split", () => {
  assert.deepEqual(lib.splitExtension("a.tar.gz"), { base: "a.tar", ext: "gz" });
  assert.deepEqual(lib.splitExtension(".bashrc"), { base: ".bashrc", ext: "" });
  assert.deepEqual(lib.splitExtension("noext"), { base: "noext", ext: "" });
  assert.deepEqual(lib.splitExtension("dir.d", true), { base: "dir.d", ext: "" });
  assert.deepEqual(lib.splitExtension("trail."), { base: "trail.", ext: "" });
});

test("multi-rename: mask placeholders", () => {
  const t = new Date(2026, 10, 20, 9, 5, 7).getTime() / 1000;
  const v = { base: "Holiday", ext: "jpg", counter: "007", parent: "Pics", mtime: t };
  assert.equal(lib.expandRenameMask("[N]", v), "Holiday");
  assert.equal(lib.expandRenameMask("[N2-4]", v), "oli");
  assert.equal(lib.expandRenameMask("[N3]", v), "l");
  assert.equal(lib.expandRenameMask("[N5-]", v), "day");
  assert.equal(lib.expandRenameMask("[P]_[C]", v), "Pics_007");
  assert.equal(lib.expandRenameMask("[Y]-[M]-[D] [h].[m].[s]", v), "2026-11-20 09.05.07");
  assert.equal(lib.expandRenameMask("[E1]", v), "j");
  assert.equal(lib.expandRenameMask("[[x] [Q]", v), "[x] [Q]");
  assert.equal(lib.expandRenameMask("[N2-3]", { base: "čšž", ext: "" }), "šž");
});

test("multi-rename: names with counter, replace and case", () => {
  const o = (x) => ({ ...lib.MULTI_RENAME_DEFAULTS, ...x });
  const e = { name: "IMG_0001.JPG", isDir: false, mtime: 0 };
  assert.equal(lib.multiRenameName(e, 0, o({}), "P"), "IMG_0001.JPG");
  assert.equal(lib.multiRenameName(e, 2, o({ name: "photo_[C]", start: 5, step: 2, digits: 3 }), "P"), "photo_009.JPG");
  assert.equal(lib.multiRenameName(e, 0, o({ case: "lower" }), "P"), "img_0001.jpg");
  assert.equal(lib.multiRenameName(e, 0, o({ search: "img_", replace: "Pic-" }), "P"), "Pic-0001.JPG");
  assert.equal(lib.multiRenameName(e, 0, o({ search: "img_", replace: "x", matchCase: true }), "P"), "IMG_0001.JPG");
  assert.equal(lib.multiRenameName(e, 0, o({ search: "(\\d+)", replace: "<$1>", regex: true }), "P"), "IMG_<0001>.JPG");
  assert.equal(lib.multiRenameName(e, 0, o({ search: "IMG", replace: "$1" }), "P"), "$1_0001.JPG");
  assert.equal(lib.multiRenameName({ name: "my song-name.mp3", mtime: 0 }, 0, o({ case: "words" }), "P"), "My Song-Name.Mp3");
  assert.equal(lib.multiRenameName({ name: "my song.mp3", mtime: 0 }, 0, o({ case: "first" }), "P"), "My song.mp3");
  assert.equal(lib.multiRenameName(e, 0, o({ ext: "" }), "P"), "IMG_0001");
  assert.equal(lib.multiRenameName({ name: "Dir.d", isDir: true, mtime: 0 }, 0, o({ name: "[N]_x" }), "P"), "Dir.d_x");
});

test("multi-rename: plan finds duplicates, existing names and bad input", () => {
  const o = (x) => ({ ...lib.MULTI_RENAME_DEFAULTS, ...x });
  const es = ["a.txt", "b.txt"].map((name) => ({ name, mtime: 0, folder: "/d" }));
  let p = lib.multiRenamePlan(es, o({ name: "x" }));
  assert.equal(p.ok, false);
  assert.deepEqual(p.rows.map((r) => r.error), ["Same name twice", "Same name twice"]);
  p = lib.multiRenamePlan(es, o({ name: "[N]_[C]" }), () => ["a_1.txt", "keep"]);
  assert.deepEqual(p.rows.map((r) => r.to), ["a_1.txt", "b_2.txt"]);
  assert.deepEqual(p.rows.map((r) => r.error), ["Already exists", null]);
  // Shifting names inside the batch is fine: b.txt leaves before a.txt takes its name.
  const sw = lib.multiRenamePlan(es, o({ search: "a", replace: "b" }), () => ["a.txt", "b.txt"]);
  assert.deepEqual(sw.rows.map((r) => r.error), ["Same name twice", "Same name twice"]);
  const shift = lib.multiRenamePlan([{ name: "a", mtime: 0, folder: "/d" }, { name: "b", mtime: 0, folder: "/d" }], o({ name: "[N]", search: "", case: "same" }), () => ["a", "b"]);
  assert.equal(shift.ok, false); // nothing changes
  const swap = lib.multiRenamePlan([{ name: "a", mtime: 0, folder: "/d" }], o({ name: "b" }), () => ["a", "b"]);
  assert.equal(swap.rows[0].error, "Already exists");
  p = lib.multiRenamePlan(es, o({ name: "", ext: "" }));
  assert.equal(p.rows[0].error, "Invalid name");
  p = lib.multiRenamePlan(es, o({ search: "(", regex: true }));
  assert.match(p.error, /^Search:/);
  p = lib.multiRenamePlan([{ name: "a", mtime: 0, folder: "/d" }], o({ case: "upper" }), () => ["a"]);
  assert.equal(p.ok, true);
  p = lib.multiRenamePlan([{ name: "a", mtime: 0, folder: "/d" }, { name: "b", mtime: 0, folder: "/d" }], o({ name: "[N]x" }), () => ["a", "b", "bx"]);
  assert.deepEqual(p.rows.map((r) => r.error), [null, "Already exists"]);
});

test("pack formats: names, suffixes and form checks", () => {
  const f = (name) => ({ name, isDir: false });
  assert.equal(lib.packArchiveName([f("report.pdf")], "/x", ".tar.gz"), "report.tar.gz");
  assert.equal(lib.packArchiveName([f("report.pdf")], "/x", ".gz"), "report.pdf.gz");
  assert.equal(lib.packFormatOf("a.TGZ"), "tgz");
  assert.equal(lib.packFormatOf("a.tar.gz"), "tgz");
  assert.equal(lib.packFormatOf("a.gz"), "gz");
  assert.equal(lib.packFormatOf("a.rar"), null);
  assert.equal(lib.withPackFormat("/x/site.zip", "7z"), "/x/site.7z");
  assert.equal(lib.withPackFormat("/x/site.tar.gz", "tar"), "/x/site.tar");
  assert.equal(lib.withPackFormat("/x/site", "zip"), "/x/site.zip");
  assert.equal(lib.packFormError({ target: "a.rar" }), "The name must end in .zip, .tar, .tar.gz, .tgz, .tar.bz2, .tar.xz, .gz or .7z.");
  assert.equal(lib.packFormatOf("x.tar.xz"), "txz");
  assert.equal(lib.withPackFormat("/x/a.tar.bz2", "zip"), "/x/a.zip");
  assert.equal(lib.archiveKindOf("x.TBZ2"), "tbz2");
  assert.equal(lib.archiveKindOf("x.tar.xz"), "txz");
  assert.equal(lib.archiveKindOf("x.xz"), null);
  assert.equal(lib.packFormError({ target: "a.tar", encrypt: true, password: "x", repeat: "x" }), "Only ZIP archives can have a password.");
  assert.match(lib.packFormError({ target: "a.gz", sources: [f("a"), f("b")] }), /exactly one file/);
  assert.equal(lib.packFormError({ target: "a.gz", sources: [f("a")] }), null);
  assert.equal(lib.packedText({ done: 2 }, "/x/a.zip", true), "Added 2 item(s) to a.zip");
});

test("player: paused is active and shows in the strip", () => {
  assert.equal(lib.audioActive({ state: "paused" }), true);
  const v = lib.playerStrip({ state: "paused", name: "s.mp3", position: 3, duration: 10 });
  assert.equal(v.paused, true);
  assert.equal(v.text, "s.mp3");
  assert.equal(lib.playerStrip({ state: "playing", name: "s", position: 0, duration: 1 }).paused, false);
});

test("multi-rename: counter per folder", () => {
  const o = { ...lib.MULTI_RENAME_DEFAULTS, name: "[P]_[C]", digits: 2, perFolder: true };
  const es = [
    { name: "a.jpg", mtime: 0, folder: "/x/A" },
    { name: "b.jpg", mtime: 0, folder: "/x/B" },
    { name: "c.jpg", mtime: 0, folder: "/x/A" },
  ];
  assert.deepEqual(lib.multiRenamePlan(es, o).rows.map((r) => r.to), ["A_01.jpg", "B_01.jpg", "A_02.jpg"]);
  assert.deepEqual(lib.multiRenamePlan(es, { ...o, perFolder: false }).rows.map((r) => r.to), ["A_01.jpg", "B_02.jpg", "A_03.jpg"]);
});

test("multi-rename: names from a file", () => {
  assert.deepEqual(lib.namesFromText("\uFEFFone.txt\r\ntwo.txt\n\n"), ["one.txt", "two.txt"]);
  const o = { ...lib.MULTI_RENAME_DEFAULTS, names: ["x.txt", "y.txt"] };
  const es = ["a", "b", "c"].map((name) => ({ name, mtime: 0, folder: "/d" }));
  assert.deepEqual(lib.multiRenamePlan(es, o).rows.map((r) => r.to), ["x.txt", "y.txt", "c"]);
  assert.deepEqual(lib.multiRenamePlan(es, { ...o, case: "upper" }).rows.map((r) => r.to), ["X.TXT", "Y.TXT", "C"]);
  assert.equal(lib.multiRenamePlan(es, { ...o, names: ["", "y"] }).rows[0].error, "Invalid name");
});

test("realLocation maps a shown server archive path back to its copy", () => {
  const m = new Map([["/tmp/fxfm/edit-3/a.zip", "sftp://web/var"]]);
  assert.equal(lib.displayLocation("/tmp/fxfm/edit-3/a.zip/in", m), "sftp://web/var/a.zip/in");
  assert.equal(lib.realLocation("sftp://web/var/a.zip/in", m), "/tmp/fxfm/edit-3/a.zip/in");
  assert.equal(lib.realLocation("sftp://web/var/a.zip", m), "/tmp/fxfm/edit-3/a.zip");
  assert.equal(lib.realLocation("sftp://web/var/a.zipx", m), "sftp://web/var/a.zipx");
  assert.equal(lib.realLocation("/Users/x", m), "/Users/x");
});

test("smbHostLabel: Bonjour host name or NetBIOS address next to the name", () => {
  assert.equal(lib.smbHostLabel({ name: "studio", host: "studio", address: null }), "studio");
  assert.equal(lib.smbHostLabel({ name: "My NAS", host: "my-nas.local", address: null }), "My NAS (my-nas.local)");
  assert.equal(lib.smbHostLabel({ name: "PC-ANA", host: "PC-ANA", address: "192.168.1.20" }), "PC-ANA (192.168.1.20)");
});

test("Find files bridge: actions are built and parsed", () => {
  const a = lib.findFilesAction("s1:1", "go", { folder: "/x", cursor: "a" });
  assert.deepEqual(a, { id: "s1:1", action: "go", args: { folder: "/x", cursor: "a" } });
  assert.deepEqual(lib.parseFindFilesAction(JSON.parse(JSON.stringify(a))), a);
  assert.deepEqual(lib.findFilesAction("s1:2", "context").args, {});
  assert.throws(() => lib.findFilesAction("s1:3", "rm"));
  assert.throws(() => lib.findFilesAction("", "go"));
  assert.equal(lib.parseFindFilesAction({ id: "x", action: "delete" }), null);
  assert.equal(lib.parseFindFilesAction({ id: 7, action: "go" }), null);
  assert.equal(lib.parseFindFilesAction(null), null);
  assert.deepEqual(lib.parseFindFilesAction({ id: "x", action: "feed", args: [1] }).args, {});
});

test("Find files bridge: replies and commands", () => {
  assert.deepEqual(lib.findFilesReply("a", { location: "/Volumes/s" }), { id: "a", ok: true, value: { location: "/Volumes/s" } });
  assert.deepEqual(lib.findFilesReply("a"), { id: "a", ok: true, value: null });
  assert.deepEqual(lib.findFilesReply("a", null, new Error("boom")), { id: "a", ok: false, error: "boom" });
  assert.deepEqual(lib.findFilesReply("a", null, "no"), { id: "a", ok: false, error: "no" });
  assert.deepEqual(lib.parseFindFilesReply({ id: "a", ok: true }), { id: "a", ok: true, value: null });
  assert.deepEqual(lib.parseFindFilesReply({ id: "a", ok: false }), { id: "a", ok: false, error: "failed" });
  assert.equal(lib.parseFindFilesReply({ id: "a" }), null);
  assert.equal(lib.parseFindFilesReply({ ok: true }), null);
  assert.deepEqual(lib.parseFindFilesCommand({ cmd: "focus", location: "/x" }), { cmd: "focus", location: "/x" });
  assert.equal(lib.parseFindFilesCommand({ cmd: "quit" }), null);
  assert.equal(lib.parseFindFilesCommand("focus"), null);
});

test("findFilesGeometry keeps a usable size and a complete position", () => {
  assert.equal(lib.findFilesGeometry(null), null);
  assert.equal(lib.findFilesGeometry({}), null);
  assert.equal(lib.findFilesGeometry({ width: 300, height: 600 }), null);
  assert.equal(lib.findFilesGeometry({ width: "900", height: 600 }), null);
  assert.deepEqual(lib.findFilesGeometry({ width: 900.4, height: 600, columns: [] }), { width: 900, height: 600, maximized: false });
  assert.deepEqual(lib.findFilesGeometry({ width: 900, height: 600, x: -1200.2, y: 40, maximized: true }), {
    width: 900, height: 600, maximized: true, x: -1200, y: 40,
  });
  assert.deepEqual(lib.findFilesGeometry({ width: 900, height: 600, x: 10 }), { width: 900, height: 600, maximized: false });
});

test("iconKey: folders, bundles, extensions", () => {
  assert.equal(lib.iconKey({ up: true, name: "..", isDir: true }, "/x"), null);
  assert.equal(lib.iconKey({ name: "Docs", isDir: true }, "/Users/u"), "dir");
  assert.equal(lib.iconKey({ name: "Safari.app", isDir: true }, "/Applications"), "path:/Applications/Safari.app");
  assert.equal(lib.iconKey({ name: "X.app", isDir: true }, "sftp://web/opt"), "dir");
  assert.equal(lib.iconKey({ name: "A.PDF", isDir: false }, "/x"), "ext:pdf");
  assert.equal(lib.iconKey({ name: ".bashrc", isDir: false }, "/x"), "file");
  assert.equal(lib.iconKey({ name: "trail.", isDir: false }, "/x"), "file");
  assert.equal(lib.iconStyle({ icons: "emoji" }), "emoji");
  assert.equal(lib.iconStyle({}), "finder");
});

test("pasteSource: own Copy/Cut while the clipboard holds it, else files copied elsewhere", () => {
  const clip = { kind: "move", paths: ["/a/x", "/a/y"], cwd: "/a", change: 7 };
  assert.deepEqual(lib.pasteSource(clip, { change: 7, files: ["/a/x", "/a/y"] }), { kind: "move", paths: ["/a/x", "/a/y"], cwd: "/a" });
  // Something else was copied since (e.g. in Finder): that is pasted, always as a copy.
  assert.deepEqual(lib.pasteSource(clip, { change: 8, files: ["/b/z"] }), { kind: "copy", paths: ["/b/z"], cwd: "/b" });
  // Own Copy of server items has no file URLs but still pastes.
  const remote = { kind: "copy", paths: ["sftp://s/x"], cwd: "sftp://s/", change: 3 };
  assert.deepEqual(lib.pasteSource(remote, { change: 3, files: [] }), { kind: "copy", paths: ["sftp://s/x"], cwd: "sftp://s/" });
  assert.equal(lib.pasteSource(null, { change: 1, files: [] }), null);
  assert.equal(lib.pasteSource(remote, { change: 4, files: [] }), null);
});

test("contextMenuItems: all items in order; empty space and .. only Paste", () => {
  const ids = (items) => items.map((i) => (i === "-" ? "-" : i[0]));
  assert.deepEqual(ids(lib.contextMenuItems({ name: "a" }, 1)), [
    "open", "cm_view", "open_with", "quick_look", "show_in_finder", "-",
    "cm_copy_names_to_clip", "cm_copy_full_names_to_clip", "-",
    "cm_cut_to_clipboard", "cm_copy_to_clipboard", "cm_paste_from_clipboard", "-",
    "cm_packfiles", "cm_delete", "cm_rename", "-", "cm_chmod",
  ]);
  assert.equal(lib.contextMenuItems({ name: "a" }, 3)[7][1], "Copy paths");
  assert.deepEqual(lib.contextMenuItems(null, 0), [["cm_paste_from_clipboard", "Paste"]]);
  assert.deepEqual(lib.contextMenuItems({ name: "..", up: true }, 0), [["cm_paste_from_clipboard", "Paste"]]);
  assert.deepEqual(lib.contextMenuItems(null, 0, ["cm_paste_from_clipboard"]), []);
});

test("contextMenuItems: switched-off items and their lines go; server items lose the local ones", () => {
  const ids = (items) => items.map((i) => (i === "-" ? "-" : i[0]));
  const off = ["cm_copy_names_to_clip", "cm_copy_full_names_to_clip", "cm_chmod", "cm_view", "open_with", "quick_look", "show_in_finder"];
  assert.deepEqual(ids(lib.contextMenuItems({ name: "a" }, 1, off)), [
    "open", "-", "cm_cut_to_clipboard", "cm_copy_to_clipboard", "cm_paste_from_clipboard", "-", "cm_packfiles", "cm_delete", "cm_rename",
  ]);
  assert.deepEqual(ids(lib.contextMenuItems({ name: "a" }, 1, [], false)).slice(0, 3), ["open", "cm_view", "-"]);
});

test("filterMatch: text inside the name, or masks with * and ?", () => {
  assert.equal(lib.filterMatch("Report.PDF", "port"), true);
  assert.equal(lib.filterMatch("Report.PDF", "*.pdf"), true);
  assert.equal(lib.filterMatch("a.txt", "*.pdf;*.txt"), true);
  assert.equal(lib.filterMatch("a.txt", "*.pdf"), false);
  assert.equal(lib.filterMatch("a.txt", "b"), false);
  assert.equal(lib.filterMatch("a.txt", "  "), true);
});

test("pushHistory: appends, replaces an equal last place, keeps HISTORY_MAX", () => {
  let h = lib.pushHistory([], { location: "/a" });
  h = lib.pushHistory(h, { location: "/b", cursorName: "x" });
  h = lib.pushHistory(h, { location: "/b", cursorName: "y" });
  assert.deepEqual(h, [{ location: "/a" }, { location: "/b", cursorName: "y" }]);
  for (let i = 0; i < 60; i++) h = lib.pushHistory(h, { location: `/${i}` });
  assert.equal(h.length, lib.HISTORY_MAX);
  assert.equal(h[h.length - 1].location, "/59");
});

test("openedByActivationKey: only Enter/Escape/Space just before the modal opened", () => {
  assert.equal(lib.openedByActivationKey({ key: "Enter", at: 1000 }, 1100), true);
  assert.equal(lib.openedByActivationKey({ key: "Escape", at: 1000 }, 1999), true);
  // F5 then Enter fast: the dialog opened by F5 takes the Enter at once.
  assert.equal(lib.openedByActivationKey({ key: "F5", at: 1000 }, 1001), false);
  assert.equal(lib.openedByActivationKey({ key: "Enter", at: 1000 }, 2000), false);
  assert.equal(lib.openedByActivationKey(null, 5), false);
});

test("copyName: Finder's copy names, extension kept, folders whole", () => {
  assert.equal(lib.copyName("a.txt", new Set(["a.txt"])), "a copy.txt");
  assert.equal(lib.copyName("a.txt", new Set(["a.txt", "a copy.txt"])), "a copy 2.txt");
  assert.equal(lib.copyName("x.tar.gz", new Set()), "x copy.tar.gz");
  assert.equal(lib.copyName("README", new Set()), "README copy");
  assert.equal(lib.copyName("v1.2", new Set(), true), "v1.2 copy");
});

test("keyTableRows: every command with its bound keys, or a dash", () => {
  const cmds = [{ id: "cm_copy", menu: "Files", title: "Copy" }, { id: "cm_x", menu: "Help", title: "X" }];
  const rows = lib.keyTableRows(cmds, { F5: "cm_copy", "Shift+F5": "cm_copy", F6: "cm_move" }, (k) => `[${k}]`);
  assert.deepEqual(rows, [["Files", "Copy", "[F5], [Shift+F5]"], ["Help", "X", "—"]]);
});

const srow = (rel, state, l, r, ld = "/L", rd = "/R") => ({
  rel,
  state,
  leftDir: ld,
  rightDir: rd,
  left: l === null ? null : { name: l ?? rel, isDir: false, size: 10, mtime: 0 },
  right: r === null ? null : { name: r ?? rel, isDir: false, size: 20, mtime: 0 },
});

test("syncDefaultAction / syncActionAllowed / syncSymbol", () => {
  assert.equal(lib.syncDefaultAction("leftOnly"), "right");
  assert.equal(lib.syncDefaultAction("leftNewer"), "right");
  assert.equal(lib.syncDefaultAction("rightOnly"), "left");
  assert.equal(lib.syncDefaultAction("rightNewer"), "left");
  for (const s of ["equal", "differs", "typeMismatch"]) assert.equal(lib.syncDefaultAction(s), "none");
  assert.equal(lib.syncActionAllowed(srow("a", "leftOnly", "a", null), "left"), false);
  assert.equal(lib.syncActionAllowed(srow("a", "leftOnly", "a", null), "right"), true);
  assert.equal(lib.syncActionAllowed(srow("a", "typeMismatch"), "right"), false);
  assert.equal(lib.syncActionAllowed(srow("a", "typeMismatch"), "none"), true);
  assert.equal(lib.syncActionAllowed(srow("a", "nameClash", "a", null), "right"), false);
  assert.equal(lib.syncModeAction("mirrorRight", srow("a", "nameClash", "a", null)), "none");
  const both = srow("d", "equal");
  both.left.isDir = true;
  both.right.isDir = true;
  assert.equal(lib.syncActionAllowed(both, "right"), false);
  assert.deepEqual(lib.syncPlan([both], ["right"]), []);
  assert.equal(lib.syncSymbol("right", "differs"), "→");
  assert.equal(lib.syncSymbol("none", "equal"), "=");
  assert.equal(lib.syncSymbol("none", "differs"), "≠");
});

test("syncPlan: one job per folder pair and direction, into the folder; never for none or mismatch", () => {
  const rows = [
    srow("a", "leftNewer"),
    srow("b", "leftOnly", "b", null),
    srow("c", "rightNewer"),
    srow("d/x", "leftOnly", "x", null, "/L/d", "/R/d"),
    srow("e", "equal"),
    srow("f", "typeMismatch"),
  ];
  const jobs = lib.syncPlan(rows, ["right", "right", "left", "right", "none", "right"]);
  assert.deepEqual(jobs, [
    { kind: "copy", sources: ["/L/a", "/L/b"], dest: "/R/", cwd: "/L" },
    { kind: "copy", sources: ["/R/c"], dest: "/L/", cwd: "/R" },
    { kind: "copy", sources: ["/L/d/x"], dest: "/R/d/", cwd: "/L/d" },
  ]);
  assert.deepEqual(lib.syncCounts(rows, ["right", "right", "left", "right", "none", "right"]), { right: 3, left: 1, rightBytes: 30, leftBytes: 20, delRight: 0, delLeft: 0 });
});

test("syncPlan: a target spelled differently (NFD/NFC) is overwritten at its exact path", () => {
  const rows = [srow("č.txt", "leftNewer", "č.txt", "č.txt", "sftp://s/a", "/R/")];
  assert.deepEqual(lib.syncPlan(rows, ["right"]), [{ kind: "copy", sources: ["sftp://s/a/č.txt"], dest: "/R/č.txt", cwd: "sftp://s/a" }]);
  assert.deepEqual(lib.syncPlan(rows, ["left"]), [{ kind: "copy", sources: ["/R/č.txt"], dest: "sftp://s/a/č.txt", cwd: "/R/" }]);
});

test("syncModeAction: mirror copies every difference one way and deletes what the source lacks", () => {
  const rows = {
    lo: srow("lo", "leftOnly", "lo", null),
    ro: srow("ro", "rightOnly", null, "ro"),
    ln: srow("ln", "leftNewer"),
    rn: srow("rn", "rightNewer"),
    df: srow("df", "differs"),
    eq: srow("eq", "equal"),
    tm: srow("tm", "typeMismatch"),
  };
  const run = (mode) => Object.fromEntries(Object.entries(rows).map(([k, r]) => [k, lib.syncModeAction(mode, r)]));
  assert.deepEqual(run("mirrorRight"), { lo: "right", ro: "delRight", ln: "right", rn: "right", df: "right", eq: "none", tm: "none" });
  assert.deepEqual(run("mirrorLeft"), { lo: "delLeft", ro: "left", ln: "left", rn: "left", df: "left", eq: "none", tm: "none" });
  assert.deepEqual(run("updateRight"), { lo: "right", ro: "none", ln: "right", rn: "none", df: "none", eq: "none", tm: "none" });
  assert.deepEqual(run("updateLeft"), { lo: "none", ro: "left", ln: "none", rn: "left", df: "none", eq: "none", tm: "none" });
  assert.deepEqual(run("deleteRight"), { lo: "none", ro: "delRight", ln: "none", rn: "none", df: "none", eq: "none", tm: "none" });
  assert.deepEqual(run("deleteLeft"), { lo: "delLeft", ro: "none", ln: "none", rn: "none", df: "none", eq: "none", tm: "none" });
  assert.deepEqual(run("twoWay"), { lo: "right", ro: "left", ln: "right", rn: "left", df: "none", eq: "none", tm: "none" });
});

test("syncActionAllowed / syncPlan: deletes need that side's item, never a shared folder; copies come first", () => {
  assert.equal(lib.syncActionAllowed(srow("lo", "leftOnly", "lo", null), "delRight"), false);
  assert.equal(lib.syncActionAllowed(srow("lo", "leftOnly", "lo", null), "delLeft"), true);
  const both = srow("d", "equal");
  both.left.isDir = true;
  both.right.isDir = true;
  assert.equal(lib.syncActionAllowed(both, "delRight"), false);
  const rows = [srow("ro", "rightOnly", null, "ro"), srow("lo", "leftOnly", "lo", null), srow("x", "leftOnly", "x", null, "/L/d", "/R/d")];
  assert.deepEqual(lib.syncPlan(rows, ["delRight", "right", "delLeft"]), [
    { kind: "copy", sources: ["/L/lo"], dest: "/R/", cwd: "/L" },
    { kind: "delete", paths: ["/R/ro"], toTrash: true },
    { kind: "delete", paths: ["/L/d/x"], toTrash: true },
  ]);
  assert.deepEqual(lib.syncCounts(rows, ["delRight", "right", "delLeft"]), { right: 1, left: 0, rightBytes: 10, leftBytes: 0, delRight: 1, delLeft: 1 });
  assert.equal(lib.syncSymbol("delRight", "rightOnly"), "→✕");
  assert.equal(lib.syncSymbol("delLeft", "leftOnly"), "✕←");
});

test("syncSort: by direction → ← = ≠, by size/date/name, missing side last, desc", () => {
  const rows = [
    srow("e", "equal"),
    srow("d", "differs"),
    srow("l", "rightNewer"),
    srow("r2", "leftOnly", "r2", null),
    srow("r1", "leftNewer"),
    srow("x", "rightOnly", null, "x"),
  ];
  const actions = ["none", "none", "left", "right", "right", "delRight"];
  const all = [0, 1, 2, 3, 4, 5];
  const rels = (idx) => idx.map((i) => rows[i].rel);
  assert.deepEqual(rels(lib.syncSort(rows, actions, all, "action")), ["r1", "r2", "x", "l", "e", "d"]);
  assert.deepEqual(rels(lib.syncSort(rows, actions, all, "action", true)), ["d", "e", "l", "x", "r1", "r2"]);
  assert.deepEqual(rels(lib.syncSort(rows, actions, all, "leftName")), ["d", "e", "l", "r1", "r2", "x"]);
  assert.deepEqual(rels(lib.syncSort(rows, actions, all, "leftName", true)), ["r2", "r1", "l", "e", "d", "x"]);
  rows[0].right.size = 5;
  assert.deepEqual(rels(lib.syncSort(rows, actions, all, "rightSize")), ["e", "d", "l", "r1", "x", "r2"]);
  assert.deepEqual(rels(lib.syncSort(rows, actions, [2, 0], null)), ["l", "e"]);
});
