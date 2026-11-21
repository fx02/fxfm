//! Shared application state.

use crate::audio::{self, Player};
use crate::editwatch::{self, EditWatch, PendingEdit};
use crate::jobs::Jobs;
use crate::remote::Connector;
use crate::search;
use crate::translocation;
use fxfm_core::config::{SessionState, Settings, SETTINGS_FILE};
use fxfm_core::hotlist::{Hotlist, HOTLIST_FILE};
use fxfm_core::keymap::Keymap;
use fxfm_core::process::pid_alive;
use fxfm_core::storage::{self, DataLocation};
use fxfm_core::vfs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tauri::{AppHandle, Emitter};

pub struct AppState {
    pub home: PathBuf,
    /// Folder holding the app (for portable mode).
    pub app_folder: Option<PathBuf>,
    pub system_dir: PathBuf,
    /// Where settings/session/keymap/hotlist are read and written.
    pub data: Mutex<DataLocation>,
    pub settings: Mutex<Settings>,
    pub keymap: Mutex<Keymap>,
    pub hotlist: Mutex<Hotlist>,
    pub jobs: Jobs,
    /// Archive index cache, passwords and temp folders.
    pub vfs: Arc<vfs::Ctx>,
    /// Cancel flag of the running `dir_size` per panel (left, right).
    pub dir_size_cancel: Mutex<[Option<Arc<AtomicBool>>; 2]>,
    pub viewer_seq: AtomicU64,
    /// Latest session from the UI that is not on disk yet; written on exit
    /// (Cmd+Q terminates without giving the webview a chance to save).
    pub pending_session: Mutex<Option<SessionState>>,
    /// Started from a macOS App Translocation path (quarantined, not moved).
    pub translocated: bool,
    /// Copies of archive and server files opened with F4 (upload back on change).
    pub edits: EditWatch,
    /// Set in `setup`; `edits` emits through it.
    pub handle: Arc<OnceLock<AppHandle>>,
    /// Not uploaded edits of dead processes and their `<tmp>/<pid>` folders
    /// (moved to the Trash by `discard_pending_edits`; kept otherwise).
    pub stale_edits: Mutex<(Vec<PendingEdit>, Vec<PathBuf>)>,
    /// Saved servers, passwords and connecting (the connections are in `vfs.sessions`).
    pub remote: Connector,
    /// F3 audio playback.
    pub audio: Player,
    /// Search (Alt+F7) index and searches; started in `setup`.
    pub search: search::Service,
}

impl AppState {
    pub fn load() -> AppState {
        let exe = std::env::current_exe().ok();
        let app_folder = exe
            .as_deref()
            .and_then(storage::app_folder)
            .map(PathBuf::from);
        let system_dir = storage::system_dir();
        let data = storage::resolve(app_folder.as_deref(), &system_dir);
        let translocated = exe.as_deref().is_some_and(translocation::is_translocated);
        let tmp = tmp_parent();
        move_aside_own(&tmp, std::process::id());
        remove_stale_temp(&tmp, &pid_alive);
        let stale_edits = editwatch::collect_pending(&tmp, &pid_alive);
        let temp_root = tmp.join(std::process::id().to_string());
        let handle: Arc<OnceLock<AppHandle>> = Arc::default();
        let h = handle.clone();
        let edits = EditWatch::new(
            Arc::new(move |id, location: &str, name: &str| {
                let Some(app) = h.get() else {
                    return;
                };
                let payload = serde_json::json!({"id": id, "location": location, "name": name});
                if let Err(e) = app.emit_to("main", "edit://changed", payload) {
                    eprintln!("fxfm: emit edit://changed: {e}");
                }
            }),
            temp_root.clone(),
        );
        let mut settings: Settings = storage::load_json(&data.dir, SETTINGS_FILE);
        // Commands added since keymap.json was saved get their free default keys.
        let keymap = fxfm_core::keymap::load_migrating(&data.dir, &mut settings);
        let h = handle.clone();
        let search = search::Service::new(
            PathBuf::from("/"),
            data.dir.clone(),
            Arc::new(move |event: &str, payload: serde_json::Value| {
                let Some(app) = h.get() else {
                    return;
                };
                // Every window: the main window (status in Options) and Find files.
                if let Err(e) = app.emit(event, payload) {
                    eprintln!("fxfm: emit {event}: {e}");
                }
            }),
            Arc::new(search::system_mounts),
        );
        let audio = audio::app_player(handle.clone());
        audio.set_output(settings.audio_output.clone());
        audio.set_volume(settings.audio_volume);
        AppState {
            vfs: Arc::new(vfs::Ctx::new(temp_root)),
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            app_folder,
            system_dir,
            settings: Mutex::new(settings),
            keymap: Mutex::new(keymap),
            hotlist: Mutex::new(storage::load_json(&data.dir, HOTLIST_FILE)),
            remote: Connector::load(crate::keychain::system_store(), &data.dir),
            data: Mutex::new(data),
            jobs: Jobs::default(),
            dir_size_cancel: Mutex::new([None, None]),
            viewer_seq: AtomicU64::new(0),
            pending_session: Mutex::new(None),
            translocated,
            edits,
            handle,
            stale_edits: Mutex::new(stale_edits),
            audio,
            search,
        }
    }
}

