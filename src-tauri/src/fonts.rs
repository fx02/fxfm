//! Options → View → Font: the fixed-pitch (monospace) font families installed on this Mac.

/// Font families whose letters all have one width, sorted; Menlo first when present.
#[tauri::command]
pub fn fixed_fonts() -> Vec<String> {
    let mut v = native::families();
    v.sort_by_key(|f| (f.as_str() != "Menlo", f.to_lowercase()));
    v.dedup();
    v
}

#[cfg(target_os = "macos")]
mod native {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFont, NSFontManager, NSFontTraitMask};

    /// AppKit's font manager lives on the main thread (sync Tauri commands run there).
    pub fn families() -> Vec<String> {
        let Some(mtm) = MainThreadMarker::new() else {
            return vec!["Menlo".into()];
        };
        let fm = NSFontManager::sharedFontManager(mtm);
        let Some(names) = fm.availableFontNamesWithTraits(NSFontTraitMask::FixedPitchFontMask)
        else {
            return vec!["Menlo".into()];
        };
        names
            .iter()
            .filter_map(|n| NSFont::fontWithName_size(&n, 12.0))
            .filter(|f| f.isFixedPitch())
            .filter_map(|f| f.familyName().map(|s| s.to_string()))
            // Hidden system fonts start with a dot.
            .filter(|f| !f.starts_with('.'))
            .collect()
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub fn families() -> Vec<String> {
        Vec::new()
    }
}
