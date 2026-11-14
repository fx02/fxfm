# fxfm (fx file manager) – design v1

Date: 2026-11-06
Requested by: Zvonimir Bužanić
Project: `~/Documents/PVT/fxfm` (sister project of the fxmp music player)

## 1. Purpose and rationale

The owner worked in Total Commander on Windows for years and moved to the Mac. The move "was not easy". There is no file manager on the Mac that looks and behaves like TC. fxfm is a dual-panel file manager for macOS that **looks like TC and behaves like TC** (same keys, same dialogs, same workflow) and adds what the owner needs every day:

- fast copying of files (php, shell scripts, zip/tar) to Linux servers on the local network, over VPN or on the internet, with per-server SSH keys
- opening and editing archive contents as if they were folders

Success: the owner opens fxfm instead of TC/Finder and works "blind" on TC muscle memory.

## 2. v1 goals

1. Main window in the TC look (see §4), two panels, Tab switches focus between them.
2. Disk/partition overview: a drive bar and a drive dropdown.
3. Basic TC operations and shortcuts (see §5), all remappable.
4. Ctrl+D location hotlist (Change directory menu), as in TC.
5. Ctrl+F server list and connecting: SFTP, SCP, FTP, FTPS, with per-server keys.
6. Archives as folders: zip, tar, tgz (tar.gz), gz, 7z. Read and write (copy in/out, delete, rename, edit a file inside an archive).
7. F3 built-in viewer, F4 external editor of the user's choice.
8. Portable or system settings storage, at the user's choice.

## 3. v1 non-goals (for later versions)

Tabs (Ctrl+T), directory compare, directory sync, multi-rename, search (Alt+F7), plugins, thumbnail view, rar.

The owner plans to add most TC functionality gradually. First after v1 are **search (Alt+F7)** and **multi-rename (Ctrl+M)**, because they are used daily. The v1 architecture must therefore make it easy to add new commands: every command registers in a central command registry (name, action, default key) that drives the menu, the F-key bar and key remapping. Alt+F7 and Ctrl+M are reserved in the registry in v1.

### Anti-goals (if any of these happen, v1 is a failure even though it "works")

- It looks like Finder or a generic Mac app, not like TC.
- Keys do not behave as in TC, or F-keys do not work directly (the owner has "Use F1, F2… as standard function keys" enabled, F-keys arrive without Fn).
- Shortcuts use Cmd instead of Ctrl (TC uses Ctrl and that stays).
- Settings, keys and passwords are forcibly locked to the system (Keychain/Library) with no portable mode.
- Editing in an archive or on a server silently loses changes.

## 4. Look (approved mockup)

References in `tc-screenshots/` next to this document:
- `fxfm-mockup-approved.html`: the approved fxfm mockup

Dialogs (Ctrl+D, Ctrl+F) follow the layout of the same-named Total Commander dialogs; screenshots are not part of the repository.

Top to bottom, for each panel:

- **Drive bar**: buttons with icon and name for `/` (Macintosh HD), `~` (home), every mounted volume from `/Volumes` (USB, DMG, SMB/AFP share) and every **active** server connection (☁️ name). The active drive is highlighted. Updates automatically when a volume is mounted or unmounted.
- **Path row**: combo for choosing the drive, `[volume name]  X k of Y k free` (number format as in TC, dot as thousands separator), and on the right `/` (root) and `..` (parent) as clickable buttons. For remote, `[sftp://user@host]` is shown instead of free space.
- **Current path** with a filter (`~/Documents/PVT/*.*`). The focused panel has a highlighted bar.
- **List**: black background, large font (approximately as in the TC screenshot), icon and name. Folders in `[brackets]`, `[..]` first. The cursor is a grey row. Sorting: folders then files, alphabetical. Clicking a column header is not required in v1 because the view is "brief/full" as in the screenshot (name only).
- **Status line** below the panel: name under the cursor, `<DIR>` or size, date `dd.MM.yyyy HH:mm`, Unix permissions (`drwxr-xr-x`).
- Below both panels an optional **command line** (`path>`), which runs the command in Terminal.app in the current folder. Then the **F-key bar** (F3 View, F4 Edit, F5 Copy, F6 Move, F7 NewFolder, F8 Delete, Alt+F4 Exit). Clicking a button does the same as the key.

Colors (defaults, all adjustable in Configuration → Colors):

