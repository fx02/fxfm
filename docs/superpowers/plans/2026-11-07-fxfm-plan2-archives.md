# fxfm plan 2: archives as folders – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enter on `.zip`, `.tar`, `.tgz`/`.tar.gz`, `.gz` and `.7z` opens the archive like a folder. Inside, F3, F4 (with "upload back"), F5, F6, F7, F8 and F2 work, and the original archive is never corrupted.

**Architecture:**
- New `fxfm-core` modules:
  - `archive/` reads the index, extracts and rewrites the archive (new temporary archive, then atomic `rename`);
  - `vfs.rs` routes every location (`String`) to the local disk (existing `listing`/`ops`, **unchanged**) or to an archive.
- Operations involving an archive go through a local **staging** folder. The source is first materialized locally (extracted), then the existing `ops::copy` does the local part with the same conflicts and progress, and finally a single `rewrite` writes into the archive. Plan 3 reuses the same model later (server = download/upload).
- **Deviation from the "After plan 1" note**:
  - `LocalFs` is not moved behind a trait;
  - `ops.rs` (2,175 lines, reviewed and safe) stays untouched, and routing lives in `vfs.rs`;
  - the trait is introduced in plan 3 if a second backend requires it.
- `src-tauri` commands and `JobRequest` keep their shape but call `vfs::*`. F4 from an archive watches the temporary copy (`notify`) and asks whether to upload it back.

**Tech Stack:**
- everything from plan 1;
- `zip` 9 (features `deflate`, `deflate64`, `aes-crypto`, `time`; no default features, to avoid C dependencies);
- `tar` 0.4;
- `flate2` 1 (default miniz_oxide);
- `sevenz-rust2` 0.25 (feature `compress` for writing, without `aes256`);
- `notify` 8 (stable, not 9-rc), only in `src-tauri`;
- `dirs` (already present).

Everything is pure Rust or statically linked, so the universal build stays Homebrew-free.

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (§2 goal 6, §4 restore rules, §5 F3/F4, §8 Archives). Plan 1: `docs/superpowers/plans/2026-11-07-fxfm-plan1-local.md` (shared types and conventions apply here too).

## Global Constraints

- Everything from plan 1's Global Constraints still applies: Tauri isolation, `fxfm-core` is built and tested on Linux, commits only as `zbuzanic@gmail.com` with a Co-Authored-By line, UI texts in English.
- **Supported formats** are recognized by name, case-insensitively:
  - `.zip` → `Zip`
  - `.tar` → `Tar`
  - `.tgz` and `.tar.gz` → `TarGz`
  - `.gz` (but not `.tar.gz`) → `Gz`
  - `.7z` → `SevenZ`
  - Other "arc" extensions from `ARCHIVE_EXT` (bz2, xz, rar, tbz2, txz) keep their color, but Enter opens them with the default application, as before.
- **A location inside an archive** is `<path to archive>/<inner path>`, e.g. `/Users/z/a.zip/src/lib`. The archive root is just `/Users/z/a.zip`. Inner paths in code have no leading or trailing `/` (the root is `""`).
- **The original is never corrupted** (spec §8):
  - every write goes to `.<name>.fxfm-<pid>-<n>.tmp` in the **same folder**, then `fsync` and `rename` over the original;
  - the original's permissions are copied to the new file;
  - on error or Cancel the temp file is deleted and the original stays byte-identical.
- **Nested archives** (an archive inside an archive) are out of scope for plan 2. Enter on them, F3 and F4 treat the file as a plain file.
- **Encrypted:**
  - a password-protected zip can be read (asks for the password);
  - writing into an archive in which any entry is encrypted is refused with the message `Writing into encrypted archives is not supported`;
  - an encrypted 7z yields `Encrypted 7z archives are not supported`.
- **`.gz`** holds exactly one file. Its name is taken from the gzip header, or, if absent, from the archive name without `.gz`. Only F3, F4, F5 out and upload back are allowed. For F7, F8, F2, F6 and F5 in, the message is `Not possible in a .gz file (it holds exactly one file)`.
- **Temporary files** live in `dirs::cache_dir()/fxfm/tmp/<pid>/`:
  - `stage-<n>/` for operations, deleted when the operation finishes;
  - `view-<n>/` for F3 and Enter on a file, deleted on exit;
  - `edit-<n>/` for F4, deleted on exit only if there are no unuploaded changes (see Task 8).
- **Deleting inside an archive** is always permanent (no Trash). The dialog says so: `Delete N file(s) from archive? This cannot be undone.`.

## Review Focus

