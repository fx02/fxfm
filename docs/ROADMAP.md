# fxfm roadmap

## Next
- **App icon**: smaller, regular-weight (not bold) "fxfm" text, so the play triangle sits closer to the middle (`src-tauri/icons/gen_app_icon.py`).
- **Pretraga (Alt+F7)**: fxfm has its **own file index**, not Spotlight (`mdfind`). Spotlight skips hidden files and dotfiles, network drives and archive contents. To settle before planning: which folders get indexed, where the index is stored (in portable mode it travels with `fxfm-data/`, so it can grow), background refresh (FSEvents).
- **Multi-rename (Ctrl+M)**.

## Servers (after plan 3)
- Connecting (15 s) cannot be interrupted (no Cancel).
- F4 upload to a server whose connection was lost in the meantime: after Reconnect the upload is not offered again, and the file has to be saved once more in the editor.
- The Keychain code is tested only by hand (tests do not touch the real Keychain): it needs a manual check on a real Mac.
- Dragging items from archives and servers into other applications (see Drag & drop).
- While a panel is in an archive from a server, the drive bar marks the temporary volume.
- Downloading an archive from a server has no progress or Cancel.
- Inside an archive from a server, the default F5/F6 target and "add current folder" in the hotlist use the temporary path.

## Audio (F3)
- With *System default*, a change of the default output device in macOS (headphones, AirPods) is not noticed until the player has been idle. A device chosen in Options takes effect immediately.
- A chosen device that is not connected: playback uses the default; when the device reconnects, it is used only after the output is closed (60 s without playing) or fails.
- Devices are identified by name: if two share a name, the first is used (Options shows the name once).
- Remaining: G.726 and other WAV codecs besides PCM/float/A-law/mu-law/MS and IMA ADPCM/GSM 6.10/MP3 are reported as unsupported; there is no pause or volume control (plays at 100 %).

## Archives (after plan 2)
- F4 upload back does not work for an archive located in a folder whose name looks like an archive (e.g. `backup.zip/real.tar`); the copy is preserved. Fix: remember the archive's real path when opening it and compare against that on upload.
- Nested archives, tar.bz2/tar.xz, per-file (rather than per-item) conflicts when copying into an archive.

## Pack files (Alt+F5)
- Other formats: gz, tar, tgz, 7z (the dialog's format list has only ZIP for now).
- Adding to an existing archive (today an existing target can only be overwritten).
- Packing from archives and servers, and into a folder on a server.
- The password is wiped on the Rust side; the JavaScript strings it passed through cannot be wiped and live until garbage collection.

## Keyboard
- **Fn as Alt or Ctrl** is not offered: WebKit's `KeyboardEvent.getModifierState("Fn")` is always false (Source/WebCore/dom/UIEventWithKeyState.cpp lists Fn as a FIXME), and macOS sets its "function" flag on every arrow, F-key and Home/End event anyway. It would need a native NSEvent monitor that tracks the Fn key itself and hands its state to the webview before the keydown, plus an answer for keyboards where the F-keys need Fn.
- Viewer windows take the Alt/Ctrl keys in use when they open; a change applies to viewers opened after it.

## Drag & drop
- Dragging from archives and servers into other applications (Finder…) is not supported yet: it works only between panels. Fix: `NSFilePromiseProvider` (the file is extracted/downloaded only when the application receives it).
- The drag image is Finder's icon (of one file or of a group), without a file count.
- Dragging out always offers copy only (the `drag` crate knows only one operation), so Finder copies, not moves, even on the same disk.

## Done
- **Plan 3 – serveri** (2026-11-14): SFTP/SCP/FTP/FTPS (Ctrl+F), per-server keys (import into `keys/`), master password in portable mode and Keychain in system mode, `known_hosts`, F3–F8/F2 and chmod on a server, safe upload (`.part` then replace), Connection lost + Reconnect, hotlist with servers, archives on a server (Enter downloads a tracked copy, changes are offered for upload back).
- **Drag & drop** (2026-11-14): dragging rows (the selected items if the pressed row is selected, otherwise just that one; never `..`) from a panel into other applications as a real macOS file drag (`drag` crate, local files only), from Finder into a panel, and between panels. Dropping opens the F5 dialog (Shift = F6) with the target = the folder under the pointer or the panel's folder; the dialog also has a "Move instead" / "Copy instead" button. Same folder, the dragged item itself, or a folder inside the dragged one: nothing happens. The hotlist dialog reorders rows with the mouse (HTML5 drag & drop does not work with the Tauri drag-drop handler).
- **F3 audio preview** (2026-11-13): F3 on an audio file plays immediately in the main window, focus stays in the panel; the bar above the F-keys (name, `m:ss / m:ss`, codec, click on the bar = seek); F3 on the same file or Esc stops it. Decoding in Rust (`symphonia` + `symphonia-opus`, own GSM 6.10 and `.au`): wav (PCM, float, A-law, mu-law, MS/IMA ADPCM, GSM WAV49, MP3-in-WAV), mp3, flac, ogg/vorbis, opus, m4a/aac, aiff, au, gsm. Also works in archives and on servers (the temporary copy is deleted when playback ends).
