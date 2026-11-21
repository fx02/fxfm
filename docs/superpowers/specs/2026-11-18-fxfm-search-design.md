# fxfm search (Alt+F7): design

Status: approved by the owner, 2026-11-18.

## Intent

The owner regularly needs to find a file **by name** somewhere on the disk without remembering where it is.
Typical cases:
- "a file *scope of work* somewhere on the disk";
- "a txt I saved somewhere that had *fxfm* in its name".

The result should come as fast as typing, like *Everything* on Windows. Searching inside archives is wanted as
an **option**. Searching the text inside files is **not** wanted. Results must be checkable the way Total
Commander does it: Enter goes to the file's folder, and all results can be sent to a panel as a list (TC "Feed
to listbox").

## Goals
- Alt+F7 opens a Find files window. Results appear while typing, over everything indexed, in well under a
  second for a typical query.
- fxfm builds its **own name index** of the Mac disk, including hidden files and `~/Library`. FSEvents keeps it
  current while fxfm runs and catches up after a restart.
- Optional: entries inside archives, searchable and openable. Formats: those fxfm already reads (zip, 7z,
  tar, tar.gz/tgz, gz). bz2 and xz are not read today and stay out of scope.
- Optional, per volume: external and network volumes (SMB shares included), refreshed on a schedule or by hand.
- Enter on a result → the active panel opens the result's folder with the cursor on it (for an archive entry:
  inside the archive).
- "Feed to listbox": all results become a list in the active panel. F3, F4, F5/F6 to the other panel, F8 and
  drag & drop work on the marked items there.

## Non-goals
- Searching file contents (text inside files).
- Search on SFTP/SCP/FTP servers.
- Duplicate finder, search by size/date/attributes (possible later as filters; not now).
- Sharing the index between machines.

## Anti-goals (a failure even if it works)
- fxfm becomes slow or sluggish while indexing: indexing runs in the background, at low priority, and never
  blocks the UI or file operations.
- The index silently goes stale. The window always shows the index state: building, up to date, or a volume
  last refreshed on <date>.
- Heavy disk or network load that the owner did not ask for. Network volumes and archives are opt-in, and big
  compressed tarballs have a size limit.
- Writing anything outside fxfm's data folder, or touching the owner's real data in tests.

## Approaches considered
- **A. Own index (chosen by the owner).** Full scan once, then FSEvents. Instant results; covers hidden files,
  folders excluded from Spotlight, and archive contents. Costs memory and disk for the index (≈50–150 MB for a
  large disk) and an initial scan of a few minutes.
- **B. Spotlight (`mdfind`).** No index to keep. Misses hidden files, Spotlight-excluded folders, network
  volumes and archive contents. Archives would need an own index anyway.
- **C. No index, walk on every search (classic TC).** Simple and always exact, but a whole-disk search takes
  from half a minute to minutes, which is not what the owner described. **Kept as the fallback**: while the
  index is still being built, "Current folder" searches walk the disk directly.

## The Find files window (Alt+F7)
```
┌ Find files ─────────────────────────────────────────────────┐
│ Name: [scope work_____________]   In: (•) Everywhere        │
│                                       ( ) Current folder    │
│ [x] Inside archives                                         │
├─────────────────────────────────────────────────────────────┤
│ Scope of Work v2.docx        ~/Documents/…/ponude    14.10. │
│ scope_of_work.pdf            ~/Downloads             02.09. │
│ SOW.zip ▸ scope-of-work.txt  ~/Desktop/stari         11.03. │
├─────────────────────────────────────────────────────────────┤
│ 3 found · index: 1,842,310 files, up to date                │
│ Enter: go to file  F3: view  Feed to listbox  Esc: close    │
└─────────────────────────────────────────────────────────────┘
```
- **Query:**
  - Words separated by spaces must all occur in the name, in any order. Matching ignores case and
    Unicode normalization (NFC/NFD: macOS file names are often NFD), so `scope work` finds `Scope of Work.docx`.
  - A word with `*` or `?` is a TC mask matched against the whole name (`matchMask` semantics). So `fxfm *.txt`
    finds every `.txt` with "fxfm" in its name.
  - Matching is on the **name only**, not the path.
