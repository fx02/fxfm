//! The macOS clipboard (general pasteboard) for the panels' Copy / Cut / Paste and the
//! Copy names / Copy paths commands. Local files go on it as file URLs (Finder can paste them,
//! and files copied in Finder paste into a panel); everything also goes on it as text.

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
pub fn clipboard_write(text: String, files: Vec<String>) -> i64 {
    native::write(&native::general(), &text, &local_files(&files))
}

/// The clipboard's change count and the local files on it (copied in Finder or by fxfm).
#[tauri::command]
pub fn clipboard_files() -> Board {
    native::read(&native::general())
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

#[cfg(not(target_os = "macos"))]
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
