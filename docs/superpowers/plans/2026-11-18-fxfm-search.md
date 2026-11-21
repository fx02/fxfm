# Search (Alt+F7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Alt+F7 finds files by name across the whole Mac disk as fast as you can type. It uses fxfm's own index, kept current by FSEvents. Archive contents and network volumes can be added as options. Results can be sent to a panel as a list (Feed to listbox).

**Architecture:**
- **`fxfm-core` (`crates/core/src/search/`)** holds the pure parts, all built and tested on Linux:
  - query parsing, matching and ranking;
  - the in-memory index;
  - the volume scanner and the direct-walk fallback;
  - the archive pass;
  - the versioned disk format.
- **`src-tauri`** holds a search service (`search.rs`) that owns the index, background threads, persistence, network refresh and Tauri commands, plus a macOS-only FSEvents binding (`fsevents.rs`).
- **UI:**
  - a Find files window (`ui/search.js`);
  - pure helpers in `ui/lib.mjs`;
  - a virtual "Search results" location in the panel;
  - an Options → Search section.

**Tech Stack:** Rust (std threads, `libc`, `unicode-normalization`, existing archive readers), CoreServices FSEvents through FFI, Tauri 2 commands and events, vanilla JS with `node --test`.

**Spec:** `docs/superpowers/specs/2026-11-18-fxfm-search-design.md` (approved 2026-11-18). Read it before any task.

## Global Constraints
- Search is by **name only**. Searching file contents is not supported.
- Searching on SFTP/SCP/FTP servers is not supported.
- Everything in the repo is in English: code, comments, UI strings, docs and commits.
- Commits use `git -c user.email=zbuzanic@gmail.com -c user.name="Zvonimir Bužanić" commit`. Each message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Nothing employer-related goes into the repo: no work host names and no work email.
- Tests:
  - never scan the real disk, `$HOME` or the real data folder; use `tempfile` trees and fake event streams only;
  - never write the real Keychain;
  - never mount anything.
- `fxfm-core` must build and test on Linux. Anything macOS-specific (`st_flags`/`SF_DATALESS`, `PRIO_DARWIN_BG`, FSEvents) sits behind `#[cfg(target_os = "macos")]`, with a no-op or `false` fallback.
- Budget for 3,000,000 entries: **≤ 250 MB RAM** and **≤ 300 MB on disk**. Typical queries return in well under a second.
- Indexing runs on background threads at low priority. It never holds a lock the UI or file operations wait on longer than a short swap.
- Default exclusions, copied from the spec:
  - the paths `/System`, `/private/var`, `/private/tmp`, `/dev`, `/Volumes` and `/cores`;
  - any folder named `.Spotlight-V100`, `.fseventsd`, `.Trashes` or `.DocumentRevisions-V100`;
  - Time Machine snapshots: `/Volumes/com.apple.TimeMachine.localsnapshots` and any mount point of another volume.
- Defaults:
  - the display cap is 1,000 results;
  - the tar.gz/tgz/gz size limit is 500 MB;
  - the network refresh is every 24 h (0 = manual only);
  - archive indexing is off, and archives on network volumes are off;
  - "Index while fxfm is running" is on.
- Supported archive formats are the ones fxfm reads: zip, 7z, tar, tar.gz/tgz and gz. bz2 and xz are out of scope. Nested archives are never entered.
- Verification for every task: `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`.
- New UI strings are plain English in the JS files (fxfm has no locale file).

## Review Focus
1. **NFD names.** A file named `Pročitaj.txt` stored in NFD on disk must be found by typing `pročitaj` (NFC). Its displayed path must still open the real file. Test in Task 1 and Task 3.
2. **Renames and deletes while running.** After a file is renamed or a folder holding thousands of files is deleted, the old names must stop matching and the new name must match. Test in Task 2 (`apply_change`) and Task 8 (fake event stream).
3. **Unreadable folders.** A folder without read permission is skipped and counted. The scan must not abort and must not loop. Test in Task 3.
4. **Feed to listbox with equal names.** Two results named `notes.txt` from different folders must be separate rows. Marking one must not mark the other, and F5 must copy the right file. Test in Task 9.
5. **Damaged index file.** A truncated or old-version `index/` file at startup must lead to a rebuild, never a crash or an empty result without explanation. Test in Task 5.

---

## File Structure
- `crates/core/src/search/mod.rs`: module root and the shared types `Hit`, `Scope`.
- `crates/core/src/search/query.rs`: query parsing, name folding, matching and ranking.
- `crates/core/src/search/index.rs`: the in-memory index (an arena of entries, parent links, folded names).
- `crates/core/src/search/scan.rs`:
  - the volume scan (exclusions, mount points, denied folders, dataless, cancel);
  - the subtree rescan;
  - the direct walk.
- `crates/core/src/search/archives.rs`: the archive-entry pass.
- `crates/core/src/search/store.rs`: the versioned disk format.
- `crates/core/src/config.rs`: `SearchSettings` inside `Settings`.
- `crates/core/src/keymap.rs`: `cm_search` becomes available, and `cm_feed_listbox` is added.
- `src-tauri/src/search.rs`: the service (state, threads, persistence, refresh, commands).
- `src-tauri/src/fsevents.rs`: the macOS FSEvents FFI stream.
- `src-tauri/src/main.rs`, `state.rs`, `commands.rs`: wiring.
- `ui/lib.mjs` and `ui/lib.test.mjs`: pure helpers for search results and the results location.
- `ui/search.js`: the Find files window and the first-run prompt.
- `ui/panel.js`, `ui/app.js`, `ui/dialogs.js`: the results location in a panel and its ops.
- `ui/config.js`: Options → Search.
- `ui/style.css`: window and greyed-row styles.
- `README.md` and `docs/ROADMAP.md`.

---

### Task 1: Query parsing, folding, matching and ranking (core)

**Files:**
- Create: `crates/core/src/search/mod.rs`
- Create: `crates/core/src/search/query.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod search;`)

