//! Options → View → Font: the fixed-pitch (monospace) font families installed here (AppKit on
//! macOS, fontconfig on Linux).

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

/// Families from `fc-list : family` output: one font per line, a family's names comma-separated
/// (the first is the family), hidden ones (starting with a dot) left out.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn fc_families(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.split(',').next())
        .map(|f| f.trim().replace('\\', ""))
        .filter(|f| !f.is_empty() && !f.starts_with('.'))
        .collect()
}

#[cfg(target_os = "linux")]
mod native {
    /// fontconfig's monospace families (`spacing=mono`, 100).
    pub fn families() -> Vec<String> {
        std::process::Command::new("fc-list")
            .args([":spacing=mono", "family"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| super::fc_families(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod native {
    pub fn families() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fontconfig_lines_give_families() {
        let out = "DejaVu Sans Mono,DejaVu Sans Mono Book\nLiberation Mono\n.Hidden Mono\n\nNoto Sans Mono\\-Bold,Noto\n";
        assert_eq!(
            super::fc_families(out),
            ["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono-Bold"]
        );
    }
}
