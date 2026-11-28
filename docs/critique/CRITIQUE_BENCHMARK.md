# fxfm UX critique: competitor benchmark

Date of research: 2026-11-21. Method: official pages fetched through a web-fetch tool that returns a model-made summary of each page (not raw HTML), plus a few web searches. Quotes are short phrases as returned by that tool. "not found in sources" means the pages I could read did not state it; it does NOT mean the product lacks it. Nothing was installed or tried.

## 1. Total Commander (Windows, v11.58.1, 2026-10-29)

URLs read:
- https://www.ghisler.com/ (overview, version/date)
- https://www.ghisler.com/featurel.htm (feature list)
- https://www.ghisler.com/whatsnew.htm (release dates only)
- https://www.ghisler.ch/wiki/index.php?title=Quick_search
- https://www.ghisler.ch/wiki/index.php?title=Search
- https://www.ghisler.ch/wiki/index.php?title=Multi-Rename_Tool
- https://www.ghisler.ch/wiki/index.php?title=Synchronize_directories
- https://www.ghisler.ch/wiki/index.php?title=Directory_hotlist
- https://www.ghisler.ch/wiki/index.php?title=Button_bar
- Search result only (forum, not read in full): https://ghisler.ch/board/viewtopic.php?p=47847 (locked tabs), https://www.ghisler.ch/board/viewtopic.php?p=49795 (Ctrl+B)
- 404 / not available: wiki pages Tabs, Find_Files, Branch_view, Keyboard, Category:Functions; ghisler.com/features.htm.

1. Tabs: overview says "Two file windows side by side, with tabs". Search summary of forum posts: a "Lock, but directory changes allowed" tab mode exists, with a "Folder Tabs" option about returning to the tab root. Button bar wiki lists `opentabs` / `appendtabs` commands, so tab sets can be saved/loaded. Per-panel switch keys: not found in sources.
2. Command palette: not found in sources (no palette). Button bar: configurable; "hold SHIFT and drag a file onto the bar", multiple bars via "Add subbar", commands include program, internal command, `cd` path, `ftpopen`; parameters like %P, %N, %S, "?" prompt. Directory hotlist: menu of favorites for "local folders, network folders, FTP folders, file system plugin folders, websites, internal functions, programs, and documents"; opened via Ctrl+D, star button, double-click panel title bar, or `cm_directoryhotlist` on the command line. Command line accepts `cm_*` commands.
3. Viewer/preview: "Quick View panel with image and video display, and thumbnail view"; built-in text editor. Lister keys: not found in sources.
4. Find files: content search ("Find text" with whole word, case, regex, hex, invert match, ANSI/UTF-16/UTF-8, Office XML and EPUB). Features page: "full text search in any files across multiple drives, even inside archives". "Feed to listbox" and "Go to file" listed among result actions; results can be viewed/edited while search still runs and searched again. Search templates/separate window: not supported by the pages I could read (wiki page says blank sections).
5. Quick search: Ctrl+S opens search dialog; typing moves cursor; "add a `*` at the front of the search string to match anywhere"; incremental Quick Filter mode ("the list shows only matching items as you type") toggled by filter icon or pressing Ctrl+S again; Ctrl+Shift+S recalls last criteria; state remembered between sessions by default. Overview also mentions "a quick file filter (Ctrl+S)".
6. Undo: not found for file operations. Multi-rename has its own Undo ("reverses renames in reverse order and still works after you close the tool"). Features page: "Restore selection as it was before the last file operation" (selection only, not undo). Recycle bin: not found in sources.
7. Background queue: "Downloads run in a separate thread", download list for later, large archives packed in background. Pause/multi-job UI: not found in sources.
8. Sync/compare: Synchronize directories compares two folders; options Subdirs, by content, ignore date, asymmetric, file filter, "selected files only"; show-filters (left to right, equal, differ, right to left), "duplicates"/"singles" quick selection, per-file copy direction override, FTP time offset, summary counts. Works over FTP. File compare by content with an editor.
9. Dark mode / VoiceOver: not found in sources (Windows product; VoiceOver n/a).
10. Remote/archives: built-in FTP client with FTPS, resumable downloads, FXP, password manager; SFTP not stated on the features page. "Archives are handled like subdirectories" (ZIP, 7ZIP, RAR, TAR and others, plus plugins).
11. Multi-rename (Ctrl+M): bracket-mask placeholders ([N], [n], separate extension mask), plugin fields `[=plugin.field.unit]`, counters `[C10+5:3]`, RegEx and Search & Replace with several pairs separated by "|", presets, live "New name" preview with `<Error!>`, nothing renamed until "Start!", multi-step ("(next step)", F5), Undo.
12. Other: Branch view: "partial branch view (Ctrl+Shift+B)" on the overview; forum posts say Ctrl+B is Branch view and Ctrl+B again goes to the selected file's folder. Thumbnails: "Preview images appear in file lists". "Custom columns view" listed. Alt+Left history back: only indirectly in a forum post. Space for folder sizes, brief/full switch: not found in sources.

