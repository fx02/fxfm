# fxfm plan 1: local file manager – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dual-panel macOS file manager that looks and behaves like Total Commander for local disks:
- panels and a drive bar;
- TC keys with remapping;
- F3, F4, F5, F6, F7, F8, F2, Shift+F4;
- Ctrl+D hotlist;
- portable or system settings;
- remembering the last location.

**Architecture:** Cargo workspace modeled on fxmp (`~/Documents/PVT/fxmp`):
- `crates/core` (`fxfm-core`) holds all logic without Tauri: listing, volumes, operations, keymap, hotlist, settings and viewer content. It must also build on Linux.
- `src-tauri` (`fxfm`) is a thin layer of Tauri commands and events, plus the macOS parts: menu, Terminal.app, quarantine/translocation.
- `ui/` is vanilla HTML/JS/CSS with no bundler. Pure functions live in `ui/lib.mjs`, tested with `node --test`.

Archives (plan 2) and servers (plan 3) will plug in later through a scheme in the path (`/x/a.zip/…`, `sftp://…`). That is why locations are passed between the UI and the backend as `String`, not `PathBuf`.

**Tech Stack:**
- Rust 1.99.1 (pinned), Tauri 2 (tauri-cli 2.12)
- crates: serde/serde_json, thiserror, libc, filetime, trash 5, encoding_rs, unicode-normalization, base64, tempfile (dev)
- `node:test` for UI helpers

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (+ `tc-screenshots/`)

## Global Constraints

- **Platform and build:**
  - macOS 15+.
  - Universal build: `cargo tauri build --target universal-apple-darwin`.
  - Identifier is `hr.pvt.fxfm`, productName `fxfm`.
- **Tauri isolation:**
  - `fxfm-core` does not depend on Tauri.
  - macOS-only code exists only in `src-tauri`, inside `#[cfg(target_os = "macos")]`.
  - `cargo build -p fxfm-core` and `cargo test -p fxfm-core` must also pass on Linux.
- **Keys:**
  - All use the physical **Ctrl**, and Option is Alt.
  - Cmd is not used for TC commands; the exceptions are Cmd+Q and the standard Edit menu (Cmd+C/V/X/A in text fields).
- **Look** per `docs/superpowers/specs/tc-screenshots/fxfm-mockup-approved.html`:
  - black list, folders as `[name]`, `[..]` always first, gray cursor;
  - status `<DIR>` or size, date `dd.MM.yyyy HH:mm`, permissions `drwxr-xr-x`;
  - free space `288.594.048 k of 971.350.180 k free`.
- **Default colors:**

  | type | color |
  |---|---|
  | folders, files | `#ffffff` |
  | hidden | `#9a9a9a` |
  | archives | `#ff6a4d` |
  | video/audio | `#8a8aff` |
  | marked | `#ff4d4d` |
  | background | `#000000` |
  | cursor | `#555555` |

- **Settings:**
  - Portable: `fxfm-data/` next to `fxfm.app`, if it exists and is writable.
  - Otherwise system: `~/Library/Application Support/fxfm/` (on Linux `$XDG_DATA_HOME/fxfm`).
  - Files: `settings.json`, `session.json`, `keymap.json`, `hotlist.json`.
  - The format is JSON with 2-space indentation, and writes are atomic (tmp + rename).
- **Session:**
  - On first launch both panels start in `~`.
  - After that the paths of both panels, the active panel and the name under the cursor are restored.
  - If a path does not exist, go to the nearest existing ancestor folder, and if none exists, to `~`.
  - A location with a scheme (`sftp://` etc.) or inside an archive is restored as `~`.
- **F8** sends to Trash, Shift+F8 deletes permanently, with confirmation in both cases.
- **UI texts** are in English, like TC (Files, Mark, Commands…, "F5 Copy"). Decision from §11 (menu in the menu bar, TC names).
- **Commits** go only as `zbuzanic@gmail.com` (already configured in the repo). Every commit ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Folder with 50 000 files** (`node_modules`, `/usr/share/man/...`):
   - Listing takes under 1 s, scrolling is smooth, the UI does not freeze.
   - Tests: Task 4 `list_dir_50k_entries_is_fast`, Task 10 `visibleRange` tests (the UI draws only the visible rows).
2. **Folder without permissions** (chmod 000, TCC-protected `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Library/Mail`):
   - The panel stays on the old folder and the status line shows `Access denied: <path>`.
   - On the first such case, opening System Settings → Privacy → Full Disk Access is offered once.
   - Tests: Task 4 `list_dir_permission_denied_is_typed_error`, Task 10 `navigateKeepsOldPathOnError`.
3. **Copying or moving into itself** or into its own subfolder, as well as moving to the same place:
   - Refused with a message and nothing is touched.
   - Symlinks are copied as symlinks (no infinite loop).
   - Tests: Task 6 `copy_into_own_subdir_is_refused`, `move_to_same_dir_is_noop_error`, `symlink_copied_as_link`.
4. **Names with spaces, quotes, Croatian characters and emojis** (NFD and NFC):
   - Sorting and the `*.php` mask work.
   - The command line and the Terminal editor quote `it's 'ok'.sh` correctly.
   - Tests: Task 4 `sort_is_case_insensitive_and_nfc`, Task 9 `shell_quote_handles_single_quotes`, Task 10 `matchMask` tests with `čćžšđ`.
5. **A volume disappears or a folder is deleted while a panel is on it** (ejected USB):
   - Ctrl+R or the auto-refresh on volume change moves the panel to the nearest existing ancestor folder, with no empty list showing an error.
   - Tests: Task 3 `restore_path_falls_back_to_nearest_ancestor`, Task 10 `refreshFallsBackOnMissingDir`.

---

## File Structure

```
Cargo.toml                       workspace (members: crates/core, src-tauri), release profile as in fxmp
rust-toolchain.toml              1.99.1, rustfmt+clippy
.gitignore                       (exists)
README.md                        build, moving to another Mac, where the data lives
crates/core/Cargo.toml           fxfm-core
crates/core/src/lib.rs
crates/core/src/storage.rs       app_folder, DataLocation, resolve, migrate, load_json/save_json
crates/core/src/config.rs        Settings, EditorConfig, Colors, SessionState, PanelState, restore_path
crates/core/src/listing.rs       Entry, list_dir, sort_entries, ListError
crates/core/src/volumes.rs       Volume, VolumeKind, list_volumes, space
crates/core/src/keymap.rs        KeyCombo, CommandDef, COMMANDS, Keymap, SYSTEM_RESERVED
crates/core/src/ops.rs           copy/move/delete/mkdir/rename, OpObserver, ConflictAnswer
crates/core/src/hotlist.rs       Hotlist, HotNode, HotItem, parse_cd, menu_label
crates/core/src/viewer.rs        ViewContent, read_for_view, decode_text
crates/core/tests/keycombo_fixture.rs   fixture shared with ui/lib.test.mjs
tests/fixtures/keycombos.json    pairs (KeyboardEvent → canonical string)
src-tauri/Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json, icons/
src-tauri/src/main.rs            bootstrap, AppState, plugin setup
src-tauri/src/state.rs           AppState (DataLocation, Settings, Keymap, Hotlist, Jobs)
src-tauri/src/commands.rs        #[tauri::command] functions
src-tauri/src/jobs.rs            background jobs, TauriObserver, events
src-tauri/src/menu.rs            menu bar built from COMMANDS
src-tauri/src/launch.rs          editor, Terminal, open, shell_quote
src-tauri/src/translocation.rs   detection + quarantine fix (macOS)
src-tauri/src/volwatch.rs        /Volumes watcher
ui/index.html, ui/style.css
ui/app.js                        bootstrap, global key dispatch, menu events
ui/panel.js                      Panel (virtual list, navigation, marks)
ui/dialogs.js                    copy/move/mkdir/delete/rename/mask/progress/conflict/error
ui/hotlist.js                    Ctrl+D popup and Configure dialog
ui/config.js                     Keyboard and Options dialogs
ui/viewer.html, ui/viewer.js     F3 window
ui/lib.mjs, ui/lib.test.mjs      pure functions + tests
```

