# UI critique — internal walk (2026-11-21)

Live walk on fxfm 0.1.2 (isolated instance, temp HOME with a copy of the data dir; dialogs opened
from the menu bar) plus source reading. Each row: what the label/surface claims, what the handler
does, and whether the difference is intended.

| # | Surface | Claim | Actual | Verdict |
|---|---|---|---|---|
| I1 | Files → Change attributes… | Offered in every location, like TC's Alt+A on local files | On a local folder it only says "Change attributes works on SFTP and SCP servers" (live) | Contradiction: the common case (local files) is refused |
| I2 | Menu bar Start, Help | Two top-level menus | Both are empty (live menu dump); `MENUS` in `src-tauri/src/menu.rs` lists them | Contradiction: menus that promise content and have none |
| I3 | Help buttons in Ctrl+F servers and Ctrl+D hotlist | Help exists per dialog | Each opens a short text (`servers.js` `showHelp`, `hotlist.js` `HELP`); there is no app-level help or key reference | Inconsistent with I2 |
| I4 | Edit → Undo (menu bar) | Undo | Only undoes text editing in input fields; file operations have no undo; Multi-rename has its own "Undo last rename" button | Intended macOS default menu, but undo is scattered and absent where it matters (see safety nets) |
| I5 | Native `<select>` in Keyboard, Multi-rename, Pack, Options | Same dialog typography | The selects render in the small system control font, visibly smaller than labels and inputs (screenshots) | Unintended visual inconsistency |
| I6 | F8 confirmation | "Do you really want to move N file(s)/dir(s) to the Trash?" (`deleteConfirmText`) | Never names the item, even for one; TC names a single file | Minor; text could name it |
| I7 | Options → Search → Volumes | Lists indexable volumes | In the walk only the startup disk was listed while two network shares were in the drive bar | UNVERIFIED: may be correct (shares not mounted in the isolated HOME); needs a check with shares mounted |
| I8 | F1 = Show hidden files | TC F1 = Help | Owner's own choice (session history) | Intended difference |
| I9 | Copy (F5) dialog | Target path, OK / Cancel | No "queue / background" option, no filter field (TC has both) | Gap, not a contradiction (see report) |
| I10 | `status("… coming in a later version")` in `ui/app.js` | — | No command is `available: false` any more | Dead UI path, no user impact (polish) |

Checked and consistent: F8 inside archives and on servers says "This cannot be undone" and is
permanent (`deleteConfirmText`); Shift+F8 is labelled "Delete permanently"; menu titles carry the
keys of the current Alt/Ctrl mapping; Find files is now a native window with its own close button.

# Internal walk (2026-11-26)

| # | Surface | Claim | Actual | Verdict |
|---|---|---|---|---|
| J1 | Synchronize dirs list, Delete key | In the panels Delete = F8 (delete) | In the sync list Delete and Backspace set the row to *skip* (`ui/syncwin.js:420`); deleting is Shift+→/← | Contradiction: the same key means "delete" in one place and "don't touch" in the other |
| J2 | Right-click *Properties…* | Properties (size, dates, kind, permissions — the Windows/Finder meaning) | Opens Change attributes (octal/rwx only) | Label overclaims; the dialog has no size, dates or kind |
| J3 | Clipboard commands | TC: Ctrl+C / Ctrl+X / Ctrl+V copy, cut, paste files in the panels | `cm_copy_to_clipboard`, `cm_cut_to_clipboard`, `cm_paste_from_clipboard` have no default keys (`keymap.rs`); only the menu, right click and Configuration → Keyboard | Gap against the TC model the app follows; Ctrl (Control) +C/X/V are free |
| J4 | Alt+Left/Right in the command line | macOS text fields: Option+Left/Right move by word | `cmdlineKeyGoesToPanel` sends every Alt+ combo to the panel (`ui/lib.mjs:179`), so with Alt = Option they run Back/Forward while typing | Likely contradiction with macOS editing (static walk — not exercised live) |
| J5 | Branch view stop | Find files and other long work stop with Esc | Ctrl+B again stops the walk; Esc does not (`ui/app.js` branchView) | Minor inconsistency; Double Commander stops flat view with Esc |
| J6 | Synchronize dirs filtering | — | Only *Show equal*; no way to show only → rows, only ← rows, only not-copied differences | Gap (see benchmark) |
| J7 | Synchronize dirs per-row editing | — | One row at a time (keys or click); *Reset to mode* / *Skip all* for every row | Gap: no selection of several rows |
| J8 | Help menu | Help | Only *Keyboard shortcuts*; the README's feature explanations (sync modes, quick filter rules) are not reachable in the app | Partial (the sync window has its own one-line hint) |

Checked and consistent: Ctrl+S filter box shows "n of m" and Esc clears it; F8 names a single
item; the right-click menu hides Open with / Quick Look / Show in Finder outside local folders;
Synchronize confirms with counts and says that server/archive deletes are permanent; dark mode
covers dialogs, menus and bars.

## 2026-11-28 walk (new surfaces)

| # | Surface | Label / claim | Actual handler | Finding |
|---|---|---|---|---|
| J10 | Find files *Text* + *In: Everywhere* | the radio says Everywhere | a text search always walks the current folder (`searchRequest`, `ui/lib.mjs`); only the field's placeholder says so (`ui/search.js:153`) | contradiction: the checked choice is not what runs |
| J11 | Operations strip | "click to see the progress" | `showBackgroundJob` opens the first background job; with only queued items (between jobs) a click does nothing | dead end in a short window; no list of the queued operations, × removes all of them at once |
| J12 | F5/F6 dialog *Queue* | button only | no key (TC: F2 in the copy dialog) | keyboard path missing for a TC user |
| J13 | Progress window *Background* | button | no key; Esc = Cancel | fine, but the most useful button for long copies has no key |
| J14 | Quit with jobs | "Quit and cancel them?" | a second Quit within 5 s quits without asking (`quit.rs`) | intentional escape hatch; documented in MANUAL only |
| J15 | Linux build labels | "Show in Finder", "Finder (macOS icons)", "Folder icon (Finder icons)", ⌥⌃⌘ glyphs, Fn option | same strings on Linux (`ui/lib.mjs:2263,2743`, `config.js:505`, `comboLabel`) | contradictions on Linux (in the port plan's UI pass) |
| J16 | Linux icons in the smoke screenshot | rows | empty boxes (no emoji font in the container; GTK theme icons added since) | to re-check after the next Linux build |
| J17 | Splitter | tooltip "double-click: 50/50" | drag 10–90 %, dblclick 50/50, kept in the session | consistent; no keyboard path (TC has none either, see benchmark) |
| J18 | Find files sort | tooltip explains 3 states | header click; no key | consistent; mouse only |