**Interfaces:**
- Produces:
  - `pub fn fold(name: &str) -> String`: NFC, then `to_lowercase()`.
  - `pub struct Query { words: Vec<Term> }`, where `enum Term { Sub(String), Mask(Regex-like matcher) }`.
  - `pub fn Query::parse(text: &str) -> Query`: splits on whitespace and folds each word. A word with `*` or `?` becomes a `Mask`. An empty query has no words.
  - `pub fn Query::is_empty(&self) -> bool`
  - `pub fn Query::score(&self, folded_name: &str) -> Option<u8>`:
    - returns `None` if any term fails;
    - otherwise returns the **minimum** of the per-term classes:
      - 2 = the substring starts the name;
      - 1 = it starts after a non-alphanumeric character (a word boundary);
      - 0 = it is inside a word;
    - a `Mask` always scores 2.
  - `pub fn rank(hits: &mut Vec<(u8, i64, u32)>, limit: usize) -> usize`:
    - each tuple is (score, mtime, entry id);
    - keeps the best `limit` hits, sorted by score descending, then mtime descending, then id ascending;
    - returns the total count before truncation;
    - uses `select_nth_unstable_by` when the total is above `limit`.
- Mask semantics are exactly those of `matchMask` in `ui/lib.mjs:261`:
  - match against the whole folded name;
  - `*` = any run, `?` = one char;
  - a mask with no dot also matches names without an extension (check `maskRegex` and mirror it).
  - Write a small hand-rolled glob matcher. No regex crate.

- [ ] **Step 1: Write failing tests in `query.rs`:**
```rust
#[test] fn and_words_any_order() { let q = Query::parse("work scope"); assert!(q.score(&fold("Scope of Work v2.docx")).is_some()); }
#[test] fn all_words_required() { assert!(Query::parse("scope budget").score(&fold("Scope of Work.docx")).is_none()); }
#[test] fn nfd_name_matches_nfc_query() { let nfd: String = "Pročitaj.txt".nfd().collect(); assert!(Query::parse("pročitaj").score(&fold(&nfd)).is_some()); }
#[test] fn mask_word() { let q = Query::parse("fxfm *.txt"); assert!(q.score(&fold("notes-fxfm.txt")).is_some()); assert!(q.score(&fold("fxfm.md")).is_none()); }
#[test] fn question_mark_is_one_char() { assert!(Query::parse("a?c").score(&fold("abc")).is_some()); assert!(Query::parse("a?c").score(&fold("abbc")).is_none()); }
#[test] fn classes() { let q = Query::parse("work"); assert_eq!(q.score("work.txt"), Some(2)); assert_eq!(q.score("scope of work.txt"), Some(1)); assert_eq!(q.score("homework.txt"), Some(0)); }
#[test] fn rank_orders_and_counts() { let mut h = vec![(0,50,1),(2,10,2),(2,30,3),(1,99,4)]; assert_eq!(rank(&mut h, 3), 4); assert_eq!(h.iter().map(|x| x.2).collect::<Vec<_>>(), vec![3,2,4]); }
#[test] fn empty_query() { assert!(Query::parse("   ").is_empty()); }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core search::query`. Expected: FAIL (module missing).
- [ ] **Step 3:** Implement `query.rs` and `mod.rs` with the interfaces above.
- [ ] **Step 4:** Run `cargo test -p fxfm-core search::query`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): query parsing, folding and ranking`.

---

### Task 2: In-memory index (core)

**Files:**
- Create: `crates/core/src/search/index.rs`
- Modify: `crates/core/src/search/mod.rs`

**Interfaces:**
- Consumes: `fold`, `Query`, `rank` from Task 1.
- Produces:
  - `pub type EntryId = u32;`
  - `pub struct Meta { pub is_dir: bool, pub size: u64, pub mtime: i64, pub in_archive: bool }`
  - `pub struct Index`, with one root per indexed volume:
    - `pub fn Index::new() -> Index`
    - `pub fn add_root(&mut self, root: &str) -> EntryId`: an absolute path such as `/` or `/Volumes/Mdisk`; it returns the existing id if present.
    - `pub fn insert(&mut self, parent: EntryId, name: &str, meta: Meta) -> EntryId`: replaces the meta when `name` already exists under `parent`.
    - `pub fn lookup(&self, path: &str) -> Option<EntryId>`: an absolute path. A path into an archive is `<archive path>/<inner path>`, as in `vfs`.
    - `pub fn remove(&mut self, id: EntryId)`: removes the entry and its whole subtree; ids are tombstoned and not reused until `compact`.
    - `pub fn children(&self, id: EntryId) -> Vec<EntryId>`
    - `pub fn path(&self, id: EntryId) -> String`: the original (not folded) names joined with `/`.
    - `pub fn meta(&self, id: EntryId) -> Meta`
    - `pub fn root_of(&self, id: EntryId) -> EntryId`
    - `pub fn search(&self, q: &Query, scope: &Scope, limit: usize, cancel: &AtomicBool) -> (Vec<EntryId>, usize)`
    - `pub fn len(&self) -> usize`: live entries.
    - `pub fn heap_bytes(&self) -> usize`: an estimate from the capacities of the internal vectors.
    - `pub fn compact(&mut self)`: drops tombstones; ids change.
  - `pub enum Scope { Everywhere { archives: bool }, Under { path: String, archives: bool } }`, in `mod.rs`.
  - `pub fn apply_change(index: &mut Index, path: &str, stat: Option<Meta>) -> Change`, where `enum Change { Removed, Upserted(EntryId), NewDir(EntryId), Unknown }`:
    - `None` stat removes the path and its subtree;
    - `Some` upserts, creating the parent chain only if the parent is already indexed (otherwise `Unknown`);
    - `NewDir` tells the caller to scan that folder.
- Layout (the builder may refine it, within budget):
  - one `String` arena for the original names and one for the folded names;
  - one struct per entry: name offset and length, folded offset and length, `parent: u32`, `first_child: u32`, `next_sibling: u32`, `mtime: i64`, `size: u64`, flags `u8`, in a `Vec`.
  - Search scans the folded arena with `std::thread::scope` over 4 chunks and checks `cancel` every 64k entries.
  - `Scope::Under` keeps an entry only if walking its parent chain reaches the scope's entry id.
  - Archive entries are skipped unless `archives` is true.

- [ ] **Step 1: Write failing tests in `index.rs`:**
```rust
#[test] fn insert_lookup_path() { let mut i = Index::new(); let r = i.add_root("/"); let u = i.insert(r, "Users", dir()); let f = i.insert(u, "a.txt", file()); assert_eq!(i.lookup("/Users/a.txt"), Some(f)); assert_eq!(i.path(f), "/Users/a.txt"); }
#[test] fn remove_drops_subtree() { /* /d/x, /d/y; remove /d → lookup None for all; search "x" finds nothing; len()==1 (root) */ }
#[test] fn rename_via_apply_change() { /* apply_change("/d/old.txt", None) then apply_change("/d/new.txt", Some(file())) → "old" 0 hits, "new" 1 hit */ }
#[test] fn apply_change_new_dir_reports_scan() { /* apply_change("/d/sub", Some(dir())) == Change::NewDir(_) */ }
#[test] fn apply_change_unknown_parent() { /* apply_change("/nope/x", Some(file())) == Change::Unknown */ }
#[test] fn scope_under() { /* /a/x.txt and /b/x.txt; Scope::Under{path:"/a"} → only /a/x.txt */ }
#[test] fn archives_hidden_unless_asked() { /* entry with in_archive=true not found with archives:false, found with true */ }
#[test] fn search_returns_total_and_cap() { /* 1500 files "f{n}.txt"; search limit 1000 → (1000 ids, 1500) */ }
#[test] fn compact_keeps_paths() { /* insert, remove some, compact; every remaining path() still lookup()s to itself */ }
#[test] #[ignore] fn budget_3m() { /* 3_000_000 synthetic entries (1000 dirs × 3000 files, names ~20 chars); assert heap_bytes() <= 250 << 20; time a query "f12 txt" in release: < 500 ms */ }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core search::index`. Expected: FAIL.
- [ ] **Step 3:** Implement `index.rs`.
- [ ] **Step 4:** Run `cargo test -p fxfm-core search::index` and `cargo test -p fxfm-core --release search::index::budget_3m -- --ignored`. Expected: all PASS. Write the measured bytes and query time into the task report.
- [ ] **Step 5:** Commit `feat(search): in-memory name index`.