- **In:**
  - *Everywhere*: all indexed volumes.
  - *Current folder*: the active panel's folder and below it (index prefix when indexed, otherwise a direct walk).
- **Inside archives** checkbox: only shown when archive indexing is enabled in Options. It includes archive
  entries in the results.
- **Results:**
  - Sorted with folders and files mixed, best matches first: whole-word and start-of-name matches before
    substring matches, then by most recent modification.
  - Columns: name (an archive entry shows `archive ▸ inner path`), folder (home shown as `~`), modified date.
  - The first 1,000 matches are shown, and the status line says when there are more.
  - Folders match too.
- **Keys:**
  - Enter = go to the file: the window closes, the active panel opens the folder, cursor on the file.
  - F3 = view without closing the window.
  - Ctrl+Enter or the button = **Feed to listbox**.
  - Esc = close.
  - The window remembers the last query and options for the session.
- **Stale entries:** a result whose file has since disappeared, for example on a network volume not yet
  refreshed, is shown greyed. Picking it says "no longer exists" and drops it from the index.

## Feed to listbox (search results in a panel)
- The active panel shows a virtual folder "Search results: <query>", with one row per result. The name column
  shows the name and the relative folder.
- On marked items or the cursor item:
  - F3 view and F4 edit.
  - F5/F6 copy or move to the other panel's folder: items from different folders all land in that one folder,
    and name clashes use the normal conflict dialog.
  - F8 delete.
  - Drag & drop out (local files).
  - Ctrl+PgUp, or Enter on a row, goes to the item's folder with the cursor on it.
- Archive entries in the list: F3 and F5 (extract to the other panel) only.
- Backspace or `..` leaves the list and returns to the folder the panel was in before.
- After F6/F8, the list drops the items that no longer exist.

## The index
- **Volumes:**
  - The startup disk is indexed by default.
  - Excluded by default, because they are system internals: `/System`, `/private/var`, `/private/tmp`, `/dev`,
    `/Volumes` (other volumes are separate), `/cores`, `.Spotlight-V100`, `.fseventsd`, `.Trashes`,
    `.DocumentRevisions-V100`, and Time Machine snapshots.
  - Hidden files and `~/Library` are included.
  - The user can add excluded folders.
- **Other local volumes** (USB, external disks) are each enabled in Options and kept current by FSEvents when
  their file system supports it.
- **Network volumes** (smbfs, afpfs, nfs, webdav) are each enabled in Options. No FSEvents: they are rescanned
  every N hours (default 24, or manual only) while mounted, plus a "Refresh now" button. An unmounted network
  volume keeps its last index, marked "last refreshed <date>". Its results show, but picking one asks to mount
  it: for a saved SMB share, through the existing SMB connect flow.
- **Archives** (opt-in, Options):
  - Every archive file in an indexed volume is listed with the existing archive readers, and its entries are
    stored.
  - An archive is re-read only when its size or mtime changes.
  - Formats that need full decompression to list (tar.gz/tgz, gz) are skipped above a size limit, default
    500 MB, configurable.
  - Archives on network volumes are not read unless a separate option is on (default off): this avoids
    pulling every zip across the network on each refresh. Their names are still indexed as files.
  - Cloud-only placeholders (iCloud, Dropbox, OneDrive under `~/Library/Mobile Documents`, `~/Library/CloudStorage`;
    files with the `SF_DATALESS` flag) are never opened, so indexing never triggers a download. Their names
    are indexed.
  - Encrypted archives with encrypted file names are skipped. Nested archives are not entered.
- **Permissions:** folders macOS refuses to read are skipped and counted. If the scan hit TCC-protected
  folders, Options and the search window say Full Disk Access is needed for a complete index (reuse the
  existing FDA hint).
