# fxfm plan 1: lokalni file manager – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Dvopanelni macOS file manager koji izgleda i ponaša se kao Total Commander za lokalne diskove:
- paneli i traka diskova;
- TC tipke s remapiranjem;
- F3, F4, F5, F6, F7, F8, F2, Shift+F4;
- Ctrl+D hotlista;
- portable ili sistemske postavke;
- pamćenje zadnje lokacije.

**Architecture:** Cargo workspace po uzoru na fxmp (`~/Documents/PVT/fxmp`):
- `crates/core` (`fxfm-core`) sadrži svu logiku bez Taurija: listing, volumeni, operacije, keymap, hotlistu, postavke i viewer sadržaj. Mora se buildati i na Linuxu.
- `src-tauri` (`fxfm`) je tanki sloj Tauri komandi i eventova, plus macOS stvari: meni, Terminal.app, quarantine/translocation.
- `ui/` je vanilla HTML/JS/CSS bez bundlera. Čiste funkcije su u `ui/lib.mjs`, testirane s `node --test`.

Arhive (plan 2) i serveri (plan 3) kasnije se priključuju preko sheme u putanji (`/x/a.zip/…`, `sftp://…`). Zato se lokacije između UI-ja i backenda prenose kao `String`, a ne `PathBuf`.

**Tech Stack:**
- Rust 1.99.1 (pinano), Tauri 2 (tauri-cli 2.12)
- crateovi: serde/serde_json, thiserror, libc, filetime, trash 5, encoding_rs, unicode-normalization, base64, tempfile (dev)
- `node:test` za UI helpere

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (+ `tc-screenshots/`)

## Global Constraints

- **Platforma i build:**
  - macOS 15+.
  - Universal build: `cargo tauri build --target universal-apple-darwin`.
  - Identifier je `hr.pvt.fxfm`, productName `fxfm`.
- **Tauri izolacija:**
  - `fxfm-core` ne ovisi o Tauriju.
  - macOS-only kod postoji samo u `src-tauri` i to unutar `#[cfg(target_os = "macos")]`.
  - `cargo build -p fxfm-core` i `cargo test -p fxfm-core` moraju proći i na Linuxu.
- **Tipke:**
  - Sve koriste fizički **Ctrl**, a Option je Alt.
  - Cmd se ne koristi za TC naredbe; izuzeci su Cmd+Q i standardni Edit meni (Cmd+C/V/X/A u tekstualnim poljima).
- **Izgled** prema `docs/superpowers/specs/tc-screenshots/fxfm-mockup-approved.html`:
  - crna lista, mape kao `[ime]`, `[..]` uvijek prvi, sivi kursor;
  - status `<DIR>` ili veličina, datum `dd.MM.yyyy HH:mm`, prava `drwxr-xr-x`;
  - slobodni prostor `288.594.048 k of 971.350.180 k free`.
- **Zadane boje:**

  | tip | boja |
  |---|---|
  | mape, datoteke | `#ffffff` |
  | skrivene | `#9a9a9a` |
  | arhive | `#ff6a4d` |
  | video/audio | `#8a8aff` |
  | označene | `#ff4d4d` |
  | pozadina | `#000000` |
  | kursor | `#555555` |

- **Postavke:**
  - Portable: `fxfm-data/` pored `fxfm.app`, ako postoji i zapisiva je.
  - Inače sistemski: `~/Library/Application Support/fxfm/` (na Linuxu `$XDG_DATA_HOME/fxfm`).
  - Datoteke: `settings.json`, `session.json`, `keymap.json`, `hotlist.json`.
  - Format je JSON s uvlakom od 2 razmaka, a zapis je atomski (tmp + rename).
- **Sesija:**
  - Kod prvog pokretanja oba panela kreću iz `~`.
  - Nakon toga se vraćaju putanje oba panela, aktivni panel i ime pod kursorom.
  - Ako putanja ne postoji, ide se na najbližu postojeću nadmapu, a ako ni nje nema, na `~`.
  - Lokacija sa shemom (`sftp://` i sl.) ili unutar arhive vraća se na `~`.
- **F8** šalje u Trash, Shift+F8 briše trajno, uz potvrdu u oba slučaja.
- **UI tekstovi** su na engleskom, kao TC (Files, Mark, Commands…, "F5 Copy"). Odluka iz §11 (meni u traci, TC nazivi).
- **Commitovi** idu samo kao `zbuzanic@gmail.com` (već postavljeno u repou). Svaki commit završava linijom `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Mapa s 50 000 datoteka** (`node_modules`, `/usr/share/man/...`):
   - Listing traje ispod 1 s, scroll je gladak, UI ne zamrzava.
   - Testovi: Task 4 `list_dir_50k_entries_is_fast`, Task 10 `visibleRange` testovi (UI crta samo vidljive retke).
2. **Mapa bez prava** (chmod 000, TCC zaštićeni `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Library/Mail`):
   - Panel ostaje na staroj mapi i u status liniji piše `Access denied: <putanja>`.
   - Kod prvog takvog slučaja jednom se ponudi otvaranje System Settings → Privacy → Full Disk Access.
   - Testovi: Task 4 `list_dir_permission_denied_is_typed_error`, Task 10 `navigateKeepsOldPathOnError`.
3. **Kopiranje ili premještanje u samog sebe** ili u vlastitu podmapu, kao i premještanje na isto mjesto:
   - Odbija se s porukom i ništa se ne dira.
   - Simlinkovi se kopiraju kao simlinkovi (bez beskonačne petlje).
   - Testovi: Task 6 `copy_into_own_subdir_is_refused`, `move_to_same_dir_is_noop_error`, `symlink_copied_as_link`.
4. **Imena s razmacima, navodnicima, hrvatskim znakovima i emojijima** (NFD i NFC):
   - Sortiranje i maska `*.php` rade.
   - Naredbeni redak i Terminal editor ispravno quotaju `it's 'ok'.sh`.
   - Testovi: Task 4 `sort_is_case_insensitive_and_nfc`, Task 9 `shell_quote_handles_single_quotes`, Task 10 `matchMask` testovi s `čćžšđ`.
5. **Volumen nestane ili se mapa obriše dok je panel na njoj** (izbačen USB):
   - Ctrl+R ili auto-refresh pri promjeni volumena prebacuje panel na najbližu postojeću nadmapu, bez prazne liste s greškom.
   - Testovi: Task 3 `restore_path_falls_back_to_nearest_ancestor`, Task 10 `refreshFallsBackOnMissingDir`.

---

## File Structure

```
Cargo.toml                       workspace (members: crates/core, src-tauri), release profil kao fxmp
rust-toolchain.toml              1.99.1, rustfmt+clippy
.gitignore                       (postoji)
README.md                        build, prijenos na drugi Mac, gdje su podaci
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
crates/core/tests/keycombo_fixture.rs   zajednički fixture s ui/lib.test.mjs
tests/fixtures/keycombos.json    parovi (KeyboardEvent → kanonski string)
src-tauri/Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json, icons/
src-tauri/src/main.rs            bootstrap, AppState, plugin setup
src-tauri/src/state.rs           AppState (DataLocation, Settings, Keymap, Hotlist, Jobs)
src-tauri/src/commands.rs        #[tauri::command] funkcije
src-tauri/src/jobs.rs            pozadinski jobovi, TauriObserver, eventovi
src-tauri/src/menu.rs            meni u gornjoj traci iz COMMANDS
src-tauri/src/launch.rs          editor, Terminal, open, shell_quote
src-tauri/src/translocation.rs   detekcija + popravak quarantine (macOS)
src-tauri/src/volwatch.rs        praćenje /Volumes
ui/index.html, ui/style.css
ui/app.js                        bootstrap, globalni key dispatch, menu eventi
ui/panel.js                      Panel (virtualna lista, navigacija, oznake)
ui/dialogs.js                    copy/move/mkdir/delete/rename/mask/progress/conflict/error
ui/hotlist.js                    Ctrl+D popup i Configure dijalog
ui/config.js                     Keyboard i Options dijalozi
ui/viewer.html, ui/viewer.js     F3 prozor
ui/lib.mjs, ui/lib.test.mjs      čiste funkcije + testovi
```