---

### Task 3: Volume scan, subtree rescan and direct walk (core)

**Files:**
- Create: `crates/core/src/search/scan.rs`

**Interfaces:**
- Consumes: `Index`, `Meta`, `EntryId`, `Query` (Tasks 1–2).
- Produces:
  - `pub struct ScanOptions { pub exclude: Vec<String>, pub skip_mounts: Vec<String>, pub cancel: Arc<AtomicBool> }`
    - `exclude` holds absolute paths, plus bare folder names that match at any depth (for example `.Trashes`).
    - `skip_mounts` holds mount points of other volumes. The caller fills it from the volume list. `/` and `/System/Volumes/Data` are never put in it.
  - `pub struct ScanReport { pub entries: usize, pub denied: Vec<String>, pub cancelled: bool }`
  - `pub fn default_excludes() -> Vec<String>`: the list from Global Constraints.
  - `pub fn scan_into(index: &mut Index, root: EntryId, root_path: &Path, opts: &ScanOptions) -> ScanReport`
    - Breadth-first, using `std::fs::read_dir` and `symlink_metadata`. It never follows symlinks (they are indexed as entries).
    - Names are stored as they are on disk; folding happens in the index.
    - A folder whose `read_dir` fails with `PermissionDenied` (or EPERM) is added to `denied` and skipped.
    - Other errors are skipped silently.
    - On macOS it does not descend into a directory whose `st_flags & SF_DATALESS` (`0x40000000`) is set. The directory itself is indexed.
  - `pub fn rescan_subtree(index: &mut Index, path: &str, opts: &ScanOptions) -> ScanReport`: removes the subtree under `path` and scans it again (used for `MustScanSubDirs` and `NewDir`).
  - `pub fn lower_priority()`: on macOS `setpriority(PRIO_DARWIN_THREAD, 0, PRIO_DARWIN_BG)`, elsewhere a no-op. The scanning thread calls it first.
  - `pub fn is_dataless(md: &std::fs::Metadata) -> bool`: macOS uses `MetadataExt::st_flags`; elsewhere it returns `false`.
  - `pub fn walk_search(root: &Path, q: &Query, archives: Option<&ArchiveOptions>, cancel: &AtomicBool, on_batch: &mut dyn FnMut(Vec<WalkHit>))`
    - It is the direct-walk fallback, with the same exclusions as `default_excludes()`.
    - It calls `on_batch` every 200 hits or every 250 ms.
    - `pub struct WalkHit { pub path: String, pub meta: Meta, pub score: u8 }`
    - `ArchiveOptions` comes from Task 4. Until Task 4 lands, accept `None` only.