- **Freshness:**
  - Live updates come from an FSEvents stream per indexed local volume, at file-level granularity.
  - The last event id is stored with the index, so a restart replays what changed while fxfm was closed.
  - If the stream reports a history gap or a root change (`MustScanSubDirs`, `EventIdsWrapped`, the volume
    UUID changed), the affected subtree or the volume is rescanned.
- **Storage:**
  - The index lives in fxfm's data folder (`index/`): in system mode `~/Library/Application Support/fxfm/index`,
    in portable mode `fxfm-data/index` (it can be large).
  - Format and in-memory layout are the builder's call, within a budget: ≤ 250 MB RAM and ≤ 300 MB on disk for
    3 million entries.
  - The disk format is versioned. An unreadable or old index is rebuilt, never fatal.
- **Work:**
  - Scanning and archive reading run on background threads at low priority. They never hold locks the UI waits
    on.
  - A scan can be cancelled and resumes on the next launch.
  - Searches run against an in-memory snapshot and are cancelled when the query changes.
- **First run:**
  - The index is not built silently at launch.
  - The first Alt+F7 (and Options → Search) explains that fxfm will index the disk in the background, that
    Full Disk Access gives a complete index without macOS asking folder by folder, and offers
    *Open Privacy settings*, *Build index* and *Not now*.
  - Once it is built, the index is kept current at every launch (FSEvents replay).
  - Without Full Disk Access, macOS may ask once per protected folder (Desktop, Documents, Downloads,
    removable and network volumes). Refused folders are skipped and listed in Options → Search, with a
    *Retry* after access is granted.
  - Until the index exists, the window searches *Current folder* by direct walk.
- **Options → Search (new section):**
  - the indexed volumes with on/off, state and entry count;
  - excluded folders (add or remove);
  - archive indexing on/off and the size limit;
  - the network refresh interval;
  - "Rebuild index" and "Refresh now";
  - "Index while fxfm is running" on/off (default on). When off, the index is not built or used, and the
    window does a direct walk of the chosen folder.

## Constraints
- macOS only for FSEvents and volume handling. The index and query code lives in `fxfm-core` and must build
  and test on Linux; FSEvents goes in a cfg-gated part.
- Existing conventions apply: the central command registry (`cm_search` already holds Alt+F7), the job and
  progress machinery, the dialog style, `matchMask`, the archive readers, and the TC terms in the keymap.
- Tests never scan the owner's real disk or data folder. They use temp trees and fake event streams.
- English everywhere in the repo.

## Decided with the owner
- Name search only, own index (approach A), results while typing, whole disk.
- Archive contents optional.
- Enter opens the file's folder. Several results can be sent to a panel as a list (Feed to listbox), where they
  can be checked and worked on.
- First run: no silent scan; the first Alt+F7 offers *Open Privacy settings*, *Build index*, *Not now*
  (approved 2026-11-18, together with the calls below).
- Network volumes: the owner did not object to the proposal that they are opt-in per volume with a refresh
  interval.

## Builder-check answers
- **First scan and Full Disk Access:** see "First run" below.
- **Scope of "My calls":** they are part of this design, approved with it. Ranking and query parsing still
  sit behind one interface, and limits are settings, so they stay cheap to change.

## My calls (the owner may correct them)
- Query syntax (AND words, masks with `*`/`?`), the 1,000-row display cap, ranking.
- The default exclusions, the 500 MB tarball limit, the 24 h network refresh.
- The F-keys available in the results list, and archive entries limited to F3/F5 there.
- *Current folder* scope and the direct-walk fallback.

## Left to the builder
- The index data structure, the disk format and the matching algorithm, within the RAM, disk and speed budget.
- How FSEvents is bound (FFI or crate).
- How ops on a multi-folder results list map onto the existing ops (for example grouped by parent folder).
- The exact Options layout.