## 2. ForkLift 4 (BinaryNights, v4.7.8, 2026-11-17)

URLs read:
- https://binarynights.com/ (feature page)
- https://binarynights.com/versionhistory
- https://binarynights.com/manual (Quick Start Guide only)
- Search results only (titles/snippets): https://blog.binarynights.com/ release notes, https://alternativeto.net/news/2025/9/forklift-4-4-1-brings-improved-tab-restoration-reliable-icon-previews-and-more

1. Tabs: "Open different folders in the same pane, instead of separate windows." Version history: 4.7.6 "option to move tabs to the other pane"; 4.7.3 "Open in New Window" for tabs; a 2025 news item (secondary source) says 4.4.1 improved tab restoration. Manual does not cover saving/restoring.
2. Palette/toolbar: "Quick Open" reaches "favorites, devices, menu commands"; 4.5 (2026-02-10) "Quick Open now shows the menu hierarchy of commands". Closest thing to a command palette. Customizable toolbar: not found in sources.
3. Preview: "The preview panel shows you useful information about the selected file." Quick Look mentioned only for archives on the features page. Built-in viewer: not found in sources.
4. Search: "by name, extension, kind, tags or content, even on remote servers"; manual: own brute-force search (remote, subfolders) plus Spotlight (local), Command-F. Saved searches / results as panel list: not found in sources.
5. Quick filter: not found in sources. "Quick Select" selects by typing filename/extension/tag.
6. Undo: version history references undo logging for Multi-Rename (4.7.1) and Move to Trash (4.5.1); a release-note snippet mentions "Undo/Redo queue" and Edit menu entries. So undo exists, scope not documented in pages read. Trash: Command-Backspace moves to Trash, Command-Option-Backspace deletes permanently (manual).
7. Queue: "Reorder transfers, set conflict management rules, error handling, limit download and upload bandwidth." Pause: not found in sources. Version history shows a simultaneous-transfer limit.
8. Sync/compare: sync "local or remote source and target folders" "one or two-way", options for add/update/delete, saved as "Synclet" in the sidebar (manual). File compare delegates to external tools ("Xcode's FileMerge or Beyond Compare").
9. Themes: "standard Dark and Light themes" plus Default following macOS (manual). VoiceOver: only an old entry (4.2.6, 2025-02-26) "VoiceOver announce the selected pane"; no formal claim.
10. Remote: "SFTP, FTP, WebDav, Amazon S3" and other volumes. Archives: "Browse local and remote archives as if they were ordinary folders."
11. Multi-rename: change case, replace text (incl. regex), add text/dates/number sequences, presets saved as favorites.
12. Other: shortcut Command-Up parent, Command-Down open (manual). Other items (Branch view, Space sizes, brief/full, history): not found in sources.

## 3. Marta (marta.sh)

URLs read:
- https://marta.sh/ ; https://marta.sh/docs/
- https://marta.sh/docs/core/actions/ ; /core/operation-queue/ ; /core/file-operations/ ; /core/display-modes/
- https://marta.sh/docs/navigation/base/ ; /navigation/recent/
- https://marta.sh/docs/actions/flatten/ ; /advanced/archive/ ; /configuration/themes/

