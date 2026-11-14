# fxfm

fxfm is a Total Commander-style dual-panel file manager for macOS (Rust + Tauri 2). The keys are TC's (F3–F8, Tab, Ctrl+D…, always the physical **Ctrl**, Option is Alt), and the menu lives in the macOS menu bar (Files, Mark, Commands, Net, Show, Configuration, Start, Help). It works with local disks, archives (as folders) and servers (SFTP, SCP, FTP, FTPS).

Requires macOS 15 (Sequoia) or later.

![Main window: two panels, a local website project on the left and music on the right](docs/screenshots/main.png)

| F3 plays audio in place | Ctrl+F: servers |
| --- | --- |
| ![F3 plays a wav file; the player strip sits above the F-key bar](docs/screenshots/audio.png) | ![Connect to server dialog with folders and servers](docs/screenshots/servers.png) |

## Build

You need Rust (the toolchain from `rust-toolchain.toml`, with the `aarch64-apple-darwin` and `x86_64-apple-darwin` targets) and `tauri-cli` 2 (`cargo install tauri-cli --version "^2"`).

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo tauri build --target universal-apple-darwin
```

Output (universal, runs on Apple Silicon and Intel Macs):

- app: `target/universal-apple-darwin/release/bundle/macos/fxfm.app`
- disk image: `target/universal-apple-darwin/release/bundle/dmg/fxfm_0.1.0_universal.dmg`

If the `.dmg` step crashes with "Finder is busy" (locked screen, SSH), run the build with `CI=true`: this skips Finder's DMG layout customization.

A plain `cargo tauri build` signs the app ad hoc (`signingIdentity` is `-` in `tauri.conf.json`). `scripts/build-macos.sh` builds with the maintainer's stable local identity "fxfm local signing" instead (kept in `~/.fxfm-signing/`, not in the repo): it unlocks that keychain, sets `APPLE_SIGNING_IDENTITY`, runs the universal build and verifies the signature and architectures. With a stable identity macOS remembers the granted folder access between builds; with an ad hoc signature it asks again after each rebuild.

Check: `lipo -archs target/universal-apple-darwin/release/bundle/macos/fxfm.app/Contents/MacOS/fxfm` prints `x86_64 arm64`.

Tests: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `node --test ui/`.

## Where the data lives

Settings are four JSON files: `settings.json`, `session.json`, `keymap.json` and `hotlist.json`. They can be edited by hand (while fxfm is not running).

- **Portable**: an `fxfm-data/` folder next to `fxfm.app`, if it exists and is writable.
- **System** (otherwise): `~/Library/Application Support/fxfm/`.

The mode is changed in Configuration → Options → Storage (Apply). fxfm then copies all settings to the new location. When switching from portable to system, the old folder stays as `fxfm-data.old`.

## Portable setup

Create a folder, e.g. on a USB drive:

```
fxfm/
├── fxfm.app
└── fxfm-data/      (empty folder)
```

On first launch fxfm detects `fxfm-data/` and stores everything there; the system folder is left alone.

## Moving to another Mac

Copy the whole `fxfm/` folder (USB, network, zip). macOS adds a *quarantine* attribute to the copied app. Because of this, Gatekeeper may block it, and *App Translocation* runs it from a random read-only path, where `fxfm-data/` is not visible.

fxfm detects this and offers:

- **Fix and restart**: removes the quarantine attribute and relaunches from the real location, with the settings from `fxfm-data/`;
- **Continue without settings**: keeps running, but without the portable settings, and the session (panels) is not saved.

The same by hand, in Terminal, before the first launch:

```sh
xattr -dr com.apple.quarantine /path/to/fxfm/fxfm.app
```

The app is signed with a local identity but not notarized (personal use). If Gatekeeper still rejects it, the simplest fix is to run the `xattr -dr com.apple.quarantine …` command above. Alternatively, try to open fxfm, then click "Open Anyway" in System Settings → Privacy & Security (on macOS 15, right-click → Open no longer bypasses Gatekeeper).

## Archives

Archives open as folders. Supported formats: `.zip`, `.tar`, `.tgz` / `.tar.gz`, `.gz` (only if it contains exactly one file) and `.7z`. Other formats (bz2, xz, rar, tbz2, txz) are opened by Enter with the default application.

- **Entering and leaving**: Enter on an archive enters it (archives on a server: see "Servers"); `[..]` at the archive root returns to the folder with the cursor on the archive. Nested archives are opened by Enter with the default application (not as folders).
- **Working inside an archive**: F3, F4, F5 and F6 (into and out of the archive), F7, F8 and F2. Shift+F4 is not available inside.
- **F8** deletes permanently: "Delete N file(s) from archive? This cannot be undone."
- **Copying into an archive** asks one conflict question per top-level item; overwriting a folder merges the contents into it.
- **Password**: for a password-protected zip, fxfm asks for it when the file is opened or extracted; a wrong password asks again.
- **F4 (edit)**: the file is edited as a copy in a temporary folder. After each save fxfm asks `"<name>" was changed. Upload it back into the archive?`. If the entry in the archive changed in the meantime, it asks once more before overwriting.
- **Unuploaded changes**: copies that were not uploaded back survive Cmd+Q and crashes. On the next launch fxfm lists them ("Edited copies from a previous session were not uploaded:"): **Show in Finder** opens them, **Keep** (default, and Esc) leaves them so the dialog shows up again on the next launch, and **Discard**, after confirmation ("Move these copies to the Trash?"), moves the copies to the Trash.
- **Writing**: every change writes a temporary archive next to the original, which replaces it only if everything succeeded. **7z** is fully repacked in the process (slower, needs free space next to the archive, the result is not solid).
- **Refusals**: encrypted archives are not modified; encrypted 7z cannot be read; 7z with unsafe entry names is not overwritten; a tar in which a hard link points to an entry being deleted or renamed is not modified; `.gz` has its own limitation (a single file).
- **Temporary files**: `~/Library/Caches/fxfm/tmp/<pid>/`. They are deleted on exit, except for unuploaded changes.

## Servers

**Ctrl+F** (Net → Connect to server) opens the server list: a tree of folders and servers, with the buttons *New connection…* and *New URL…*. Protocols: **SFTP**, **SCP** (default port 22), **FTP** and **FTPS** (21). *New URL…* connects once, without saving, e.g. `sftp://user@host:2222/var/www`; FTP without a user logs in as anonymous.

