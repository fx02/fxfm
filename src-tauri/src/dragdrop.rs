//! Drag & drop: the native drag-out of panel files (`drag` crate) and the modifier state
//! the UI reads when a drop arrives (the drop event itself carries none).

use fxfm_core::archive::split_location;
use fxfm_core::remote::parse_remote;
use serde::Serialize;
use std::path::PathBuf;

/// Error of `start_drag` for archive and server entries (also the UI's status text).
pub const NOT_LOCAL: &str = "Dragging out of archives and servers is not supported yet";

/// Checks the paths of a drag-out: absolute local paths that exist (symlinks included),
/// not on a server and not inside an archive (an archive file itself is a local file).
pub fn validate_paths(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() {
        return Err("Nothing to drag".into());
    }
    if paths.iter().any(|p| {
        parse_remote(p).is_some() || split_location(p).is_some_and(|a| !a.inner.is_empty())
    }) {
        return Err(NOT_LOCAL.into());
    }
    paths
        .iter()
        .map(|p| {
            // Raw segments: `Path::components` silently drops inner "." components.
            let dotted = p.split('/').any(|c| c == "." || c == "..");
            if !p.starts_with('/') || dotted {
                return Err(format!("Not a local path: {p}"));
            }
            let path = PathBuf::from(p);
            match path.symlink_metadata() {
                Ok(_) => Ok(path),
                Err(_) => Err(format!("Not found: {p}")),
            }
        })
        .collect()
}

/// `drag://ended` payload: `result` is "dropped", "cancelled" or "error" (with `message`).
#[derive(Serialize, Clone, Debug)]
pub struct DragEnded {
    pub result: &'static str,
    pub message: Option<String>,
}

/// Modifier keys held right now (read when a drop arrives).
#[derive(Serialize, Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
}

/// Starts a native file drag of `paths` (files the panel listed) out of the window.
/// Returns once the drag is queued on the main thread; `drag://ended` reports how it ended.
#[tauri::command]
pub fn start_drag(window: tauri::WebviewWindow, paths: Vec<String>) -> Result<(), String> {
    let files = validate_paths(&paths)?;
    native::start(window, files)
}

/// The modifier keys held at this moment (the drop event carries none).
#[tauri::command]
pub fn drag_modifiers() -> Modifiers {
    native::modifiers()
}

#[cfg(target_os = "macos")]
mod native {
    use super::{DragEnded, Modifiers};
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventModifierFlags, NSWorkspace,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use std::path::PathBuf;
    use tauri::{Emitter, WebviewWindow};

    /// Shown when the file icon cannot be rendered.
    const FALLBACK_ICON: &[u8] = include_bytes!("../icons/32x32.png");
    /// Drag image size in points.
    const ICON_SIZE: f64 = 64.0;

