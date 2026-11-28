# Linux (Debian) port — plan

**Goal:** fxfm as Debian packages for amd64 and arm64 with everything the macOS app does,
where Linux has an equivalent. Owner's decision (2026-11-27): both architectures, full
feature parity, new dependencies allowed for it. The macOS app must not change behavior.

**Build:** `scripts/build-linux.sh` builds `.deb` packages in Docker (Rancher Desktop,
`scripts/linux/Dockerfile`: Debian 12, WebKitGTK 4.1, the project's Rust toolchain, Tauri CLI);
arm64 runs natively on the M2, amd64 under emulation. Cargo caches and target dirs live in
Docker volumes per architecture, and the packages are copied to `~/Documents/PVT/fxfm-builds/linux/`.

**Verification:**
- `cargo test --workspace` and `node --test ui/` run in the container.
- Smoke test: install the `.deb` in a clean Debian container with Xvfb, start fxfm with a temp HOME, and take a screenshot of the window.
- The macOS checks (clippy, tests, build) stay green after every phase.

## Phase 1 — it builds and starts on Debian

- Compile errors on Linux get fixed with `cfg` gates. Every macOS-only feature returns "not available" instead of failing.
- Platform paths:
  - log: `$XDG_STATE_HOME/fxfm` (else `~/.local/state/fxfm`);
  - data: `dirs::data_dir()` (`~/.local/share/fxfm`), already the case;
  - cache: `dirs::cache_dir()`.
- `tauri.conf.json`: the deb bundle (dependencies, desktop entry, icons, category).
- `scripts/build-linux.sh` and the smoke test above.

## Phase 2 — Linux equivalents (one module at a time, each with tests where it can run headless)

| macOS piece (module) | Linux equivalent |
|---|---|
| Trash with Undo (`ops::trash`, `undo`) | freedesktop Trash (`trash` crate); the trashed item's path for Undo from `$XDG_DATA_HOME/Trash/info` |
| Clipboard with files (`clipboard.rs`) | GTK clipboard: `text/uri-list` + `x-special/gnome-copied-files` (Cut/Copy/Paste with Nautilus, Dolphin, Thunar) |
| Finder icons, folder icons (`icons.rs`) | GTK icon theme via `gio` content types |
| Fixed-pitch font list (`fonts.rs`) | fontconfig (`fc-list :spacing=mono family`) |
| Open with… (`openwith.rs`) | `gio::AppInfo::all_for_type` / launch |
| Show in Finder | `org.freedesktop.FileManager1.ShowItems` (D-Bus), else open the folder |
| Quick Look | GNOME Sushi (`org.gnome.NautilusPreviewer`) when present, else F3 / default app |
| Keychain (`keychain.rs`) | Secret Service through `secret-tool` (libsecret-tools; the secret on stdin, never an argument) |
| SMB mount, unmount, mount list, Shares… (`smb.rs`) | gvfs: gio `mount_enclosing_volume` with a `MountOperation` answering the login in code, mounts under `/run/user/<uid>/gvfs`, `/proc/self/mounts` for kernel cifs; unmount with `gio mount -u` / `umount`; Shares… via `smbclient -L` when installed; Bonjour via Avahi |
| Volumes and the drive menu (`volumes.rs`, `volwatch.rs`) | `/proc/self/mounts` (`/media`, `/run/media`, `/mnt`, network types) and gvfs shares, polled every 2 s |
| Live index (FSEvents, `search/live.rs`) | inotify (`notify` crate): the home folder for the startup disk, the whole root for other local volumes; the timer refreshes the rest (and anything a watch cannot cover) |
| Drag out of fxfm (`dragdrop.rs`) | GTK drag source with `text/uri-list` (archive and server items are extracted first, since Linux has no file promises) |
| Native drag image, Fn key, App Translocation | not on Linux: hidden |
| Menu bar | GTK menu bar in the window (Tauri), with the same commands |

## Phase 3 — release

- Smoke test of each Linux feature in the container (screenshots), plus manual checks on a real Debian desktop (owner).
- README/MANUAL Linux notes, a Linux section in the releases (`.deb` for both architectures).