1. **Interruption in the middle of writing into an archive** (Cancel, full disk, source read error): the original stays byte-identical and no `.fxfm-*.tmp` is left in the folder. Tests in Task 3: `rewrite_cancel_leaves_original_identical` and `rewrite_source_error_leaves_original_identical`.
2. **Malicious names in an archive** (`../../x`, `/etc/x`, `a/../../b`, `C:\x`, duplicates): extraction never writes outside the destination. Such an entry is reported through `obs.error` as `unsafe path in archive: <name>`, and the listing does not show it. Tests: Task 2 `extract_refuses_path_traversal` and Task 1 `index_skips_unsafe_names`.
3. **Large tgz archive** (100,000 entries): the index is read once and cached by (path, size, mtime). Entering subfolders does not extract again. Tests in Task 1: `index_cache_reused_until_file_changes` and `list_inner_100k_entries_is_fast` (< 1 s in release mode, `#[ignore]` in debug as in plan 1).
4. **F4 changes must not be lost** (spec anti-goal):
   - the editor saves twice within 200 ms → asks once;
   - the user says No → asks again on the next save;
   - exit with unuploaded changes → the copy stays on disk, and the next launch reports where it is.

   Tests in Task 8: `debounce_coalesces_saves`, `unchanged_content_is_not_reported`, `pending_edits_survive_cleanup`.
5. **Archives without dir entries and with odd names**:
   - zip without `dir/` entries → folders are synthesized;
   - CP437 names from a Windows zip are decoded;
   - NFD names from a Mac are normalized to NFC for comparison;
   - `__MACOSX/` and `._*` are visible, but grayed out like hidden files.

   Tests in Task 1: `implicit_dirs_synthesized`, `zip_cp437_names_decode` and `appledouble_entries_are_hidden`.

---

## File Structure

```
crates/core/Cargo.toml                 + zip, tar, flate2, sevenz-rust2
crates/core/src/lib.rs                 + pub mod archive; pub mod vfs;
crates/core/src/archive/mod.rs         ArchiveKind, ArchiveLoc, split_location, ArchiveError, IndexEntry, ArchiveIndex, IndexCache, list_inner
crates/core/src/archive/read.rs        read_index per format, extract
crates/core/src/archive/write.rs       Edit, rewrite (zip/tar/tgz/gz/7z)
crates/core/src/archive/testutil.rs    #[cfg(test)] builders: make_zip/make_tar/make_tgz/make_gz/make_7z
crates/core/src/vfs.rs                 Loc, parse, list/nearest/dir_size/mkdir/rename/delete/copy/move over local or archive; Temp
crates/core/src/config.rs              restore_path: location in an archive → folder containing the archive
src-tauri/Cargo.toml                   + notify 8
src-tauri/src/state.rs                 + vfs: Arc<vfs::Ctx>, edits: EditWatch
src-tauri/src/commands.rs              commands via vfs; set_archive_password; materialize; upload_back; pending edits in Bootstrap
src-tauri/src/jobs.rs                  JobRequest → vfs; + JobRequest::Upload
src-tauri/src/editwatch.rs             F4 from an archive: watching, debounce, edit://changed, pending manifest
ui/lib.mjs                             archiveKindOf, isArchiveLocation, splitArchive (pure, by name)
ui/panel.js                            Enter opens the archive
ui/app.js                              password, edit://changed prompt, pending edits message, Shift+F4/cmdline in an archive
ui/dialogs.js                          delete-from-archive text, passwordRequired errors
README.md                              "Archives" section
```

## Shared types (new)

```rust
// archive/mod.rs
pub enum ArchiveKind { Zip, Tar, TarGz, Gz, SevenZ }          // Serialize camelCase
pub struct ArchiveLoc { pub archive: PathBuf, pub kind: ArchiveKind, pub inner: String }
pub struct IndexEntry { pub path: String /*NFC, no leading/trailing '/'*/, pub is_dir: bool,
    pub is_symlink: bool, pub size: u64, pub mtime: i64, pub mode: u32, pub encrypted: bool }
pub struct ArchiveIndex { pub entries: Vec<IndexEntry> /*sorted by path, includes synthesized folders*/ }
#[derive(Error)] pub enum ArchiveError {
    #[error("password required")] PasswordRequired,
    #[error("wrong password")] WrongPassword,
    #[error("{0}")] Unsupported(String),          // gz limits, encrypted, 7z AES
    #[error("not found in archive: {0}")] NotFound(String),
    #[error("already exists in archive: {0}")] Exists(String),
    #[error("corrupt archive: {0}")] Corrupt(String),
    #[error("{0}")] Io(String),
    #[error("operation cancelled")] Cancelled,
}
```

`ArchiveError` is serialized as `{kind, message}` (kinds: `passwordRequired`, `wrongPassword`, `unsupported`, `notFound`, `exists`, `corrupt`, `io`, `cancelled`), modeled on `OpError`. `vfs` functions that return `OpError` map `ArchiveError` as follows:
- `PasswordRequired` → `OpError::Io("password required: <archive>")`. The UI recognizes the prefix `password required: ` and asks for the password.
- `WrongPassword` → `OpError::Io("wrong password: <archive>")`.
- Anything else → `OpError::Io(e.to_string())`, and `Cancelled` → `OpError::Cancelled`.

---

### Task 1: Location detection, index and archive listing

**Files:** Create `crates/core/src/archive/{mod.rs,read.rs,testutil.rs}`; Modify `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/config.rs`