- [ ] **Step 1: Write failing tests (tempdir trees only):**
```rust
#[test] fn scans_tree_with_hidden() { /* a/.hidden, a/b/c.txt → all indexed, lookup works */ }
#[test] fn excludes_by_name_and_path() { /* x/.Trashes/f and excluded abs path <tmp>/skip/f → not indexed */ }
#[test] fn skip_mounts() { /* skip_mounts=[<tmp>/vol] → <tmp>/vol/f not indexed, <tmp>/vol itself indexed */ }
#[test] fn symlink_not_followed() { /* loop symlink a/l -> a ; scan terminates, l indexed once */ }
#[test] fn denied_folder_counted() { /* chmod 000 <tmp>/locked; report.denied == [<tmp>/locked]; rest indexed; restore mode in a guard. Skip the test when running as root (euid 0). */ }
#[test] fn nfd_name_round_trip() { /* create file with NFD name; lookup(path using the on-disk name) ok; Query "pročitaj" finds it; path() equals the on-disk name */ }
#[test] fn cancel_stops() { /* cancel set before scan → report.cancelled */ }
#[test] fn rescan_subtree_picks_up_changes() { /* scan, add/remove files in a subfolder, rescan_subtree → index matches disk */ }
#[test] fn walk_search_batches() { /* 450 matching files → batches sum to 450 hits; cancel mid-walk stops */ }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core search::scan`. Expected: FAIL.
- [ ] **Step 3:** Implement `scan.rs`.
- [ ] **Step 4:** Run `cargo test -p fxfm-core search::scan`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): volume scan, subtree rescan and direct walk`.

---

### Task 4: Archive entries (core)

**Files:**
- Create: `crates/core/src/search/archives.rs`
- Modify: `crates/core/src/search/scan.rs` (`walk_search` uses it)

**Interfaces:**
- Consumes:
  - `crate::archive::{ArchiveKind, IndexCache or the readers in archive/read.rs, ArchiveIndex, IndexEntry}`. Read `crates/core/src/archive/mod.rs:263-330` and `read.rs` to find the function that lists an archive without extracting it; reuse it rather than writing a new reader.
  - `Index`, `Meta` (Task 2).
- Produces:
  - `pub struct ArchiveOptions { pub gz_limit: u64, pub network: bool }`. `gz_limit` defaults to 500 MB = `500 * 1024 * 1024` bytes.
  - `pub struct ArchiveStamp { pub size: u64, pub mtime: i64 }`
  - `pub fn index_archive(index: &mut Index, archive: EntryId, path: &Path, opts: &ArchiveOptions, on_network: bool, md: &Metadata) -> ArchiveOutcome`, where `enum ArchiveOutcome { Indexed(usize), Skipped(&'static str) }`.
    - It returns `Skipped` in these cases:
      - `"network"` when `on_network && !opts.network`;
      - `"cloud"` when `is_dataless(md)`;
      - `"too big"` for TarGz/Gz above `gz_limit`;
      - `"encrypted names"` when the reader returns `PasswordRequired` while listing;
      - `"unreadable"` on any other error.
    - On success it removes the archive entry's old children and inserts the inner entries (with intermediate folders) as children, with `in_archive = true`.
    - Inner archives are indexed as plain entries and are not entered.
  - `pub fn needs_reindex(old: Option<ArchiveStamp>, md: &Metadata) -> bool`: true when size or mtime changed, or `old` is `None`.
  - `pub fn index_archives_under(index: &mut Index, root: EntryId, opts: &ArchiveOptions, on_network: bool, stamps: &mut HashMap<String, ArchiveStamp>, cancel: &AtomicBool) -> usize`
    - It finds every file in the subtree whose name `ArchiveKind::from_name` accepts.
    - It skips archives whose stamp is unchanged.
    - It returns the number re-read.

- [ ] **Step 1: Write failing tests.** Build the archives with the existing test helpers in `crate::archive::testutil` (zip, tar, tgz, 7z):
```rust
#[test] fn zip_entries_searchable() { /* SOW.zip containing docs/scope-of-work.txt; Scope archives:true finds "<tmp>/SOW.zip/docs/scope-of-work.txt"; archives:false does not */ }
#[test] fn unchanged_archive_not_reread() { /* second index_archives_under returns 0 */ }
#[test] fn changed_archive_reread() { /* rewrite zip with other entry → returns 1, old inner name gone */ }
#[test] fn tgz_over_limit_skipped() { /* gz_limit = 10 bytes → Skipped("too big"), archive file itself still indexed */ }
#[test] fn network_off_skips() { /* on_network=true, opts.network=false → Skipped("network") */ }
#[test] fn nested_not_entered() { /* outer.zip containing inner.zip with a.txt → inner.zip found, a.txt not */ }
#[test] fn corrupt_archive_unreadable() { /* garbage bytes named x.zip → Skipped("unreadable"), no panic */ }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core search::archives`. Expected: FAIL.
- [ ] **Step 3:** Implement `archives.rs`. Then let `walk_search` take `Some(&ArchiveOptions)` and list matching inner entries of every archive it meets. The paths it reports are `<archive>/<inner>`.
- [ ] **Step 4:** Run `cargo test -p fxfm-core search::`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): archive entries in the index`.

---

### Task 5: Disk format (core)

**Files:**
- Create: `crates/core/src/search/store.rs`

**Interfaces:**
- Consumes: `Index` (Task 2), `ArchiveStamp` (Task 4).
- Produces:
  - `pub const FORMAT_VERSION: u32 = 1;`
  - `pub struct VolumeState { pub root: String, pub uuid: Option<String>, pub event_id: u64, pub refreshed: i64 /* unix s */, pub network: bool, pub source: Option<String> /* mntfrom for network */, pub denied: Vec<String>, pub complete: bool }`
  - `pub struct Stored { pub index: Index, pub volumes: Vec<VolumeState>, pub stamps: HashMap<String, ArchiveStamp> }`
  - `pub fn save(dir: &Path, s: &Stored) -> io::Result<()>`:
    - writes `dir/index.bin.tmp`, fsyncs it and renames it to `dir/index.bin`;
    - creates `dir` if missing.
  - `pub fn load(dir: &Path) -> Result<Stored, LoadError>`, where `enum LoadError { Missing, OldVersion(u32), Corrupt(String) }`.
  - Format:
    - an 8-byte magic `b"FXFMIDX\0"`, `FORMAT_VERSION` as u32 LE and a payload length;
    - then a hand-written little-endian binary encoding of the compacted index arrays, volumes and stamps;
    - then a trailing CRC-32 or SHA-256 prefix of the payload. `sha2` is already a dependency of the Tauri crate; for core, a simple CRC-32 written inline is fine.
  - `save` calls `compact()` on a clone or works on an already-compacted index; the caller decides (document which).

- [ ] **Step 1: Write failing tests:**
```rust
#[test] fn round_trip() { /* index with dirs, files, archive entries, 2 volumes, stamps → save → load equal paths, metas, volumes, stamps */ }
#[test] fn missing() { assert!(matches!(load(empty_tmp), Err(LoadError::Missing))); }
#[test] fn old_version() { /* write header with version 0 → Err(OldVersion(0)) */ }
#[test] fn truncated_is_corrupt() { /* save, truncate file to half → Err(Corrupt(_)) without panic */ }
#[test] fn flipped_byte_is_corrupt() { /* flip a payload byte → Err(Corrupt(_)) */ }
#[test] #[ignore] fn budget_3m_disk() { /* 3M synthetic entries → file size <= 300 << 20; record size and save/load time in the report */ }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core search::store`. Expected: FAIL.
- [ ] **Step 3:** Implement `store.rs`.
- [ ] **Step 4:** Run `cargo test -p fxfm-core search::store` and `cargo test -p fxfm-core --release search::store::budget_3m_disk -- --ignored`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): versioned index file`.

---

### Task 6: Search settings and keymap entries (core)

**Files:**
- Modify: `crates/core/src/config.rs` (`Settings` at line 215 and its `Default`)
- Modify: `crates/core/src/keymap.rs:338` (`cm_search` becomes `available: true`) and add `cm_feed_listbox`
- Test: the existing tests in `config.rs` and `keymap.rs`, and `tests/fixtures` if settings fixtures exist

**Interfaces:**
- Produces:
```rust
#[serde(rename_all = "camelCase", default)]
pub struct SearchSettings {
    /// None = never asked (the first Alt+F7 shows the prompt); Some(true) = build and keep current; Some(false) = off.
    pub index: Option<bool>,
    pub volumes: Vec<SearchVolume>,          // other local and network volumes the user enabled
    pub exclude: Vec<String>,                // user exclusions (absolute paths), on top of default_excludes()
    pub archives: bool,                      // false
    pub gz_limit_mb: u64,                    // 500
    pub network_archives: bool,              // false
    pub network_refresh_hours: u32,          // 24; 0 = manual only
}
pub struct SearchVolume { pub path: String, pub enabled: bool }
```
- The startup disk is always indexed when `index == Some(true)` and is not listed in `volumes`.
- `Settings` gets `pub search: SearchSettings`. An old `settings.json` without it loads with the defaults.
- `cm_feed_listbox`:
  - menu "Commands", title "Feed search results to listbox";
  - no default key in the main keymap (the window uses Ctrl+Enter itself);
  - `available: true`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test] fn search_defaults() { let s = SearchSettings::default(); assert_eq!(s.index, None); assert!(!s.archives); assert_eq!(s.gz_limit_mb, 500); assert!(!s.network_archives); assert_eq!(s.network_refresh_hours, 24); }
#[test] fn old_settings_without_search_load() { /* serde_json from an object without "search" → Settings.search == SearchSettings::default() */ }
#[test] fn cm_search_available() { /* lookup cm_search → available == true, default Alt+F7 */ }
```
- [ ] **Step 2:** Run `cargo test -p fxfm-core config keymap`. Expected: FAIL.
- [ ] **Step 3:** Implement. Update every test that lists all commands or all settings fields.
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): search settings and command entries`.

---

### Task 7: Search service and commands (src-tauri, without FSEvents)

**Files:**
- Create: `src-tauri/src/search.rs`
- Modify: `src-tauri/src/state.rs` (`AppState` gets `pub search: search::Service`)
- Modify: `src-tauri/src/main.rs` (register the commands; start the service in `setup`; save on exit next to `flush_session`)
- Modify: `src-tauri/src/commands.rs`: `save_settings` calls `search.apply_settings(&settings.search)`.

**Interfaces:**
- Consumes: everything from Tasks 1–6.
  - The volume list comes from `fxfm_core::volumes::list_volumes`.
  - For network volumes, `mntfrom` comes from the existing `getfsstat` code in `src-tauri/src/smb.rs`. Expose a small `pub fn mounts() -> Vec<(String /*mount point*/, String /*fstype*/, String /*mntfrom*/)>` there and reuse it.
- Produces the Tauri commands (camelCase JSON):
  - `search_status() -> Status`
    - `Status { state: "off"|"notAsked"|"building"|"ready", entries: u64, volumes: Vec<VolStatus>, denied: Vec<String>, fdaNeeded: bool }`
    - `VolStatus { path, name, kind: "startup"|"local"|"network", enabled, mounted, entries, refreshed: i64, state: "building"|"ready"|"stale"|"off" }`
    - The list contains every currently mounted volume, plus enabled volumes that are not mounted.
  - `search_build()`: sets `settings.search.index = Some(true)`, saves the settings and starts the startup-disk scan.
  - `search_start(req: SearchReq) -> u64`
    - `SearchReq { query: String, under: Option<String>, archives: bool, limit: u32 }`
    - It cancels the previous search.
    - With a ready index it searches the index on a worker thread.
    - Without one (index off, not asked yet, or `under` not covered by a complete index), it runs `walk_search` on `under`. With no `under`, it returns an error string `"no index"`.
    - It emits `search://batch { id, items: Vec<Item>, total: u64, done: bool }` to the main window.
    - `Item { path, name, folder, mtime, size, isDir, inArchive, mounted }`. `folder` is the parent location; for an archive entry `name` is `"<archive name> ▸ <inner path>"`.
    - When `archives` is set and `settings.search.archives` is on, it passes `ArchiveOptions` built from `gz_limit_mb` and `network_archives` to the walk as well.
  - `search_cancel(id: u64)`
  - `search_forget(path: String)`: removes the path from the index (a stale result).
  - `search_rebuild(path: Option<String>)`: None = everything; also used for "Retry" on denied folders.
  - `search_refresh(path: String)`: rescans one network volume now.