## Zajednički tipovi (koriste ih svi taskovi)

Rust strukture koje putuju u UI derive-aju `Serialize, Deserialize, Clone, Debug, PartialEq`, s `#[serde(rename_all = "camelCase")]`. Enumi koriste `#[serde(tag = "kind", rename_all = "camelCase")]`.

---

### Task 1: Skelet projekta

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `crates/core/Cargo.toml`, `crates/core/src/lib.rs`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/icons/*`
- Create: `ui/index.html`, `ui/style.css`, `ui/app.js`, `ui/lib.mjs`, `ui/lib.test.mjs`

**Interfaces:**
- Produces:
  - workspace koji se builda;
  - prozor `main`, 1300×820, title `fxfm`, `visible: false` (UI ga prikaže nakon bootstrapa);
  - CSP kao u fxmp-u, uz `img-src 'self' data: asset: http://asset.localhost` i `frame-src asset: http://asset.localhost` za viewer;
  - Tauri feature `protocol-asset`.

- [ ] **Step 1:** Kopiraj obrazac iz fxmp-a.
  - `Cargo.toml` workspace s `members = ["crates/core", "src-tauri"]` i istim `[profile.release]`, bez `[patch]`.
  - `rust-toolchain.toml` (1.99.1, minimal, rustfmt+clippy).
  - `tauri.conf.json`: `frontendDist: "../ui"`, `withGlobalTauri: true`, bundle targets `["app","dmg"]`, `minimumSystemVersion: "15.0"`, category `Utility`.
- [ ] **Step 2:** U `crates/core/src/lib.rs` dodaj `pub fn version() -> &'static str` (vraća `env!("CARGO_PKG_VERSION")`) i test `version_is_set`. U `ui/lib.test.mjs` dodaj test `smoke` koji importa `lib.mjs`.
- [ ] **Step 3:** Ikona.
  - Kopiraj `~/Documents/PVT/fxmp/src-tauri/icons/gen_app_icon.py` u `src-tauri/icons/`.
  - Promijeni sadržaj: tamnoplavi kvadrat, dva bijela "panela" (dva uspravna pravokutnika) i ispod `fxfm`.
  - Pokreni `python3 src-tauri/icons/gen_app_icon.py && cargo tauri icon src-tauri/icons/app-icon-source.png`.
- [ ] **Step 4:** Provjera.
  - Run: `rustup target add x86_64-apple-darwin && cargo test -p fxfm-core && node --test ui/ && cargo build -p fxfm`
  - Expected: svi testovi PASS, build OK.
- [ ] **Step 5:** `cargo tauri dev`.
  - Expected: otvara se prozor "fxfm" s praznom stranicom (`app.js` za sada samo radi `getCurrentWindow().show()`).
- [ ] **Step 6:** Commit `chore: fxfm workspace skeleton (core + tauri + ui)`.

### Task 2: Lokacija podataka i JSON spremanje

**Files:** Create `crates/core/src/storage.rs`

**Interfaces:**
- Produces:
  - `pub const DATA_DIR_NAME: &str = "fxfm-data";`
  - `pub fn app_folder(exe: &Path) -> Option<&Path>`: ista semantika kao `fxmp/src-tauri/src/portable.rs::app_folder`.
  - `pub enum StorageMode { Portable, System }`
  - `pub struct DataLocation { pub mode: StorageMode, pub dir: PathBuf }`
  - `pub fn system_dir() -> PathBuf`: na macOS-u `~/Library/Application Support/fxfm`, inače `$XDG_DATA_HOME/fxfm` ili `~/.local/share/fxfm`.
  - `pub fn resolve(app_folder: Option<&Path>, system_dir: &Path) -> DataLocation`: portable ako `app_folder/fxfm-data` postoji, mapa je i zapisiva (provjera kreiranjem i brisanjem `.fxfm-write-test`). Inače sistemski, uz `create_dir_all`.
  - `pub fn migrate(from: &DataLocation, to: StorageMode, app_folder: Option<&Path>, system_dir: &Path) -> Result<DataLocation, StorageError>`: rekurzivno kopira sav sadržaj (i `keys/`, ako postoji).
    - Portable → System: nakon uspjeha preimenuje `fxfm-data` u `fxfm-data.old` (ako postoji, onda `fxfm-data.old-2`, `-3`…), jer bi inače portable opet imao prednost.
    - System → Portable: kreira `fxfm-data` pa kopira, a sistemsku mapu ne dira.
    - Ne smije prepisati postojeći neprazni cilj: vraća `StorageError::TargetNotEmpty`.
  - `pub fn load_json<T: DeserializeOwned + Default>(dir: &Path, name: &str) -> T`: nepostojeća datoteka daje `Default`. Pokvaren JSON se preimenuje u `name.broken` i također vraća `Default` (logira se warning).
  - `pub fn save_json<T: Serialize>(dir: &Path, name: &str, v: &T) -> io::Result<()>`: zapisuje `name.tmp` pa radi rename.

- [ ] **Step 1: testovi** (u `storage.rs`, tempdir):
  - `app_folder_beside_exe_or_bundle`: isti slučajevi kao fxmp test, s `fxfm.app`.
  - `resolve_portable_only_with_writable_dir`: bez mape daje System. Datoteka imena `fxfm-data` daje System. Mapa daje Portable. Mapa s `chmod 0o555` daje System (test preskoči ako radi kao root).
  - `migrate_system_to_portable_copies_everything`: `settings.json` i `keys/a.key` moraju postojati u cilju.
  - `migrate_portable_to_system_renames_old`: nakon migracije `fxfm-data` ne postoji, `fxfm-data.old` postoji, a `resolve` vraća System.
  - `migrate_refuses_nonempty_target`.
  - `save_then_load_roundtrip` i `broken_json_is_set_aside`: nakon load-a postoji `settings.json.broken`, a rezultat je `Default`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core storage`. Expected: FAIL (ne kompajlira).
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): portable/system data location, migration, atomic json`.

### Task 3: Postavke i sesija

**Files:** Create `crates/core/src/config.rs`

**Interfaces:**
- Consumes: `load_json`/`save_json` (Task 2)
- Produces:
  - `pub struct Settings { pub editor: EditorConfig, pub colors: Colors, pub show_command_line: bool /*true*/, pub font_size: u32 /*17*/, pub fda_hint_shown: bool /*false*/ }`, uz `impl Default`.
  - `pub enum EditorConfig { System, App { path: String }, Terminal { command: String } }`
    - `System` znači: VS Code ako postoji `/Applications/Visual Studio Code.app`, inače TextEdit. Rješava se u Tasku 9.
  - `pub struct Colors { pub normal, hidden, archive, media, marked, background, cursor: String }`: zadane vrijednosti iz Global Constraints.
  - `pub enum Side { Left, Right }`
  - `pub struct PanelState { pub location: String, pub cursor_name: Option<String> }`
  - `pub struct SessionState { pub left: Option<PanelState>, pub right: Option<PanelState>, pub active: Side, pub window: Option<WindowGeom> }`
  - `pub struct WindowGeom { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }`
  - `pub fn restore_path(location: &str, home: &Path) -> PathBuf`:
    - za `location` sa shemom (`://`) vraća `home`;
    - za putanju koja prolazi kroz datoteku (npr. `/a/b.zip/x`, gdje je `b.zip` datoteka) vraća `home`;
    - inače se penje do prve postojeće mape, a ako ništa ne postoji, vraća `home`.
  - Konstante imena: `SETTINGS_FILE = "settings.json"`, `SESSION_FILE = "session.json"`.

