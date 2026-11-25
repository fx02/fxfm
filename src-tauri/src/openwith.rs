//! Right-click *Open with…* and *Show in Finder* for local files.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// An application that can open a file: its shown name and bundle path.
#[derive(Serialize, Debug, PartialEq)]
pub struct App {
    pub name: String,
    pub path: String,
}

/// The applications macOS offers for `path`, the default one first (empty for a non-local path).
#[tauri::command]
pub async fn open_with_apps(path: String) -> Vec<App> {
    tauri::async_runtime::spawn_blocking(move || native::apps(&path))
        .await
        .unwrap_or_default()
}

/// Opens `path` with the application bundle `app`.
#[tauri::command]
pub async fn open_with(path: String, app: String) -> Result<(), String> {
    run(Command::new("open").arg("-a").arg(app).arg("--").arg(path))
}

/// Selects `path` in a Finder window.
#[tauri::command]
pub async fn show_in_finder(path: String) -> Result<(), String> {
    run(Command::new("open").arg("-R").arg("--").arg(path))
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// "TextEdit" for "/System/Applications/TextEdit.app".
fn app_name(path: &str) -> String {
    let p = Path::new(path);
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Default app first, then the others in name order, each bundle once.
fn ordered(default: Option<String>, mut all: Vec<String>) -> Vec<App> {
    all.sort_by_key(|p| app_name(p).to_lowercase());
    all.dedup();
    let mut out: Vec<String> = default.into_iter().collect();
    for p in all {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.into_iter()
        .map(|path| App {
            name: app_name(&path),
            path,
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod native {
    use super::{ordered, App};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    pub fn apps(path: &str) -> Vec<App> {
        if !path.starts_with('/') || std::fs::symlink_metadata(path).is_err() {
            return Vec::new();
        }
        let ws = NSWorkspace::sharedWorkspace();
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let path_of = |u: &NSURL| u.path().map(|p| p.to_string());
        let default = ws
            .URLForApplicationToOpenURL(&url)
            .and_then(|u| path_of(&u));
        let all = ws
            .URLsForApplicationsToOpenURL(&url)
            .iter()
            .filter_map(|u| path_of(&u))
            .collect();
        ordered(default, all)
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub fn apps(_path: &str) -> Vec<super::App> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_first_then_by_name_once() {
        let apps = ordered(
            Some("/A/Zed.app".into()),
            vec![
                "/A/beta.app".into(),
                "/A/Zed.app".into(),
                "/A/Alpha.app".into(),
            ],
        );
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Zed", "Alpha", "beta"]);
    }

    #[test]
    fn a_text_file_has_apps_and_a_missing_one_none() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(!native::apps(&f.to_string_lossy()).is_empty());
        assert!(native::apps(&t.path().join("no.txt").to_string_lossy()).is_empty());
        assert!(native::apps("relative.txt").is_empty());
    }
}