- Service rules:
  - The index sits in `RwLock<Option<Stored>>`. Scans build into a **separate** `Index` and swap it in under a short write lock, so searches never wait for a scan.
  - Incremental changes (Task 8) take the write lock per event batch.
  - The index directory is `<data.dir>/index` (`DataLocation.dir`). It follows a storage mode change: on `set_storage_mode`, save to the new folder.
  - Load on startup when `index == Some(true)`:
    - on `LoadError::OldVersion` or `Corrupt`, log it and rebuild;
    - on `Missing`, build.
  - Save after a scan finishes, every 10 minutes while dirty, and on exit.
  - Scan threads call `scan::lower_priority()` first.
  - `skip_mounts` = the mount points of every other volume.
  - The startup-disk root is `/`, and the excludes are `default_excludes()` plus the user's.
  - Archive pass: after each volume scan, if `archives` is on, run `index_archives_under`. Network volumes take `on_network = true`.
  - Network refresh: a timer thread checks every 10 minutes and rescans each enabled mounted network volume whose `refreshed` is older than `network_refresh_hours` (skipped when 0).
  - `fdaNeeded` is true when the startup-disk scan has any denied folder.
  - When an enabled volume is unmounted, its entries stay in the index with `mounted: false` in results. `volumes://changed` (see `volwatch.rs`) triggers a status re-read.