## Shared Types (used by all tasks)

Rust structs that travel to the UI derive `Serialize, Deserialize, Clone, Debug, PartialEq`, with `#[serde(rename_all = "camelCase")]`. Enums use `#[serde(tag = "kind", rename_all = "camelCase")]`.

---

### Task 1: Project skeleton

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `crates/core/Cargo.toml`, `crates/core/src/lib.rs`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/icons/*`
- Create: `ui/index.html`, `ui/style.css`, `ui/app.js`, `ui/lib.mjs`, `ui/lib.test.mjs`

**Interfaces:**
- Produces:
  - a workspace that builds;
  - window `main`, 1300×820, title `fxfm`, `visible: false` (the UI shows it after bootstrap);
  - CSP as in fxmp, plus `img-src 'self' data: asset: http://asset.localhost` and `frame-src asset: http://asset.localhost` for the viewer;
  - Tauri feature `protocol-asset`.

- [ ] **Step 1:** Copy the pattern from fxmp.
  - `Cargo.toml` workspace with `members = ["crates/core", "src-tauri"]` and the same `[profile.release]`, no `[patch]`.
  - `rust-toolchain.toml` (1.99.1, minimal, rustfmt+clippy).
  - `tauri.conf.json`: `frontendDist: "../ui"`, `withGlobalTauri: true`, bundle targets `["app","dmg"]`, `minimumSystemVersion: "15.0"`, category `Utility`.
- [ ] **Step 2:** In `crates/core/src/lib.rs` add `pub fn version() -> &'static str` (returns `env!("CARGO_PKG_VERSION")`) and the test `version_is_set`. In `ui/lib.test.mjs` add a test `smoke` that imports `lib.mjs`.
- [ ] **Step 3:** Icon.
  - Copy `~/Documents/PVT/fxmp/src-tauri/icons/gen_app_icon.py` to `src-tauri/icons/`.
  - Change the content: a dark blue square, two white "panels" (two upright rectangles) and `fxfm` below.
  - Run `python3 src-tauri/icons/gen_app_icon.py && cargo tauri icon src-tauri/icons/app-icon-source.png`.
- [ ] **Step 4:** Verification.
  - Run: `rustup target add x86_64-apple-darwin && cargo test -p fxfm-core && node --test ui/ && cargo build -p fxfm`
  - Expected: all tests PASS, build OK.
- [ ] **Step 5:** `cargo tauri dev`.
  - Expected: a window "fxfm" opens with an empty page (for now `app.js` only does `getCurrentWindow().show()`).
- [ ] **Step 6:** Commit `chore: fxfm workspace skeleton (core + tauri + ui)`.

### Task 2: Data location and JSON storage

**Files:** Create `crates/core/src/storage.rs`

**Interfaces:**
- Produces:
  - `pub const DATA_DIR_NAME: &str = "fxfm-data";`
  - `pub fn app_folder(exe: &Path) -> Option<&Path>`: same semantics as `fxmp/src-tauri/src/portable.rs::app_folder`.
  - `pub enum StorageMode { Portable, System }`
  - `pub struct DataLocation { pub mode: StorageMode, pub dir: PathBuf }`
  - `pub fn system_dir() -> PathBuf`: on macOS `~/Library/Application Support/fxfm`, otherwise `$XDG_DATA_HOME/fxfm` or `~/.local/share/fxfm`.
  - `pub fn resolve(app_folder: Option<&Path>, system_dir: &Path) -> DataLocation`: portable if `app_folder/fxfm-data` exists, is a folder and is writable (checked by creating and deleting `.fxfm-write-test`). Otherwise system, with `create_dir_all`.
  - `pub fn migrate(from: &DataLocation, to: StorageMode, app_folder: Option<&Path>, system_dir: &Path) -> Result<DataLocation, StorageError>`: recursively copies all content (including `keys/`, if present).
    - Portable → System: after success renames `fxfm-data` to `fxfm-data.old` (if it exists, then `fxfm-data.old-2`, `-3`…), because otherwise portable would take precedence again.
    - System → Portable: creates `fxfm-data` and then copies, leaving the system folder untouched.
    - Must not overwrite an existing non-empty target: returns `StorageError::TargetNotEmpty`.
  - `pub fn load_json<T: DeserializeOwned + Default>(dir: &Path, name: &str) -> T`: a missing file yields `Default`. Broken JSON is renamed to `name.broken` and also yields `Default` (a warning is logged).
  - `pub fn save_json<T: Serialize>(dir: &Path, name: &str, v: &T) -> io::Result<()>`: writes `name.tmp` and then does a rename.

- [ ] **Step 1: tests** (in `storage.rs`, tempdir):
  - `app_folder_beside_exe_or_bundle`: same cases as the fxmp test, with `fxfm.app`.
  - `resolve_portable_only_with_writable_dir`: without the folder gives System. A file named `fxfm-data` gives System. A folder gives Portable. A folder with `chmod 0o555` gives System (the test is skipped when running as root).
  - `migrate_system_to_portable_copies_everything`: `settings.json` and `keys/a.key` must exist in the target.
  - `migrate_portable_to_system_renames_old`: after migration `fxfm-data` does not exist, `fxfm-data.old` exists, and `resolve` returns System.
  - `migrate_refuses_nonempty_target`.
  - `save_then_load_roundtrip` and `broken_json_is_set_aside`: after load, `settings.json.broken` exists and the result is `Default`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core storage`. Expected: FAIL (does not compile).
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): portable/system data location, migration, atomic json`.

### Task 3: Settings and session

**Files:** Create `crates/core/src/config.rs`

**Interfaces:**
- Consumes: `load_json`/`save_json` (Task 2)
- Produces:
  - `pub struct Settings { pub editor: EditorConfig, pub colors: Colors, pub show_command_line: bool /*true*/, pub font_size: u32 /*17*/, pub fda_hint_shown: bool /*false*/ }`, with `impl Default`.
  - `pub enum EditorConfig { System, App { path: String }, Terminal { command: String } }`
    - `System` means: VS Code if `/Applications/Visual Studio Code.app` exists, otherwise TextEdit. Resolved in Task 9.
  - `pub struct Colors { pub normal, hidden, archive, media, marked, background, cursor: String }`: default values from Global Constraints.
  - `pub enum Side { Left, Right }`
  - `pub struct PanelState { pub location: String, pub cursor_name: Option<String> }`
  - `pub struct SessionState { pub left: Option<PanelState>, pub right: Option<PanelState>, pub active: Side, pub window: Option<WindowGeom> }`
  - `pub struct WindowGeom { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }`
  - `pub fn restore_path(location: &str, home: &Path) -> PathBuf`:
    - for a `location` with a scheme (`://`) returns `home`;
    - for a path that goes through a file (e.g. `/a/b.zip/x`, where `b.zip` is a file) returns `home`;
    - otherwise climbs to the first existing folder, and if nothing exists, returns `home`.
  - Name constants: `SETTINGS_FILE = "settings.json"`, `SESSION_FILE = "session.json"`.