**Interfaces:**
- Consumes: `listing::{Entry, ListError, sort_entries}`.
- Produces:
  - `ArchiveKind::from_name(name: &str) -> Option<ArchiveKind>`;
  - `pub fn split_location(loc: &str) -> Option<ArchiveLoc>`:
    - walks the components from the root;
    - the first component that exists as a **regular file** (`symlink_metadata`, following a link only if the target is a file) and has an `ArchiveKind` is the archive;
    - the rest is `inner`;
    - a local folder named `x.zip` is not an archive.
  - `read::read_index(path: &Path, kind: ArchiveKind) -> Result<ArchiveIndex, ArchiveError>`;
  - `pub struct IndexCache` (`Mutex<HashMap<PathBuf, (u64 size, i64 mtime_ns, Arc<ArchiveIndex>)>>`, at most 8 archives, LRU eviction) with methods `get(&self, path, kind) -> Result<Arc<ArchiveIndex>, ArchiveError>` and `invalidate(&self, path)`;
  - `pub fn list_inner(index: &ArchiveIndex, inner: &str) -> Result<Vec<Entry>, ListError>`:
    - direct children under `inner`, sorted with `sort_entries`;
    - `hidden` is true for `.*`, `__MACOSX` and `._*`;
    - a nonexistent `inner` yields `NotFound(<loc>)`, and a file yields `NotADirectory`.
  - `config::restore_path`: when an ancestor is an existing regular file with an `ArchiveKind`, returns its parent folder (spec §4: "the panel opens on the local folder it was navigated from"). Other rules stay.

Index rules:
- Names are normalized to NFC.
- `\` is not treated as a separator.
- Entries with a `..` component, an absolute path or an `X:` prefix are skipped with `log::warn!`.
- Folders that appear only as a prefix are synthesized with the `mtime` of the newest child and `mode` `0o40755`.
- Duplicates: the last one wins (tar semantics).
- For zip, `encrypted` is read from the entry flags (the name is readable without a password).
- For `Gz`, the size is read from the ISIZE trailer (mod 2³²; informational for > 4 GiB).

- [ ] **Step 1: tests** (`archive/mod.rs` and `read.rs`, with builders from `testutil.rs`; the builders use the same crates for writing):
  - `kind_from_name`:
    - `a.ZIP` → `Zip`;
    - `x.tar.gz` and `x.tgz` → `TarGz`;
    - `x.gz` → `Gz`;
    - `x.7z` → `SevenZ`;
    - `x.rar` and `x.bz2` → `None`.
  - `split_location`:
    - for `<t>/a.zip/src/lib` gives `inner == "src/lib"`, and for `<t>/a.zip` gives `inner == ""`;
    - a folder `<t>/d.zip/` gives `None`;
    - `<t>/nema.zip/x` (the file does not exist) gives `None`.
  - `implicit_dirs_synthesized`: a zip with the sole entry `a/b/c.txt` gives a root listing `[a]`, and a listing of `a` gives `[b]`.
  - `index_skips_unsafe_names`: a tar with `../evil`, `/abs` and `ok.txt` gives a root listing that contains only `ok.txt`.
  - `zip_cp437_names_decode`: a zip without the UTF-8 flag containing byte `0x87` gives a name that contains `ç`.
  - `appledouble_entries_are_hidden`: `__MACOSX/` and `._a` have `hidden == true`.
  - `gz_single_entry`: a gzip with the header name `notes.txt` gives exactly one entry `notes.txt` with the correct size; without a header name, `report.csv.gz` gives `report.csv`.
  - `zip_encrypted_flag`: an AES zip has `encrypted == true`, and listing works without a password.
  - `seven_z_lists`: a 7z with `d/x.txt` gives a root listing `[d]`.
  - `index_cache_reused_until_file_changes`:
    - two `get` calls return the same `Arc` (`Arc::ptr_eq`);
    - after the file is rewritten (different size), a new `Arc` is returned.
  - `list_inner_100k_entries_is_fast` (`#[ignore]`): a tar with 100,000 files in 100 folders; `get` + `list_inner` for one folder takes < 1 s in release mode.
  - `restore_path_archive_goes_to_containing_folder`: `<t>/a.zip/src` gives `<t>`.
  - A broken archive (the first 100 bytes of a zip) gives `ArchiveError::Corrupt`.
- [ ] **Step 2:** `cargo add -p fxfm-core zip@9 --no-default-features -F deflate,deflate64,aes-crypto,time && cargo add -p fxfm-core tar@0.4 flate2@1 && cargo add -p fxfm-core sevenz-rust2@0.25 -F compress` (without `aes256`). If a feature does not exist under that name, pick the closest one from `cargo info` and record the decision in the report.
- [ ] **Step 3:** Run `cargo test -p fxfm-core archive`. Expected: FAIL.
- [ ] **Step 4:** Implement.
- [ ] **Step 5:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 6:** Commit `feat(core): archive locations, index and listing`.

### Task 2: Extraction

**Files:** Modify `crates/core/src/archive/read.rs`, `archive/mod.rs`

