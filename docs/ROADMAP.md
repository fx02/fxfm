# fxfm roadmap

## Next
- **Help → Statistics** (owner's idea, 2026-11-28): memory (physical footprint) and CPU of fxfm and its WebKit processes (Web Content, GPU, Networking; on macOS found by their responsible app), their sum, the index's entries and size, background jobs, uptime; refreshed every second.
From the UI critique (docs/critique/UI_CRITIQUE_REPORT.md), not started:
- A list of the background operations (TC's transfer manager: the queue, remove one, + / −), opened from the strip.
- Text search options as in TC's Find text (whole words, UTF-16 and other encodings, inside archives).
- Folder tabs per panel.
- Thumbnails.
- Screen-reader labels (VoiceOver).

## Right-click menu
- Open with, Quick Look and Show in Finder work on local files only (an archive or server item would first need a temporary copy).

## Synchronize dirs
- No saved comparisons; by content only for local files.

## Search
- A full rebuild briefly holds two indexes in memory (about 2x RAM) while the new one replaces the old. By design: dropping the old index first would leave searches without results for the whole scan.

## Servers (after plan 3)
- The Keychain code is tested only by hand (tests do not touch the real Keychain): it needs a manual check on a real Mac.

## SMB shares
- **Domain login needs a check on the real domain share**: NetFS gets only the bare user name and the domain travels in the URL (`DOMAIN;user@host`); since 2026-11-21 a rejected domain login is tried once more with no user argument at all. Kerberos (*Use the Kerberos ticket*) and *Shares…* (`smbutil view` on a pseudo-terminal) also still need a check against a real server.
- NetBIOS discovery asks only the local /24 (or smaller) network of each interface, never a VPN; a Windows firewall that blocks UDP 137 (the default for the Public network profile) or NetBIOS over TCP/IP turned off hides the computer, and then only typing its name or address helps. Workgroup/domain browsing (the master browser list) is not done.
- Cancel while mounting only stops waiting: NetFS cannot be interrupted from fxfm, so the mount may still finish in the background (the drive then shows up as mounted; the typed password is not kept or saved, and no panel moves).
- The password is handed to NetFS as a CFString, which cannot be wiped; the JavaScript strings it passed through live until garbage collection.

## Audio (F3)
- WAV codecs other than PCM/float/A-law/mu-law/MS and IMA ADPCM/GSM 6.10/MP3 (e.g. G.726) are reported as unsupported; not planned.

## Pack files (Alt+F5)
- The password is wiped on the Rust side; the JavaScript strings it passed through cannot be wiped and live until garbage collection.

## Drag & drop
- A file promise dropped where an item with the same name exists is written as `name 2.ext` beside it (never overwritten, no question). Whether Finder shows that new name at once needs a check with a real drag.
- Writing promised items has no progress or Cancel in fxfm (only what the receiver shows itself).

## Linux
- Built and tested in containers only (unit tests, a start under Xvfb); every desktop feature (Secret Service, gvfs mounting, clipboard and drag with a file manager, Sushi, icon theme) still needs a check on a real Debian/Ubuntu desktop.
- Drag out starts a GTK drag from the page's mouse press; on Wayland a drag without a fresh event may not start.

## Done
- **Linux packages** (2026-11-28): Debian packages for amd64 and arm64 (Docker build, scripts/build-linux.sh) with Linux equivalents of the macOS parts (see docs/superpowers/plans/2026-11-27-linux-port.md and MANUAL "Linux").
- **Fix: a move to a share left the sources** (2026-11-28): a cross-device move compared the copy's mtime with the source, and SMB shares (also FAT, FTP) do not keep it, so every source was kept. It now compares the source with its own size and mtime taken when it was copied.
- **TC habits and Linux wording** (2026-11-28): Ctrl/Cmd+click and Shift+click mark in the panels; F2 with a path moves the item (`../x`, `sub/x`); ←/→ walk the buttons of a question; F2 = Queue in the F5/F6 dialog and Background in a progress window; right-click presets on the panel splitter; Find files turns Everywhere off while a text search runs (it reads the current folder); the Linux build names keys Ctrl/Alt/Super and says "file manager" and "System icons".
- **Panel splitter** (2026-11-27): drag the bar between the panels (10–90 %), double-click for 50/50; kept in the session.
- **Operations in the background and a queue** (2026-11-27): a Background button in every progress window, a Queue button in the F5/F6 dialog (one after the other), a row above the function keys with progress and the queue count (click: progress window; ×: empty the queue); quitting with operations running asks first.
- **Find files: content search and sorting** (2026-11-27): a *Text* field finds files whose content has the text (current folder, case sensitive or not, results stream in); a click on a column header sorts the results (again: reverse, a third time: best match first).
- **Connect to server window and groups** (2026-11-27): Ctrl+F can be moved, resized and maximized inside the main window (size kept); servers in groups up to three levels (New group…, drag into a group, Alt+←/→ out of / into a group). The server rows no longer take the Synchronize dirs grid layout (a class name clash since 0.1.10).
- **Permanent delete on network shares** (2026-11-27): F8 on an SMB/AFP/NFS/WebDAV share deletes permanently instead of trying the missing Trash; the question says so first. **Copy network path** (2026-11-27): Windows `\\server\share\…` and `smb://` forms of items on an SMB share.
- **Font and icon styles** (2026-11-26): Options → View → Font picks the panels' font from the Mac's fixed-pitch families (default Menlo) or the system font; Icons adds Classic (drawn yellow folders) and Outline (line icons); Finder icons show the special folders' own icons and offer macOS's folder icons (CoreTypes, incl. one-color sidebar glyphs) for ordinary folders; the status line keeps size, date and mode in fixed columns.
- **Undo** (2026-11-26): Ctrl+Z undoes the last local move, Trash delete (items come back from the Trash), F2 rename or F7 new folder, after a confirmation and without overwriting anything. Copies, server and archive operations and permanent deletes are not undoable.
- **Critique fixes** (2026-11-26): Ctrl+C/X/V copy, cut and paste files (TC keys); Synchronize dirs gets Show →/←/≠/= filters and several picked rows at once, Delete no longer means "skip" there; Alt+Left/Right in the command line move by word; Esc stops a branch view scan; the right-click "Properties…" is called "Change attributes…".
- **Branch view** (2026-11-26): Ctrl+B lists every file below the panel's folder (local, archive, server) as a results list named by relative path; Ctrl+B again returns. Copy name on a results list copies the bare name.
- **Synchronize dirs** (2026-11-25): Commands → Synchronize dirs… opens its own window and compares the two panels' folders (name, size, date, optionally content; subfolders; case- and NFC/NFD-insensitive names); modes Two-way, Mirror, Update and Delete-missing propose a direction per row, every row can be changed, and Synchronize runs the copies and Trash deletes as normal jobs after one confirmation.
- **Quick filter, folder history, more right-click items** (2026-11-25): Ctrl+S filters the panel by text or mask; Alt+Left/Right go back/forward per panel, Alt+Down lists earlier folders; the right-click menu adds View, Open with…, Quick Look, Show in Finder, Pack files and Properties, and every item can be switched off in Options.
- **Small fixes** (2026-11-25): Help → Keyboard shortcuts lists every command with its keys (the empty Start menu is gone); Paste into the folder the items came from makes Finder's `name copy`; F8 names a single item; a file promise dropped onto an existing name is kept beside it as `name 2.ext`; dialogs, menus and bars follow the system dark mode (panel colors stay as set in Options → Colors).
- **Fast F5, Enter and F2 selection** (2026-11-25): Enter pressed right after F5/F6/F7… (under 150 ms) is no longer swallowed: the short key delay applies only to a dialog opened by Enter, Escape or Space. F2 selects the whole name, extension included.
- **Right-click menu and clipboard** (2026-11-24): right click (or Shift+F10) on a row: Open, Copy name, Copy path, Cut, Copy, Paste, Delete, Rename; Copy/Cut use the macOS clipboard (local files paste in Finder, Finder's copied files paste in a panel); Paste runs the copy/move without the F5 dialog.
- **Change attributes on local files** (2026-11-24): Files → Change attributes… (octal and rwx boxes) also works on local files and folders (a symlink: its target), not only on SFTP/SCP; items inside archives are refused. Dates and macOS flags (hidden, locked) are not changed there yet.
- **Finder icons** (2026-11-24): panel rows show the icons macOS uses in Finder (folder, file type, an application's own icon); Options → View → Icons switches back to emoji. Special folder icons (Desktop, Downloads…) and image thumbnails are not shown.
- **Find files in its own window** (2026-11-21): Alt+F7 opens a native window (move, resize, maximize, full screen, other screens; geometry kept) instead of a dialog in the main window; going to a file, Feed to listbox, mounting a share and F3 on audio run in the main window over a small `findfiles://` event bridge. The maximize button is gone.
- **Drag archive results** (2026-11-21): archive entries in a search results list can be dragged out as file promises too (not mixed with plain files).
- **Drag-out from archives and servers** (2026-11-21): own AppKit dragging session instead of the `drag` crate; archive and server items (folders too) go out as file promises, extracted or downloaded on a background queue only when the receiver asks; local files may be moved (Finder's rules) and the panel refreshes after a move; a count badge on the drag image for several items.
- **Audio devices with one name** (2026-11-21): devices that share a name are listed as `Name (1)`, `Name (2)` and saved by their CoreAudio id; unique names are still saved by name.
- **Fn as Alt or Ctrl** (2026-11-21): Configuration → Keyboard offers Fn (globe); a native monitor tracks the Fn key (WebKit never reports it) and tells every window.
- **Search: SMB shares under another name, smaller fold** (2026-11-21): a result on an unmounted share finds its saved entry by the mount's host and share (`mntfrom`); changes recorded during a full scan past 50k paths fold to the deepest folder level that fits instead of top-level folders.
- **Nested archives** (2026-11-21): an archive inside an archive (`outer.zip/inner.tar.gz/dir`, any depth) opens as a folder; browse, F3/F4, F5/F6 in and out, F7, F8 and F2 work, and changes are written back into the outer archive (one atomic rewrite per level).
- **Per-file conflicts in archives** (2026-11-21): copying or moving a folder into an archive merges into an existing folder and asks about every existing file, as a local copy does ("all" answers included).
- **SMB shares, part 2** (2026-11-21): *Shares…* lists a server's shares (`smbutil view`, the password typed on a pseudo-terminal, never on a command line); Browse… also finds Windows file servers over NetBIOS (node status requests to the local /24, no root); *Use the Kerberos ticket* mounts without a password; a share mounted under an alias of the entry's host (same address) counts as mounted; a rejected domain login is tried once more without the user argument.
- **Audio device follows the system** (2026-11-21): while playing, a new macOS default output (headphones, AirPods) or the chosen device connecting again is taken within 2 s.
- **Audio pause and volume** (2026-11-21): ⏸/▶ in the player bar, a volume slider that is kept.
- **tar.bz2 and tar.xz** (2026-11-21): opened, written and packed like tar.gz (pure-Rust bzip2 and xz).
- **Pack formats and adding** (2026-11-21): Alt+F5 packs into ZIP, TAR, TAR.GZ, GZ (one file) and 7Z, and can add to an existing archive.
- **SMB at startup and Ctrl+Shift+F** (2026-11-20): saved shares can be mounted when fxfm starts (*Connect at startup*); Ctrl+Shift+F unmounts the share the active panel is on.
- **Multi-rename (Ctrl+M)** (2026-11-20): name/ext masks ([N], [N2-5], [E], [C], [P], date/time), search and replace (plain or regex), case, counter; live preview with clash checks; safe two-phase renaming (swaps, case-only); undo of the last batch; works in local folders and search result lists.
- **Find files result list** (2026-11-20): resizable window (size kept; since 2026-11-21 also a maximize button / double-click on the title), Name/Folder/Size/Date header with resizable and hideable columns, tooltips for cut-off names, Cmd/Ctrl+click and Shift selection; Feed to listbox sends the selected results.
- **Quick search** (2026-11-19): Cmd+letters in the active panel jump to the first name with that start (TC Alt+letters); Up/Down for the other matches, Backspace, Esc or a 3 s pause; keys set in Configuration → Keyboard (Command, Control+Command, Option, letters alone, off). Still open: wildcard `*` and matching inside the name.
- **Search (Alt+F7)** (2026-11-19): Find files by name over fxfm's own index (startup disk, enabled local and network volumes, optional archive entries; FSEvents, first-run prompt, Full Disk Access hint), a current-folder walk when there is no index, results as a panel list (Feed to listbox: F3/F4/F5/F6/F8, drag out), and Configuration → Options → Search.
- **SMB shares** (2026-11-17): an SMB (network share) connection type in Ctrl+F; macOS mounts the share (NetFS) under /Volumes and the panel opens it as a local folder; saved shares are drives in Alt+F1/Alt+F2 and the drive bar (mounted on pick), and Disconnect runs `diskutil unmount`.
- **App icon** (2026-11-17): smaller, regular-weight "fxfm" text and a larger, centred pair of panels (`src-tauri/icons/gen_app_icon.py`).
- **Colors like TC** (2026-11-14): Configuration → Options → Colors with font, background, background 2, mark, cursor and cursor font colors, inverted cursor or a frame, an Example box, and "Define colors by file type" (ordered mask rules for files and folders, first match wins; defaults for documents, executables, images, archives, video and audio). Old color settings migrate.
- **Plan 3 – serveri** (2026-11-14): SFTP/SCP/FTP/FTPS (Ctrl+F), per-server keys (import into `keys/`), master password in portable mode and Keychain in system mode, `known_hosts`, F3–F8/F2 and chmod on a server, safe upload (`.part` then replace), Connection lost + Reconnect, hotlist with servers, archives on a server (Enter downloads a tracked copy, changes are offered for upload back).
- **Drag & drop** (2026-11-14): dragging rows (the selected items if the pressed row is selected, otherwise just that one; never `..`) from a panel into other applications as a real macOS file drag (`drag` crate, local files only), from Finder into a panel, and between panels. Dropping opens the F5 dialog (Shift = F6) with the target = the folder under the pointer or the panel's folder; the dialog also has a "Move instead" / "Copy instead" button. Same folder, the dragged item itself, or a folder inside the dragged one: nothing happens. The hotlist dialog reorders rows with the mouse (HTML5 drag & drop does not work with the Tauri drag-drop handler).
- **F3 audio preview** (2026-11-13): F3 on an audio file plays immediately in the main window, focus stays in the panel; the bar above the F-keys (name, `m:ss / m:ss`, codec, click on the bar = seek); F3 on the same file or Esc stops it. Decoding in Rust (`symphonia` + `symphonia-opus`, own GSM 6.10 and `.au`): wav (PCM, float, A-law, mu-law, MS/IMA ADPCM, GSM WAV49, MP3-in-WAV), mp3, flac, ogg/vorbis, opus, m4a/aac, aiff, au, gsm. Also works in archives and on servers (the temporary copy is deleted when playback ends).
