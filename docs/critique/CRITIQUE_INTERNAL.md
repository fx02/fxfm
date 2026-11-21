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