**Interfaces:**
- Consumes: `ArchiveIndex` and `IndexCache` (Task 1), `ops::{OpObserver, Progress, ErrorAnswer}`.
- Produces: `pub fn extract(path: &Path, kind: ArchiveKind, items: &[String], dest: &Path, password: Option<&str>, obs: &dyn OpObserver) -> Result<u64 /*file count*/, ArchiveError>`.
  - `items` are inner paths; a folder pulls in its whole subtree.
  - The structure is kept **relative to the item's parent**: `items = ["src/lib"]` writes `dest/lib/...`.
  - Sets `mtime` and permissions (`mode & 0o7777`; no setuid/setgid).
  - Symlink entries are written as symlinks, but only if the target, once resolved, stays inside `dest`. Otherwise `obs.error("unsafe link in archive: <name>")` is called.
  - Progress is in bytes of uncompressed content; `obs.is_cancelled()` is checked every 1 MiB.
  - `dest` must exist and be empty (staging), so there are no conflicts.
  - Password (zip):
    - an encrypted entry without a password gives `PasswordRequired`;
    - a wrong password gives `WrongPassword`.

    The check happens on the first encrypted entry, before anything is written.
  - `TarGz`, `Tar` and `Gz` read the archive in a single pass and skip entries outside `items`.
  - `SevenZ` uses the `sevenz-rust2` decoder with a name filter.

- [ ] **Step 1: tests**:
  - `extract_subtree_relative_to_parent`: `src/lib/a.rs` and `src/lib/m/b.rs` with `items=["src/lib"]` give `dest/lib/a.rs` and `dest/lib/m/b.rs`, with the content and `mtime` from the archive.
  - `extract_refuses_path_traversal`: a hand-built tar with the entry `../evil` and a symlink `l -> ../../etc` with `items=[""]` does not create `dest/../evil` and does not create the link `l`. `obs.error` is called at least once, and the other files exist.
  - `extract_all_formats`: for Zip, Tar, TarGz, Gz and SevenZ the same small content (`a.txt`, `d/b.txt`; for Gz just one file) is extracted byte-identically.
  - `extract_password`:
    - an AES zip without a password gives `PasswordRequired`, with a wrong one `WrongPassword`, and with the correct one the content comes out;
    - on error `dest` is empty.
  - `extract_cancel_stops`: an observer that returns `is_cancelled` after the first progress yields `Err(Cancelled)` for an archive with a 5 MiB file.
- [ ] **Step 2:** Run `cargo test -p fxfm-core archive::read`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): archive extraction with path-safety and passwords`.

### Task 3: Archive rewrite (add, delete, rename, new folder)

**Files:** Create `crates/core/src/archive/write.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces:
  - `pub enum Edit { Add { src: PathBuf, inner: String }, Delete { inner: String }, Rename { from: String, to: String }, Mkdir { inner: String } }`;
  - `pub fn rewrite(path: &Path, kind: ArchiveKind, edits: &[Edit], obs: &dyn OpObserver) -> Result<(), ArchiveError>`.

Rules for `rewrite`:
- `Add` with a local folder adds the whole subtree under `inner`. An existing entry with the same name (or subtree) is replaced; conflicts are asked by `vfs` before the call, not by `rewrite`.
- `Delete` and `Rename` on a folder act on the whole subtree.
- `Rename` to an existing name gives `Exists`, and a nonexistent `from` gives `NotFound`.
- `Mkdir` on an existing name gives `Exists`.
- Edits are applied in order to a virtual listing, and then the archive is written once.
- An encrypted entry anywhere in the archive gives `Unsupported("Writing into encrypted archives is not supported")` before writing.
- `Gz` allows only:
  - a single `Add` whose `inner` equals the existing name (content replacement);
  - a single `Rename` (changes the name in the gzip header).

  Anything else gives `Unsupported("Not possible in a .gz file (it holds exactly one file)")`.
- **Zip**: unchanged entries are copied raw, without recompression (`ZipWriter::raw_copy_file`, with renaming via `raw_copy_file_rename`). New entries are Deflate. mtime and Unix permissions are kept.
- **Tar and TarGz**: a new archive is written, unchanged entries are streamed from the old one, and TarGz uses the gzip default level.
- **SevenZ**: extraction into staging under the temp root, then `sevenz-rust2` compresses the whole tree (7z cannot be raw-copied). Record this in a comment and in the README because it is slower.
- Atomicity is from Global Constraints:
  - temp in the same folder;
  - `sync_all`;
  - `set_permissions` with the original's permissions;
  - `rename`;
  - `fsync` of the parent folder;
  - finally `IndexCache::invalidate`.

  So `rewrite` also takes `cache: &IndexCache` as the last argument. Final signature: `rewrite(path, kind, edits, cache, obs)`.
- Progress is in bytes of written content. On Cancel the temp file is deleted and `Err(Cancelled)` is returned.