- Tests (Rust unit tests in `search.rs` on a `Service` constructed with a temp data dir and a temp "startup root"):
  - make the root path a constructor parameter, so tests never touch `/`;
  - `search_build` scans the temp root and returns hits;
  - a corrupt `index.bin` leads to a rebuild;
  - a new `search_start` cancels the previous one (the old id gets no further batches);
  - `under` without an index uses the walk;
  - `forget` drops an entry.

- [ ] **Step 1:** Write the failing tests above in `src-tauri/src/search.rs` (`#[cfg(test)]`). Use a fake emitter closure in place of `AppHandle`: the service takes `Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>`.
- [ ] **Step 2:** Run `cargo test -p fxfm search`. Expected: FAIL.
- [ ] **Step 3:** Implement the service and the commands, and wire up `state.rs`, `main.rs` and `commands.rs`.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): index service and commands`.

---

### Task 8: FSEvents live updates and replay (src-tauri, macOS)

**Files:**
- Create: `src-tauri/src/fsevents.rs` (`#[cfg(target_os = "macos")]`)
- Modify: `src-tauri/src/search.rs`
- Modify: `src-tauri/build.rs` or the `#[link(name = "CoreServices", kind = "framework")]` attribute

**Interfaces:**
- Produces:
  - `pub struct Stream`, with `pub fn start(paths: &[String], since: u64, latency: f64, on: impl Fn(Vec<Event>) + Send + 'static) -> Result<Stream, String>`.
  - `Drop` stops, invalidates and releases the stream.
  - `pub struct Event { pub path: String, pub flags: u32, pub id: u64 }`
  - `pub fn current_event_id() -> u64` and `pub fn volume_uuid(dev: libc::dev_t) -> Option<String>` (`FSEventsCopyUUIDForDevice`).
  - Flags used:
    - `kFSEventStreamCreateFlagFileEvents` (0x10) and `NoDefer` (0x02);
    - event flags `MustScanSubDirs` (0x1), `UserDropped` (0x2), `KernelDropped` (0x4), `EventIdsWrapped` (0x8), `HistoryDone` (0x10), `RootChanged` (0x20).
  - Latency is 1.0 s. The stream runs on its own thread with a `CFRunLoop`, or on a dispatch queue (`FSEventStreamSetDispatchQueue`); the builder chooses.
- Service handling in `search.rs`, through a pure function that is unit-testable on any OS:
  - `fn handle_events(index, events, opts) -> Vec<Followup>`
  - `enum Followup { RescanSubtree(String), RescanVolume(String), SetEventId(u64) }`
  - Rules:
    - For each event path, `symlink_metadata` gives the stat (None if gone), then `apply_change` runs.
    - `NewDir` leads to `RescanSubtree`.
    - `MustScanSubDirs` leads to `RescanSubtree(path)`.
    - `UserDropped`, `KernelDropped`, `EventIdsWrapped` and `RootChanged` lead to `RescanVolume`.
    - Paths under an exclusion are ignored.
    - The last event id is stored per volume.
  - Archive files whose stamp changes are re-read when `archives` is on.
  - At startup the stream starts with `since = stored event_id`.
  - If the stored `uuid` differs from `volume_uuid` of the root device, or no id is stored, the volume is rescanned and the stream starts from `current_event_id()` taken **before** the scan.
- **Verify first (read-only probe):** before writing the service code, check on this Mac what event paths a stream on `/` reports for a file you create under the session scratchpad folder: `/private/tmp/...` or `/tmp/...`, and for `$HOME`-side data `/Users/...` or `/System/Volumes/Data/Users/...`. Write the answer into the report. If events arrive as `/System/Volumes/Data/...`, map them back to firmlinked paths by stripping that prefix before `apply_change`.
- Do not create files in `$HOME`. The scratchpad folder is `/private/tmp/...`. Excluded `/private/tmp` is fine for a probe, because the probe does not go through the service.

- [ ] **Step 1:** Write failing unit tests for `handle_events` on an `Index` built over a tempdir:
  - created file → hit;
  - deleted folder with 2,000 files → 0 hits for its names;
  - rename (old gone, new present) → old 0, new 1;
  - `MustScanSubDirs` → `RescanSubtree`;
  - `KernelDropped` → `RescanVolume`;
  - an event under an excluded path → no change.
- [ ] **Step 2:** Run `cargo test -p fxfm handle_events`. Expected: FAIL.
- [ ] **Step 3:** Implement `handle_events`, the FFI in `fsevents.rs`, the startup replay and the UUID check.
- [ ] **Step 4:** Add a macOS-only test, `#[ignore]`, that starts a `Stream` on a tempdir, creates a file and receives an event within 5 s. Run it with `cargo test -p fxfm fsevents -- --ignored`, then run the whole verification. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): FSEvents live updates and replay`.

---

### Task 9: UI helpers for results and the results location (lib.mjs)

**Files:**
- Modify: `ui/lib.mjs`
- Modify: `ui/lib.test.mjs`

**Interfaces:**
- Produces:
  - `export const SEARCH_LOCATION = "search:results";`. It is not matched by `parseRemote` (the regex there needs `scheme://`).
  - `export function isSearchLocation(loc) -> boolean`
  - `export function homeDisplay(path, home) -> string`: `/Users/x/Docs/a` becomes `~/Docs/a`.
  - `export function resultEntries(items, home) -> Entry[]`, which maps search `Item`s to panel entries:
    - `{ name: <display key>, path: item.path, isDir, size, mtime, hidden:false, inArchive, mounted, mode: 0, isSymlink:false }`;
    - the display key is `homeDisplay(item.path, home)`; it is unique per path, so marks keyed by `name` stay separate for equal file names (Review Focus 4).
  - `operationPaths(location, sources)` (existing, `ui/lib.mjs:514`) changes to use `e.path` when present, otherwise `joinLocation(location, e.name)`. Its `cwd` for a results list is the folder of the first source.
  - `export function resultOps(entries) -> { view: bool, edit: bool, copy: bool, move: bool, del: bool, drag: bool }`:
    - the allowed ops for the selection;
    - if any entry has `inArchive`, only `view` (single) and `copy` are true;
    - if any entry has `mounted === false`, all are false.
  - `export function dropMissing(entries, existsSet) -> Entry[]`: keeps the entries whose `path` is in `existsSet` (used after F6/F8).
  - `export function resultRow(item, home) -> { name, folder, date }`: for the search window rows; the folder uses `homeDisplay` and the date `formatDate`.