- **Location** is `<protocol>://<server name>/<path>`, e.g. `sftp://web-1/var/www/html`. The server name must be unique across the whole list. Connected servers appear as drives in the bar above the panels (☁️ name), and their address is in the header. The hotlist accepts the same locations (`cd sftp://<ime>/var/www` on the command line connects and opens).
- **Authentication**: password, key, or key + passphrase. A private key is picked from disk or imported (*Import…*): the copy goes to `keys/` in the data folder, with permissions 0600. Per-server options: a local folder that opens in the OTHER panel after connecting, filename encoding on FTP (UTF-8, Windows-1250, Latin-1), older SSH algorithms, FTP active/passive mode.
- **Host key**: on first connection fxfm shows the server's key fingerprint (the certificate for FTPS); accepted ones are remembered in `known_hosts` in the data folder. A changed key is not silently accepted: the dialog offers *Replace and connect* or Cancel.
- **Passwords** never go into `servers.json`.
  - *System mode*: macOS Keychain, service `fxfm`, account `<id servera>/password` or `<id servera>/passphrase`.
  - *Portable mode*: `secrets.json` in `fxfm-data/`, encrypted with a **master password** (Argon2id 64 MiB, XChaCha20-Poly1305). The master password is asked for once per launch, when needed, and is kept in memory only. Without it, passwords are not saved (a password that was entered but not saved is valid until exit, so reconnecting works).
- **Operations**: F3, F4, F5, F6, F7, F8 and F2 also work on a server, as does **chmod** (Files → Change attributes…, no default key; SFTP and SCP only). F8 deletes permanently: "Delete N file(s) from the server? This cannot be undone.". An upload writes to a temporary `.<ime>.fxfm-<pid>-<n>.part` next to the target and then replaces it, so an interruption never leaves a truncated original; on SFTP and SCP the existing file's permissions are kept (FTP and FTPS do not set permissions).
- **Symbolic links**: Enter on a link to a folder enters the folder, and F8 deletes only the link. Some FTP servers show a link to a folder as a plain folder; there, F8 on it deletes the contents of the target folder.
- **F4** on a file on a server edits a temporary copy; after saving, fxfm asks `"<name>" was changed. Upload it back to the server?`.
- **Archives on a server**: Enter on an archive downloads it as an F4 copy and enters it. Changes inside (F5, F8, F2…) modify the copy, so fxfm offers to upload it back to the server. `[..]` at the archive root returns to the folder on the server.
- **Ctrl+Shift+F** (Net → Disconnect) closes the active panel's connection; panels on that server go to the server's local folder or to home. Temporary copies of that server's files (without unuploaded changes) are deleted.
- **Connection lost**: connecting takes at most 15 s, and a transfer or request with no progress 60 s; SSH sends a keepalive every 30 s, and FTP sends `NOOP` after 30 s of inactivity. A dead connection is reported with the dialog `Connection to <ime> lost` with the buttons **Reconnect** (the panels re-list their folders) and **Close**.
- **Data**: `servers.json` (the tree, no passwords), `known_hosts`, `keys/` and (portable) `secrets.json`, in the data folder.