- [ ] **Step 1: tests:**
  - `settings_defaults`: `font_size == 17`, `colors.archive == "#ff6a4d"`, `editor == EditorConfig::System`.
  - `restore_path_falls_back_to_nearest_ancestor`: `tmp/a/b/c` where only `tmp/a` exists gives `tmp/a`.
  - `restore_path_scheme_goes_home`: `"sftp://web-1/var"` gives `home`.
  - `restore_path_through_file_goes_home`.
  - `restore_path_nothing_exists_goes_home` (e.g. `/Volumes/NEMA/x` gives `/Volumes`). Note: `/` always exists, so "nothing" in practice means the root. The test therefore checks that `/Volumes/NEMA-<pid>/x` gives `/Volumes`.
  - `session_roundtrip_json` (camelCase keys: `cursorName`).
- [ ] **Step 2:** Run `cargo test -p fxfm-core config`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): settings and session state with path restore`.

### Task 4: Local listing and volumes

**Files:** Create `crates/core/src/listing.rs`, `crates/core/src/volumes.rs`

**Interfaces:**
- Produces:
  - `pub struct Entry { pub name: String, pub is_dir: bool, pub is_symlink: bool, pub size: u64, pub mtime: i64 /*unix s*/, pub mode: u32, pub hidden: bool }`
    - `hidden` is true if the name starts with `.` or if on macOS `UF_HIDDEN` is set in `st_flags`.
    - For a symlink to a folder `is_dir = true` and `is_symlink = true`, and `size` and `mtime` come from the target. A broken symlink is a file with `size = 0`.
  - `pub enum ListError { NotFound(String), PermissionDenied(String), NotADirectory(String), Io(String) }`: `thiserror`, serialized as `{kind, path}`.
  - `pub fn list_dir(path: &Path) -> Result<Vec<Entry>, ListError>`: without `.` and `..`, sorted.
  - `pub fn sort_entries(v: &mut [Entry])`: folders before files. Within a group sorts by the key `name.nfc().to_lowercase()`, and on a tie by the original name.
  - `pub enum VolumeKind { Root, Home, Volume, Network }`
  - `pub struct Volume { pub name: String, pub path: String, pub kind: VolumeKind, pub total: u64, pub free: u64 }`
  - `pub fn space(path: &Path) -> io::Result<(u64 /*total*/, u64 /*free*/)>`: via `statvfs`, `f_frsize * f_blocks` and `f_frsize * f_bavail`.
  - `pub fn list_volumes(home: &Path) -> Vec<Volume>` returns in order:
    1. `/` named after the root disk volume (on macOS the name of the symlink in `/Volumes` that points to `/`, typically "Macintosh HD"; fallback "/");
    2. `home` (name `~`);
    3. every entry in `/Volumes` except the symlink to `/`, alphabetically. `Network` if `f_fstypename` from `statfs` is one of `smbfs|afpfs|nfs|webdav`, otherwise `Volume`.

    On Linux: `/`, `~`, then the mount points from `/proc/mounts` under `/media`, `/mnt` and `/run/media`.

- [ ] **Step 1: tests** (tempdir):
  - `list_dir_sorts_dirs_first`: folders `b`, `A`, files `c.txt`, `.hidden` give the order `A`, `b`, `.hidden`, `c.txt`; `.hidden.hidden == true`.
  - `sort_is_case_insensitive_and_nfc`: `"Carl"`, `"čapek"` (NFD form `"c\u{30C}apek"`), `"Ante"` give `Ante`, `Carl`, `čapek`. NFD and NFC forms of the same names sort together.
  - `list_dir_reports_symlinks`: a symlink to a folder has `is_dir && is_symlink`, a broken symlink `!is_dir`.
  - `list_dir_permission_denied_is_typed_error`: `chmod 000` gives `ListError::PermissionDenied` (skipped when root).
  - `list_dir_missing_is_not_found`.
  - `list_dir_50k_entries_is_fast`: creates 50 000 empty files; `list_dir` must take `< 1 s` in a release build and `< 5 s` in a debug build, with `#[cfg_attr(debug_assertions, ignore)]` and run with `--release`.
  - `volumes_start_with_root_and_home`: `[0].kind == Root`, `[0].path == "/"`, `[1].kind == Home`, `[0].total > 0`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core listing volumes`. Expected: FAIL.
- [ ] **Step 3:** Implement (`libc` for `statvfs`, `statfs` and `st_flags`; `unicode-normalization`).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo test -p fxfm-core --release list_dir_50k`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): directory listing and volumes`.

### Task 5: Command registry and keymap

**Files:** Create `crates/core/src/keymap.rs`, `tests/fixtures/keycombos.json`, `crates/core/tests/keycombo_fixture.rs`

**Interfaces:**
- Produces:
  - `pub struct KeyCombo { pub ctrl: bool, pub alt: bool, pub shift: bool, pub cmd: bool, pub key: String }`
    - `FromStr` and `Display` in canonical form: modifiers in the order `Ctrl+Alt+Shift+Cmd+`, then the key. Parsing is case-insensitive, and `Option` is a synonym for `Alt`.
    - Key names: `F1`–`F12`, `A`–`Z`, `0`–`9`, `Tab`, `Enter`, `Backspace`, `Delete`, `Insert`, `Space`, `Escape`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Home`, `End`, `NumAdd`, `NumSubtract`, `NumMultiply`, `=`, `-`, `\`.
  - `pub struct CommandDef { pub id: &'static str, pub menu: &'static str, pub title: &'static str, pub defaults: &'static [&'static str], pub available: bool }`
  - `pub static COMMANDS: &[CommandDef]`. Exact content (`id` | menu | title | default keys | `available`):

| id | menu | title | defaults | avail |
|---|---|---|---|---|
| cm_view | Files | View | F3 | ✓ |
| cm_edit | Files | Edit | F4 | ✓ |
| cm_edit_new | Files | Edit new file | Shift+F4 | ✓ |
| cm_copy | Files | Copy | F5 | ✓ |
| cm_move | Files | Move | F6 | ✓ |
| cm_rename | Files | Rename | F2, Shift+F6 | ✓ |
| cm_mkdir | Files | New folder | F7 | ✓ |
| cm_delete | Files | Delete | F8, Delete | ✓ |
| cm_delete_permanent | Files | Delete permanently | Shift+F8, Shift+Delete | ✓ |
| cm_exit | Files | Exit | Alt+F4 | ✓ |
| cm_mark | Mark | Mark/unmark | Space, Insert | ✓ |
| cm_mark_mask | Mark | Select group | NumAdd, Ctrl+= | ✓ |
| cm_unmark_mask | Mark | Unselect group | NumSubtract, Ctrl+- | ✓ |
| cm_invert | Mark | Invert selection | NumMultiply, Ctrl+8 | ✓ |
| cm_mark_all | Mark | Select all | Ctrl+A | ✓ |
| cm_switch_panel | Commands | Switch panel | Tab | ✓ |
| cm_hotlist | Commands | Directory hotlist | Ctrl+D | ✓ |
| cm_parent | Commands | Parent directory | Backspace, Ctrl+PageUp | ✓ |
| cm_root | Commands | Root directory | Ctrl+\ | ✓ |
| cm_swap_panels | Commands | Swap panels | Ctrl+U | ✓ |
| cm_target_equal_source | Commands | Target = Source | (none) | ✓ |
| cm_search | Commands | Search | Alt+F7 | ✗ |
| cm_multi_rename | Files | Multi-rename | Ctrl+M | ✗ |
| cm_connect | Net | Connect to server | Ctrl+F | ✗ |
| cm_disconnect | Net | Disconnect | Ctrl+Shift+F | ✗ |
| cm_drive_left | Show | Left drive | Alt+F1 | ✓ |
| cm_drive_right | Show | Right drive | Alt+F2 | ✓ |
| cm_refresh | Show | Refresh | Ctrl+R | ✓ |
| cm_toggle_hidden | Show | Show hidden files | Ctrl+H | ✓ |
| cm_cmdline_focus | Commands | Command line | Ctrl+Enter | ✓ |
| cm_config_keyboard | Configuration | Keyboard… | (none) | ✓ |
| cm_config_options | Configuration | Options… | (none) | ✓ |

  Additional notes on the table:
  - Unavailable commands (`✗`) appear in the menu as disabled. Their key shows `"<title>: coming in a later version"` in the status line.
  - Cursor movement (Up, Down, PgUp, PgDn, Home, End), Enter and Escape are fixed panel keys and are not in the registry. This is a plan decision: they are not TC "commands" that get remapped.
  - Enter on `[..]` and on a folder enters it, and on a file opens it with the default application.

  Keymap API:
  - `pub struct Keymap { bindings: BTreeMap<String /*canonical combo*/, String /*cmd id*/> }`, serde as a map.
  - `Keymap::defaults() -> Keymap`
  - `fn resolve(&self, c: &KeyCombo) -> Option<&str>`
  - `fn bind(&mut self, c: &KeyCombo, cmd: &str) -> Option<String /*the command that lost this key*/>`
  - `fn unbind(&mut self, c: &KeyCombo)`
  - `fn combos_for(&self, cmd: &str) -> Vec<KeyCombo>`
  - `fn load_or_default(dir: &Path) -> Keymap`: reads `keymap.json` and drops unknown cmd ids and combo strings that do not parse.
  - `pub const SYSTEM_RESERVED: &[&str] = &["Ctrl+Up","Ctrl+Down","Ctrl+Left","Ctrl+Right","Ctrl+Space","Ctrl+Alt+Space"]`
  - `fn system_conflicts(&self) -> Vec<(String, String)>`

  The fixture `tests/fixtures/keycombos.json` is an array of `{ "event": {code, key, ctrlKey, altKey, shiftKey, metaKey}, "combo": "Ctrl+Shift+F6" }` with at least these cases:
  - `F5`, `Shift+F6`, `Alt+F1` (Mac gives `key:"¡"`, `code:"F1"`), `Ctrl+D`, `Alt+F7`;
  - `NumAdd` (`code:"NumpadAdd"`), `Ctrl+=` (`code:"Equal"`), `Ctrl+-` (`code:"Minus"`), `Ctrl+\` (`code:"Backslash"`), `Ctrl+8` (`code:"Digit8"`);
  - `Space`, `Tab`, `Enter` (and `code:"NumpadEnter"` gives `Enter`), `Ctrl+PageUp`, `Delete`, `Shift+Delete`, `Backspace`;
  - Option+letter on a Mac (`code:"KeyA"`, `key:"å"`, `altKey:true`) gives `Alt+A`.

  The key is derived from `event.code`, not from `event.key`.

- [ ] **Step 1: tests**
  - `combo_parse_display_roundtrip`: `"ctrl+shift+f6"` gives `"Ctrl+Shift+F6"`, and `"Option+F1"` gives `"Alt+F1"`.
  - `defaults_cover_table`: `F5→cm_copy`, `F2→cm_rename`, `Shift+F6→cm_rename`, `Space→cm_mark`, `Ctrl+=→cm_mark_mask`, `Alt+F7→cm_search`, `Tab→cm_switch_panel`.
  - `bind_reports_displaced`: `bind(F5, cm_view)` returns `Some("cm_copy")`.
  - `load_drops_unknown`.
  - `system_conflicts_lists_reserved`: after `bind(Ctrl+Up, cm_parent)` the list contains `("Ctrl+Up","cm_parent")`.
  - `defaults_have_no_duplicates`.
  - `crates/core/tests/keycombo_fixture.rs`: every `combo` from the fixture parses and prints identically.
- [ ] **Step 2:** In `ui/lib.test.mjs` add the test `comboFromEvent matches fixture`: it reads `../tests/fixtures/keycombos.json` and for each case expects `comboFromEvent(event) === combo`.
- [ ] **Step 3:** Run `cargo test -p fxfm-core keymap keycombo && node --test ui/`. Expected: FAIL.
- [ ] **Step 4:** Implement `keymap.rs` and `export function comboFromEvent(e): string | null` in `ui/lib.mjs`. It returns `null` for a bare modifier (`code` is `ShiftLeft` etc.).
- [ ] **Step 5:** Run the same. Expected: PASS.
- [ ] **Step 6:** Commit `feat(core,ui): command registry, TC keymap, shared key-combo fixture`.

### Task 6: File operations

**Files:** Create `crates/core/src/ops.rs`

**Interfaces:**
- Produces:
  - `pub struct Progress { pub current: String, pub done_bytes: u64, pub total_bytes: u64, pub done_files: u64, pub total_files: u64 }`
  - `pub struct ConflictInfo { pub source: String, pub target: String, pub source_size: u64, pub source_mtime: i64, pub target_size: u64, pub target_mtime: i64 }`
  - `pub enum ConflictAnswer { Overwrite, OverwriteAll, Skip, SkipAll, OverwriteAllOlder, Rename { new_name: String }, Cancel }`
  - `pub enum ErrorAnswer { Retry, Skip, SkipAll, Cancel }`
  - `pub trait OpObserver: Send + Sync { fn progress(&self, p: &Progress); fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer; fn error(&self, path: &str, msg: &str) -> ErrorAnswer; fn is_cancelled(&self) -> bool; }`
  - `pub struct OpSummary { pub done: u64, pub skipped: u64, pub cancelled: bool }`
  - `pub enum OpError { IntoItself(String), SameLocation(String), Io(String), Cancelled }`
  - `pub enum Dest { IntoDir(PathBuf), AsFile(PathBuf) }`
  - `pub fn resolve_dest(sources: &[PathBuf], dest: &str, cwd: &Path) -> Dest`: a relative `dest` is resolved against `cwd`.
    - `IntoDir` if `dest` ends with `/`, if it is an existing folder or if there are multiple sources.
    - Otherwise `AsFile` (a copy of a single file under another name, like TC).
  - `pub fn copy(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn move_(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn delete(paths: &[PathBuf], to_trash: bool, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn mkdir(parent: &Path, name: &str) -> io::Result<PathBuf>`: `name` may contain `/` (creates a chain).
  - `pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf>`: rejects `/` in the name and an existing target (`AlreadyExists`).
  - `pub fn dir_size(path: &Path, cancel: &AtomicBool) -> io::Result<(u64 /*bytes*/, u64 /*files*/)>`: recursive, does not follow symlinks, stops when `cancel` is set.
  - Rules:
    - Before starting, `total_files` and `total_bytes` are counted.
    - A copy preserves `mtime` (`filetime`) and Unix permissions. Symlinks are copied as symlinks.
    - Copying is done in 1 MiB chunks, calling `progress` and checking `is_cancelled` between chunks. An interrupted file is deleted.
    - `move_` does a `rename` on the same device; on `EXDEV` it does copy and then deletes the source.
    - The decisions `OverwriteAll`, `SkipAll` and `OverwriteAllOlder` apply until the end of the operation. `OverwriteAllOlder` overwrites only if the target is older, otherwise it skips.
    - `IntoItself`: the destination is the source or is inside the source (comparison of canonicalized paths).
    - `SameLocation`: move or copy of a file into its own folder without a new name.
    - `delete(to_trash: true)` uses the `trash` crate. If Trash is unavailable (e.g. a network volume), `error()` returns the message `"Cannot move to Trash"`, and the UI then offers permanent deletion.

- [ ] **Step 1: tests** (tempdir, `TestObserver` with scripted answers and recorded calls):
  - `copy_dir_recursive_preserves_mtime_and_mode`.
  - `copy_into_own_subdir_is_refused`: `copy([a], IntoDir(a/b))` returns `Err(IntoItself)`, and nothing was created.
  - `move_to_same_dir_is_noop_error`: returns `SameLocation`.
  - `conflict_overwrite_all_asks_once`: two collisions, one `conflict` call.
  - `conflict_skip_keeps_target`.
  - `conflict_overwrite_all_older`: a newer target stays, an older one is overwritten.
  - `conflict_rename_uses_new_name`.
  - `symlink_copied_as_link`: and the loop `a/loop -> a` terminates.
  - `cancel_mid_file_removes_partial`: the observer cancels after the first chunk of a 5 MiB file.
  - `progress_totals_counted_upfront`.
  - `resolve_dest_rules`: a single file and `"/x/novo.txt"` give `AsFile`; `"/x/"` gives `IntoDir`; two sources give `IntoDir`.
  - `mkdir_nested`, `rename_rejects_existing_and_slash`.
  - `dir_size_sums_recursive`: does not count the symlink target.
  - `delete_permanent_recursive`.

  The Trash test is `#[ignore]` because it touches the user's Trash. The manual check is in Task 11.
- [ ] **Step 2:** Run `cargo test -p fxfm-core ops`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy/move/delete/mkdir/rename with conflicts, progress, cancel`.

### Task 7: Hotlist model

**Files:** Create `crates/core/src/hotlist.rs`

**Interfaces:**
- Produces:
  - `pub struct HotItem { pub title: String, pub command: String, pub target: String }`
  - `pub enum HotNode { Item(HotItem), Submenu { title: String, children: Vec<HotNode> } }`, serde tag `kind`.
  - `pub struct Hotlist { pub nodes: Vec<HotNode>, pub hide_char: Option<char> /*None*/ }`
  - `HOTLIST_FILE = "hotlist.json"`
  - `pub fn parse_cd(command: &str, home: &Path) -> Option<String>`: accepts `cd X` (case-insensitive, with trim). Expands a leading `~`. Returns a location with a scheme (`sftp://…`) untouched. Anything else returns `None`.
  - `pub fn menu_label(title: &str) -> (String /*without &*/, Option<char> /*accelerator, lowercase*/)`: `&&` gives a literal `&`.
  - `impl Hotlist { fn add_current(&mut self, location: &str, home: &Path) /*title = last component, command = "cd <loc>" with ~ for home*/; fn visible(&self) -> Vec<HotNode> /*without items whose title starts with hide_char, recursively*/ }`

- [ ] **Step 1: tests:**
  - `parse_cd_variants`: `"cd ~/Downloads"` gives `<home>/Downloads`; `"CD /tmp "` gives `/tmp`; `"cd sftp://web-1/var/www"` stays the same; `"ls"` gives `None`.
  - `menu_label_accelerator`: `"&Downloads"` gives `("Downloads", Some('d'))`, `"A&&B"` gives `("A&B", None)`.
  - `add_current_uses_tilde`: `<home>/Documents/PVT` gives title `"PVT"` and command `"cd ~/Documents/PVT"`.
  - `visible_hides_prefixed`: with `hide_char = Some('?')` the item `"?tajno"` is hidden, also in a submenu.
  - `json_roundtrip`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core hotlist`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): directory hotlist model`.

### Task 8: Content for the F3 viewer

**Files:** Create `crates/core/src/viewer.rs`

**Interfaces:**
- Produces:
  - `pub enum ViewContent { Text { encoding: String, text: String, truncated: bool }, Image { mime: String }, Pdf, Hex { bytes: Vec<u8>, offset: u64, total: u64 } }`
  - `pub const TEXT_LIMIT: u64 = 10 * 1024 * 1024; pub const HEX_LIMIT: usize = 1024 * 1024;`
  - `pub fn read_for_view(path: &Path) -> io::Result<ViewContent>`:
    - the extensions `png jpg jpeg gif webp bmp tiff heic svg` give `Image` (mime by extension);
    - `pdf` gives `Pdf`;
    - otherwise reads up to `TEXT_LIMIT`. If there is a NUL in the first 8 KiB, gives `Hex` (the first `HEX_LIMIT`), otherwise `Text` via `decode_text`.
  - `pub fn decode_text(bytes: &[u8]) -> (String, &'static str)`:
    - a UTF-8 BOM or valid UTF-8 gives `"UTF-8"` (the BOM is stripped);
    - a UTF-16 LE/BE BOM gives `"UTF-16LE"` or `"UTF-16BE"`;
    - otherwise `"windows-1250"` (`encoding_rs`).
  - The UI loads images and PDFs itself through the asset protocol (`convertFileSrc`), so the backend does not send the bytes.

- [ ] **Step 1: tests:**
  - `utf8_with_bom`.
  - `cp1250_croatian`: the bytes `9A E6 E8 9E F0` give `"šćčžđ"` and `"windows-1250"`.
  - `binary_goes_hex`.
  - `truncates_large_text`: `TEXT_LIMIT + 10` bytes give `truncated == true`.
  - `image_and_pdf_by_extension`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core viewer`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): viewer content detection and text decoding`.

### Task 9: Tauri backend

**Files:** Create `src-tauri/src/{state.rs,commands.rs,jobs.rs,launch.rs,volwatch.rs}`, Modify `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–8.
- Produces (Tauri commands, name = JS `invoke` name; errors are returned as `String` or a serialized `ListError`):
  - `bootstrap() -> Bootstrap { home: String, user: String, version: String, session: SessionState, settings: Settings, keymap: BTreeMap<String,String>, commands: Vec<CommandInfo{id,menu,title,available}>, hotlist: Hotlist, storage: StorageInfo{mode, dir}, systemConflicts: Vec<(String,String)>, translocated: bool }`. This is where `restore_path` is applied to `session.left` and `session.right`; if they are `None`, `home` is used.
  - `list_dir(location: String) -> Result<Vec<Entry>, ListError>`
  - `list_volumes() -> Vec<Volume>`
  - `nearest_existing(location: String) -> String`: `restore_path`.
  - `dir_size(location: String) -> Result<(u64,u64), String>`: calls `ops::dir_size` on a background thread. A new call for the same panel cancels the previous one.
  - `save_session(session: SessionState)`, `save_settings(settings: Settings)`, `save_hotlist(hotlist: Hotlist)`
  - `save_keymap(bindings: BTreeMap<String,String>) -> Vec<(String,String)>` (returns the new system conflicts), `reset_keymap() -> BTreeMap<String,String>`
  - `set_storage_mode(mode: StorageMode) -> Result<StorageInfo, String>`: `storage::migrate`, then `AppState.data` is switched to the new location.
  - `start_job(req: JobRequest) -> u64`, where `JobRequest = Copy{sources:Vec<String>, dest:String, cwd:String} | Move{…} | Delete{paths:Vec<String>, toTrash:bool}`.
  - `answer_conflict(job: u64, answer: ConflictAnswer)`, `answer_error(job: u64, answer: ErrorAnswer)`, `cancel_job(job: u64)`
  - `mkdir(parent: String, name: String) -> Result<String,String>`, `rename(path: String, newName: String) -> Result<String,String>`
  - `view_content(path: String) -> Result<ViewContent,String>`, `open_viewer(path: String)` (new window `viewer-<n>`, URL `viewer.html?path=<enc>`, 1000×700)
  - `edit_file(path: String) -> Result<(),String>`, `edit_new(dir: String, name: String) -> Result<String,String>` (creates an empty file if it does not exist, then opens it in the editor)
  - `open_default(path: String)` (`tauri-plugin-opener`), `run_in_terminal(dir: String, command: String)`, `open_fda_settings()` (`open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`)
  - Events:
    - `job://progress {job, progress}`
    - `job://conflict {job, info}`
    - `job://error {job, path, msg}`
    - `job://done {job, summary | error}`
    - `volumes://changed Vec<Volume>`
  - `launch.rs`:
    - `pub fn shell_quote(s: &str) -> String`: POSIX single-quote; `'` becomes `'\''`.
    - `pub fn editor_command(cfg: &EditorConfig, file: &Path) -> std::process::Command`:
      - `System`: `open -a "Visual Studio Code" <f>` if `/Applications/Visual Studio Code.app` exists, otherwise `open -e <f>`;
      - `App{path}`: `open -a <path> <f>`;
      - `Terminal{command}`: osascript `tell application "Terminal" to do script "cd <q dir> && <command> <q file>"`, then `activate`.
    - `pub fn terminal_command(dir: &Path, cmd: &str) -> std::process::Command`: the same osascript pattern.
    - On Linux `xdg-open` and `$TERMINAL` behind `cfg`; this is not part of v1 testing.
  - `jobs.rs`:
    - Each job runs in its own thread.
    - `TauriObserver::conflict` emits `job://conflict` and blocks on a `std::sync::mpsc::Receiver` until `answer_conflict` arrives. `cancel_job` sets an `AtomicBool` and sends `Cancel` on the channel if the job is waiting.
    - `progress` is emitted at most every 100 ms.
  - `volwatch.rs`: a thread that calls `list_volumes` every 2 s and emits `volumes://changed` only when the set of `path`s changes.
  - Capabilities: `core:default`, `opener:default`, `core:window:allow-create`, `core:webview:allow-create-webview-window`; asset scope `["**"]`.

- [ ] **Step 1: tests** (unit, in `launch.rs` and `jobs.rs`):
  - `shell_quote_handles_single_quotes`: `it's 'ok'.sh` gives `'it'\''s '\''ok'\''.sh'`.
  - `editor_command_terminal_quotes_path`: args contain the quoted path with a space.
  - `job_bridge_delivers_answer`: `ConflictBridge` (the pure Rust part of `TauriObserver`, without Tauri) receives an answer from another thread; `cancel` unblocks the wait with `Cancel`.
- [ ] **Step 2:** Run `cargo test -p fxfm`. Expected: FAIL.
- [ ] **Step 3:** Implement. Similar to fxmp, `AppState` holds a `Mutex<…>` for settings, keymap and hotlist. Every `save_*` writes immediately to `DataLocation.dir`.
- [ ] **Step 4:** Run `cargo test -p fxfm && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): tauri commands, background jobs, launchers, volume watch`.

### Task 10: Main window and panels

**Files:** Modify `ui/index.html`, `ui/style.css`, `ui/app.js`, `ui/lib.mjs`, `ui/lib.test.mjs`; Create `ui/panel.js`

**Interfaces:**
- Consumes: commands and events from Task 9, `comboFromEvent` from Task 5.
- Produces:
  - `lib.mjs` (pure functions, all tested):
    - `formatK(bytes: number): string`: `floor(bytes/1024)`, dot as thousands separator, suffix `" k"`. Example: `295520305152` gives `"288.594.048 k"`.
    - `freeLine(name, free, total)` gives `"[Macintosh HD]  288.594.048 k of 971.350.180 k free"` (two spaces after `]`).
    - `formatDate(unixSecs, tz?)` gives `"05.11.2026 15:56"`; the test uses a fixed `TZ=Europe/Zagreb`.
    - `formatSize(entry)`: `"<DIR>"` or dot separators (`4218` gives `"4.218"`).
    - `formatMode(mode, isDir, isSymlink)` gives `"drwxr-xr-x"`, `"lrwxr-xr-x"`, `"-rw-r--r--"`.
    - `displayName(entry)`: `"[DEBIAN]"` for a folder, otherwise the name.
    - `colorClass(entry)`: `"hid" | "arc" | "med" | ""`.
      - Archives: `zip tar tgz gz 7z bz2 xz rar tbz2 txz`.
      - Media: `mp4 mkv mov avi webm mp3 flac wav m4a ogg aac`.
      - Hidden takes precedence.
    - `matchMask(name, mask)`: masks separated by `;` or a space, `*` and `?`, case-insensitive, NFC normalization. `*.*` also matches names without a dot, like TC.
    - `visibleRange(scrollTop, rowH, viewH, count, overscan=10) -> {start, end}`.
    - `parentLocation(loc)` (`"/"` stays `"/"`), `joinLocation(dir, name)`, `lastComponent(loc)`.
    - `markSummary(entries, marked)` gives the TC status `"4.218 k / 288.594 k in 2 / 31 file(s)"`.
    - `navigate(state, target, listFn)`: an async helper; returns `{ok, location, entries, cursorName}` or `{ok:false, error}` and does not change `state`. The panel uses it for every navigation.
  - `panel.js`: `class Panel { constructor(root, side, api); async go(location, cursorName?); refresh(); entries; cursorIndex; marked:Set<string>; location; focus(); }`
    - `[..]` is shown when the location is not `/`.
    - Enter on `[..]` goes to the parent folder and puts the cursor on the folder that was just left.
    - Space marks and moves the cursor down (TC Insert behavior). On a folder it additionally calls `dir_size`, and the size is shown in the size column instead of `<DIR>` and in `markSummary` (spec §5).
    - Marks are cleared after a successful operation.
    - A click sets the cursor, a double click does Enter, a click on the panel gives it focus.
  - Layout (per the mockup):
    - drive bar (buttons from `list_volumes`, icon by `kind`, active one highlighted);
    - path row (combo for the disk, `freeLine`, buttons `/` and `..`);
    - current path bar (`location + "/*.*"`; the active panel `#2f6fd0` with white text);
    - black list;
    - status line;
    - below both panels the command line (hidden when `settings.showCommandLine == false`) and the F-bar (F3 View, F4 Edit, F5 Copy, F6 Move, F7 NewFolder, F8 Delete, Alt+F4 Exit), where a click executes the command.
    - Font `-apple-system`, `settings.fontSize` px, row height `fontSize * 1.47`.
    - Panels split 50/50.
  - `app.js`:
    - global `keydown`: `comboFromEvent`, `keymap` lookup, `dispatch(cmdId)` and `preventDefault`. When focus is in an input or a dialog, the global dispatch does not run, except `Escape` and `Tab` inside dialogs;
    - in this task `dispatch` implements `cm_switch_panel`, `cm_parent`, `cm_root`, `cm_mark*`, `cm_invert`, `cm_mark_all`, `cm_refresh`, `cm_swap_panels`, `cm_toggle_hidden`, `cm_drive_left/right` (a popup with volumes next to the panel, arrows and Enter), `cm_exit` and `cm_cmdline_focus`; the other commands show a message in the status for now;
    - the session is saved (debounce 500 ms) after every navigation and focus change and on `onCloseRequested`; the window geometry and position are saved too;
    - `volumes://changed`: refresh the drive bars. A panel whose folder no longer exists does `nearest_existing` and `go`;
    - `PermissionDenied`: status `Access denied: <loc>`. If `!settings.fdaHintShown`, show the dialog "fxfm needs Full Disk Access…" once, with the buttons Open Settings and Later, then `fdaHintShown = true`;
    - window title `"<user> – fxfm <version>"`.

- [ ] **Step 1: tests** in `ui/lib.test.mjs`:
  - `formatK`, `freeLine`, `formatDate`, `formatSize`, `formatMode` with the exact examples above.
  - `colorClass` for `.DS_Store` gives `hid`, `a.tgz` gives `arc`, `.x.zip` gives `hid`.
  - `matchMask('index.PHP','*.php')`, `matchMask('čćžšđ.txt','*.txt')`, an NFD name against an NFC mask, `matchMask('Makefile','*.*') === true`, `matchMask('a.sh','*.php;*.sh')`.
  - `visibleRange(0,25,500,50000)` gives `{start:0,end:30}`, and `visibleRange(25*1000,25,500,50000)` gives `{start:990,end:1030}`.
  - `parentLocation('/')`, `parentLocation('/a/b')`.
  - `navigateKeepsOldPathOnError`: `listFn` throws `{kind:'permissionDenied'}`, the result is `ok:false`, and `state.location` is unchanged.
  - `refreshFallsBackOnMissingDir`: `listFn` throws `notFound`, then `nearest` returns the parent and `navigate` ends up on it.
  - `markSummary`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement `lib.mjs`, then `panel.js`, `index.html`, `style.css` and `app.js`.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: manual check** (`cargo tauri dev`):
  - first launch: both panels are in `~`;
  - Tab switches the active panel (blue path bar);
  - Enter and Backspace through folders, and after `[..]` the cursor returns to the folder just left;
  - Space and Ctrl+= `*.md` mark in red;
  - Alt+F1 offers a choice of disk;
  - plug in a USB drive: the button appears within ~2 s; eject it while a panel is on it: the panel goes to `/Volumes`;
  - close and reopen: same folders and cursor;
  - open `~/Library/Mail`: the message and the FDA dialog appear once;
  - open a folder with 50 000 files (`mkdir /tmp/big && cd /tmp/big && seq 50000 | xargs touch`): scrolling is smooth.
- [ ] **Step 6:** Commit `feat(ui): TC main window, panels, drive bar, navigation, session restore`.

### Task 11: Operation dialogs

**Files:** Create `ui/dialogs.js`; Modify `ui/app.js`, `ui/style.css`

**Interfaces:**
- Consumes: `start_job`, `answer_*`, `cancel_job`, `mkdir`, `rename`, `edit_new`, `job://*` events.
- Produces: `dispatch` for `cm_copy`, `cm_move`, `cm_delete`, `cm_delete_permanent`, `cm_mkdir`, `cm_rename`, `cm_edit_new`, `cm_mark_mask`, `cm_unmark_mask`.
  - All dialogs are modal HTML (TC gray palette: `#f0f0f0`, buttons with a border). Enter does OK, Esc does Cancel, and Tab and Shift+Tab cycle within the dialog.
  - Sources are the marked items; if there are none, the item under the cursor. `[..]` is never taken.
  - **F5 / F6**:
    - title "Copy" or "Move";
    - text `Copy 3 file(s) to:` or `Copy "a.txt" to:`;
    - an input with the value `<other panel location>/` (for a single file its name is appended, fully selected, like TC);
    - OK starts `start_job`.
  - **Progress**: shown if the job lasts longer than 300 ms. Contains the current file, a per-file bar, a total bar (bytes) and a Cancel button. After `job://done` both panels are refreshed and the marks are cleared.
  - **Conflict**: text "File exists" with the size and date of the source and the target. Buttons: Overwrite, Overwrite all, Skip, Skip all, Overwrite older, Rename, Cancel. Rename opens an input with the suggested name `name (2).ext`.
  - **Error**: a message with the buttons Retry, Skip, Skip all, Cancel. For `"Cannot move to Trash"` a "Delete permanently" button is added, which starts a new job with `toTrash:false` for the remaining items.
  - **F8**: `Do you really want to move 3 file(s)/dir(s) to the Trash?`. Shift+F8: `…delete 3 file(s)/dir(s) permanently?`.
  - **F7**: a "New folder" input; after creation the cursor stands on the new folder.
  - **F2 / Shift+F6**: in-place rename, i.e. an input over the row. For files the part of the name without the extension is selected, for folders the whole name. Enter saves, Esc cancels.
  - **Shift+F4**: a "New file name" input, then `edit_new`.
  - **Num+ / Num−**: "Select files" or "Unselect files" with a mask (default `*.*`, remembers the last one within the UI session); `matchMask` is applied. Folders are not marked, unless the mask ends with `/`.

- [ ] **Step 1: tests** in `lib.test.mjs` for the new pure functions:
  - `suggestRename('a.txt', existing)` gives `'a (2).txt'`, and if that exists too, `'a (3).txt'`.
  - `renameSelection('archive.tar.gz')` gives `{start:0,end:7}` (up to the first dot for `.tar.*`, otherwise up to the last dot), and for a folder the whole name.
  - `copyTargetDefault(otherLoc, sources)`: for a single file `'/x/a.txt'`, for several `'/x/'`.
  - `operationSources(entries, marked, cursor)`: ignores `[..]`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: manual check**:
  - copy a folder with subfolders between panels;
  - create a collision and test all the buttons;
  - Cancel in the middle of a large file (`mkfile 2g /tmp/big.bin`) deletes the partial target;
  - F6 between `/tmp` and a USB drive;
  - F8 sends to Trash (check in Finder), Shift+F8 deletes permanently;
  - F7 `a/b/c`;
  - F2 rename;
  - F5 of a folder into its own subfolder gives the message "Cannot copy a folder into itself".
- [ ] **Step 6:** Commit `feat(ui): copy/move/delete/mkdir/rename dialogs with progress and conflicts`.

### Task 12: F3 viewer and F4 editor

**Files:** Create `ui/viewer.html`, `ui/viewer.js`; Modify `ui/app.js`

**Interfaces:**
- Consumes: `open_viewer`, `view_content`, `edit_file`, `settings.editor`.
- Produces:
  - `cm_view` on a file calls `open_viewer`. On a folder and on `[..]` it does nothing.
  - `cm_edit` calls `edit_file`.
  - Viewer window:
    - `Text`: `<pre>` with a monospace font, black background and white text. At the bottom is the status `UTF-8 · 12.345 bytes`, with the mark `(truncated)` when truncated.
    - `Image`: `<img src=convertFileSrc(path)>`, scaled to fit.
    - `Pdf`: `<iframe src=convertFileSrc(path)>`. If WKWebView does not render PDF through the asset protocol, the fallback is an "Open in Quick Look" button (`qlmanage -p`, a new command `quick_look(path)` in the style of Task 9). Record the decision in the README.
    - `Hex`: 16 bytes per row, in the format `00000000  48 65 6c …  |Hel…|`.
  - Keys in the viewer: Esc, F3 and Ctrl+W close; Ctrl+F searches the text (simply: an input and then `window.find`); for text the keys `1`, `2` and `3` manually switch the encoding between UTF-8, windows-1250 and ISO-8859-2 (a new command `view_text_as(path, encoding)`, in core `decode_as(bytes, label)`).
  - Configuration → Options has the editor (Task 14). Until then it is `System`.

- [ ] **Step 1: test** in core: `decode_as_iso_8859_2` (the byte `0xB9` gives `"š"`). In `lib.test.mjs`: `hexLines(bytes, offset)` gives the exact row for `"Hello"`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core viewer && node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same. Expected: PASS.
- [ ] **Step 5: manual check**: F3 on `.php`, `.sh`, a cp1250 `.txt`, `.png` and `.pdf`, and on a binary file (`/bin/ls`). F4 opens VS Code (or TextEdit if VS Code is not installed).
- [ ] **Step 6:** Commit `feat: F3 viewer window and F4 external editor`.

### Task 13: Ctrl+D hotlist

**Files:** Create `ui/hotlist.js`; Modify `ui/app.js`, `ui/style.css`

**Interfaces:**
- Consumes: `bootstrap().hotlist`, `save_hotlist`, `parse_cd` semantics (the UI repeats it through the lib function `parseCd`, tested with the same examples as Task 7).
- Produces: `cm_hotlist`.
  - **Popup** (TC look, at the top left corner of the active panel):
    - `visible()` items;
    - submenus open with the right arrow or Enter;
    - accelerator from `&` (underlined letter, and pressing the letter executes the item);
    - below a separator "Add current dir" (`a`) and "Configure…" (`c`).
  - **Item selection**: `parseCd(command)` gives the location for the active panel. If `target` is not empty, `parseCd("cd "+target)` sends the other panel there. A location with a scheme (`sftp://`) in plan 1 shows the status `Remote locations come in a later version`.
  - **Configure dialog** modeled on the Total Commander Ctrl+D dialog:
    - title "Change directory menu";
    - on the left a list ("Directory menu:" with the item count at the top right), submenus indented with `--`;
    - on the right the buttons Add Item…, Add submenu…, Delete Item, Change Title…, OK, Cancel, Help (Help shows a short built-in help text about the Command and Target path fields);
    - at the bottom the fields Command (with a 🔍 button that opens a `tauri-plugin-dialog` folder picker and writes `cd <path>`) and Target path;
    - a checkbox "Hide menu items starting with this character:" with a single-character input, and the text "Double click menu item to hide/show it (add/remove the character)". A double click adds or removes the character at the start of the title;
    - the order is changed with Ctrl+↑/↓, the ↑/↓ buttons and drag & drop;
    - OK calls `save_hotlist`, Cancel discards the changes.

- [ ] **Step 1: tests** in `lib.test.mjs`:
  - `parseCd` with all the examples from Task 7;
  - `menuLabel` (same examples);
  - `moveNode(nodes, path, delta)`, which moves an item within the same parent and does not go out of bounds;
  - `toggleHidden(title, ch)`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: manual check**:
  - add Downloads and DEBIAN, plus a submenu with two items;
  - an item with a Target path changes both panels;
  - hiding by double click works;
  - after a restart the items are still there, in `fxfm-data/hotlist.json` or the system folder.
- [ ] **Step 6:** Commit `feat(ui): Ctrl+D directory hotlist with TC configure dialog`.

### Task 14: Menu, Configuration, command line, translocation, release

**Files:** Create `src-tauri/src/menu.rs`, `src-tauri/src/translocation.rs`, `ui/config.js`; Modify `src-tauri/src/main.rs`, `ui/app.js`; Create `README.md`

**Interfaces:**
- Consumes: `COMMANDS`, all commands from Task 9.
- Produces:
  - **Menu bar**:
    - app menu `fxfm` (About, Hide, Quit = Cmd+Q);
    - the standard Edit menu (predefined Undo/Redo/Cut/Copy/Paste/Select All, without which Cmd+C/V in inputs does not work);
    - then Files, Mark, Commands, Net, Show, Configuration, Start, Help, assembled from `COMMANDS` by the `menu` field, in the order of the table. `Start` is empty with one disabled item "(empty)".
    - Items **have no native accelerator**, because the menu would swallow keys before the webview. The default key is therefore appended to the title, e.g. `Copy    F5`.
    - A click emits `menu://command {id}`, which the UI forwards to `dispatch`.
    - Unavailable commands are disabled.
  - **Configuration → Keyboard** (`config.js`):
    - a table with the columns Command (menu › title) and Keys (chips with ×);
    - the "Add key…" button waits for a key press (`comboFromEvent`). If the key is taken, it asks `"Ctrl+R is used by Refresh. Reassign?"`;
    - a "Reset to TC defaults" button;
    - at the top a yellow warning if `systemConflicts` is not empty: `"macOS uses Ctrl+Up for Mission Control. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key."`, with an Open Keyboard Shortcuts button (`open "x-apple.systempreferences:com.apple.Keyboard-Settings.extension"`);
    - the same warning is shown once on first launch if a default key overlaps.
  - **Configuration → Options**:
    - *Storage*: radio Portable / System, the path of the current folder and an Apply button (`set_storage_mode`, with the confirmation `"Move all settings to …?"`). Portable cannot be chosen if the folder next to the app is not writable; a message is shown then.
    - *Editor*: radio System default / Application (`.app` chosen via a dialog) / Terminal command (input, e.g. `nano`).
    - *Colors*: 7 color pickers and Reset.
    - *Font size*: 12–24.
    - *Show command line*.
    - Saving does `save_settings` and applies the changes immediately.
  - **Command line**:
    - Enter does `run_in_terminal(active panel location, text)` and clears the input;
    - Ctrl+Enter from the panel focuses the line;
    - Esc returns focus to the panel;
    - if the line is empty, Enter from the panel works normally;
    - the prompt is `<location>>`.
  - **translocation.rs** (macOS):
    - `pub fn is_translocated(exe: &Path) -> bool`: the path contains `/AppTranslocation/`;
    - `pub fn original_path(exe: &Path) -> Option<PathBuf>`: FFI to `SecTranslocateCreateOriginalPathForURL` from Security.framework (`core-foundation` crate);
    - the command `fix_quarantine() -> Result<(), String>`: `xattr -dr com.apple.quarantine <original .app>`, then `open -n <original .app>` and `app.exit(0)`;
    - if `bootstrap().translocated`, the UI first of all shows the dialog `"macOS started fxfm from a temporary read-only location, so your settings next to the app can't be found."` with a command to copy and the buttons "Fix and restart" and "Continue without settings" (the session is then not saved).
  - **README.md**: what fxfm is, the build (`cargo tauri build --target universal-apple-darwin`, with the paths to `.app` and `.dmg`), portable setup (a folder `fxfm/` with `fxfm.app` and an empty `fxfm-data/`), moving to another Mac and the `xattr` command, where the data is, a table of default keys (generated from `COMMANDS`) and the note about PDF from Task 12.

- [ ] **Step 1: tests:**
  - `translocation::is_translocated` for `/private/var/folders/x/T/AppTranslocation/ABC/d/fxfm.app/Contents/MacOS/fxfm` is `true`, and for `/Applications/...` `false`.
  - `menu::menu_title_with_key("Copy", &["F5"])` gives `"Copy\tF5"`.
  - `lib.test.mjs`: `conflictMessage("Ctrl+Up")` gives the text from the spec.
- [ ] **Step 2:** Run `cargo test --workspace && node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 5: release build**
  - Run: `cargo tauri build --target universal-apple-darwin && lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm`
  - Expected: `x86_64 arm64`.
- [ ] **Step 6: manual check of the portable and translocation flow**:
  1. `mkdir -p /tmp/fxfm-port/fxfm-data && cp -R <bundle>/fxfm.app /tmp/fxfm-port/`, launch it, add a hotlist item. Check that `/tmp/fxfm-port/fxfm-data/hotlist.json` exists and that the system folder was not touched.
  2. `xattr -w com.apple.quarantine "0081;$(printf %x $(date +%s));Safari;" /tmp/fxfm-port/fxfm.app`, then launch by double click from Finder. Expect the translocation dialog; after "Fix and restart" the application works with the settings from `fxfm-data`.
  3. Options → Storage → System, Apply. The settings move to `~/Library/Application Support/fxfm`, and `fxfm-data.old` exists.
  4. Keyboard: remap F5 to Ctrl+K. After a restart Ctrl+K copies, and F5 does nothing. Then Reset.
  5. The command line `ls -la` opens Terminal in the panel's folder.
- [ ] **Step 7:** Commit `feat: menu bar, keyboard/options config, command line, translocation fix, README`.

---

## After plan 1

- **Plan 2 (archives)** adds a VFS trait to `fxfm-core`: `LocalFs` from Tasks 4 and 6 moves behind the trait, then `ArchiveFs` (zip, tar, tgz, gz, 7z) and "upload back" tracking of F4 copies (`notify`).
- **Plan 3 (servers)** adds `SftpFs`, `FtpFs`, the Ctrl+F dialog, keys, master password, Keychain and `known_hosts`.

Both plans are written after plan 1 is done, based on the actual code.
