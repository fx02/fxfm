//! Finder's icons for the panel rows (Options → View → Icons: Finder). The UI asks once per
//! icon key and caches the result:
//! - `dir`: the plain folder icon;
//! - `file`: the plain document icon (no extension);
//! - `ext:<extension>`: the icon of that file type;
//! - `path:<absolute path>`: the icon of that item itself (applications and other bundles);
//! - `sys:<name>`: a macOS system icon from `CoreTypes.bundle` (folder icons: Desktop,
//!   Downloads, Smart folder…), read from this Mac, never shipped with fxfm.

use std::collections::HashMap;

/// Side of the bitmap in pixels: rows show icons at about 16 points, so 32 covers Retina.
const SIZE: f64 = 32.0;

/// PNG data URLs for `keys`; a key whose icon cannot be made is left out.
#[tauri::command]
pub async fn file_icons(keys: Vec<String>) -> HashMap<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        keys.into_iter()
            .filter_map(|k| native::png(&k).map(|png| (k, data_url(&png))))
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn data_url(png: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

/// Where macOS keeps its system icons.
const CORE_TYPES: &str = "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources";

/// The `.icns` file of a `sys:` key: letters and digits only, and it must exist.
fn system_icon(key: &str) -> Option<std::path::PathBuf> {
    let name = key.strip_prefix("sys:")?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let p = std::path::Path::new(CORE_TYPES).join(format!("{name}.icns"));
    p.is_file().then_some(p)
}

/// The file type NSWorkspace is asked about for a key, or None for `path:` and unknown keys.
fn file_type(key: &str) -> Option<&str> {
    match key {
        "dir" => Some("public.folder"),
        "file" => Some("public.data"),
        k => k
            .strip_prefix("ext:")
            .filter(|e| !e.is_empty() && !e.contains('/')),
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{file_type, system_icon, SIZE};
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    pub fn png(key: &str) -> Option<Vec<u8>> {
        let ws = NSWorkspace::sharedWorkspace();
        let img = if key.starts_with("sys:") {
            let file = system_icon(key)?;
            NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(file.to_str()?))?
        } else if let Some(path) = key.strip_prefix("path:") {
            if !path.starts_with('/') || std::fs::symlink_metadata(path).is_err() {
                return None;
            }
            ws.iconForFile(&NSString::from_str(path))
        } else {
            // The UTType-based replacement needs another framework binding for the same icon.
            #[allow(deprecated)]
            ws.iconForFileType(&NSString::from_str(file_type(key)?))
        };
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE, SIZE));
        // SAFETY: `rect` is valid for the call; no context and no hints.
        let cg = unsafe { img.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
        // SAFETY: an empty properties dictionary.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(data.to_vec())
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub fn png(_key: &str) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_file_types() {
        assert_eq!(file_type("dir"), Some("public.folder"));
        assert_eq!(file_type("file"), Some("public.data"));
        assert_eq!(file_type("ext:pdf"), Some("pdf"));
        assert_eq!(file_type("ext:"), None);
        assert_eq!(file_type("ext:a/b"), None);
        assert_eq!(file_type("path:/Applications/Safari.app"), None);
        assert_eq!(file_type("other"), None);
    }

    #[test]
    fn folder_and_pdf_icons_are_png() {
        if !cfg!(target_os = "macos") {
            return;
        }
        for key in ["dir", "ext:pdf", "file"] {
            let png = native::png(key).expect(key);
            assert_eq!(&png[..4], b"\x89PNG", "{key}");
        }
        assert!(native::png("path:relative").is_none());
        let sys = native::png("sys:GenericFolderIcon").expect("system folder icon");
        assert_eq!(&sys[..4], b"\x89PNG");
        assert!(native::png("sys:../../etc/passwd").is_none());
        assert!(native::png("sys:NoSuchIcon").is_none());
        assert!(native::png("path:/no/such/file").is_none());
        assert!(data_url(b"x").starts_with("data:image/png;base64,"));
    }
}