## F3 audio preview

F3 on an audio file plays it immediately, in the main window; focus stays in the panel. The bar above the F-keys shows the name, `m:ss / m:ss` and the codec; clicking the bar seeks to that position. F3 on the same file or Esc stops it. It also works in archives and on servers. Formats: wav (PCM, float, A-law, μ-law, ADPCM, GSM 6.10), mp3, flac, ogg, opus, m4a, aiff.

The output device is chosen in Options → Audio → *Output device* (default *System default*); a change takes effect immediately, including for what is currently playing. If the chosen device is not connected, playback uses the default one, and the bar reports it once.

## Drag & drop

- **Out**: dragging the selected items (or the row under the pointer) from a panel into other applications, e.g. Finder. Copy only, and only for local files.
- **In**: from Finder into a panel.
- **Between panels**: dropping opens the F5 dialog with the target = the folder under the pointer or the panel's folder. **Shift** while dropping gives F6 (move). The dialog also has a *Move instead* / *Copy instead* button, in case you miss Shift.

## Notes

- **Trash (F8)**: fxfm moves items to the Trash via `NSFileManager`, not via Finder. It is faster and does not need Automation permission, but Finder's "Put Back" does not work for items deleted by fxfm.
- **PDF in the F3 viewer**: the PDF is shown in an iframe via the asset protocol, with an "Open in Quick Look" button always visible next to it. Whether WKWebView really renders the PDF in the iframe has not yet been checked by hand; if not, Quick Look is the fallback.
- **Full Disk Access**: for some folders (Mail, Messages, Safari…) you need to enable fxfm in System Settings → Privacy & Security → Full Disk Access. fxfm explains this the first time it hits a denial.
- **Keys macOS takes**: Ctrl+arrows (Mission Control, Spaces) and Ctrl+Space (input source) are intercepted by macOS before fxfm. Configuration → Keyboard warns if such a key is assigned and offers to open System Settings → Keyboard → Keyboard Shortcuts.
- Cmd is not used for TC commands; the exceptions are Cmd+Q and the standard Edit menu (Cmd+C/V/X/A in text fields).

## Default keys

The table is generated from the command registry (`COMMANDS` in `crates/core/src/keymap.rs`); the test `readme_key_table_matches_commands` fails if it goes stale and prints the new one. Keys are changed in Configuration → Keyboard ("Reset to TC defaults" restores these).

<!-- keys:start -->
| Menu | Command | Keys |
|---|---|---|
| Files | View | `F3` |
| Files | Edit | `F4` |
| Files | Edit new file | `Shift+F4` |
| Files | Copy | `F5` |
| Files | Move | `F6` |
| Files | Rename | `F2`, `Shift+F6` |
| Files | New folder | `F7` |
| Files | Delete | `F8`, `Delete` |
| Files | Delete permanently | `Shift+F8`, `Shift+Delete` |
| Files | Exit | `Alt+F4` |
| Mark | Mark/unmark | `Space`, `Insert` |
| Mark | Select group | `NumAdd`, `Ctrl+=` |
| Mark | Unselect group | `NumSubtract`, `Ctrl+-` |
| Mark | Invert selection | `NumMultiply`, `Ctrl+8` |
| Mark | Select all | `Ctrl+A` |
| Commands | Switch panel | `Tab` |
| Commands | Directory hotlist | `Ctrl+D` |
| Commands | Parent directory | `Backspace`, `Ctrl+PageUp` |
| Commands | Root directory | `Ctrl+\` |
| Commands | Swap panels | `Ctrl+U` |
| Commands | Target = Source | — |
| Commands | Search (later version) | `Alt+F7` |
| Files | Multi-rename (later version) | `Ctrl+M` |
| Files | Change attributes… | — |
| Net | Connect to server | `Ctrl+F` |
| Net | Disconnect | `Ctrl+Shift+F` |
| Show | Left drive | `Alt+F1` |
| Show | Right drive | `Alt+F2` |
| Show | Refresh | `Ctrl+R` |
| Show | Show hidden files | `F1`, `Ctrl+H` |
| Commands | Command line | `Ctrl+Enter` |
| Configuration | Keyboard… | — |
| Configuration | Options… | — |
<!-- keys:end -->

Fixed keys in a panel (not remappable): Up/Down, PageUp/PageDown, Home/End, **Left = first visible row in the window**, **Right = last visible row in the window**; if the cursor is already there, the next press moves by a whole window (the first row of the previous window, or the last row of the next one), Enter.