- [ ] **Step 1: tests** (all formats where it makes sense; after each `rewrite` the archive is re-read and verified with `read_index` and `extract`):
  - `add_file_and_dir`: add `x.txt` to `""` and a local folder `m/` (with `m/y.txt`) to `src`. The index then contains `x.txt`, `src/m/y.txt` and all old entries byte-identically.
  - `add_replaces_existing`: `Add` on `a.txt` changes the content, and the entry count stays the same.
  - `delete_dir_subtree`, `rename_dir_subtree` (`d` → `e` moves `d/b.txt` to `e/b.txt`), `rename_to_existing_is_exists`, `mkdir_creates_empty_dir`, `mkdir_existing_is_exists`.
  - `gz_rules`:
    - content replacement works;
    - `Rename` changes the name `read_index` returns;
    - `Mkdir` and `Delete` give `Unsupported`.
  - `encrypted_zip_write_refused`: `Unsupported`, and the file is byte-identical.
  - `rewrite_cancel_leaves_original_identical`: cancel after the first progress gives `Err(Cancelled)`. The original is byte-identical (compare `fs::read`), and the folder has no file that starts with `.` and ends with `.tmp`.
  - `rewrite_source_error_leaves_original_identical`: `Add` with a nonexistent `src` gives `Err(Io)`, and the original is identical with no temp file.
  - `rewrite_keeps_permissions`: an original with `0o640` is still `0o640` after writing.
  - `zip_raw_copy_keeps_compressed_bytes`: an unchanged entry has the same `compressed_size` and `crc32` before and after.
- [ ] **Step 2:** Run `cargo test -p fxfm-core archive::write`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): atomic archive rewrite (add, delete, rename, mkdir)`.

### Task 4: VFS routing — listing, navigation and operations without transfer

**Files:** Create `crates/core/src/vfs.rs`; Modify `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1–3, `listing::list_dir`, `ops::{mkdir, rename, delete, dir_size}`, `config::restore_path`.
- Produces:
  - `pub enum Loc { Local(PathBuf), Archive(ArchiveLoc) }` and `pub fn parse(loc: &str) -> Loc`. `split_location` gives `Archive`, otherwise `Local`.
  - `pub struct Ctx` contains:
    - `pub cache: IndexCache`;
    - `pub temp_root: PathBuf`;
    - `passwords: Mutex<HashMap<PathBuf, String>>`;
    - counter `n: AtomicU64`.

    Methods:
    - `Ctx::new(temp_root: PathBuf)`;
    - `set_password(&self, archive: &Path, pw: String)`;
    - `password(&self, archive: &Path) -> Option<String>`;
    - `new_temp_dir(&self, prefix: &str) -> io::Result<PathBuf>` creates `temp_root/<prefix>-<n>`;
    - `cleanup_dir(&self, dir: &Path)`.
  - Functions:
    - `pub fn list(ctx: &Ctx, loc: &str) -> Result<Vec<Entry>, ListError>`. For an archive this is `cache.get` + `list_inner`. `ArchiveError::Corrupt` and the others give `ListError::Io(<loc>)` with a `log::warn!` carrying the message.
    - `pub fn nearest(ctx: &Ctx, loc: &str, home: &Path) -> String`. For an archive: the nearest existing inner folder, then the archive root. If the archive no longer exists, returns `restore_path`.
    - `pub fn dir_size(ctx: &Ctx, loc: &str, cancel: &AtomicBool) -> io::Result<(u64, u64)>`. For an archive it is summed from the index.
    - `pub fn mkdir(ctx: &Ctx, parent: &str, name: &str) -> Result<String, String>`. Archive: `rewrite([Mkdir])` with `NullObserver`.
    - `pub fn rename(ctx: &Ctx, loc: &str, new_name: &str) -> Result<String, String>`. Archive: `rewrite([Rename])`. Both use the name rule from the private `ops::valid_name`; make it `pub` (no behavior change). The new location is returned as a `String`.
    - `pub fn delete(ctx: &Ctx, locs: &[String], to_trash: bool, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`. All locations are from the same folder (the UI guarantees this). Archive: `to_trash` is ignored and a single `rewrite` is done with all `Delete`s. `OpSummary.done` is the number of deleted non-folder entries.
  - `pub(crate) struct NullObserver` (overwrite, skip on error, never cancel) for short operations.
  - `pub fn local_dir_for(loc: &str) -> PathBuf`: for an archive it is the folder containing the archive, for a local location the location itself. Used by Terminal and the command line.