    pub fn start(window: WebviewWindow, files: Vec<PathBuf>) -> Result<(), String> {
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                // The button went up before the drag got here: a session started now
                // would stick to the pointer until the next click.
                if NSEvent::pressedMouseButtons() & 1 == 0 {
                    let ended = DragEnded {
                        result: "cancelled",
                        message: None,
                    };
                    let _ = w.emit("drag://ended", ended);
                    return;
                }
                let image = icon_png(&files)
                    .map(drag::Image::Raw)
                    .unwrap_or_else(|| drag::Image::Raw(FALLBACK_ICON.to_vec()));
                let ended = w.clone();
                let r = drag::start_drag(
                    &w,
                    drag::DragItem::Files(files),
                    image,
                    move |result, _| {
                        let result = match result {
                            drag::DragResult::Dropped => "dropped",
                            drag::DragResult::Cancel => "cancelled",
                        };
                        let _ = ended.emit(
                            "drag://ended",
                            DragEnded {
                                result,
                                message: None,
                            },
                        );
                    },
                    // Copy only: receivers never move (delete) the panel's files.
                    drag::Options::default(),
                );
                if let Err(e) = r {
                    let message = Some(e.to_string());
                    let _ = w.emit(
                        "drag://ended",
                        DragEnded {
                            result: "error",
                            message,
                        },
                    );
                }
            })
            .map_err(|e| e.to_string())
    }

    /// Finder's icon of the file (one) or of the selection (several) as PNG bytes.
    fn icon_png(files: &[PathBuf]) -> Option<Vec<u8>> {
        let ws = NSWorkspace::sharedWorkspace();
        let names: Vec<_> = files
            .iter()
            .map(|p| NSString::from_str(&p.to_string_lossy()))
            .collect();
        let img = match names.as_slice() {
            [one] => ws.iconForFile(one),
            _ => ws.iconForFiles(&NSArray::from_retained_slice(&names))?,
        };
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(ICON_SIZE, ICON_SIZE));
        // SAFETY: `rect` is a valid pointer for the call; no hints.
        let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        rep.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
        // SAFETY: an empty properties dictionary.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(data.to_vec())
    }

    pub fn modifiers() -> Modifiers {
        let flags = NSEvent::modifierFlags_class();
        Modifiers {
            shift: flags.contains(NSEventModifierFlags::Shift),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::Modifiers;
    use std::path::PathBuf;

    pub fn start(_window: tauri::WebviewWindow, _files: Vec<PathBuf>) -> Result<(), String> {
        Err("Dragging files out is only supported on macOS".into())
    }

    pub fn modifiers() -> Modifiers {
        Modifiers::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn accepts_existing_local_files_and_folders() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir(t.path().join("dir")).unwrap();
        let paths = vec![s(&t.path().join("a.txt")), s(&t.path().join("dir"))];
        let got = validate_paths(&paths).unwrap();
        assert_eq!(got, vec![t.path().join("a.txt"), t.path().join("dir")]);
    }

    #[test]
    fn an_archive_file_itself_is_local() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        let p = s(&t.path().join("x.zip"));
        assert_eq!(validate_paths(&[p]).unwrap(), vec![t.path().join("x.zip")]);
    }

    #[test]
    fn a_dangling_symlink_is_still_draggable() {
        let t = tempfile::tempdir().unwrap();
        let link = t.path().join("gone");
        std::os::unix::fs::symlink(t.path().join("nowhere"), &link).unwrap();
        assert!(validate_paths(&[s(&link)]).is_ok());
    }

    #[test]
    fn refuses_nothing() {
        assert_eq!(validate_paths(&[]).unwrap_err(), "Nothing to drag");
    }

    #[test]
    fn refuses_missing_paths() {
        let t = tempfile::tempdir().unwrap();
        let p = s(&t.path().join("missing.txt"));
        assert_eq!(
            validate_paths(&[p.clone()]).unwrap_err(),
            format!("Not found: {p}")
        );
    }

    #[test]
    fn refuses_archive_entries() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        let ok = s(&t.path().join("x.zip"));
        let inner = s(&t.path().join("x.zip/inner.txt"));
        assert_eq!(validate_paths(&[ok, inner]).unwrap_err(), NOT_LOCAL);
    }

    #[test]
    fn refuses_server_locations() {
        for loc in [
            "sftp://user@host/home/a.txt",
            "ftp://host/a",
            "scp://h/x",
            "ftps://h/y",
        ] {
            assert_eq!(
                validate_paths(&[loc.to_string()]).unwrap_err(),
                NOT_LOCAL,
                "{loc}"
            );
        }
    }

    #[test]
    fn refuses_relative_and_dotted_paths() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d")).unwrap();
        for p in [
            "a.txt".to_string(),
            "".to_string(),
            format!("{}/d/..", s(t.path())),
            format!("{}/./d", s(t.path())),
        ] {
            let e = validate_paths(&[p.clone()]).unwrap_err();
            assert_eq!(e, format!("Not a local path: {p}"));
        }
    }
}