- [ ] **Step 1: Write failing tests in `ui/lib.test.mjs`:**
```js
test("search location is not remote", () => { assert.equal(parseRemote(SEARCH_LOCATION), null); assert.ok(isSearchLocation(SEARCH_LOCATION)); });
test("equal names stay separate rows", () => { const e = resultEntries([{path:"/Users/x/a/notes.txt",name:"notes.txt"},{path:"/Users/x/b/notes.txt",name:"notes.txt"}], "/Users/x"); assert.deepEqual(e.map(x=>x.name), ["~/a/notes.txt","~/b/notes.txt"]); });
test("operationPaths uses entry path", () => { const e = resultEntries([{path:"/v/a/x.txt"}], "/h"); assert.deepEqual(operationPaths(SEARCH_LOCATION, e).paths, ["/v/a/x.txt"]); });
test("operationPaths unchanged for folders", () => { assert.deepEqual(operationPaths("/d", [{name:"f"}]).paths, ["/d/f"]); });
test("archive entries allow view and copy only", () => { const o = resultOps([{inArchive:true}]); assert.deepEqual(o, {view:true, edit:false, copy:true, move:false, del:false, drag:false}); });
test("unmounted entries allow nothing", () => { assert.equal(resultOps([{mounted:false}]).copy, false); });
test("dropMissing", () => { assert.deepEqual(dropMissing([{path:"/a"},{path:"/b"}], new Set(["/b"])).map(e=>e.path), ["/b"]); });
test("homeDisplay", () => { assert.equal(homeDisplay("/Users/x/D/a", "/Users/x"), "~/D/a"); assert.equal(homeDisplay("/Volumes/M/a", "/Users/x"), "/Volumes/M/a"); });
```
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement the helpers.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS (all old tests still pass).
- [ ] **Step 5:** Commit `feat(search): UI helpers for search results`.

---

### Task 10: Find files window and first-run prompt (ui/search.js)

**Files:**
- Create: `ui/search.js`
- Modify: `ui/app.js` (handler `cm_search` opens the window; listen to `search://batch`)
- Modify: `ui/index.html` (script import, if modules are listed there)
- Modify: `ui/style.css`