- [ ] **Step 1: tests**:
  - `list_routes_local_and_archive`: a local folder with `a.zip` gives `list("<t>")`, which contains `a.zip`, and `list("<t>/a.zip/d")` with `b.txt`.
  - `nearest_inside_archive`:
    - `<t>/a.zip/d/nema` gives `<t>/a.zip/d`;
    - `<t>/a.zip/nema/x` gives `<t>/a.zip`;
    - `<t>/obrisano.zip/x` gives `<t>`.
  - `mkdir_rename_delete_in_archive`: `mkdir("<t>/a.zip", "n")` returns `<t>/a.zip/n`; then `rename("<t>/a.zip/n", "m")`, then `delete(["<t>/a.zip/m", "<t>/a.zip/a.txt"])`. The listing afterwards matches expectations, and `done == 1`.
  - `gz_mkdir_is_refused_with_message`: the error contains `Not possible in a .gz file`.
  - `dir_size_in_archive`: sum of sizes and file count of the subtree.
  - `local_paths_unchanged`: `mkdir`, `rename` and `delete` on local locations give the same result as direct `ops::*`.
  - `local_dir_for_archive`: `<t>/a.zip/d` gives `<t>`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): vfs routing for listing, mkdir, rename, delete`.

### Task 5: VFS copy and move via staging

**Files:** Modify `crates/core/src/vfs.rs`

**Interfaces:**
- Consumes: Task 4, `ops::{copy, move_, resolve_dest, Dest, ConflictInfo, ConflictAnswer}`.
- Produces:
  - `pub fn copy(ctx: &Ctx, sources: &[String], dest: &str, cwd: &str, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`;
  - `pub fn move_(ctx: &Ctx, sources: &[String], dest: &str, cwd: &str, obs: &dyn OpObserver) -> Result<OpSummary, OpError>`.

Algorithm:
- All sources are from the same location. The destination is `dest`, relative to `cwd` if not absolute, and is parsed with `parse`. `IntoDir` and `AsFile` are determined as in `ops::resolve_dest`, but "is it a folder" for an archive is answered by the index.
1. **Local → local**: calls `ops::copy` or `ops::move_` directly, with no behavior change.
2. **Archive → local**:
   - `extract` the sources into `stage-<n>`;
   - then `ops::copy` from staging to the local destination with the real `obs`, so conflicts, Rename and Overwrite older work as in the local case;
   - for `AsFile` the staged file is copied under the new name;
   - afterwards `move_` does a single `rewrite([Delete…])` only for sources that were actually copied (not skipped). This is tracked through an observer that counts top-level skips by name, as a wrapper around `obs`.
3. **Local → archive**:
   - for each top-level source check whether `dest_inner/<name>` exists in the index;
   - if it does, call `obs.conflict` with `ConflictInfo` (`target_size` and `target_mtime` from the index). Skip leaves it out, Rename changes the name, Overwrite older compares mtime, and `*All` answers are remembered;
   - then a single `rewrite([Add…])`;
   - `move_` deletes the local sources (`ops::delete` permanently, `NullObserver`) **only after** a successful `rewrite`.

   A conflict is at the level of the top-level item (an existing folder = one question), which is a simplification compared to the local per-file behavior. Record this in a comment.
4. **Archive → archive** (same or different): `extract` into staging, then step 3 with staging paths as sources. For `move_` within the **same** archive, `rewrite([Rename…])` is used instead of extract+add.
5. **Into itself**:
   - a destination location equal to the source or below it gives `OpError::IntoItself`;
   - a destination equal to the source's parent for `move_` gives `SameLocation`;
   - the same applies to inner paths.
6. Staging is always deleted at the end (also on error).
7. Progress:
   - the extraction phase reports `current` as `<name> (extracting)`;
   - the write phase as `<name> (packing)`;
   - `total_bytes` is the sum from the index, or from local sizes.

- [ ] **Step 1: tests** (with a new test observer `RecordingObserver` in the `vfs.rs` tests, modeled on `TestObserver` in `ops.rs`, which returns preset answers and records calls):
  - `copy_archive_to_local`: `<t>/a.zip/d` into `<t>/out` gives `<t>/out/d/b.txt`, and the staging folder no longer exists.
  - `copy_archive_to_local_conflict_uses_ops_rules`: `<t>/out/a.txt` already exists, and the answer is Skip. The content stays unchanged and `skipped == 1`.
  - `copy_local_into_archive_with_conflict`:
    - `x.txt` is already in the archive and the answer is `Rename{newName:"x (2).txt"}`;
    - the index then has both, and the old one is byte-identical;
    - `ConflictInfo.target_size` matches the index.
  - `move_local_into_archive_deletes_source_after_success`: the local source is deleted, and it is in the archive.
  - `move_local_into_archive_keeps_source_on_failure`: the archive is locked for writing (`chmod 0o500` on the folder), so the result is `Err`. The local source exists, and the original archive is identical.
  - `move_archive_to_local_skipped_not_deleted`: Skip on one of two sources leaves that one in the archive, and the other is deleted from the archive.
  - `move_within_same_archive_is_rename`: `d/b.txt` into `e/` gives `e/b.txt`, and `d/b.txt` no longer exists.
  - `copy_between_archives`: zip → tgz.
  - `copy_into_itself_in_archive_refused`: `<t>/a.zip/d` into `<t>/a.zip/d/x` gives `IntoItself`.
  - `password_required_maps_to_io_prefix`: an encrypted source without a password gives `OpError::Io`, which starts with `password required: `. After `ctx.set_password` the copy succeeds.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy and move between local folders and archives`.

### Task 6: Tauri backend via vfs

**Files:** Modify `src-tauri/src/{state.rs,commands.rs,jobs.rs,main.rs}`

**Interfaces:**
- Consumes: `vfs::*` (Tasks 4 and 5).
- Produces:
  - `AppState.vfs: Arc<vfs::Ctx>`, where `temp_root = dirs::cache_dir()/fxfm/tmp/<pid>`. At startup, `tmp/<pid>` folders whose process no longer exists (`libc::kill(pid, 0)` gives `ESRCH`) are deleted, except those that have `pending-edits.json` (Task 8). On `RunEvent::Exit` the process's own `temp_root` is deleted (Task 8 narrows this).
  - Commands `list_dir`, `nearest_existing`, `dir_size`, `mkdir`, `rename` call `vfs::*`. Names and arguments stay the same, so the UI does not change.
  - `run_job` calls `vfs::copy`, `vfs::move_` and `vfs::delete`. The new `JobRequest::Upload { local: String, target: String }` does `vfs::copy([local], target)` with an observer that always answers `Overwrite` on conflict (used by Task 8). The serde shape is `{"kind":"upload","local":…,"target":…}`, like the existing variants.
  - New command `set_archive_password(archive: String, password: String)`. `archive` is the path to the archive file, which the UI gets from the error message.
  - New command `materialize(location: String) -> Result<String, String>`:
    - for a local location returns it unchanged;
    - for a file in an archive does `extract` into `view-<n>/` and returns the local path;
    - `PasswordRequired` gives `Err("password required: <archive>")`.
  - `open_viewer(path)` and `open_default(path)` call `materialize` first. The viewer title is the full location in the archive.
  - `run_in_terminal(dir, …)` uses `vfs::local_dir_for(dir)`.
  - `edit_new` inside an archive gives `Err("Shift+F4 is not available inside archives")`.
  - The existing Rust `nearest_existing` logic for `://` stays.

- [ ] **Step 1: tests** (`jobs.rs` and `commands.rs` unit tests, without Tauri where possible):
  - `job_request_upload_serde`: `{"kind":"upload","local":"/t/x","target":"/t/a.zip/x"}` is parsed.
  - `run_job_routes_archive_copy`: `run_job` with Copy from an archive into a temp folder copies the file. Uses a small test observer in the `jobs.rs` tests (`TauriObserver` needs an AppHandle).
  - `upload_overwrites_without_asking`: an observer that panics on `conflict` is not called.
  - `stale_temp_dirs_removed_except_pending`: `tmp/999999/` is deleted, and `tmp/999998/` with `pending-edits.json` stays.
- [ ] **Step 2:** Run `cargo test --workspace`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(app): commands and jobs go through vfs (archives)`.

### Task 7: UI — entering an archive, password, delete, messages

**Files:** Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/panel.js`, `ui/app.js`, `ui/dialogs.js`

**Interfaces:**
- Consumes: commands from Task 6.
- Produces:
  - `lib.mjs`:
    - `archiveKindOf(name) -> "zip"|"tar"|"tgz"|"gz"|"7z"|null`, with the same rules as Rust `from_name`;
    - `archiveOf(location) -> string|null`, the location prefix up to the first component with `archiveKindOf`. Pure and by name only, used just for UI decisions; the backend is the authority.
    - `passwordArchive(message) -> string|null`, which returns the path from `"password required: /x/a.zip"` or `"wrong password: …"`.
  - `panel.enter()`:
    - a file with `archiveKindOf` when the panel is **not** already in an archive does `go(joinLocation(loc, name))`;
    - inside an archive, a nested archive goes to `openDefault`, and other files go to `openDefault` (the backend materializes).
  - If `go` fails with an error containing `password required` (listing does not need a password, but F3, F5 and Enter on a file do):
    - `app.js` has a helper `withPassword(fn)`: on such an error it opens the prompt `Password for <archive name>:` (input type password, OK and Cancel), calls `set_archive_password` and retries `fn` once;
    - on `wrong password` the prompt repeats with the text `Wrong password for <name>. Try again:`;
    - it is used by F3, Enter on a file and job errors (`dialogs.js`: when a job ends with a password-type error, offer the prompt and rerun the same `req`).
  - Delete: if `archiveOf(panel.location)`, the dialog is `Delete N file(s) from archive? This cannot be undone.`, and `toTrash: false` is sent. Shift+F8 is the same.
  - Shift+F4 in an archive shows a status message from the backend (no dialog).
  - The command-line prompt stays `<location>>`, and the backend chooses the folder.

- [ ] **Step 1: tests** (`lib.test.mjs`):
  - `archiveKindOf`: `A.ZIP` → `zip`; `x.tar.gz` and `x.tgz` → `tgz`; `x.gz` → `gz`; `x.7z` → `7z`; `x.rar` and `x.bz2` → `null`.
  - `archiveOf`: `/u/a.zip/src` → `/u/a.zip`; `/u/a.zip` → `/u/a.zip`; `/u/src` → `null`.
  - `passwordArchive`: the two message forms give the path, and other messages give `null`.
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `node --test ui/`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): browse archives, password prompt, delete from archive`.

### Task 8: F4 from an archive with upload back

**Files:** Create `src-tauri/src/editwatch.rs`; Modify `src-tauri/Cargo.toml`, `state.rs`, `commands.rs`, `main.rs`, `ui/app.js`

**Interfaces:**
- Consumes: `materialize` (with `edit-<n>/` instead of `view-<n>/`), `JobRequest::Upload`.
- Produces:
  - `pub struct EditWatch` in `AppState.edits`:
    - `register(&self, location: String, local: PathBuf) -> u64`, which remembers the SHA-256 of the content (crate `sha2` in `src-tauri`) and starts watching the parent folder via `notify` 8 `RecommendedWatcher`;
    - on every event for that file it starts a **500 ms** debounce;
    - afterwards it recomputes the hash, and if it differs from the last **confirmed** one, emits `edit://changed { id, location, name }` to `main`. Until the UI responds, there is no new event for the same `id`;
    - `confirm(&self, id, uploaded: bool)`: `uploaded=true` sets the last confirmed hash to the current one, and `false` leaves the old one, so the next save asks again;
    - `pending(&self) -> Vec<(String location, PathBuf local)>` returns those whose current hash differs from the confirmed one.
  - `edit_file(path)` for a location in an archive does `materialize` into `edit-<n>/`, `register`, then opens the editor. A local file goes as before.
  - Command `upload_back(id) -> u64`: starts `JobRequest::Upload { local, target: location }` and returns the job id (the UI follows it with the existing progress dialog). On success the UI calls `edit_confirm(id, true)`.
  - Command `edit_confirm(id: u64, uploaded: bool)`.
  - **Exit:** on `RunEvent::Exit`, if `pending()` is not empty:
    - `pending-edits.json` (`[{location, local}]`) is written into `temp_root`;
    - `edit-*` folders with pending files are **not** deleted, and everything else is deleted.
  - **Start:**
    - `Bootstrap.pendingEdits: Vec<{location, local}>` collects `pending-edits.json` from the folders of dead processes;
    - the UI shows `Edited copies from a previous session were not uploaded:` with a list, with buttons `Show in Finder` (`open -R <local>`) and OK;
    - after OK it calls `dismiss_pending_edits()`, which deletes those folders.
  - **UI** on `edit://changed` shows the dialog `"<name>" was changed. Upload it back into the archive?` with buttons Yes and No:
    - Yes does `upload_back` (and `edit_confirm(id,true)` after the job succeeds);
    - No does `edit_confirm(id,false)`.

