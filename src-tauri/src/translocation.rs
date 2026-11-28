//! macOS App Translocation: a quarantined app that was not moved by the
//! Finder runs from a random read-only copy, so `fxfm-data/` beside the real
//! `fxfm.app` cannot be found. Detection and the one-click quarantine fix.
// macOS only; the other systems use none of this.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};

/// The executable runs from an App Translocation mount.
pub fn is_translocated(exe: &Path) -> bool {
    exe.to_string_lossy().contains("/AppTranslocation/")
}

/// The `.app` bundle containing `exe` (or `exe` itself outside a bundle).
fn bundle_of(exe: &Path) -> &Path {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .unwrap_or(exe)
}

/// Where the translocated app really is (the `.app` the user started).
#[cfg(target_os = "macos")]
pub fn original_path(exe: &Path) -> Option<PathBuf> {
    use core_foundation::base::TCFType;
    use core_foundation::error::{CFError, CFErrorRef};
    use core_foundation::url::{CFURLRef, CFURL};

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecTranslocateCreateOriginalPathForURL(
            translocated_path: CFURLRef,
            error: *mut CFErrorRef,
        ) -> CFURLRef;
    }

    if !is_translocated(exe) {
        return None;
    }
    let url = CFURL::from_path(bundle_of(exe), true)?;
    let mut error: CFErrorRef = std::ptr::null_mut();
    // SAFETY: `url` is a valid CFURL for the call; the returned URL and the
    // error follow the Create rule, so ownership passes to the wrappers.
    let original = unsafe {
        let r = SecTranslocateCreateOriginalPathForURL(url.as_concrete_TypeRef(), &mut error);
        if !error.is_null() {
            let e = CFError::wrap_under_create_rule(error);
            eprintln!("fxfm: SecTranslocateCreateOriginalPathForURL: {e}");
        }
        if r.is_null() {
            return None;
        }
        CFURL::wrap_under_create_rule(r)
    };
    original.to_path()
}

#[cfg(not(target_os = "macos"))]
pub fn original_path(_exe: &Path) -> Option<PathBuf> {
    None
}

/// Removes the quarantine attribute from the original app, starts it again
/// from there and quits this (translocated) instance.
#[tauri::command]
pub fn fix_quarantine(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let original = original_path(&exe).ok_or("cannot find where fxfm.app really is")?;
    let status = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&original)
        .status()
        .map_err(|e| format!("xattr: {e}"))?;
    if !status.success() {
        return Err(format!("xattr failed ({status})"));
    }
    std::process::Command::new("open")
        .arg("-n")
        .arg(&original)
        .spawn()
        .map_err(|e| format!("open: {e}"))?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_translocated_exe() {
        assert!(is_translocated(Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/fxfm.app/Contents/MacOS/fxfm"
        )));
        assert!(!is_translocated(Path::new(
            "/Applications/fxfm.app/Contents/MacOS/fxfm"
        )));
    }

    #[test]
    fn bundle_of_finds_the_app() {
        assert_eq!(
            bundle_of(Path::new(
                "/x/T/AppTranslocation/A/d/fxfm.app/Contents/MacOS/fxfm"
            )),
            Path::new("/x/T/AppTranslocation/A/d/fxfm.app")
        );
        assert_eq!(
            bundle_of(Path::new("/usr/bin/fxfm")),
            Path::new("/usr/bin/fxfm")
        );
    }

    #[test]
    fn original_path_is_none_when_not_translocated() {
        assert_eq!(
            original_path(Path::new("/Applications/fxfm.app/Contents/MacOS/fxfm")),
            None
        );
    }
}