- [ ] **Step 1: testovi:**
  - `settings_defaults`: `font_size == 17`, `colors.archive == "#ff6a4d"`, `editor == EditorConfig::System`.
  - `restore_path_falls_back_to_nearest_ancestor`: `tmp/a/b/c` gdje postoji samo `tmp/a` daje `tmp/a`.
  - `restore_path_scheme_goes_home`: `"sftp://web-1/var"` daje `home`.
  - `restore_path_through_file_goes_home`.
  - `restore_path_nothing_exists_goes_home` (npr. `/Volumes/NEMA/x` daje `/Volumes`). Napomena: `/` uvijek postoji, pa "ništa" u praksi znači korijen. Test zato provjerava da `/Volumes/NEMA-<pid>/x` daje `/Volumes`.
  - `session_roundtrip_json` (camelCase ključevi: `cursorName`).
- [ ] **Step 2:** Run `cargo test -p fxfm-core config`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): settings and session state with path restore`.

### Task 4: Lokalni listing i volumeni

**Files:** Create `crates/core/src/listing.rs`, `crates/core/src/volumes.rs`

**Interfaces:**
- Produces:
  - `pub struct Entry { pub name: String, pub is_dir: bool, pub is_symlink: bool, pub size: u64, pub mtime: i64 /*unix s*/, pub mode: u32, pub hidden: bool }`
    - `hidden` je istina ako ime počinje s `.` ili ako na macOS-u postoji `UF_HIDDEN` u `st_flags`.
    - Za simlink na mapu `is_dir = true` i `is_symlink = true`, a `size` i `mtime` su od cilja. Pokvaren simlink je datoteka s `size = 0`.
  - `pub enum ListError { NotFound(String), PermissionDenied(String), NotADirectory(String), Io(String) }`: `thiserror`, Serialize kao `{kind, path}`.
  - `pub fn list_dir(path: &Path) -> Result<Vec<Entry>, ListError>`: bez `.` i `..`, sortirano.
  - `pub fn sort_entries(v: &mut [Entry])`: mape prije datoteka. Unutar grupe sortira po ključu `name.nfc().to_lowercase()`, a kod jednakosti po originalnom imenu.
  - `pub enum VolumeKind { Root, Home, Volume, Network }`
  - `pub struct Volume { pub name: String, pub path: String, pub kind: VolumeKind, pub total: u64, pub free: u64 }`
  - `pub fn space(path: &Path) -> io::Result<(u64 /*total*/, u64 /*free*/)>`: preko `statvfs`, `f_frsize * f_blocks` i `f_frsize * f_bavail`.
  - `pub fn list_volumes(home: &Path) -> Vec<Volume>` daje redom:
    1. `/` s imenom volumena root diska (na macOS-u ime simlinka u `/Volumes` koji pokazuje na `/`, tipično "Macintosh HD"; fallback "/");
    2. `home` (ime `~`);
    3. svaki unos u `/Volumes` osim simlinka na `/`, abecedno. `Network` ako je `f_fstypename` iz `statfs` jedan od `smbfs|afpfs|nfs|webdav`, inače `Volume`.

    Na Linuxu: `/`, `~`, pa točke montiranja iz `/proc/mounts` pod `/media`, `/mnt` i `/run/media`.

- [ ] **Step 1: testovi** (tempdir):
  - `list_dir_sorts_dirs_first`: mape `b`, `A`, datoteke `c.txt`, `.hidden` daju poredak `A`, `b`, `.hidden`, `c.txt`; `.hidden.hidden == true`.
  - `sort_is_case_insensitive_and_nfc`: `"Cesta"`, `"čaj"` (NFD zapis `"c\u{30C}aj"`), `"Ante"` daju `Ante`, `Cesta`, `čaj`. NFD i NFC ista imena sortiraju se zajedno.
  - `list_dir_reports_symlinks`: simlink na mapu ima `is_dir && is_symlink`, pokvaren simlink `!is_dir`.
  - `list_dir_permission_denied_is_typed_error`: `chmod 000` daje `ListError::PermissionDenied` (preskoči ako je root).
  - `list_dir_missing_is_not_found`.
  - `list_dir_50k_entries_is_fast`: kreira 50 000 praznih datoteka; `list_dir` mora biti `< 1 s` u release i `< 5 s` u debug buildu, s `#[cfg_attr(debug_assertions, ignore)]` i pokretanjem uz `--release`.
  - `volumes_start_with_root_and_home`: `[0].kind == Root`, `[0].path == "/"`, `[1].kind == Home`, `[0].total > 0`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core listing volumes`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj (`libc` za `statvfs`, `statfs` i `st_flags`; `unicode-normalization`).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo test -p fxfm-core --release list_dir_50k`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): directory listing and volumes`.

### Task 5: Registar naredbi i keymap

**Files:** Create `crates/core/src/keymap.rs`, `tests/fixtures/keycombos.json`, `crates/core/tests/keycombo_fixture.rs`

**Interfaces:**
- Produces:
  - `pub struct KeyCombo { pub ctrl: bool, pub alt: bool, pub shift: bool, pub cmd: bool, pub key: String }`
    - `FromStr` i `Display` u kanonskom obliku: modifikatori redom `Ctrl+Alt+Shift+Cmd+`, pa ključ. Parsiranje ne pazi na velika i mala slova, a `Option` je sinonim za `Alt`.
    - Imena ključeva: `F1`–`F12`, `A`–`Z`, `0`–`9`, `Tab`, `Enter`, `Backspace`, `Delete`, `Insert`, `Space`, `Escape`, `Up`, `Down`, `Left`, `Right`, `PageUp`, `PageDown`, `Home`, `End`, `NumAdd`, `NumSubtract`, `NumMultiply`, `=`, `-`, `\`.
  - `pub struct CommandDef { pub id: &'static str, pub menu: &'static str, pub title: &'static str, pub defaults: &'static [&'static str], pub available: bool }`
  - `pub static COMMANDS: &[CommandDef]`. Točan sadržaj (`id` | meni | naslov | zadane tipke | `available`):

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
| cm_target_equal_source | Commands | Target = Source | (nema) | ✓ |
| cm_search | Commands | Search | Alt+F7 | ✗ |
| cm_multi_rename | Files | Multi-rename | Ctrl+M | ✗ |
| cm_connect | Net | Connect to server | Ctrl+F | ✗ |
| cm_disconnect | Net | Disconnect | Ctrl+Shift+F | ✗ |
| cm_drive_left | Show | Left drive | Alt+F1 | ✓ |
| cm_drive_right | Show | Right drive | Alt+F2 | ✓ |
| cm_refresh | Show | Refresh | Ctrl+R | ✓ |
| cm_toggle_hidden | Show | Show hidden files | Ctrl+H | ✓ |
| cm_cmdline_focus | Commands | Command line | Ctrl+Enter | ✓ |
| cm_config_keyboard | Configuration | Keyboard… | (nema) | ✓ |
| cm_config_options | Configuration | Options… | (nema) | ✓ |

  Dodatne napomene uz tablicu:
  - Nedostupne naredbe (`✗`) stoje u meniju kao disabled. Njihova tipka u UI-ju pokazuje `"<title>: coming in a later version"` u status liniji.
  - Kretanje kursorom (Up, Down, PgUp, PgDn, Home, End), Enter i Escape su fiksne tipke panela i nisu u registru. To je odluka plana: nisu TC "naredbe" koje se remapiraju.
  - Enter na `[..]` i mapi ulazi, a na datoteci otvara zadanom aplikacijom.

  Keymap API:
  - `pub struct Keymap { bindings: BTreeMap<String /*kanonski combo*/, String /*cmd id*/> }`, serde kao mapa.
  - `Keymap::defaults() -> Keymap`
  - `fn resolve(&self, c: &KeyCombo) -> Option<&str>`
  - `fn bind(&mut self, c: &KeyCombo, cmd: &str) -> Option<String /*naredba koja je izgubila tu tipku*/>`
  - `fn unbind(&mut self, c: &KeyCombo)`
  - `fn combos_for(&self, cmd: &str) -> Vec<KeyCombo>`
  - `fn load_or_default(dir: &Path) -> Keymap`: čita `keymap.json` i odbacuje nepoznate cmd id-eve i combo stringove koji se ne parsiraju.
  - `pub const SYSTEM_RESERVED: &[&str] = &["Ctrl+Up","Ctrl+Down","Ctrl+Left","Ctrl+Right","Ctrl+Space","Ctrl+Alt+Space"]`
  - `fn system_conflicts(&self) -> Vec<(String, String)>`

  Fixture `tests/fixtures/keycombos.json` je niz `{ "event": {code, key, ctrlKey, altKey, shiftKey, metaKey}, "combo": "Ctrl+Shift+F6" }` s barem ovim slučajevima:
  - `F5`, `Shift+F6`, `Alt+F1` (Mac daje `key:"¡"`, `code:"F1"`), `Ctrl+D`, `Alt+F7`;
  - `NumAdd` (`code:"NumpadAdd"`), `Ctrl+=` (`code:"Equal"`), `Ctrl+-` (`code:"Minus"`), `Ctrl+\` (`code:"Backslash"`), `Ctrl+8` (`code:"Digit8"`);
  - `Space`, `Tab`, `Enter` (i `code:"NumpadEnter"` daje `Enter`), `Ctrl+PageUp`, `Delete`, `Shift+Delete`, `Backspace`;
  - Option+slovo na Macu (`code:"KeyA"`, `key:"å"`, `altKey:true`) daje `Alt+A`.

  Ključ se izvodi iz `event.code`, ne iz `event.key`.

- [ ] **Step 1: testovi**
  - `combo_parse_display_roundtrip`: `"ctrl+shift+f6"` daje `"Ctrl+Shift+F6"`, a `"Option+F1"` daje `"Alt+F1"`.
  - `defaults_cover_table`: `F5→cm_copy`, `F2→cm_rename`, `Shift+F6→cm_rename`, `Space→cm_mark`, `Ctrl+=→cm_mark_mask`, `Alt+F7→cm_search`, `Tab→cm_switch_panel`.
  - `bind_reports_displaced`: `bind(F5, cm_view)` vraća `Some("cm_copy")`.
  - `load_drops_unknown`.
  - `system_conflicts_lists_reserved`: nakon `bind(Ctrl+Up, cm_parent)` lista sadrži `("Ctrl+Up","cm_parent")`.
  - `defaults_have_no_duplicates`.
  - `crates/core/tests/keycombo_fixture.rs`: svaki `combo` iz fixturea parsira se i ispisuje identično.
- [ ] **Step 2:** U `ui/lib.test.mjs` dodaj test `comboFromEvent matches fixture`: čita `../tests/fixtures/keycombos.json` i za svaki slučaj očekuje `comboFromEvent(event) === combo`.
- [ ] **Step 3:** Run `cargo test -p fxfm-core keymap keycombo && node --test ui/`. Expected: FAIL.
- [ ] **Step 4:** Implementiraj `keymap.rs` i `export function comboFromEvent(e): string | null` u `ui/lib.mjs`. Vraća `null` za čisti modifikator (`code` je `ShiftLeft` i sl.).
- [ ] **Step 5:** Run isto. Expected: PASS.
- [ ] **Step 6:** Commit `feat(core,ui): command registry, TC keymap, shared key-combo fixture`.

### Task 6: Operacije nad datotekama

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
  - `pub fn resolve_dest(sources: &[PathBuf], dest: &str, cwd: &Path) -> Dest`: relativni `dest` gleda se prema `cwd`.
    - `IntoDir` ako `dest` završava s `/`, ako je postojeća mapa ili ako ima više izvora.
    - Inače `AsFile` (kopija jedne datoteke pod drugim imenom, kao TC).
  - `pub fn copy(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn move_(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn delete(paths: &[PathBuf], to_trash: bool, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`
  - `pub fn mkdir(parent: &Path, name: &str) -> io::Result<PathBuf>`: `name` smije sadržavati `/` (kreira lanac).
  - `pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf>`: odbija `/` u imenu i postojeći cilj (`AlreadyExists`).
  - `pub fn dir_size(path: &Path, cancel: &AtomicBool) -> io::Result<(u64 /*bajtovi*/, u64 /*datoteke*/)>`: rekurzivno, ne prati simlinkove, prekida se kad je `cancel` postavljen.
  - Pravila:
    - Prije početka se izbroji `total_files` i `total_bytes`.
    - Kopija čuva `mtime` (`filetime`) i Unix prava. Simlinkovi se kopiraju kao simlinkovi.
    - Kopira se u chunkovima od 1 MiB, uz poziv `progress` i provjeru `is_cancelled` između chunkova. Prekinuta datoteka se briše.
    - `move_` radi `rename` na istom uređaju; kod `EXDEV` radi copy pa delete izvora.
    - Odluke `OverwriteAll`, `SkipAll` i `OverwriteAllOlder` vrijede do kraja operacije. `OverwriteAllOlder` prepisuje samo ako je cilj stariji, inače preskače.
    - `IntoItself`: odredište je izvor ili je unutar izvora (usporedba kanoniziranih putanja).
    - `SameLocation`: move ili copy datoteke u njenu vlastitu mapu bez novog imena.
    - `delete(to_trash: true)` koristi crate `trash`. Ako Trash nije dostupan (npr. mrežni volumen), `error()` vraća poruku `"Cannot move to Trash"`, a UI tada nudi trajno brisanje.

- [ ] **Step 1: testovi** (tempdir, `TestObserver` sa skriptiranim odgovorima i zabilježenim pozivima):
  - `copy_dir_recursive_preserves_mtime_and_mode`.
  - `copy_into_own_subdir_is_refused`: `copy([a], IntoDir(a/b))` vraća `Err(IntoItself)`, a ništa nije kreirano.
  - `move_to_same_dir_is_noop_error`: vraća `SameLocation`.
  - `conflict_overwrite_all_asks_once`: dvije kolizije, jedan poziv `conflict`.
  - `conflict_skip_keeps_target`.
  - `conflict_overwrite_all_older`: noviji cilj ostaje, stariji se prepisuje.
  - `conflict_rename_uses_new_name`.
  - `symlink_copied_as_link`: i petlja `a/loop -> a` završava.
  - `cancel_mid_file_removes_partial`: observer otkaže nakon prvog chunka 5 MiB datoteke.
  - `progress_totals_counted_upfront`.
  - `resolve_dest_rules`: jedna datoteka i `"/x/novo.txt"` daju `AsFile`; `"/x/"` daje `IntoDir`; dva izvora daju `IntoDir`.
  - `mkdir_nested`, `rename_rejects_existing_and_slash`.
  - `dir_size_sums_recursive`: ne broji cilj simlinka.
  - `delete_permanent_recursive`.

  Trash test je `#[ignore]` jer dira korisnikov Trash. Ručna provjera je u Tasku 11.
- [ ] **Step 2:** Run `cargo test -p fxfm-core ops`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy/move/delete/mkdir/rename with conflicts, progress, cancel`.

### Task 7: Hotlist model

**Files:** Create `crates/core/src/hotlist.rs`

**Interfaces:**
- Produces:
  - `pub struct HotItem { pub title: String, pub command: String, pub target: String }`
  - `pub enum HotNode { Item(HotItem), Submenu { title: String, children: Vec<HotNode> } }`, serde tag `kind`.
  - `pub struct Hotlist { pub nodes: Vec<HotNode>, pub hide_char: Option<char> /*None*/ }`
  - `HOTLIST_FILE = "hotlist.json"`
  - `pub fn parse_cd(command: &str, home: &Path) -> Option<String>`: prima `cd X` (bez obzira na velika i mala slova, uz trim). Širi vodeći `~`. Lokaciju sa shemom (`sftp://…`) vraća netaknutu. Ostalo vraća `None`.
  - `pub fn menu_label(title: &str) -> (String /*bez &*/, Option<char> /*akcelerator, lowercase*/)`: `&&` daje doslovni `&`.
  - `impl Hotlist { fn add_current(&mut self, location: &str, home: &Path) /*naslov = zadnja komponenta, command = "cd <loc>" s ~ za home*/; fn visible(&self) -> Vec<HotNode> /*bez stavki čiji naslov počinje s hide_char, rekurzivno*/ }`

- [ ] **Step 1: testovi:**
  - `parse_cd_variants`: `"cd ~/Downloads"` daje `<home>/Downloads`; `"CD /tmp "` daje `/tmp`; `"cd sftp://web-1/var/www"` ostaje isti; `"ls"` daje `None`.
  - `menu_label_accelerator`: `"&Downloads"` daje `("Downloads", Some('d'))`, `"A&&B"` daje `("A&B", None)`.
  - `add_current_uses_tilde`: `<home>/Documents/PVT` daje title `"PVT"` i command `"cd ~/Documents/PVT"`.
  - `visible_hides_prefixed`: s `hide_char = Some('?')` stavka `"?tajno"` je skrivena, i u podmeniju.
  - `json_roundtrip`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core hotlist`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): directory hotlist model`.

### Task 8: Sadržaj za F3 viewer

**Files:** Create `crates/core/src/viewer.rs`

**Interfaces:**
- Produces:
  - `pub enum ViewContent { Text { encoding: String, text: String, truncated: bool }, Image { mime: String }, Pdf, Hex { bytes: Vec<u8>, offset: u64, total: u64 } }`
  - `pub const TEXT_LIMIT: u64 = 10 * 1024 * 1024; pub const HEX_LIMIT: usize = 1024 * 1024;`
  - `pub fn read_for_view(path: &Path) -> io::Result<ViewContent>`:
    - ekstenzije `png jpg jpeg gif webp bmp tiff heic svg` daju `Image` (mime po ekstenziji);
    - `pdf` daje `Pdf`;
    - inače čita do `TEXT_LIMIT`. Ako u prvih 8 KiB postoji NUL, daje `Hex` (prvih `HEX_LIMIT`), inače `Text` kroz `decode_text`.
  - `pub fn decode_text(bytes: &[u8]) -> (String, &'static str)`:
    - UTF-8 BOM ili ispravan UTF-8 daje `"UTF-8"` (BOM se uklanja);
    - UTF-16 LE/BE BOM daje `"UTF-16LE"` ili `"UTF-16BE"`;
    - inače `"windows-1250"` (`encoding_rs`).
  - Slike i PDF UI učitava sam preko asset protokola (`convertFileSrc`), pa backend ne šalje bajtove.

- [ ] **Step 1: testovi:**
  - `utf8_with_bom`.
  - `cp1250_croatian`: bajtovi `9A E6 E8 9E F0` daju `"šćčžđ"` i `"windows-1250"`.
  - `binary_goes_hex`.
  - `truncates_large_text`: `TEXT_LIMIT + 10` bajtova daje `truncated == true`.
  - `image_and_pdf_by_extension`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core viewer`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): viewer content detection and text decoding`.

### Task 9: Tauri backend

**Files:** Create `src-tauri/src/{state.rs,commands.rs,jobs.rs,launch.rs,volwatch.rs}`, Modify `src-tauri/src/main.rs`

**Interfaces:**
- Consumes: sve iz Taskova 2–8.
- Produces (Tauri komande, ime = JS `invoke` ime; greške se vraćaju kao `String` ili serijalizirani `ListError`):
  - `bootstrap() -> Bootstrap { home: String, user: String, version: String, session: SessionState, settings: Settings, keymap: BTreeMap<String,String>, commands: Vec<CommandInfo{id,menu,title,available}>, hotlist: Hotlist, storage: StorageInfo{mode, dir}, systemConflicts: Vec<(String,String)>, translocated: bool }`. Ovdje se primjenjuje `restore_path` na `session.left` i `session.right`; ako su `None`, koristi se `home`.
  - `list_dir(location: String) -> Result<Vec<Entry>, ListError>`
  - `list_volumes() -> Vec<Volume>`
  - `nearest_existing(location: String) -> String`: `restore_path`.
  - `dir_size(location: String) -> Result<(u64,u64), String>`: poziva `ops::dir_size` u pozadinskoj niti. Novi poziv za isti panel otkazuje prethodni.
  - `save_session(session: SessionState)`, `save_settings(settings: Settings)`, `save_hotlist(hotlist: Hotlist)`
  - `save_keymap(bindings: BTreeMap<String,String>) -> Vec<(String,String)>` (vraća nove system konflikte), `reset_keymap() -> BTreeMap<String,String>`
  - `set_storage_mode(mode: StorageMode) -> Result<StorageInfo, String>`: `storage::migrate`, pa se `AppState.data` prebacuje na novu lokaciju.
  - `start_job(req: JobRequest) -> u64`, gdje je `JobRequest = Copy{sources:Vec<String>, dest:String, cwd:String} | Move{…} | Delete{paths:Vec<String>, toTrash:bool}`.
  - `answer_conflict(job: u64, answer: ConflictAnswer)`, `answer_error(job: u64, answer: ErrorAnswer)`, `cancel_job(job: u64)`
  - `mkdir(parent: String, name: String) -> Result<String,String>`, `rename(path: String, newName: String) -> Result<String,String>`
  - `view_content(path: String) -> Result<ViewContent,String>`, `open_viewer(path: String)` (novi prozor `viewer-<n>`, URL `viewer.html?path=<enc>`, 1000×700)
  - `edit_file(path: String) -> Result<(),String>`, `edit_new(dir: String, name: String) -> Result<String,String>` (kreira praznu datoteku ako ne postoji, pa je otvara u editoru)
  - `open_default(path: String)` (`tauri-plugin-opener`), `run_in_terminal(dir: String, command: String)`, `open_fda_settings()` (`open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`)
  - Eventi:
    - `job://progress {job, progress}`
    - `job://conflict {job, info}`
    - `job://error {job, path, msg}`
    - `job://done {job, summary | error}`
    - `volumes://changed Vec<Volume>`
  - `launch.rs`:
    - `pub fn shell_quote(s: &str) -> String`: POSIX single-quote; `'` postaje `'\''`.
    - `pub fn editor_command(cfg: &EditorConfig, file: &Path) -> std::process::Command`:
      - `System`: `open -a "Visual Studio Code" <f>` ako postoji `/Applications/Visual Studio Code.app`, inače `open -e <f>`;
      - `App{path}`: `open -a <path> <f>`;
      - `Terminal{command}`: osascript `tell application "Terminal" to do script "cd <q dir> && <command> <q file>"`, pa `activate`.
    - `pub fn terminal_command(dir: &Path, cmd: &str) -> std::process::Command`: isti osascript obrazac.
    - Na Linuxu `xdg-open` i `$TERMINAL` iza `cfg`; to nije dio v1 testiranja.
  - `jobs.rs`:
    - Svaki job radi u svojoj niti.
    - `TauriObserver::conflict` emitira `job://conflict` i blokira na `std::sync::mpsc::Receiver` dok ne stigne `answer_conflict`. `cancel_job` postavlja `AtomicBool` i šalje `Cancel` na kanal ako job čeka.
    - `progress` se emitira najviše svakih 100 ms.
  - `volwatch.rs`: nit koja svake 2 s poziva `list_volumes` i emitira `volumes://changed` samo kad se skup `path`ova promijeni.
  - Capabilities: `core:default`, `opener:default`, `core:window:allow-create`, `core:webview:allow-create-webview-window`; asset scope `["**"]`.

- [ ] **Step 1: testovi** (unit, u `launch.rs` i `jobs.rs`):
  - `shell_quote_handles_single_quotes`: `it's 'ok'.sh` daje `'it'\''s '\''ok'\''.sh'`.
  - `editor_command_terminal_quotes_path`: args sadrže quotanu putanju s razmakom.
  - `job_bridge_delivers_answer`: `ConflictBridge` (čisti Rust dio `TauriObserver`-a, bez Taurija) prima odgovor iz druge niti; `cancel` odblokira čekanje s `Cancel`.
- [ ] **Step 2:** Run `cargo test -p fxfm`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj. Slično fxmp-u, `AppState` drži `Mutex<…>` za settings, keymap i hotlist. Svaki `save_*` odmah zapisuje u `DataLocation.dir`.
- [ ] **Step 4:** Run `cargo test -p fxfm && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): tauri commands, background jobs, launchers, volume watch`.

### Task 10: Glavni prozor i paneli

**Files:** Modify `ui/index.html`, `ui/style.css`, `ui/app.js`, `ui/lib.mjs`, `ui/lib.test.mjs`; Create `ui/panel.js`

**Interfaces:**
- Consumes: komande i eventi iz Taska 9, `comboFromEvent` iz Taska 5.
- Produces:
  - `lib.mjs` (čiste funkcije, sve testirane):
    - `formatK(bytes: number): string`: `floor(bytes/1024)`, točka za tisućice, sufiks `" k"`. Primjer: `295520305152` daje `"288.594.048 k"`.
    - `freeLine(name, free, total)` daje `"[Macintosh HD]  288.594.048 k of 971.350.180 k free"` (dva razmaka iza `]`).
    - `formatDate(unixSecs, tz?)` daje `"05.11.2026 15:56"`; test koristi fiksni `TZ=Europe/Zagreb`.
    - `formatSize(entry)`: `"<DIR>"` ili točkasti separatori (`4218` daje `"4.218"`).
    - `formatMode(mode, isDir, isSymlink)` daje `"drwxr-xr-x"`, `"lrwxr-xr-x"`, `"-rw-r--r--"`.
    - `displayName(entry)`: `"[DEBIAN]"` za mapu, inače ime.
    - `colorClass(entry)`: `"hid" | "arc" | "med" | ""`.
      - Arhive: `zip tar tgz gz 7z bz2 xz rar tbz2 txz`.
      - Mediji: `mp4 mkv mov avi webm mp3 flac wav m4a ogg aac`.
      - Skriveno ima prednost.
    - `matchMask(name, mask)`: maske odvojene s `;` ili razmakom, `*` i `?`, bez obzira na velika i mala slova, NFC normalizacija. `*.*` se poklapa i s imenima bez točke, kao TC.
    - `visibleRange(scrollTop, rowH, viewH, count, overscan=10) -> {start, end}`.
    - `parentLocation(loc)` (`"/"` ostaje `"/"`), `joinLocation(dir, name)`, `lastComponent(loc)`.
    - `markSummary(entries, marked)` daje TC status `"4.218 k / 288.594 k in 2 / 31 file(s)"`.
    - `navigate(state, target, listFn)`: async helper; vraća `{ok, location, entries, cursorName}` ili `{ok:false, error}` i ne mijenja `state`. Panel ga koristi za svaku navigaciju.
  - `panel.js`: `class Panel { constructor(root, side, api); async go(location, cursorName?); refresh(); entries; cursorIndex; marked:Set<string>; location; focus(); }`
    - `[..]` se prikazuje kad lokacija nije `/`.
    - Enter na `[..]` ide u nadmapu i postavlja kursor na mapu iz koje se izašlo.
    - Space označava i pomiče kursor dolje (TC Insert ponašanje). Na mapi dodatno poziva `dir_size`, a veličina se prikazuje u koloni veličine umjesto `<DIR>` i u `markSummary` (spec §5).
    - Oznake se čiste nakon uspješne operacije.
    - Klik postavlja kursor, dvoklik radi Enter, klik na panel daje fokus.
  - Raspored (prema mockupu):
    - traka diskova (gumbi iz `list_volumes`, ikona po `kind`, aktivni istaknut);
    - red s putanjom (combo za disk, `freeLine`, gumbi `/` i `..`);
    - traka trenutne putanje (`location + "/*.*"`; aktivni panel `#2f6fd0` s bijelim tekstom);
    - crna lista;
    - status linija;
    - ispod oba panela naredbeni redak (skriven kad je `settings.showCommandLine == false`) i F-traka (F3 View, F4 Edit, F5 Copy, F6 Move, F7 NewFolder, F8 Delete, Alt+F4 Exit), gdje klik izvršava naredbu.
    - Font `-apple-system`, `settings.fontSize` px, visina retka `fontSize * 1.47`.
    - Paneli se dijele 50/50.
  - `app.js`:
    - globalni `keydown`: `comboFromEvent`, `keymap` lookup, `dispatch(cmdId)` i `preventDefault`. Kad je fokus u inputu ili dijalogu, globalni dispatch ne radi, osim `Escape` i `Tab` unutar dijaloga;
    - `dispatch` implementira u ovom tasku `cm_switch_panel`, `cm_parent`, `cm_root`, `cm_mark*`, `cm_invert`, `cm_mark_all`, `cm_refresh`, `cm_swap_panels`, `cm_toggle_hidden`, `cm_drive_left/right` (popup s volumenima uz panel, strelice i Enter), `cm_exit` i `cm_cmdline_focus`; ostale naredbe zasad pokazuju poruku u statusu;
    - sesija se sprema (debounce 500 ms) nakon svake navigacije i promjene fokusa te pri `onCloseRequested`; spremaju se i geometrija i pozicija prozora;
    - `volumes://changed`: osvježi trake diskova. Panel čija mapa više ne postoji radi `nearest_existing` i `go`;
    - `PermissionDenied`: status `Access denied: <loc>`. Ako `!settings.fdaHintShown`, jednom dijalog "fxfm needs Full Disk Access…" s gumbima Open Settings i Later, pa `fdaHintShown = true`;
    - naslov prozora `"<user> – fxfm <version>"`.

- [ ] **Step 1: testovi** u `ui/lib.test.mjs`:
  - `formatK`, `freeLine`, `formatDate`, `formatSize`, `formatMode` s točnim primjerima iznad.
  - `colorClass` za `.DS_Store` daje `hid`, `a.tgz` daje `arc`, `.x.zip` daje `hid`.
  - `matchMask('index.PHP','*.php')`, `matchMask('čćžšđ.txt','*.txt')`, NFD ime protiv NFC maske, `matchMask('Makefile','*.*') === true`, `matchMask('a.sh','*.php;*.sh')`.
  - `visibleRange(0,25,500,50000)` daje `{start:0,end:30}`, a `visibleRange(25*1000,25,500,50000)` daje `{start:990,end:1030}`.
  - `parentLocation('/')`, `parentLocation('/a/b')`.
  - `navigateKeepsOldPathOnError`: `listFn` baci `{kind:'permissionDenied'}`, rezultat je `ok:false`, a `state.location` je nepromijenjen.
  - `refreshFallsBackOnMissingDir`: `listFn` baci `notFound`, pa `nearest` vraća roditelja i `navigate` završi na njemu.
  - `markSummary`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj `lib.mjs`, pa `panel.js`, `index.html`, `style.css` i `app.js`.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: ručna provjera** (`cargo tauri dev`):
  - prvo pokretanje: oba panela su u `~`;
  - Tab mijenja aktivni panel (plava traka putanje);
  - Enter i Backspace kroz mape, a kursor se nakon `[..]` vraća na izlaznu mapu;
  - Space i Ctrl+= `*.md` označavaju crveno;
  - Alt+F1 nudi izbor diska;
  - utakni USB: gumb se pojavi unutar ~2 s; izbaci ga dok je panel na njemu: panel ide na `/Volumes`;
  - zatvori i ponovno otvori: iste mape i kursor;
  - otvori `~/Library/Mail`: poruka i FDA dijalog jednom;
  - otvori mapu s 50 000 datoteka (`mkdir /tmp/big && cd /tmp/big && seq 50000 | xargs touch`): scroll je gladak.
- [ ] **Step 6:** Commit `feat(ui): TC main window, panels, drive bar, navigation, session restore`.

### Task 11: Dijalozi operacija

**Files:** Create `ui/dialogs.js`; Modify `ui/app.js`, `ui/style.css`

**Interfaces:**
- Consumes: `start_job`, `answer_*`, `cancel_job`, `mkdir`, `rename`, `edit_new`, `job://*` eventi.
- Produces: `dispatch` za `cm_copy`, `cm_move`, `cm_delete`, `cm_delete_permanent`, `cm_mkdir`, `cm_rename`, `cm_edit_new`, `cm_mark_mask`, `cm_unmark_mask`.
  - Svi dijalozi su modalni HTML (TC siva paleta: `#f0f0f0`, gumbi s rubom). Enter radi OK, Esc Cancel, a Tab i Shift+Tab kruže unutar dijaloga.
  - Izvori su označene stavke; ako ih nema, stavka pod kursorom. `[..]` se nikad ne uzima.
  - **F5 / F6**:
    - naslov "Copy" ili "Move";
    - tekst `Copy 3 file(s) to:` ili `Copy "a.txt" to:`;
    - input s vrijednošću `<lokacija drugog panela>/` (pri jednoj datoteci dodaje se njeno ime, cijelo selektirano, kao TC);
    - OK pokreće `start_job`.
  - **Progress**: prikazuje se ako job traje dulje od 300 ms. Sadrži trenutnu datoteku, traku za datoteku, ukupnu traku (bajtovi) i gumb Cancel. Nakon `job://done` refreshaju se oba panela i čiste oznake.
  - **Conflict**: tekst "File exists" s veličinom i datumom izvora i cilja. Gumbi: Overwrite, Overwrite all, Skip, Skip all, Overwrite older, Rename, Cancel. Rename otvara input s predloženim imenom `ime (2).ext`.
  - **Error**: poruka s gumbima Retry, Skip, Skip all, Cancel. Kod `"Cannot move to Trash"` dodaje se gumb "Delete permanently", koji pokreće novi job s `toTrash:false` za preostale stavke.
  - **F8**: `Do you really want to move 3 file(s)/dir(s) to the Trash?`. Shift+F8: `…delete 3 file(s)/dir(s) permanently?`.
  - **F7**: "New folder" input; nakon kreiranja kursor stoji na novoj mapi.
  - **F2 / Shift+F6**: rename na mjestu, tj. input preko retka. Kod datoteka je selektiran dio imena bez ekstenzije, kod mapa cijelo ime. Enter sprema, Esc odustaje.
  - **Shift+F4**: input "New file name", pa `edit_new`.
  - **Num+ / Num−**: "Select files" ili "Unselect files" s maskom (zadano `*.*`, pamti zadnju u sesiji UI-ja); primjenjuje se `matchMask`. Mape se ne označavaju, osim ako maska završava s `/`.

- [ ] **Step 1: testovi** u `lib.test.mjs` za nove čiste funkcije:
  - `suggestRename('a.txt', existing)` daje `'a (2).txt'`, a ako i to postoji, `'a (3).txt'`.
  - `renameSelection('archive.tar.gz')` daje `{start:0,end:7}` (do prve točke za `.tar.*`, inače do zadnje točke), a za mapu cijelo ime.
  - `copyTargetDefault(otherLoc, sources)`: za jednu datoteku `'/x/a.txt'`, za više `'/x/'`.
  - `operationSources(entries, marked, cursor)`: ignorira `[..]`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: ručna provjera**:
  - kopiraj mapu s podmapama između panela;
  - napravi koliziju i provjeri sve gumbe;
  - Cancel usred velike datoteke (`mkfile 2g /tmp/big.bin`) briše djelomični cilj;
  - F6 između `/tmp` i USB-a;
  - F8 šalje u Trash (provjeri u Finderu), Shift+F8 briše trajno;
  - F7 `a/b/c`;
  - F2 rename;
  - F5 mape u njenu podmapu daje poruku "Cannot copy a folder into itself".
- [ ] **Step 6:** Commit `feat(ui): copy/move/delete/mkdir/rename dialogs with progress and conflicts`.

### Task 12: F3 viewer i F4 editor

**Files:** Create `ui/viewer.html`, `ui/viewer.js`; Modify `ui/app.js`

**Interfaces:**
- Consumes: `open_viewer`, `view_content`, `edit_file`, `settings.editor`.
- Produces:
  - `cm_view` na datoteci poziva `open_viewer`. Na mapi i na `[..]` ne radi ništa.
  - `cm_edit` poziva `edit_file`.
  - Viewer prozor:
    - `Text`: `<pre>` s monospace fontom, crnom pozadinom i bijelim tekstom. Na dnu je status `UTF-8 · 12.345 bytes`, uz oznaku `(truncated)` kad je skraćeno.
    - `Image`: `<img src=convertFileSrc(path)>`, skalirano da stane.
    - `Pdf`: `<iframe src=convertFileSrc(path)>`. Ako WKWebView ne renderira PDF kroz asset protokol, fallback je gumb "Open in Quick Look" (`qlmanage -p`, nova komanda `quick_look(path)` u Tasku 9 stilu). Odluku zapiši u README.
    - `Hex`: 16 bajtova po retku, u formatu `00000000  48 65 6c …  |Hel…|`.
  - Tipke u vieweru: Esc, F3 i Ctrl+W zatvaraju; Ctrl+F traži u tekstu (jednostavno: input pa `window.find`); kod teksta tipke `1`, `2` i `3` ručno prebacuju kodiranje između UTF-8, windows-1250 i ISO-8859-2 (nova komanda `view_text_as(path, encoding)`, u core-u `decode_as(bytes, label)`).
  - Configuration → Options ima editor (Task 14). Do tada je `System`.

- [ ] **Step 1: test** u core-u: `decode_as_iso_8859_2` (bajt `0xB9` daje `"š"`). U `lib.test.mjs`: `hexLines(bytes, offset)` daje točan redak za `"Hello"`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core viewer && node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run isto. Expected: PASS.
- [ ] **Step 5: ručna provjera**: F3 na `.php`, `.sh`, cp1250 `.txt`, `.png` i `.pdf`, te na binarnoj datoteci (`/bin/ls`). F4 otvara VS Code (ili TextEdit ako VS Code nije instaliran).
- [ ] **Step 6:** Commit `feat: F3 viewer window and F4 external editor`.

### Task 13: Ctrl+D hotlista

**Files:** Create `ui/hotlist.js`; Modify `ui/app.js`, `ui/style.css`

**Interfaces:**
- Consumes: `bootstrap().hotlist`, `save_hotlist`, `parse_cd` semantika (UI je ponavlja preko lib funkcije `parseCd`, testirane istim primjerima kao Task 7).
- Produces: `cm_hotlist`.
  - **Popup** (TC izgled, uz gornji lijevi kut aktivnog panela):
    - stavke `visible()`;
    - podmeniji se otvaraju strelicom desno ili Enterom;
    - akcelerator iz `&` (podcrtano slovo, a pritisak slova izvršava stavku);
    - ispod separatora "Add current dir" (`a`) i "Configure…" (`c`).
  - **Odabir stavke**: `parseCd(command)` daje lokaciju aktivnog panela. Ako `target` nije prazan, `parseCd("cd "+target)` šalje drugi panel tamo. Lokacija sa shemom (`sftp://`) u planu 1 pokazuje status `Remote locations come in a later version`.
  - **Configure dijalog** prema Ctrl+D dijalogu Total Commandera:
    - naslov "Change directory menu";
    - lijevo lista ("Directory menu:" s brojem stavki desno gore), podmeniji uvučeni s `--`;
    - desno gumbi Add Item…, Add submenu…, Delete Item, Change Title…, OK, Cancel, Help (Help prikazuje kratak ugrađeni tekst pomoći o poljima Command i Target path);
    - dolje polja Command (s gumbom 🔍 koji otvara `tauri-plugin-dialog` izbor mape i upisuje `cd <putanja>`) i Target path;
    - checkbox "Hide menu items starting with this character:" s inputom od jednog znaka, te tekst "Double click menu item to hide/show it (add/remove the character)". Dvoklik dodaje ili uklanja znak na početku naslova;
    - redoslijed se mijenja s Ctrl+↑/↓, gumbima ↑/↓ i drag & dropom;
    - OK poziva `save_hotlist`, Cancel odbacuje promjene.

- [ ] **Step 1: testovi** u `lib.test.mjs`:
  - `parseCd` sa svim primjerima iz Taska 7;
  - `menuLabel` (isti primjeri);
  - `moveNode(nodes, path, delta)`, koji pomiče stavku unutar istog roditelja i ne izlazi van granica;
  - `toggleHidden(title, ch)`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5: ručna provjera**:
  - dodaj Downloads i DEBIAN, plus podmeni s dvije stavke;
  - stavka s Target pathom mijenja oba panela;
  - skrivanje dvoklikom radi;
  - nakon restarta stavke su tu, i to u `fxfm-data/hotlist.json` ili sistemskoj mapi.
- [ ] **Step 6:** Commit `feat(ui): Ctrl+D directory hotlist with TC configure dialog`.

### Task 14: Meni, Configuration, naredbeni redak, translocation, release

**Files:** Create `src-tauri/src/menu.rs`, `src-tauri/src/translocation.rs`, `ui/config.js`; Modify `src-tauri/src/main.rs`, `ui/app.js`; Create `README.md`

**Interfaces:**
- Consumes: `COMMANDS`, sve komande iz Taska 9.
- Produces:
  - **Meni u gornjoj traci**:
    - app meni `fxfm` (About, Hide, Quit = Cmd+Q);
    - standardni Edit meni (predefinirani Undo/Redo/Cut/Copy/Paste/Select All, bez kojih Cmd+C/V u inputima ne radi);
    - zatim Files, Mark, Commands, Net, Show, Configuration, Start, Help, sastavljeni iz `COMMANDS` po `menu` polju, redom kao u tablici. `Start` je prazan uz jednu disabled stavku "(empty)".
    - Stavke **nemaju native accelerator**, jer bi meni progutao tipke prije webviewa. Zadana tipka se zato dopisuje u naslov, npr. `Copy    F5`.
    - Klik emitira `menu://command {id}`, koji UI prosljeđuje u `dispatch`.
    - Nedostupne naredbe su disabled.
  - **Configuration → Keyboard** (`config.js`):
    - tablica s kolonama Command (menu › title) i Keys (chipovi s ×);
    - gumb "Add key…" čeka pritisak tipke (`comboFromEvent`). Ako je tipka zauzeta, pita `"Ctrl+R is used by Refresh. Reassign?"`;
    - gumb "Reset to TC defaults";
    - na vrhu žuto upozorenje ako `systemConflicts` nije prazno: `"macOS uses Ctrl+Up for Mission Control. Turn it off in System Settings → Keyboard → Keyboard Shortcuts or choose another key."`, s gumbom Open Keyboard Shortcuts (`open "x-apple.systempreferences:com.apple.Keyboard-Settings.extension"`);
    - isto upozorenje prikazuje se jednom pri prvom pokretanju ako se zadana tipka preklapa.
  - **Configuration → Options**:
    - *Storage*: radio Portable / System, putanja trenutne mape i gumb Apply (`set_storage_mode`, s potvrdom `"Move all settings to …?"`). Portable se ne može odabrati ako mapa pored aplikacije nije zapisiva; tada se prikazuje poruka.
    - *Editor*: radio System default / Application (izbor `.app` preko dialoga) / Terminal command (input, npr. `nano`).
    - *Colors*: 7 color pickera i Reset.
    - *Font size*: 12–24.
    - *Show command line*.
    - Spremanje radi `save_settings` i odmah primjenjuje promjene.
  - **Naredbeni redak**:
    - Enter radi `run_in_terminal(location aktivnog panela, text)` i čisti input;
    - Ctrl+Enter iz panela fokusira redak;
    - Esc vraća fokus u panel;
    - ako je redak prazan, Enter iz panela radi normalno;
    - prompt je `<location>>`.
  - **translocation.rs** (macOS):
    - `pub fn is_translocated(exe: &Path) -> bool`: putanja sadrži `/AppTranslocation/`;
    - `pub fn original_path(exe: &Path) -> Option<PathBuf>`: FFI na `SecTranslocateCreateOriginalPathForURL` iz Security.frameworka (`core-foundation` crate);
    - komanda `fix_quarantine() -> Result<(), String>`: `xattr -dr com.apple.quarantine <original .app>`, pa `open -n <original .app>` i `app.exit(0)`;
    - ako je `bootstrap().translocated`, UI prije svega ostalog prikazuje dijalog `"macOS started fxfm from a temporary read-only location, so your settings next to the app can't be found."` s naredbom za kopiranje i gumbima "Fix and restart" i "Continue without settings" (sesija se tada ne sprema).
  - **README.md**: što je fxfm, build (`cargo tauri build --target universal-apple-darwin`, s putanjama do `.app` i `.dmg`), portable postavljanje (mapa `fxfm/` s `fxfm.app` i praznom `fxfm-data/`), prijenos na drugi Mac i `xattr` naredba, gdje su podaci, tablica zadanih tipki (generirana iz `COMMANDS`) te napomena za PDF iz Taska 12.

- [ ] **Step 1: testovi:**
  - `translocation::is_translocated` za `/private/var/folders/x/T/AppTranslocation/ABC/d/fxfm.app/Contents/MacOS/fxfm` je `true`, a za `/Applications/...` `false`.
  - `menu::menu_title_with_key("Copy", &["F5"])` daje `"Copy\tF5"`.
  - `lib.test.mjs`: `conflictMessage("Ctrl+Up")` daje tekst iz specifikacije.
- [ ] **Step 2:** Run `cargo test --workspace && node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 5: release build**
  - Run: `cargo tauri build --target universal-apple-darwin && lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm`
  - Expected: `x86_64 arm64`.
- [ ] **Step 6: ručna provjera portable i translocation toka**:
  1. `mkdir -p /tmp/fxfm-port/fxfm-data && cp -R <bundle>/fxfm.app /tmp/fxfm-port/`, pokreni, dodaj hotlist stavku. Provjeri da `/tmp/fxfm-port/fxfm-data/hotlist.json` postoji, a sistemska mapa nije dirana.
  2. `xattr -w com.apple.quarantine "0081;$(printf %x $(date +%s));Safari;" /tmp/fxfm-port/fxfm.app`, pa pokreni dvoklikom iz Findera. Očekuj translocation dijalog; nakon "Fix and restart" aplikacija radi s postavkama iz `fxfm-data`.
  3. Options → Storage → System, Apply. Postavke prelaze u `~/Library/Application Support/fxfm`, a `fxfm-data.old` postoji.
  4. Keyboard: remapiraj F5 na Ctrl+K. Nakon restarta Ctrl+K kopira, a F5 ne radi ništa. Zatim Reset.
  5. Naredbeni redak `ls -la` otvara Terminal u mapi panela.
- [ ] **Step 7:** Commit `feat: menu bar, keyboard/options config, command line, translocation fix, README`.

---

## Nakon plana 1

- **Plan 2 (arhive)** dodaje VFS trait u `fxfm-core`: `LocalFs` iz Taskova 4 i 6 seli se iza traita, zatim `ArchiveFs` (zip, tar, tgz, gz, 7z) i "upload back" praćenje F4 kopija (`notify`).
- **Plan 3 (serveri)** dodaje `SftpFs`, `FtpFs`, Ctrl+F dijalog, ključeve, master lozinku, Keychain i `known_hosts`.

Oba plana pišu se nakon što plan 1 bude gotov, na temelju stvarnog koda.