/// `<cache dir>/fxfm/tmp`: one `<pid>` folder per running instance.
fn tmp_parent() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("fxfm")
        .join("tmp")
}

/// For a folder name in `<tmp>`: `<pid>` is dead when the process is gone,
/// `<pid>.prev-<n>` (see `move_aside_own`) always; None for other names.
pub fn temp_dir_dead(name: &str, alive: &dyn Fn(u32) -> bool) -> Option<bool> {
    if let Some((pid, n)) = name.split_once(".prev-") {
        return (pid.parse::<u32>().is_ok() && n.parse::<u32>().is_ok()).then_some(true);
    }
    name.parse::<u32>().ok().map(|pid| !alive(pid))
}

/// A `<tmp>/<pid>` folder with our own pid was left by a dead process that had the
/// same pid; it is renamed to `<pid>.prev-<n>` so its edits are not mistaken for ours.
pub fn move_aside_own(tmp: &Path, pid: u32) {
    let own = tmp.join(pid.to_string());
    if !own.exists() {
        return;
    }
    for n in 0.. {
        let to = tmp.join(format!("{pid}.prev-{n}"));
        if !to.exists() {
            if let Err(e) = std::fs::rename(&own, &to) {
                eprintln!("fxfm: moving aside {}: {e}", own.display());
            }
            return;
        }
    }
}

/// Removes `<tmp>/<pid>` folders left by dead processes, except those holding
/// `pending-edits.json` (copies of archive files, see `editwatch::collect_pending`).
pub fn remove_stale_temp(tmp: &Path, alive: &dyn Fn(u32) -> bool) {
    let Ok(rd) = std::fs::read_dir(tmp) else {
        return;
    };
    let ctx = vfs::Ctx::new(tmp.to_path_buf());
    for e in rd.flatten() {
        let dead = e.file_name().to_str().and_then(|n| temp_dir_dead(n, alive));
        let dir = e.path();
        if dead == Some(true) && !dir.join(PENDING_EDITS).exists() {
            ctx.cleanup_dir(&dir);
        }
    }
}

pub const PENDING_EDITS: &str = "pending-edits.json";

/// Locks, ignoring poisoning (the data is plain values).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_temp_dirs_removed_except_pending() {
        let t = tempfile::tempdir().unwrap();
        let tmp = t.path();
        for (pid, pending) in [("999999", false), ("999998", true), ("4242", false)] {
            let d = tmp.join(pid).join("view-0");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("f"), b"x").unwrap();
            if pending {
                std::fs::write(tmp.join(pid).join(PENDING_EDITS), b"[]").unwrap();
            }
        }
        std::fs::create_dir(tmp.join("not-a-pid")).unwrap();
        remove_stale_temp(tmp, &|pid| pid == 4242);
        assert!(!tmp.join("999999").exists());
        assert!(tmp.join("999998").exists());
        assert!(tmp.join("4242").exists());
        assert!(tmp.join("not-a-pid").exists());
    }

    #[test]
    fn reused_pid_folder_is_moved_aside_and_dead() {
        let t = tempfile::tempdir().unwrap();
        let tmp = t.path();
        std::fs::create_dir_all(tmp.join("4242").join("edit-0")).unwrap();
        std::fs::create_dir_all(tmp.join("4242.prev-0")).unwrap();
        move_aside_own(tmp, 4242);
        assert!(!tmp.join("4242").exists());
        assert!(tmp.join("4242.prev-1").join("edit-0").exists());
        move_aside_own(tmp, 4242); // nothing to move
        assert!(tmp.join("4242.prev-1").exists());
        // Dead although pid 4242 is alive.
        assert_eq!(temp_dir_dead("4242.prev-1", &|_| true), Some(true));
        assert_eq!(temp_dir_dead("4242", &|_| true), Some(false));
        assert_eq!(temp_dir_dead("4242", &|_| false), Some(true));
        assert_eq!(temp_dir_dead("x.prev-1", &|_| true), None);
        assert_eq!(temp_dir_dead("not-a-pid", &|_| true), None);
        remove_stale_temp(tmp, &|_| true);
        assert!(!tmp.join("4242.prev-0").exists());
        assert!(!tmp.join("4242.prev-1").exists());
    }
}