1. Tabs: homepage "Open as many windows and tabs as you want." Docs: "Recent Locations" (default Option-3) is tab-local and forgotten when the tab closes; "Clone to Right" action. Saving tab sets: not found in sources. Back/forward history: not found on pages read.
2. Palette: "Actions Panel" opened with Cmd-Shift-P; "Enter a few letters of the action name, and the list will be filtered". Also an "Action Bar" row of buttons below the panes with hotkeys; "All key bindings are configurable."
3. Preview: "Press Space to preview a file, similar to Finder" (homepage). Built-in viewer: not found in sources.
4. Search: "Look Up" system-wide search using macOS indices (homepage). Content search / saved searches: not found in sources.
5. Quick search: "Quick Select" selects the first match as you type; Up/Down move between matches; query starting with `/` is regex, `\` substring; word-boundary matching by default, prefix via `matchOnlyPrefix`. Selects rather than filters (page says so). Homepage separately claims "Quick Search filters a file list by substring or regular expression" (marketing wording; the two pages differ).
6. Undo: file-operations page "doesn't mention undo". F8 moves to Trash "if supported by the file system"; Shift-F8 deletes permanently; confirmations configurable.
7. Queue: operations "queued and executed one after another"; progress bar top-right in every window, shared queue; popup lets you pause or cancel each; `=` opens queue, Space/P pause, A/D abort, "designed to be fully accessible with a keyboard."
8. Sync/compare: homepage says "two-way directory synchronization" via the built-in terminal; no sync UI documented. File diff: not found.
9. Themes: five (Dark, Kon, Classic, Sakura, Commander), custom allowed; "Switch Theme" action previews live. Follow-system appearance: page does not say. VoiceOver: only the queue's keyboard accessibility claim.
10. Remote: not found in sources (homepage silent). Archives: browse as folders including nested; read-write for ZIP family, read-only for tar, RAR, XAR, CAB, LHA, CPIO, ISO, RPM, Ar; "Open As" archive; Edit action writes changes back into the archive.
11. Multi-rename: not found in sources.
12. Other: "Flatten" shows all files of the hierarchy as "a continous list" (Branch view analogue, files only); "Analyze Disk Usage", "Go To Path", Favorites, Volumes in doc index; display modes: "Table (detailed)" and multi-column (1, 2 or 3), per pane; selection not tied to cursor, Select Group by regex; Space previews (so not folder size).

## 4. Commander One (Eltima)

URLs read:
- https://mac.eltima.com/file-manager.html (feature page)
- https://en.wikipedia.org/wiki/Commander_One (secondary)
- Search results only: https://apps.apple.com/us/app/commander-one-pro-ftp-cloud/id1035237815
- Unreachable: https://commander.com/ (socket hang up), https://www.eltima.com/products/commander-one/ (404).

1. Tabs: "an unlimited number of tabs" (free). Save/switch: not found.
2. Palette: not found. Hotkeys: "Any action can be assigned with a hotkey". Toolbar customization: not found.
3. Viewer: "Built-in viewer that supports hex and binary files, plus text, media, image, and HTML." Quick Look/preview pane: not found on that page (a search snippet mentions "file previews").
4. Search: "regular expressions, case-sensitive options, content search, and Spotlight search". Saved searches / results as list: not found.
5. Quick filter: not found.
6. Undo/trash: not found.
7. Queue: "File operations queue" that lets you "Queue the operations accordingly." Pause UI not found.
8. Compare/sync: a Free vs PRO table exists but cells did not come through; I cannot say. Not found.
9. Themes: "Customizable fonts & colors" and themes (Wikipedia: PRO adds theme customization). Dark mode, VoiceOver: not found.
10. Remote: "FTP, SFTP, FTPS, WebDAV, Amazon S3, Dropbox, Google Drive, OneDrive, Box, Backblaze B2, and OpenStack" - mostly in the paid PRO Pack (one-time purchase, 15-day trial). Archives: ZIP, RAR, TBZ, TGZ, 7z; "Compressing to RAR is still in development."
11. Multi-rename: "Multi-rename: not found; closest is "Rename files during copy and move operations"."
12. Other: Wikipedia: "Three view modes: Full, Brief, and Thumbs"; terminal emulator; process viewer; hidden-files toggle; root access.

## 5. Nimble Commander (Magnum Bytes)

URLs read:
- https://magnumbytes.com/ (v1.9.0 build 4329, macOS 12+)
- https://raw.githubusercontent.com/mikekazakov/NimbleCommander/main/README.md
- https://github.com/mikekazakov/nimble-commander
- https://raw.githubusercontent.com/mikekazakov/nimble-commander/main/WHATS_NEW.md
- https://apps.apple.com/us/app/nimble-commander/id905202937
- Note: one fetch of the GitHub page said "Nimble Commander is no longer actively maintained"; a separate web search found no such statement. Conflicting; treat as unverified. WHATS_NEW lists 1.9.0 dated 2026-11-01, the App Store page lists 1.8.0 (2025-12-22).

1. Tabs: "Each panel supports tabs. You can close other tabs, and reopen recently closed ones." (App Store). Close Other Tabs Alt+Cmd+W (WHATS_NEW 1.2.5). Saving tab sets: not found.
2. Palette: not found. "keyboard shortcuts for more than a hundred actions"; "up to four hotkeys per action" (1.8.0); customizable hotkeys and external tools. Button bar: not found.
3. Viewer: "text or hex content", "loads only a small part of large files"; App Store: text, hex, preview modes, syntax highlighting, QuickLook listed.
4. Search: "Find items by attributes or content, using custom search or Spotlight. Results can be opened in a panel." (magnumbytes.com); App Store: "by name, size, or content, including inside archives". Saved searches: not found.
5. Quick search: "Find files instantly as you type," exact and fuzzy (fuzzy since 1.5.0). Filter vs jump: not stated.
6. Undo: WHATS_NEW 1.2.2 mentions "undo/redo for in-place renaming" fix; scope for file operations not found. Trash vs permanent: not found.
7. Queue: only WHATS_NEW notes on refresh during file operations; queue/pause UI not found.
8. Sync/compare: "Compare: Not mentioned" in WHATS_NEW; not found elsewhere.
9. Themes: 1.4.0 "themes follow the macOS appearance, switching automatically between two selected themes"; custom colors/fonts. VoiceOver: not found.
10. Remote: "FTP, SFTP, and WebDAV"; remote files editable and uploaded back; Dropbox VFS removed in 1.8.0. Archives: browse "like folders" (7z, RAR, ZIP, TAR, ISO), encrypted ZIP creation.
11. Batch rename: patterns, counters, date/time insertion, search and replace, regex, case changes.
12. Other: "Gallery" presentation mode (1.8.0); "Classic mode" monospaced; tags and links management; admin mode; built-in terminal synced with panels (standalone build); Shift+F7 to create a directory in the opposite panel (1.9.0); attribute editing.

## 6. Finder (macOS conventions)

- https://support.apple.com/guide/mac-help/use-quick-look-to-preview-files-mh14119/mac : select items and press Space; "a fast, full-size preview of nearly any kind of file without opening the file"; Space again closes.
- Undo: the Apple page I fetched for undo was about the Dock; Finder undo of move/rename/trash: not found in sources. Tags: not found in sources (not fetched).

## 7. Comparison table

yes = stated by a source above; partial = stated with a caveat; no = source says it lacks / only inferior form; unknown = not found in sources. "unknown" is not "no".

| Aspect | Total Commander | ForkLift 4 | Marta | Commander One | Nimble Commander |
|---|---|---|---|---|---|
| 1 Tabs per panel | yes (ghisler.com overview; lock modes via forum) | yes (binarynights.com; versionhistory) | yes (marta.sh homepage, per window) | yes (mac.eltima.com/file-manager.html) | yes (App Store page) |
| 1 Saved tab sets | partial (opentabs/appendtabs in button bar wiki) | unknown (restoration only, secondary) | unknown | unknown | unknown |
| 2 Command palette | no / unknown (none found) | partial (Quick Open, versionhistory 4.5) | yes (marta.sh/docs/core/actions/) | unknown | unknown |
| 2 Custom toolbar / button bar | yes (wiki Button_bar) | unknown | partial (Action Bar, docs/core/actions) | unknown | unknown |
| 3 Quick Look / preview | partial (Quick View panel, ghisler.com) | partial (preview panel, binarynights.com) | yes (Space preview, marta.sh) | unknown | partial (QuickLook listed, App Store) |
| 3 Built-in viewer | yes (Lister/editor, featurel.htm) | unknown | unknown | yes (eltima page) | yes (magnumbytes.com) |
| 4 Content search | yes (wiki Search) | yes (binarynights.com) | unknown | yes (eltima page) | yes (magnumbytes.com) |
| 4 Results as panel list | yes (Feed to listbox, wiki Search) | unknown | unknown | unknown | yes (magnumbytes.com "opened in a panel") |
| 4 Saved search templates | unknown (not on page read) | unknown | unknown | unknown | unknown |
| 5 Quick search (type-ahead) | yes (wiki Quick_search) | partial (Quick Select, binarynights.com) | yes (Quick Select, docs/navigation/base) | unknown | yes (App Store, fuzzy) |
| 5 Quick filter (list narrows) | yes (wiki Quick_search, Ctrl+S toggle) | unknown | partial (homepage claim vs docs: selects) | unknown | unknown |
| 6 Undo file ops | no for file ops, yes for rename (wiki Multi-Rename) | partial (versionhistory undo entries) | no (docs/core/file-operations silent) | unknown | partial (rename only seen, WHATS_NEW) |
| 6 Trash vs permanent | unknown | yes (manual shortcuts) | yes (docs/core/file-operations) | unknown | unknown |
| 7 Background queue | partial (featurel.htm) | partial (binarynights.com, no pause info) | yes (docs/core/operation-queue) | partial (eltima page) | unknown |
| 8 Folder sync | yes (wiki Synchronize_directories) | yes (binarynights.com, manual Synclets) | partial (terminal-based, homepage) | unknown | unknown |
| 8 File diff | yes (featurel.htm) | partial (external tools) | unknown | unknown | unknown |
| 9 Dark mode / themes | unknown | yes (manual: Dark/Light/Default) | partial (themes; follow-system not stated) | partial (themes) | yes (WHATS_NEW 1.4.0 follows appearance) |
| 9 VoiceOver claims | n/a | partial (versionhistory 4.2.6 entry) | partial (keyboard-accessible queue claim) | unknown | unknown |
| 10 SFTP/FTP/WebDAV/S3 | partial (FTP/FTPS; SFTP not stated) | yes (binarynights.com) | unknown | yes, PRO (eltima page) | partial (FTP/SFTP/WebDAV, no S3; magnumbytes.com) |
| 10 Archives as folders | yes (featurel.htm) | yes (binarynights.com) | yes (docs/advanced/archive) | yes (eltima page) | yes (magnumbytes.com) |
| 11 Multi-rename | yes (wiki Multi-Rename_Tool; most complete: undo, presets, plugins) | yes (presets, regex; manual) | unknown | unknown | yes (magnumbytes.com) |
| 12 Branch view / flatten | yes (Ctrl+Shift+B, ghisler.com) | unknown | partial (Flatten, docs/actions/flatten) | unknown | unknown |
| 12 Thumbnails view | yes (featurel.htm) | unknown | no (docs: table + multi-column only) | yes (Wikipedia "Thumbs") | yes (Gallery mode, App Store) |
| 12 Brief/full switch | unknown | unknown | partial (table vs 1-3 columns) | yes (Wikipedia Full/Brief/Thumbs) | unknown |
| 12 History back/forward | partial (forum Alt+Left) | unknown | unknown (only tab-local Recent Locations) | unknown | unknown |
| 12 Directory hotlist/favorites | yes (wiki Directory_hotlist) | partial (favorites in Quick Open, binarynights.com) | yes (Favorites in docs index, not read) | unknown | unknown |

## 8. Limits

- WebFetch returns model summaries; wording may be compressed. Quotes are as returned, not verified against raw HTML.
- Unreachable or 404: commander.com (socket hang up), eltima.com/products/commander-one, ghisler.com/features.htm, TC wiki pages Tabs, Find_Files, Branch_view, Keyboard, Category:Functions, magnumbytes.com/features, Apple Finder undo page content (wrong page).
- TC help/manual (F1 help, keyboard.txt) was not read; shortcut claims (Alt+Left, Space sizes, Ctrl+B details) rest on forum search snippets or are marked unknown. TC dark mode: whatsnew page does not describe features.
- Commander One: Free vs PRO table cells did not render; Wikipedia is secondary and partly old (reviews 2017-2019). Features may be gated to PRO.
- ForkLift: marketing page and Quick Start Guide only; the full manual and release notes were not read, so several "unknown" items (undo scope, command palette depth, tabs persistence) may exist. Tab-restoration claim is from a secondary news site snippet.
- Marta: remote access, multi-rename, tabs docs not found; several statements come from the homepage (marketing). Homepage and docs differ on Quick Search vs Quick Select.
- Nimble Commander: conflicting maintenance status (README fetch says "no longer actively maintained", search found none); version dates differ between sources (1.9.0 on 2026-11-01 vs App Store 1.8.0).
- Nothing installed, run, or tested; no screenshots or flow verification. No VoiceOver testing; claims are only what sources say.

# Benchmark for the new surfaces (2026-11-26)

Sources were read by a fetch agent; claims are limited to what the pages say. TC wiki pages are
old (Synchronize_directories last edited 2009, Quick_search 2012).

| Topic | Tool | Observed | URL |
|---|---|---|---|
| Sync criteria | TC | Asymmetric, Subdirs, by content, ignore date | https://www.ghisler.ch/wiki/index.php?title=Synchronize_directories |
| Sync criteria | Double Commander | name, size, date; asymmetric, subdirs, by content, ignore date | https://doublecmd.github.io/doc/en/syncdirs.html |
| Sync criteria | FreeFileSync | time and size, content, size | https://freefilesync.org/manual.php?topic=comparison-settings |
| Sync modes | FreeFileSync | Two way, Mirror, Update, Custom | https://freefilesync.org/manual.php?topic=synchronization-settings |
| Sync modes | ForkLift 4 | left→right, right→left, both; add / update / "Delete orphaned items" | https://binarynights.com/manual#sync |
| Row filters | TC | Show buttons →, =, ≠, ←, duplicates, singles | TC wiki (above) |
| Row filters | Double Commander | Show buttons >, =, ≠, ?, <, duplicates, singles | DC syncdirs (above) |
| Per-row edit | Double Commander | right-click on selected rows: "Select for copying ->", "Reverse copy direction", "Remove selection"; Space unchecks | DC syncdirs (above) |
| Summary / confirm | TC, DC, ForkLift | confirmation before copying; ForkLift shows counts added/updated/deleted | as above |
| Sync deletes | Double Commander | follows "Delete to recycle bin" (Shift reverses) | DC syncdirs (above) |
| Quick filter | TC | Ctrl+S, then filter mode toggles with Ctrl+S / filter icon | https://www.ghisler.ch/wiki/index.php?title=Quick_search |
| Quick filter | Double Commander | separate quick filter command; Esc or X resets (from a summarized page) | https://doublecmd.github.io/doc/en/help.html |
| Flat view | Double Commander | cm_FlatView lists all files of subfolders; Esc aborts the scan; calling again returns | https://doublecmd.github.io/doc/en/help.html |

Limits: TC branch view pages returned 404; FreeFileSync and ForkLift pages did not describe the
result-list layout or row filters; nothing was installed or run.

# Benchmark for the surfaces of 2026-11-27 (read 2026-11-28)

Read by a fetch agent. Its summaries are paraphrase-grade. TC's manual (.chm) was not readable online, so TC claims come from history.txt, the wiki and the forum.

| Topic | Tool | Observed | URL |
|---|---|---|---|
| Background / queue | TC | F5 then F2 puts the copy into the Background Transfer Manager (forum); the manager has + / − buttons (history.txt 11.50) | https://www.ghisler.com/history.txt ; https://ghisler.ch/board/viewtopic.php?p=439972 |
| Background / queue | Double Commander | Operations Viewer (Alt+V) lists the operations; change the order in the queue, stop or pause; progress in its own window, minimized, or a panel above the function keys | https://doublecmd.github.io/doc/en/help.html ; https://doublecmd.github.io/doc/en/cmds.html |
| Background / queue | ForkLift 4 | the manual mentions only an "Activities" view | https://binarynights.com/manual |
| Content search | TC | "Find text"; ANSI, ASCII, UTF-16, UTF-8 at once (8.50); FTP search (11.57) | https://www.ghisler.com/advanced.htm ; https://www.ghisler.com/850.htm |
| Content search | Double Commander | case sensitive, hexadecimal, regular expression, encoding choice, search in archives | https://doublecmd.github.io/doc/en/findfiles.html |
| Content search | ForkLift 4 | own search reads files one by one, also on remote connections; Spotlight for local content | https://binarynights.com/manual |
| Results sorting | TC, DC, ForkLift | header sorting not stated; feed to listbox in TC and DC | as above |
| Splitter | TC | right-click on the divider gives presets incl. 50 %; double-click 50/50 (forum, TC 7 era); Ctrl+drag snaps to 25 %; cm_50Percent | https://www.ghisler.ch/wiki/index.php/Position_of_window_divider |
| Splitter | Double Commander | drag with a width tooltip; right-click 20/80 … 80/20; cm_PanelsSplitterPerPos | https://doublecmd.github.io/doc/en/help.html |
| Connection dialogs | ForkLift 4 | favorites organized into groups in the sidebar | https://binarynights.com/manual |
| Connection dialogs | TC, DC | resizing and groups not stated | as above |

**Limits:**
- These pages returned 404: DC `queue.html`, TC wiki Queue and Background_Transfer_Manager.
- history.txt was read only partly.
- Nothing was installed or run.