**Interfaces:**
- Consumes: the commands from Task 7, the helpers from Task 9, and `dialog`, `pushModal`, `popModal` and `handleModalKey` from `ui/dialogs.js`.
- Produces: `export async function openSearch(ctx)`. `ctx` gives the active panel, `home`, `settings`, `invoke`, and `feed(items, query)` (Task 11).
- Behavior, as in the spec section "The Find files window":
  - **Fields:** Name; In: Everywhere / Current folder; an "Inside archives" checkbox, shown only when `settings.search.archives` is on.
  - **Typing:** debounce 120 ms, then `search_start`. Rows replace the previous ones; the batches of a walk append.
  - **Status line:** "N found" (plus "· showing first 1,000" when total > 1,000), followed by the index state from `search_status`:
    - "index: 1,842,310 files, up to date";
    - "building index…";
    - "volume <name> last refreshed <date>";
    - "searching folder…" during a walk.
  - **Keys:**
    - Up/Down/PgUp/PgDn move the row;
    - Enter checks that the result exists (call `list_dir` on its folder):
      - if it exists, close the window and `panel.go(folder, name)`; for an archive entry, folder = `<archive>/<inner parent>`;
      - if it is missing, show "<name> no longer exists.", call `search_forget` and remove the row;
      - if the result's `mounted` is false: when a saved SMB server matches its volume (reuse `serverDrives`/`connectFlow` from `ui/servers.js`), connect, then go; otherwise show "Volume <name> is not mounted.";
    - F3 views the row with the same code path as panel F3 (`viewOrPlay`/`open_viewer`); the window stays open;
    - Ctrl+Enter and the "Feed to listbox" button call `ctx.feed(items, query)` and close the window;
    - Esc closes.
  - **Greyed rows:** rows with `mounted: false` get CSS class `stale`.
  - **Session memory:** the last query, In and Archives are kept in module state for the session.
  - **First run:** when `search_status().state === "notAsked"`, show the prompt first (`dialog` with three buttons):
    - text: "fxfm can index this Mac's disk in the background so search finds files instantly. With Full Disk Access the index is complete; without it macOS may ask about some folders, and refused folders are skipped.";
    - buttons: *Open Privacy settings* (reuse the existing FDA hint's opener in `app.js:620`), *Build index* (`search_build`), *Not now*;
    - after *Not now* or *Open Privacy settings*, the window opens with "Current folder" selected and works by walk;
    - the prompt shows again on the next launch, until Build or off in Options.
  - **Index off:** the window shows "Current folder" only, with the note "Index is off (Options → Search)".
  - **Results:** never call `search_start` with an empty query; clear the rows instead.

- [ ] **Step 1:** Write failing tests in `ui/lib.test.mjs` for the window's pure logic. Extract it to `lib.mjs` as `export function searchWindowState(prev, event)`, a reducer over `{rows, total, done, id}` for `batch` events:
```js
test("batch of old id ignored", ...);   // state.id=2, event id 1 → unchanged
test("walk batches append", ...);       // two batches id 2 → rows concatenated, total from last
test("index batch replaces", ...);      // done:true single batch → rows replaced
test("cap note", ...);                  // statusText({total:1500, shown:1000}) === "1,500 found · showing first 1,000"
```
  Also add `export function statusText({ total, shown, state, walking })`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement the reducer and `statusText`, then `ui/search.js`, the `cm_search` handler and the styles.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS. Do not launch the app; the controller smoke-tests the build. List the manual checks in the report.
- [ ] **Step 5:** Commit `feat(search): Find files window (Alt+F7)`.

---

### Task 11: Feed to listbox: results in a panel

**Files:**
- Modify: `ui/app.js` (`feed`, the panel API's `listDir` wrapper, `onNavigated`)
- Modify: `ui/panel.js` (name column for results rows; `goParent` and `rootLocation` for the results location; Ctrl+PgUp/Enter behavior)
- Modify: `ui/dialogs.js` (F3/F4/F5/F6/F8 use `resultOps`; refresh after jobs)
- Modify: `ui/dragdrop.js` (drag from a results panel uses `e.path`)
- Modify: `ui/lib.test.mjs`

**Interfaces:**
- Consumes: `SEARCH_LOCATION`, `resultEntries`, `operationPaths`, `resultOps` and `dropMissing` (Task 9), and `feed` from Task 10.
- Produces:
  - `app.results = { side → { entries, query, back: {location, cursorName} } }`, held in `app.js`.
  - `feed(items, query)`:
    - stores `entries` = `resultEntries(items, home)` for the active side and `back` = the panel's current location and cursor;
    - then `panel.go(SEARCH_LOCATION)`.
  - The panel's `listDir(location)`: for `isSearchLocation(location)` it returns `app.results[side].entries` (filtered for existence on refresh); otherwise it calls `list_dir` as today.
  - The path row shows `Search results: <query>`; extend `displayLocation` in `lib.mjs` with a test.
- Behavior, from the spec section "Feed to listbox":
  - **Enter on a row** (also on a folder row) and **Ctrl+PgUp** go to the item's folder with the cursor on it. For an archive entry, the folder is the location inside the archive.
  - **Backspace or `..`** returns to `back`.
  - **F3, F4, F5/F6, F8 and drag-out** act on the marked items or the cursor item through `operationPaths`. When `resultOps` forbids an op, the status line says "Not available for items inside archives" (or "…on a volume that is not mounted").
  - **F5/F6** go to the other panel's folder (the existing dialog); name clashes use the existing conflict dialog.
  - **Refresh:** after a finished F6 or F8 job, or a manual refresh (Ctrl+R) of a results panel, `stat` every result path and keep only those that exist (`dropMissing`).
    - Use a new command `paths_exist(paths: Vec<String>) -> Vec<String>` in `commands.rs`. For archive entries, keep them unless the archive file itself is gone.
  - **Session:** saving the session while a panel shows results stores `back.location` instead.
  - **Volume and free space:** the drive bar and the free-space line show nothing special for the results location (`volumeFor` returns null).
- [ ] **Step 1:** Write failing tests:
  - `displayLocation(SEARCH_LOCATION, {query:"fxfm"})` returns `"Search results: fxfm"`;
  - `panelState` for a results panel stores `back.location` (extract a pure `sessionLocation(panel, results)` into lib.mjs);
  - `parentLocation(SEARCH_LOCATION)` returns `null`;
  - Rust: `paths_exist` keeps existing local paths, drops missing ones, and keeps `<zip>/inner` while the zip exists.
- [ ] **Step 2:** Run `node --test ui/` and `cargo test -p fxfm paths_exist`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the full verification. Expected: PASS. Write the manual-check list into the report: feed 3 results from 3 folders, mark 2, F5 to the other panel, F8 one, Backspace back, Enter on a row.
- [ ] **Step 5:** Commit `feat(search): feed results to a panel`.

---

### Task 12: Options → Search, and docs

**Files:**
- Modify: `ui/config.js` (new `section("Search")` after "Storage", using the existing `section`/`radio` helpers at `ui/config.js:302`)
- Modify: `README.md` (a Search section) and `docs/ROADMAP.md` (move "Pretraga (Alt+F7)" from Next to Done with a one-line summary; add anything deferred)

**Interfaces:**
- Consumes: `search_status`, `search_build`, `search_rebuild`, `search_refresh` and `save_settings`.
- Section contents, from the spec section "Options → Search":
  - a checkbox "Index while fxfm is running" mapped to `settings.search.index` (on = `Some(true)`, off = `Some(false)`); turning it on runs `search_build`;
  - a volumes table:
    - one row per `VolStatus`, with a checkbox enabled/disabled, the name, the kind, the entry count, the state and "last refreshed <date>" for network volumes;
    - the startup disk is always on and its checkbox is disabled;
    - network rows have a "Refresh now" button;
  - an excluded-folders list (add via the folder picker `tauri-plugin-dialog`, already a dependency; remove with a button), with the defaults shown read-only above it;
  - a checkbox "Index files inside archives", plus "Skip tar.gz/gz larger than [500] MB" and "Also read archives on network volumes";
  - "Refresh network volumes every [24] hours (0 = only by hand)";
  - the denied folders list, with "Retry" (`search_rebuild` with each path) and the FDA note when `fdaNeeded`;
  - a "Rebuild index" button.
- [ ] **Step 1:** Write failing tests for a pure `searchSettingsPatch(settings, change)` helper in lib.mjs:
  - toggling a volume adds or updates it in `volumes`;
  - setting the gz limit to `"abc"` keeps 500;
  - `0` hours is kept.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement the helper, the section and the docs.
- [ ] **Step 4:** Run the full verification. Expected: PASS.
- [ ] **Step 5:** Commit `feat(search): Options → Search, docs`.

---

## After the last task (controller)
1. Run the final whole-branch review.
2. Build with `scripts/build-macos.sh`.
3. Smoke-test the fresh bundle only, with a temp HOME holding a copy of the data dir:
   - confirm the window appears;
   - check that Alt+F7 shows the first-run prompt;
   - check that *Not now* gives a folder walk that finds a file in the temp HOME.
   - Do **not** click *Build index* in the smoke test: that would scan the real disk under the owner's account.
4. Kill the app.
5. Copy the dmg to `~/Documents/PVT/fxfm-builds/macos/` and move the old one to the Trash.
6. Give the owner the manual-check lists from the Task 10 and Task 11 reports.
