//! The system clipboard for the panels' Copy / Cut / Paste and the Copy names / Copy paths
//! commands: the general pasteboard on macOS, the GTK clipboard on Linux. Local files go on it
//! as file URLs (Finder, Nautilus, Dolphin… can paste them, and files copied there paste into
//! a panel); everything also goes on it as text.

use serde::Serialize;

/// What `clipboard_files` found: the pasteboard's change count and the local files on it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub change: i64,
    pub files: Vec<String>,
}

/// Puts `text` on the clipboard, plus `files` as file URLs when every one is a local file or
/// folder (not inside an archive, not on a server). Returns the new change count, by which the
/// UI later tells whether the clipboard still holds what it put there.
#[tauri::command]
pub async fn clipboard_write(app: tauri::AppHandle, text: String, files: Vec<String>) -> i64 {
    // The files are checked off the main thread: a stat on a share that went away blocks.
    let files = tauri::async_runtime::spawn_blocking(move || local_files(&files))
        .await
        .unwrap_or_default();
    #[cfg(target_os = "linux")]
    return linux::on_main(&app, move || linux::write(&text, &files))
        .await
        .unwrap_or(0);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        native::write(&native::general(), &text, &files)
    }
}

/// The clipboard's change count and the local files on it (copied in a file manager or by fxfm).
#[tauri::command]
pub async fn clipboard_files(app: tauri::AppHandle) -> Board {
    #[cfg(target_os = "linux")]
    return linux::on_main(&app, linux::read).await.unwrap_or(Board {
        change: 0,
        files: Vec::new(),
    });
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        native::read(&native::general())
    }
}

/// A file list as `text/uri-list` lines (`file://` URIs, CRLF-separated, as the spec asks).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn uri_list(files: &[String]) -> String {
    files
        .iter()
        .map(|f| file_uri(f))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// `file://` URI of an absolute path, bytes outside the unreserved set and `/` percent-encoded.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Local paths from `text/uri-list` or `x-special/gnome-copied-files` data (the latter starts
/// with a `copy` or `cut` line); comments, other schemes and remote hosts are left out.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn paths_from_uris(data: &str) -> Vec<String> {
    data.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && *l != "copy" && *l != "cut")
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            path.starts_with('/').then(|| percent_decode(path))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1..i + 3].iter().all(u8::is_ascii_hexdigit) {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Linux has no change counter: the "change" is a fingerprint of what is on the clipboard (its
/// files, else its text), so fxfm still tells its own Copy/Cut from anything copied since.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn fingerprint(files: &[String], text: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if files.is_empty() {
        ("t", text).hash(&mut h);
    } else {
        ("f", files).hash(&mut h);
    }
    // Within JavaScript's exact integers.
    (h.finish() & ((1u64 << 52) - 1)) as i64
}

