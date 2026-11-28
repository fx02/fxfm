//! Right-click *Open with…*, *Show in Finder* and Quick Look for local files: NSWorkspace and
//! `open` on macOS; gio (desktop files, content types) and the freedesktop D-Bus file manager
//! and GNOME previewer on Linux.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// An application that can open a file: its shown name and bundle path (on Linux its desktop
/// file id, e.g. `org.gnome.TextEditor.desktop`).
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

/// Opens `path` with the application `app` (see [`App`]).
#[tauri::command]
pub async fn open_with(path: String, app: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || native::open_with(&path, &app))
        .await
        .map_err(|e| e.to_string())?
}

/// Selects `path` in the file manager (a Finder window on macOS).
#[tauri::command]
pub async fn show_in_finder(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || native::show(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// "TextEdit" for "/System/Applications/TextEdit.app".
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_name(path: &str) -> String {
    let p = Path::new(path);
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Default app first, then the others in name order, each bundle once.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
    use super::{ordered, run, App};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};
    use std::process::Command;

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

    pub fn open_with(path: &str, app: &str) -> Result<(), String> {
        run(Command::new("open").arg("-a").arg(app).arg("--").arg(path))
    }

    pub fn show(path: &str) -> Result<(), String> {
        run(Command::new("open").arg("-R").arg("--").arg(path))
    }
}

#[cfg(target_os = "linux")]
pub mod native {
    use super::App;
    use gtk::gio::{self, prelude::*};
    use gtk::glib::ToVariant;
    use std::collections::HashSet;

    fn local(path: &str) -> bool {
        path.starts_with('/') && std::fs::symlink_metadata(path).is_ok()
    }

    pub fn apps(path: &str) -> Vec<App> {
        if !local(path) {
            return Vec::new();
        }
        let Some(ct) = gio::File::for_path(path)
            .query_info(
                "standard::content-type",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            )
            .ok()
            .and_then(|i| i.content_type())
        else {
            return Vec::new();
        };
        let mut all = gio::AppInfo::all_for_type(&ct);
        all.sort_by_key(|a| a.display_name().to_lowercase());
        let mut seen = HashSet::new();
        gio::AppInfo::default_for_type(&ct, false)
            .into_iter()
            .chain(all)
            .filter_map(|a| {
                let id = a.id()?.to_string();
                seen.insert(id.clone()).then(|| App {
                    name: a.display_name().to_string(),
                    path: id,
                })
            })
            .collect()
    }

    pub fn open_with(path: &str, app: &str) -> Result<(), String> {
        let info =
            gio::DesktopAppInfo::new(app).ok_or_else(|| format!("{app}: no such application"))?;
        info.launch(&[gio::File::for_path(path)], gio::AppLaunchContext::NONE)
            .map_err(|e| e.to_string())
    }

    fn session_call(dest: &str, object: &str, method: &str, args: gtk::glib::Variant) -> bool {
        gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .and_then(|c| {
                c.call_sync(
                    Some(dest),
                    object,
                    dest,
                    method,
                    Some(&args),
                    None,
                    gio::DBusCallFlags::NONE,
                    3000,
                    gio::Cancellable::NONE,
                )
            })
            .is_ok()
    }

    /// The file manager's own selection (org.freedesktop.FileManager1: Nautilus, Dolphin,
    /// Nemo, Thunar…), else the folder opened by its default application.
    pub fn show(path: &str) -> Result<(), String> {
        if !local(path) {
            return Err(format!("{path}: not a local file"));
        }
        let uri = gio::File::for_path(path).uri().to_string();
        let args = (vec![uri], "").to_variant();
        if session_call(
            "org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "ShowItems",
            args,
        ) {
            return Ok(());
        }
        let parent = std::path::Path::new(path)
            .parent()
            .unwrap_or(std::path::Path::new("/"));
        gio::AppInfo::launch_default_for_uri(
            &gio::File::for_path(parent).uri(),
            gio::AppLaunchContext::NONE,
        )
        .map_err(|e| e.to_string())
    }

    /// Quick Look: GNOME's previewer (Sushi) when it is installed; false otherwise.
    pub fn preview(path: &str) -> bool {
        let uri = gio::File::for_path(path).uri().to_string();
        session_call(
            "org.gnome.NautilusPreviewer2",
            "/org/gnome/NautilusPreviewer2",
            "ShowFile",
            (uri.clone(), "", false).to_variant(),
        ) || session_call(
            "org.gnome.NautilusPreviewer",
            "/org/gnome/NautilusPreviewer",
            "ShowFile",
            (uri, 0i32, false).to_variant(),
        )
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod native {
    pub fn apps(_path: &str) -> Vec<super::App> {
        Vec::new()
    }
    pub fn open_with(_path: &str, _app: &str) -> Result<(), String> {
        Err("not available".into())
    }
    pub fn show(_path: &str) -> Result<(), String> {
        Err("not available".into())
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