| type | color |
|---|---|
| folders, regular files | white |
| hidden (dotfiles, .DS_Store) | grey |
| archives (zip, tar, tgz, gz, 7z) | orange-red (#ff6a4d) |
| video/audio | purple-blue |
| marked (Insert/Space) | red |

The main menu (Files, Mark, Commands, Net, Show, Configuration, Start, Help) lives in the **macOS menu bar**, not in the window. Window title: `user – fxfm version`.

**Start folder:** on the **first** launch both panels start in the owner's home directory (`~`, where Desktop, Documents, Downloads etc. are). After that fxfm **remembers the last state and restores it on every launch**: the paths of both panels, the active panel and the cursor position.

Restore rules:
- If the remembered path no longer exists (unmounted USB, deleted folder), the panel goes to the nearest existing parent, and if there is none, to `~`.
- If the panel was on a server or in an archive, it opens on the local folder it was entered from (for a server that is `~`). My decision: fxfm does not connect to a server automatically at startup.

Window size and position are always remembered.

## 5. Keys and operations

All keys use the physical **Ctrl** (not Cmd). Option on the Mac serves as Alt.

**The owner's keyboard is a Mac**: the built-in MacBook keyboard or a Logitech MX Keys for Mac. There is no Insert key; numpad and Home/End/PgUp/PgDn exist only sometimes (on a MacBook they are Fn+arrows). The default map therefore has a **Mac alternative** for every TC action that depends on Insert/Num keys:
- Space marks (Insert stays bound if the keyboard has it)
- Ctrl+= / Ctrl+− do the same as Num + / Num −
- Ctrl+8 does the same as Num *

Num keys stay additionally bound for keyboards with a numpad.

**System conflicts**: macOS itself intercepts some Ctrl combinations (Ctrl+↑/↓ and Ctrl+←/→ for Mission Control, Ctrl+Space for switching input language, possibly Ctrl/Option+F-keys). fxfm **does not forcibly take them over** (no CGEventTap and no Accessibility permission). On first launch, and in Configuration → Keyboard, it shows which default fxfm keys are taken by the system and offers to open System Settings → Keyboard Shortcuts or to remap in fxfm. For reordering in the hotlist there are also ↑/↓ buttons and drag & drop.

| key | action |
|---|---|
| Tab | switch focus to the other panel |
| ↑ ↓ PgUp PgDn Home End | cursor movement |
| Enter | enter a folder or archive; for a file, open it with the default macOS application |
| Backspace | parent folder |
| Ctrl+\\ | root |
| Ctrl+PgUp | parent folder (TC alternative) |
| Insert / Space | mark or unmark (Space on a folder also calculates its size) |
| Num + / Num − | mark or unmark by mask (dialog with `*.php` etc.) |
| Num * | invert marks |
| Ctrl+A | mark all |
| F2 | in-place rename (the owner's request) |
| Shift+F6 | in-place rename (TC default, this stays too) |
| F3 | built-in viewer |
| F4 | edit in external editor |
| Shift+F4 | new file and open in editor |
| F5 | copy to the other panel (dialog with destination, as in TC) |
| F6 | move to the other panel |
| F7 | new folder |
| F8 / Delete | delete (confirmation; locally goes to Trash, option for permanent delete with Shift+F8) |
| Alt+F1 / Alt+F2 | drive selection for the left/right panel |
| Ctrl+D | hotlist (§6) |
| Ctrl+F | connect to server (§7) |
| Ctrl+Shift+F | disconnect |
| Ctrl+R | refresh panel |
| Ctrl+U | swap panels |
| Alt+F4 | exit (besides Cmd+Q) |

**F5/F6/F8 operations** work the same for local ↔ local, local ↔ server, server ↔ server (via a local temporary store), local ↔ archive and archive ↔ server. If there are marked files, the operation works on them; otherwise on the file under the cursor. Long operations show TC's progress dialog (current file, total progress, Cancel) and do not block the UI. On a conflict it asks as TC does: Overwrite / Overwrite all / Skip / Skip all / Rename / Overwrite older.

**Remapping**: Configuration → Keyboard shows a list of all commands with their current combination, and the user presses a new combination. There is a conflict warning and a "Reset to TC defaults" button. Every key (including F1–F12, Tab, Insert, Num keys) can be assigned to any command.

**F3 viewer**: a built-in window. Text with encoding detection (UTF-8, Windows-1250 for Croatian characters), images, PDF, and hex for everything else. Esc or F3 closes it. Works for files on a server and in an archive too (it downloads them to a temporary folder first).

**F4 editor**: Configuration → Editor holds a path to an application or a command (e.g. Visual Studio Code, Sublime, or `nano` in Terminal). For a local file it opens directly. For a file from a server or an archive:
1. it downloads it to a temporary folder and opens it in the editor;
2. it watches that copy (editors like VS Code do not block until closed, so it does not wait for the process to exit);
3. on every save of the file it asks as TC does: "The file has changed. Upload back to the server/archive?" (Yes / No);
4. temporary copies are cleaned up when the connection is closed or on exit.

## 6. Ctrl+D hotlist ("Change directory menu")

- Ctrl+D opens a popup menu at the cursor: first "Add current dir" and "Configure…", then the user's items and submenus. Items have TC's `&` for the accelerator key.
- Configure opens a dialog 1:1 with TC (second screenshot): item list, buttons Add Item, Add submenu, Delete Item, Change Title, OK, Cancel, Help, fields **Command** (`cd /path`) with a folder-picker button, **Target path** (if set, the other panel goes there) and "Hide menu items starting with this character" (double-click hides or shows the item).
- Reordering by drag & drop or Ctrl+↑/↓.
- Command can also be remote: `cd sftp://web-1/var/www/html`, where `web-1` is a name from the server list. Selecting it connects (if not already connected) and opens that folder.

## 7. Servers (Ctrl+F)

The **"Connect to server" dialog** is 1:1 with TC (third screenshot): a tree with folders and servers, buttons Connect, New connection…, New URL…, Duplicate entry…, New folder…, Edit…, Delete, Encrypt…, Cancel, Help. Double-click or Enter connects.

**Per-server data**:
- name
- protocol: SFTP, SCP, FTP, FTPS (explicit TLS); SCP uses an SSH connection, and lists folders via SFTP or `ls` if the server has no SFTP
- host, port
- user
- authentication: password, private key, or key + passphrase
  - a key can be **picked from a location** on disk (the path is remembered) or **imported** (copied into `keys/` in the fxfm data folder and travels with it)
  - supported OpenSSH formats (RSA, ECDSA, Ed25519) and PEM
- start directory locally and on the server
- option "Allow legacy algorithms (ssh-rsa/SHA1)" for old servers
- FTP: passive or active mode, name encoding
- option "Save password"

**Key/server password**: if not saved, it asks when connecting.

**Host key**: on first connection a fingerprint is shown with a question whether to accept it. Accepted keys are stored in fxfm's `known_hosts` in the data folder. If the key changes, a loud warning follows.

After connecting, the server appears as a "drive" in the drive bar and the panel browses it as a folder. All operations stay available: F3, F4, F5, F6, F7, F8, F2, and for SFTP also changing permissions (Files → Change attributes, chmod). Multiple simultaneous connections are allowed. Disconnect is Ctrl+Shift+F. If the connection drops, the panel shows this clearly and offers to reconnect.

**New URL…** accepts `sftp://user@host:port/path` or `ftp://…` for quick connecting without saving.

## 8. Archives

- Enter on `.zip`, `.tar`, `.tgz`/`.tar.gz`, `.gz`, `.7z` enters the archive like a folder. `[..]` at the archive root returns to the folder containing the archive. The path is shown as `/path/to/archive.zip/inner/folder`.
- Inside an archive, F3, F4 (with the "upload back" flow as for servers), F5 (out and in), F6, F7, F8 and F2 work.
- `.gz` (without tar) contains a single file. It is shown as an archive with one file.
- Writing to an archive goes through a new temporary archive that **atomically replaces** the original only after success. The original must never be corrupted if the operation fails.
- Archives on a server: Enter downloads them to a temporary folder and opens them. Changes are offered for upload back as with F4.
- Password-protected zip: asks for the password to read. 7z with a password and writing to encrypted archives are not in v1 (libarchive does not read encrypted 7z; if needed, that is a separate backend later).
- Creating new archives (Alt+F5 in TC) is not in v1, unless trivial with the same library.

## 9. Settings and portable mode

Configuration → Options → **Storage mode**:

- **Portable**: everything in `fxfm-data/` next to `fxfm.app` (config, hotlist, servers, keymap, colors, known_hosts, `keys/`). Passwords and passphrases are encrypted with a **master password** (the Encrypt… button in the Ctrl+F dialog, as in TC). The master password is asked once per launch, when needed. Without a master password, passwords are not saved.
- **System**: settings in `~/Library/Application Support/fxfm/`, passwords in the macOS Keychain.

Rules:
- If `fxfm-data/` exists next to the application and is writable, portable mode is used. Otherwise system mode is used.
- Switching modes migrates all data (including passwords) and confirms this clearly.
- Formats are human-readable text (JSON or INI), so the user can edit or version them by hand.

## 10. Constraints

- macOS 15 (Sequoia) or newer. The development Mac has macOS 15.7, Rust 1.99.1, tauri-cli 2.12, Node 26.
- The application must be self-contained: all libraries (SSH, FTP, archives) **embedded in the .app**. No Homebrew or other installs on the target Mac, because portable must work by copying a folder.
- Local access to the whole disk requires Full Disk Access for some folders. On first launch the application explains this if it hits a denial.
- Distribution: personal use, local build. Signing/notarization are not needed in v1 (ad-hoc signature).
- **Carrying to other Macs** (USB, network copy, zip) is expected:
  - A copied application gets the quarantine attribute, so Gatekeeper may block it, and App Translocation runs it from a random read-only path. Then `fxfm-data/` next to the .app cannot be found.
  - fxfm must recognize a translocated path (`SecTranslocateIsTranslocatedURL` or a path under `/private/var/folders/.../AppTranslocation/`). It then shows a clear message with the fix: the command `xattr -dr com.apple.quarantine <path>/fxfm.app`, which can be copied or run with one click, after which the application restarts.
  - fxfm must not silently fall back to system mode or start with empty settings.
  - A short README "how to move to another Mac" ships with the project.
- Git commits for this project only as `zbuzanic@gmail.com` (same rule as for fxmp).

## 10a. Platform and Linux

fxfm is a **macOS-first** project on **Rust + Tauri 2**, modeled on the owner's fxmp (`~/Documents/PVT/fxmp`): a Cargo workspace with `crates/core` + `src-tauri`, UI in plain JS/HTML/CSS without a bundler (`ui/`), Rust version pinned in `rust-toolchain.toml`. The owner made this decision ("if Rust is better, go Rust"), because it also brings a Linux build almost for free.

- **fxfm-core** (`crates/core`): all the logic, namely the VFS (local, archive, SFTP/SCP/FTP), operations with progress and conflicts, command registry and keymap, configuration, password encryption, hotlist and server list. No dependency on Tauri. Must build on macOS and Linux.
- **src-tauri**: a thin layer that exposes commands to the UI, plus macOS specifics (menu bar, Keychain via `keyring`, Trash, Terminal.app).
- **ui/**: TC look, panel drawing, keyboard.
- A Linux build is not a v1 goal, but the code must not have macOS-only assumptions outside `cfg(target_os = "macos")` blocks.

## 11. Decisions made by Claude (not explicitly chosen by the owner)

- Menu in the macOS menu bar.
- Unix permissions instead of Windows attributes in the status line.
- Default colors from §4.
- F8 deletes to Trash locally (Shift+F8 permanent).
- Server ↔ server copying goes through a local temporary store.
- Remote items in the hotlist (`cd sftp://name/path`).
- Mac alternatives for Insert/Num keys (Space, Ctrl+=, Ctrl+−, Ctrl+8) and no takeover of system shortcuts (§5).
- 7z with a password is not in v1.
- Split into fxfm-core (portable to Linux) and Tauri UI (§10a).
- **Universal build (arm64 + x86_64)**, because the owner may carry fxfm to other Macs and it is not known whether all are Apple Silicon. All C libraries are built as universal.
- No import from Total Commander (the owner explicitly does not need it).
- A server or archive is not restored automatically at launch (the panel goes to a local folder), and a nonexistent path falls back to the nearest parent.

## 12. Left to the builder

- Choice of crates. Pure Rust crates or mature bindings that link statically into the bundle are preferred:
  - SFTP/SCP: e.g. `ssh2` with vendored OpenSSL or `russh`
  - FTP/FTPS: e.g. `suppaftp`
  - archives: `zip`, `tar`, `flate2`, `sevenz-rust2`
- Architecture of the VFS layer (a common trait for local, archive and remote is expected).
- Config file format, cryptography for the master password (standard KDF + AEAD).
- File icons.
- Code and test organization (modeled on fxmp).