/// `files` when all are existing local items, else none (they cannot become file URLs).
fn local_files(files: &[String]) -> Vec<String> {
    let local = |f: &String| match fxfm_core::vfs::parse(f) {
        fxfm_core::vfs::Loc::Local(p) => p.is_absolute() && std::fs::symlink_metadata(p).is_ok(),
        fxfm_core::vfs::Loc::Archive(a) => a.inner.is_empty(),
        fxfm_core::vfs::Loc::Remote(_) => false,
    };
    if files.iter().all(local) {
        files.to_vec()
    } else {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::Board;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::ClassType;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSString, NSURL};

    pub fn general() -> Retained<NSPasteboard> {
        NSPasteboard::generalPasteboard()
    }

    pub fn write(pb: &NSPasteboard, text: &str, files: &[String]) -> i64 {
        pb.clearContents();
        if !files.is_empty() {
            let urls: Vec<_> = files
                .iter()
                .map(|f| NSURL::fileURLWithPath(&NSString::from_str(f)))
                .collect();
            let writers: Vec<_> = urls
                .iter()
                .map(|u| ProtocolObject::from_ref(&**u))
                .collect();
            pb.writeObjects(&NSArray::from_slice(&writers));
        }
        // SAFETY: a static AppKit constant.
        pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        pb.changeCount() as i64
    }

    pub fn read(pb: &NSPasteboard) -> Board {
        let change = pb.changeCount() as i64;
        let classes = NSArray::from_slice(&[NSURL::class()]);
        // SAFETY: an array of classes and no options, as the method expects.
        let found = unsafe { pb.readObjectsForClasses_options(&classes, None) };
        let files = found
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| o.downcast::<NSURL>().ok())
                    .filter(|u| u.isFileURL())
                    .filter_map(|u| u.path().map(|p| p.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Board { change, files }
    }

    #[cfg(test)]
    pub fn scratch() -> Retained<NSPasteboard> {
        NSPasteboard::pasteboardWithUniqueName()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{fingerprint, paths_from_uris, uri_list, Board};
    use gtk::gdk;

    const GNOME: &str = "x-special/gnome-copied-files";
    const URIS: &str = "text/uri-list";

    /// Runs `f` on the main thread (GTK) and waits for its result.
    pub async fn on_main<T: Send + 'static>(
        app: &tauri::AppHandle,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || drop(tx.send(f()))).ok()?;
        tauri::async_runtime::spawn_blocking(move || rx.recv().ok())
            .await
            .ok()
            .flatten()
    }

    fn clipboard() -> gtk::Clipboard {
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD)
    }

    pub fn write(text: &str, files: &[String]) -> i64 {
        let cb = clipboard();
        if files.is_empty() {
            cb.set_text(text);
            cb.store();
            return fingerprint(files, text);
        }
        let targets = [
            gtk::TargetEntry::new(GNOME, gtk::TargetFlags::empty(), 0),
            gtk::TargetEntry::new(URIS, gtk::TargetFlags::empty(), 1),
            gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 2),
            gtk::TargetEntry::new("text/plain;charset=utf-8", gtk::TargetFlags::empty(), 2),
            gtk::TargetEntry::new("STRING", gtk::TargetFlags::empty(), 2),
        ];
        let uris = uri_list(files);
        let gnome = format!("copy\n{}", uris.replace("\r\n", "\n"));
        let text = text.to_string();
        cb.set_with_data(&targets, move |_, sel, info| match info {
            0 => sel.set(&gdk::Atom::intern(GNOME), 8, gnome.as_bytes()),
            1 => sel.set(&gdk::Atom::intern(URIS), 8, uris.as_bytes()),
            _ => {
                sel.set_text(&text);
            }
        });
        fingerprint(files, "")
    }

    pub fn read() -> Board {
        let cb = clipboard();
        let data = |target: &str| {
            cb.wait_for_contents(&gdk::Atom::intern(target))
                .map(|d| String::from_utf8_lossy(&d.data()).into_owned())
        };
        let files = data(GNOME)
            .or_else(|| data(URIS))
            .map(|d| paths_from_uris(&d))
            .unwrap_or_default();
        let text = if files.is_empty() {
            cb.wait_for_text()
                .map(|t| t.to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        Board {
            change: fingerprint(&files, &text),
            files,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod native {
    use super::Board;
    pub struct Pb;
    pub fn general() -> Pb {
        Pb
    }
    pub fn write(_: &Pb, _: &str, _: &[String]) -> i64 {
        0
    }
    pub fn read(_: &Pb) -> Board {
        Board {
            change: 0,
            files: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_round_trip_local_paths() {
        let files = vec!["/home/me/a b.txt".to_string(), "/tmp/č%.md".to_string()];
        let list = uri_list(&files);
        assert_eq!(
            list,
            "file:///home/me/a%20b.txt\r\nfile:///tmp/%C4%8D%25.md"
        );
        assert_eq!(paths_from_uris(&list), files);
        let gnome = format!("cut\n{}", list.replace("\r\n", "\n"));
        assert_eq!(paths_from_uris(&gnome), files);
        assert_eq!(
            paths_from_uris("# comment\nhttp://x/y\nfile://host/z\nfile://localhost/ok\n"),
            ["/ok"]
        );
        assert_eq!(paths_from_uris("file:///bad%2"), ["/bad%2"]);
    }

    #[test]
    fn the_fingerprint_follows_the_content() {
        let a = fingerprint(&["/a".into()], "");
        assert_eq!(
            a,
            fingerprint(&["/a".into()], "ignored when there are files")
        );
        assert_ne!(a, fingerprint(&["/b".into()], ""));
        assert_ne!(fingerprint(&[], "x"), fingerprint(&[], "y"));
        assert!((0..1i64 << 52).contains(&a));
    }

    #[test]
    fn only_local_items_become_file_urls() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a b.txt");
        std::fs::write(&f, b"x").unwrap();
        let local = f.to_string_lossy().into_owned();
        assert_eq!(
            local_files(std::slice::from_ref(&local)),
            vec![local.clone()]
        );
        assert!(local_files(&[local.clone(), "sftp://srv/x".into()]).is_empty());
        assert!(local_files(&["relative".into()]).is_empty());
        assert!(local_files(&[t.path().join("missing").to_string_lossy().into()]).is_empty());
    }

    /// On a private pasteboard: the user's clipboard is never touched.
    #[cfg(target_os = "macos")]
    #[test]
    fn files_and_text_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a b.txt");
        std::fs::write(&f, b"x").unwrap();
        let path = std::fs::canonicalize(&f)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let pb = native::scratch();
        let change = native::write(&pb, "names", std::slice::from_ref(&path));
        assert_eq!(
            native::read(&pb),
            Board {
                change,
                files: vec![path]
            }
        );
        let change = native::write(&pb, "only text", &[]);
        assert_eq!(
            native::read(&pb),
            Board {
                change,
                files: vec![]
            }
        );
    }
}