- [ ] **Step 1: tests** (`editwatch.rs`, logic separated from Tauri via a closure `emit: Fn(u64, &str)`):
  - `debounce_coalesces_saves`: two writes 100 ms apart give exactly one emit within 1.5 s.
  - `unchanged_content_is_not_reported`: a write of the same content (touch) gives no emit.
  - `declined_change_asks_again`: after `confirm(id,false)` the next change again gives an emit.
  - `pending_edits_survive_cleanup`: an unuploaded change after `exit_cleanup(temp_root)` leaves the file and `pending-edits.json`. An uploaded change means the folder is deleted.
- [ ] **Step 2:** `cargo add -p fxfm notify@8 sha2@0.10`. Run `cargo test --workspace`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: edit files inside archives with upload back`.

### Task 9: README, release build and manual verification

**Files:** Modify `README.md`

- [ ] **Step 1:** README section **Archives** (in Croatian, like the rest of the README):
  - formats;
  - what works and what does not (gz rules, encrypted, nested, 7z is fully repacked on write);
  - where the temporary files are;
  - what the message about unuploaded changes means.
- [ ] **Step 2:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 3:** Run `cargo tauri build --target universal-apple-darwin && lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm`. Expected: `x86_64 arm64`.
- [ ] **Step 4: manual verification** (a checklist for the user, if no GUI is available):
  1. Enter on zip, tgz, 7z and gz; `[..]` returns to the archive with the cursor on it.
  2. F5 from an archive to a local folder and vice versa, with a conflict (Rename).
  3. F6 within the same archive, F7, F2 and F8 (text "from archive").
  4. F3 on a `.txt` and on an image in an archive.
  5. F4 on a `.php` in a zip: change, save, Yes; close, reopen the zip and verify the change. Then change, No, and change again (asks again).
  6. AES zip: F3 asks for the password; a wrong password asks again.
  7. Cmd+Q with an unuploaded F4 change: the next launch shows a message with the path.
- [ ] **Step 5:** Commit `docs: archives in README`.

---

## After plan 2

- **Plan 3 (servers)**: `SftpFs` and `FtpFs`, as a third branch in `vfs::Loc`. The staging model from Task 5 applies to a server too: download into staging, then upload. At that point we evaluate whether to introduce a trait.
- **Candidates for later:**
  - packing new archives (Alt+F5), since `rewrite` on an empty archive almost does it already;
  - nested archives;
  - bz2 and xz tar;
  - per-file conflicts when copying into an archive.
