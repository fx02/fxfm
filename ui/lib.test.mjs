import { test } from "node:test";
import assert from "node:assert/strict";
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

test("formatK floors to KiB with dot separators", () => {
  assert.equal(lib.formatK(295520305152), "288.594.048 k");
  assert.equal(lib.formatK(0), "0 k");
  assert.equal(lib.formatK(1023), "0 k");
  assert.equal(lib.formatK(1024 * 1000), "1.000 k");
});

test("freeLine", () => {
  assert.equal(
    lib.freeLine("Macintosh HD", 295520305152, 994662584320),
    "[Macintosh HD]  288.594.048 k of 971.350.180 k free",
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
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 4218 }), "4.218");
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 7 }), "7");
  assert.equal(lib.formatSize({ name: "a", isDir: false, size: 1234567 }), "1.234.567");
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

test("colorClass", () => {
  const f = (name, hidden = name.startsWith(".")) => lib.colorClass({ name, isDir: false, hidden });
  assert.equal(f(".DS_Store"), "hid");
  assert.equal(f("a.tgz"), "arc");
  assert.equal(f(".x.zip"), "hid");
  assert.equal(f("B.TAR"), "arc");
  assert.equal(f("film.MKV"), "med");
  assert.equal(f("song.mp3"), "med");
  assert.equal(f("notes.txt"), "");
  assert.equal(f("zip"), "");
  assert.equal(lib.colorClass({ name: "x.zip", isDir: true, hidden: false }), "");
  assert.equal(f("flagged.txt", true), "hid");
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
  assert.equal(lib.markSummary(entries, new Set(["a", "b"])), "4 k / 288.594.048 k in 2 / 31 file(s)");
  const small = entries.map((e) => ({ ...e }));
  small[1].size = 295520300 - 4319359;
  small[30].size = 4319359 - 2319;
  small[31].size = 2319;
  assert.equal(lib.markSummary(small, new Set(["a", "b"])), "4.218 k / 288.594 k in 2 / 31 file(s)");
  assert.equal(lib.markSummary(small, new Set()), "0 k / 288.594 k in 0 / 31 file(s)");
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
  assert.equal(lib.formatViewStatus("UTF-8", 12345, false), "UTF-8 · 12.345 bytes");
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

test("conflictMessage names the macOS feature", () => {
  assert.equal(
    lib.conflictMessage("Ctrl+Up"),
    "macOS uses Ctrl+Up for Mission Control. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key.",
  );
});

test("conflictMessage falls back for unknown combos", () => {
  assert.match(lib.conflictMessage("Ctrl+Space"), /^macOS uses Ctrl\+Space for Select the previous input source\./);
  assert.match(lib.conflictMessage("Ctrl+F12"), /^macOS uses Ctrl\+F12 for a system shortcut\./);
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
  assert.deepEqual(s, { text: "c.wav", clock: "1:01 / 2:03", codec: "GSM 6.10, 8000 Hz, mono", progress: 0.5 });
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
    legacyAlgorithms: false, ftpPassive: true, ftpEncoding: "utf8",
  });
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

test("CHMOD_ONLY_REMOTE matches the backend text", () => {
  // Same as `CHMOD_ONLY_REMOTE` in src-tauri/src/remote.rs; change both together.
  assert.equal(lib.CHMOD_ONLY_REMOTE, "Change attributes works on SFTP and SCP servers");
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
