//! Tauri commands (the JS `invoke` names).

use crate::editwatch::PendingEdit;
use crate::jobs::JobRequest;
use crate::launch::{editor_command, terminal_command};
use crate::remote::{self, watch};
use crate::state::{lock, AppState};
use crate::{menu, translocation};
use fxfm_core::archive::split_location;
use fxfm_core::config::{
    restore_path, Colors, PanelState, SessionState, Settings, Side, SESSION_FILE, SETTINGS_FILE,
};
use fxfm_core::hotlist::{Hotlist, HOTLIST_FILE};
use fxfm_core::keymap::{command_ids, KeyCombo, Keymap, COMMANDS, KEYMAP_FILE};
use fxfm_core::listing::{Entry, ListError};
use fxfm_core::ops::{ConflictAnswer, ErrorAnswer};
use fxfm_core::remote::parse_remote;
use fxfm_core::storage::{self, DataLocation, StorageMode};
use fxfm_core::vfs;
use fxfm_core::viewer::{self, ViewContent};
use fxfm_core::volumes::{self, Volume};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub id: String,
    pub menu: String,
    pub title: String,
    pub available: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub mode: StorageMode,
    pub dir: String,
    /// `fxfm-data` beside the app (None without an app folder).
    pub portable_dir: Option<String>,
    /// Portable mode is possible: the folder beside the app is writable.
    pub portable_writable: bool,
    pub system_dir: String,
}

fn storage_info(state: &AppState, d: &DataLocation) -> StorageInfo {
    let folder = state.app_folder.as_deref();
    StorageInfo {
        mode: d.mode,
        dir: path_string(&d.dir),
        portable_dir: folder.map(|f| path_string(&f.join(storage::DATA_DIR_NAME))),
        portable_writable: folder.is_some_and(storage::is_writable_dir),
        system_dir: path_string(&state.system_dir),
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub home: String,
    pub user: String,
    pub version: String,
    pub session: SessionState,
    pub settings: Settings,
    pub keymap: BTreeMap<String, String>,
    pub commands: Vec<CommandInfo>,
    pub hotlist: Hotlist,
    pub storage: StorageInfo,
    pub system_conflicts: Vec<(String, String)>,
    pub translocated: bool,
    /// The real `fxfm.app` when translocated (for the `xattr` command).
    pub original_app: Option<String>,
    pub default_colors: Colors,
    /// Edited copies of archive files a previous session did not upload.
    pub pending_edits: Vec<PendingEdit>,
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn keymap_bindings(k: &Keymap) -> BTreeMap<String, String> {
    serde_json::to_value(k)
        .and_then(serde_json::from_value)
        .unwrap_or_default()
}

/// Spawns `cmd` and reaps it in the background.
fn spawn_detached(mut cmd: Command) -> Result<(), String> {
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn restore_panel(p: Option<PanelState>, home: &Path) -> PanelState {
    match p {
        Some(p) => PanelState {
            location: path_string(&restore_path(&p.location, home)),
            cursor_name: p.cursor_name,
        },
        None => PanelState {
            location: path_string(home),
            cursor_name: None,
        },
    }
}

/// Runs blocking I/O off the main thread and the async workers, so a dead
/// mount cannot freeze the UI.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// Same rules as `ops::rename`: one path component, not `.`/`..`.
fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn build_bootstrap(state: &AppState) -> Bootstrap {
    let home = &state.home;
    let data = lock(&state.data).clone();
    let mut session: SessionState = storage::load_json(&data.dir, SESSION_FILE);
    session.left = Some(restore_panel(session.left.take(), home));
    session.right = Some(restore_panel(session.right.take(), home));
    let keymap = lock(&state.keymap).clone();
    // Locked once, outside the struct literal: a guard taken in a field expression lives
    // until the end of the whole literal, so locking the same mutex twice in it deadlocks.
    let settings = lock(&state.settings).clone();
    let system_conflicts = keymap.system_conflicts(&settings.modifiers);
    Bootstrap {
        home: path_string(home),
        user: std::env::var("USER").unwrap_or_else(|_| "user".into()),
        version: fxfm_core::version().to_string(),
        session,
        settings,
        keymap: keymap_bindings(&keymap),
        commands: COMMANDS
            .iter()
            .map(|c| CommandInfo {
                id: c.id.into(),
                menu: c.menu.into(),
                title: c.title.into(),
                available: c.available,
            })
            .collect(),
        hotlist: lock(&state.hotlist).clone(),
        storage: storage_info(state, &data),
        system_conflicts,
        translocated: state.translocated,
        original_app: state
            .translocated
            .then(|| std::env::current_exe().ok())
            .flatten()
            .and_then(|exe| translocation::original_path(&exe))
            .map(|p| path_string(&p)),
        default_colors: Colors::default(),
        pending_edits: lock(&state.stale_edits).0.clone(),
    }
}

#[tauri::command]
pub async fn bootstrap(app: AppHandle) -> Result<Bootstrap, String> {
    blocking(move || build_bootstrap(&app.state::<AppState>())).await
}

#[tauri::command]
pub async fn list_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    location: String,
) -> Result<Vec<Entry>, ListError> {
    let loc = location.clone();
    let ctx = state.vfs.clone();
    blocking(move || watch(&app, &ctx, || vfs::list(&ctx, &location)))
        .await
        .unwrap_or(Err(ListError::Io(loc)))
}

#[tauri::command]
pub async fn list_volumes(app: AppHandle) -> Result<Vec<Volume>, String> {
    blocking(move || volumes::list_volumes(&app.state::<AppState>().home)).await
}

#[tauri::command]
pub async fn nearest_existing(app: AppHandle, location: String) -> Result<String, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        watch(&app, &state.vfs, || {
            vfs::nearest(&state.vfs, &location, &state.home)
        })
    })
    .await
}

/// A new call for the same panel cancels that panel's previous call.
#[tauri::command]
pub async fn dir_size(
    app: AppHandle,
    state: State<'_, AppState>,
    location: String,
    panel: Side,
) -> Result<(u64, u64), String> {
    let i = side_index(panel);
    let flag = Arc::new(AtomicBool::new(false));
    if let Some(old) = lock(&state.dir_size_cancel)[i].replace(flag.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    let f = flag.clone();
    let ctx = state.vfs.clone();
    let r = blocking(move || watch(&app, &ctx, || vfs::dir_size(&ctx, &location, &f))).await?;
    {
        let mut cur = lock(&state.dir_size_cancel);
        if cur[i].as_ref().is_some_and(|c| Arc::ptr_eq(c, &flag)) {
            cur[i] = None;
        }
    }
    r.map_err(|e| e.to_string())
}

/// Remembers the UI's current session in memory (cheap; called on every change).
#[tauri::command]
pub fn set_session(state: State<'_, AppState>, session: SessionState) {
    *lock(&state.pending_session) = Some(session);
}

#[tauri::command]
pub async fn save_session(app: AppHandle, session: SessionState) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let data = lock(&state.data);
        storage::save_json(&data.dir, SESSION_FILE, &session).map_err(|e| e.to_string())?;
        let mut pending = lock(&state.pending_session);
        if pending.as_ref() == Some(&session) {
            *pending = None;
        }
        Ok(())
    })
    .await?
}

/// Writes the pending session (if any) to `dir`; returns whether it wrote.
fn flush_pending(pending: &Mutex<Option<SessionState>>, dir: &Path) -> std::io::Result<bool> {
    let Some(session) = lock(pending).take() else {
        return Ok(false);
    };
    storage::save_json(dir, SESSION_FILE, &session)?;
    Ok(true)
}

/// Called on exit: persists the latest session the UI reported.
pub fn flush_session(state: &AppState) {
    let dir = lock(&state.data).dir.clone();
    if let Err(e) = flush_pending(&state.pending_session, &dir) {
        eprintln!("fxfm: saving session on exit: {e}");
    }
}

/// Saves the settings; returns the system conflicts of the keymap under the (maybe changed)
/// Alt/Ctrl mapping. A changed mapping rebuilds the menu (its titles show the keys).
#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    mut settings: Settings,
) -> Result<Vec<(String, String)>, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let mods = settings.modifiers;
        let changed = {
            // Held while saving, as before: a storage move waits for it.
            let data = lock(&state.data);
            let mut cur = lock(&state.settings);
            // The backend keeps this list; the UI's copy may be older.
            settings.keymap_commands = cur.keymap_commands.clone();
            storage::save_json(&data.dir, SETTINGS_FILE, &settings).map_err(|e| e.to_string())?;
            state.audio.set_output(settings.audio_output.clone());
            let changed = cur.modifiers != mods;
            *cur = settings;
            changed
        };
        let keymap = lock(&state.keymap).clone();
        if changed {
            menu::refresh(&app, &keymap, &mods);
        }
        Ok(keymap.system_conflicts(&mods))
    })
    .await?
}

#[tauri::command]
pub async fn save_hotlist(app: AppHandle, hotlist: Hotlist) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let data = lock(&state.data);
        storage::save_json(&data.dir, HOTLIST_FILE, &hotlist).map_err(|e| e.to_string())?;
        *lock(&state.hotlist) = hotlist;
        Ok(())
    })
    .await?
}

/// Records that the saved keymap knows every current command (`Settings::keymap_commands`),
/// so a key the user removed is not added back as a new default at the next start.
fn record_keymap_commands(state: &AppState, dir: &Path) {
    let ids = command_ids();
    let mut settings = lock(&state.settings);
    if settings.keymap_commands.as_ref() == Some(&ids) {
        return;
    }
    settings.keymap_commands = Some(ids);
    if let Err(e) = storage::save_json(dir, SETTINGS_FILE, &*settings) {
        eprintln!("fxfm: saving settings: {e}");
    }
}

/// Replaces all bindings (unparsable combos and unknown commands are dropped);
/// returns the new system conflicts.
#[tauri::command]
pub async fn save_keymap(
    app: AppHandle,
    bindings: BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let mut keymap = Keymap::default();
    for (combo, cmd) in &bindings {
        match combo.parse::<KeyCombo>() {
            Ok(c) if COMMANDS.iter().any(|d| d.id == cmd) => {
                keymap.bind(&c, cmd);
            }
            _ => eprintln!("fxfm: keymap: dropping {combo:?} -> {cmd:?}"),
        }
    }
    blocking(move || {
        let state = app.state::<AppState>();
        let dir = lock(&state.data).dir.clone();
        storage::save_json(&dir, KEYMAP_FILE, &keymap).map_err(|e| e.to_string())?;
        record_keymap_commands(&state, &dir);
        let mods = lock(&state.settings).modifiers;
        // Menu titles show the keys; built on the main thread, so no lock is held.
        menu::refresh(&app, &keymap, &mods);
        let conflicts = keymap.system_conflicts(&mods);
        *lock(&state.keymap) = keymap;
        Ok(conflicts)
    })
    .await?
}

#[tauri::command]
pub async fn reset_keymap(app: AppHandle) -> Result<BTreeMap<String, String>, String> {
    blocking(move || {
        let keymap = Keymap::defaults();
        let state = app.state::<AppState>();
        let dir = lock(&state.data).dir.clone();
        storage::save_json(&dir, KEYMAP_FILE, &keymap).map_err(|e| e.to_string())?;
        record_keymap_commands(&state, &dir);
        let out = keymap_bindings(&keymap);
        let mods = lock(&state.settings).modifiers;
        menu::refresh(&app, &keymap, &mods);
        *lock(&state.keymap) = keymap;
        Ok(out)
    })
    .await?
}

/// `StorageInfo` plus what happened to the saved server passwords.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StorageChange {
    #[serde(flatten)]
    pub info: StorageInfo,
    /// Saved passwords and passphrases that moved with the data.
    pub moved_secrets: usize,
    /// Confirmation for the user (empty when nothing changed).
    pub message: String,
}

/// Moves the data folder and the saved passwords (see `remote::change_storage`): to System
/// needs the vault unlocked (or `master`), to Portable needs `master` for the new vault.
#[tauri::command]
pub async fn set_storage_mode(
    app: AppHandle,
    mode: StorageMode,
    master: Option<String>,
) -> Result<StorageChange, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let mut data = lock(&state.data);
        let changed = data.mode != mode;
        let (to, n) = remote::change_storage(
            &state.remote,
            &data,
            mode,
            master,
            state.app_folder.as_deref(),
            &state.system_dir,
        )?;
        *data = to;
        let message = match (changed, mode) {
            (false, _) => String::new(),
            (true, StorageMode::System) => {
                format!("Settings moved. {n} saved password(s) moved to the system keychain.")
            }
            (true, StorageMode::Portable) => format!(
                "Settings moved. {n} saved password(s) moved to the encrypted password file."
            ),
        };
        Ok(StorageChange {
            info: storage_info(&state, &data),
            moved_secrets: n,
            message,
        })
    })
    .await?
}

#[tauri::command]
pub fn start_job(app: AppHandle, state: State<'_, AppState>, req: JobRequest) -> u64 {
    state.jobs.start(app, state.vfs.clone(), req)
}

#[tauri::command]
pub fn answer_conflict(state: State<'_, AppState>, job: u64, answer: ConflictAnswer) {
    if let Some(b) = state.jobs.get(job) {
        b.answer_conflict(answer);
    }
}

#[tauri::command]
pub fn answer_error(state: State<'_, AppState>, job: u64, answer: ErrorAnswer) {
    if let Some(b) = state.jobs.get(job) {
        b.answer_error(answer);
    }
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job: u64) {
    if let Some(b) = state.jobs.get(job) {
        b.cancel();
    }
}

#[tauri::command]
pub async fn mkdir(
    app: AppHandle,
    state: State<'_, AppState>,
    parent: String,
    name: String,
) -> Result<String, String> {
    let ctx = state.vfs.clone();
    blocking(move || watch(&app, &ctx, || vfs::mkdir(&ctx, &parent, &name))).await?
}

#[tauri::command]
pub async fn rename(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    new_name: String,
) -> Result<String, String> {
    let ctx = state.vfs.clone();
    blocking(move || watch(&app, &ctx, || vfs::rename(&ctx, &path, &new_name))).await?
}

#[tauri::command]
pub async fn view_content(path: String) -> Result<ViewContent, String> {
    blocking(move || viewer::read_for_view(Path::new(&path)).map_err(|e| e.to_string())).await?
}

#[tauri::command]
pub async fn view_text_as(path: String, encoding: String) -> Result<ViewContent, String> {
    blocking(move || viewer::read_text_as(Path::new(&path), &encoding).map_err(|e| e.to_string()))
        .await?
}

/// PDF fallback for the viewer: Quick Look (`qlmanage -p`) on macOS, else the default app.
#[tauri::command]
pub async fn quick_look(app: AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = &app;
        blocking(move || {
            let mut cmd = Command::new("qlmanage");
            cmd.arg("-p")
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            spawn_detached(cmd)
        })
        .await?
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.opener()
            .open_path(path, None::<&str>)
            .map_err(|e| e.to_string())
    }
}

/// Remembers the password of an archive (the path comes from the "password required" error).
#[tauri::command]
pub fn set_archive_password(state: State<'_, AppState>, archive: String, password: String) {
    state.vfs.set_password(Path::new(&archive), password);
}

/// True when `location` is an archive root or a folder inside an archive (the archive must be a
/// regular file; a real folder named `x.zip` is not).
fn archive_location(location: &str) -> bool {
    fxfm_core::archive::split_location(location).is_some()
}

#[tauri::command]
pub fn location_in_archive(location: String) -> bool {
    archive_location(&location)
}

/// Inside an archive (not the archive file itself).
fn in_archive(loc: &str) -> bool {
    split_location(loc).is_some_and(|a| !a.inner.is_empty())
}

/// Opening the file needs a local copy: inside an archive or on a server.
pub(crate) fn needs_copy(loc: &str) -> bool {
    in_archive(loc) || parse_remote(loc).is_some()
}

/// The local copy of `path` (F3, open with default app); copies of server files are
/// removed when their connection is closed.
fn view_copy(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let state = app.state::<AppState>();
    let local = watch(app, &state.vfs, || {
        vfs::materialize(&state.vfs, path, "view")
    })?;
    state.remote.record_view(path, &local);
    Ok(local)
}

/// Opens a viewer window `viewer-<n>` on `viewer.html?path=<local path>`; the title is the
/// original location (the full one inside an archive or on a server). Async: creating a
/// window from a sync command can deadlock.
#[tauri::command]
pub async fn open_viewer(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let (a, loc) = (app.clone(), path.clone());
    let local = blocking(move || view_copy(&a, &loc)).await??;
    let n = state.viewer_seq.fetch_add(1, Ordering::Relaxed) + 1;
    let title = if needs_copy(&path) {
        path.clone()
    } else {
        Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone())
    };
    // The viewer maps Alt/Ctrl like the main window (as of opening).
    let mods = serde_json::to_value(lock(&state.settings).modifiers).map_err(|e| e.to_string())?;
    let key = |k: &str| mods[k].as_str().unwrap_or_default().to_string();
    let url = format!(
        "viewer.html?path={}&alt={}&ctrl={}",
        url_encode(&path_string(&local)),
        key("alt"),
        key("ctrl")
    );
    WebviewWindowBuilder::new(&app, format!("viewer-{n}"), WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(1000.0, 700.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// F4. A file inside an archive or on a server is copied into `edit-<n>/` and watched;
/// changes are offered for upload (`edit://changed`). F4 again on the same file reopens
/// that copy.
#[tauri::command]
pub async fn edit_file(app: AppHandle, path: String) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let local = if needs_copy(&path) {
            watch(&app, &state.vfs, || edit_copy(&state, &path))?
        } else {
            PathBuf::from(&path)
        };
        let editor = lock(&state.settings).editor.clone();
        spawn_detached(editor_command(&editor, &local))
    })
    .await?
}

/// Enter on an archive on a server: downloads it as a tracked F4 copy (changes made inside
/// are offered for upload back, `edit://changed`) and returns the local path to enter.
#[tauri::command]
pub async fn open_remote_archive(app: AppHandle, location: String) -> Result<String, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        if parse_remote(&location).is_none()
            || fxfm_core::archive::ArchiveKind::from_name(&location).is_none()
        {
            return Err(format!("not an archive on a server: {location}"));
        }
        let local = watch(&app, &state.vfs, || edit_copy(&state, &location))?;
        Ok(path_string(&local))
    })
    .await?
}

/// The watched local copy of archive or server file `location` (see
/// `EditWatch::open_for_edit`).
fn edit_copy(state: &AppState, location: &str) -> Result<PathBuf, String> {
    state
        .edits
        .open_for_edit(&state.vfs, location)
        .map(|(_, local)| local)
}

/// Uploads the edited copy `id` back into its archive or to its server; returns the job id
/// (the UI confirms with `edit_confirm` once the job ends). Without `force`, a source
/// changed since the copy was made gives `conflict: …` and no job.
#[tauri::command]
pub async fn upload_back(app: AppHandle, id: u64, force: Option<bool>) -> Result<u64, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let (target, local) = watch(&app, &state.vfs, || {
            state
                .edits
                .prepare_upload(&state.vfs, id, force.unwrap_or(false))
        })?;
        let req = JobRequest::Upload {
            local: path_string(&local),
            target,
        };
        Ok(state.jobs.start(app.clone(), state.vfs.clone(), req))
    })
    .await?
}

#[tauri::command]
pub async fn edit_confirm(app: AppHandle, id: u64, uploaded: bool) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        watch(&app, &state.vfs, || {
            state.edits.confirm_upload(&state.vfs, id, uploaded)
        });
    })
    .await
}

/// Discard in the startup dialog of the previous sessions' not uploaded edits: their
/// copies go to the Trash (never deleted permanently), then their emptied temp folders.
#[tauri::command]
pub async fn discard_pending_edits(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let (edits, dirs) = std::mem::take(&mut *lock(&state.stale_edits));
        crate::editwatch::discard_pending(&edits, &dirs, &|p| {
            fxfm_core::ops::trash(p).map_err(|e| e.to_string())
        })
    })
    .await?
}

/// Shows `path` in Finder (selected); elsewhere opens its folder.
#[tauri::command]
pub fn reveal_file(app: AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = &app;
        let mut c = Command::new("open");
        c.arg("-R").arg(&path);
        spawn_detached(c)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir = Path::new(&path)
            .parent()
            .map_or_else(|| path.clone(), path_string);
        app.opener()
            .open_path(dir, None::<&str>)
            .map_err(|e| e.to_string())
    }
}

/// Creates `dir/name` (empty) unless it exists, then opens it in the editor.
#[tauri::command]
pub async fn edit_new(app: AppHandle, dir: String, name: String) -> Result<String, String> {
    if !valid_file_name(&name) {
        return Err(format!("invalid name '{name}'"));
    }
    if split_location(&dir).is_some() {
        return Err("Shift+F4 is not available inside archives".into());
    }
    if parse_remote(&dir).is_some() {
        return Err("Shift+F4 is not available on servers".into());
    }
    blocking(move || {
        let path: PathBuf = Path::new(&dir).join(&name);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let editor = lock(&app.state::<AppState>().settings).editor.clone();
        spawn_detached(editor_command(&editor, &path))?;
        Ok(path_string(&path))
    })
    .await?
}

#[tauri::command]
pub async fn open_default(app: AppHandle, path: String) -> Result<(), String> {
    let a = app.clone();
    let local = blocking(move || view_copy(&a, &path)).await??;
    app.opener()
        .open_path(path_string(&local), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_in_terminal(
    state: State<'_, AppState>,
    dir: String,
    command: String,
) -> Result<(), String> {
    spawn_detached(terminal_command(
        &vfs::local_dir_for(&dir, &state.home),
        &command,
    ))
}

#[tauri::command]
pub fn open_fda_settings() -> Result<(), String> {
    let mut c = Command::new("open");
    c.arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles");
    spawn_detached(c)
}

#[tauri::command]
pub fn open_keyboard_settings() -> Result<(), String> {
    let mut c = Command::new("open");
    c.arg("x-apple.systempreferences:com.apple.Keyboard-Settings.extension");
    spawn_detached(c)
}

#[cfg(test)]
mod tests {
    #[test]
    fn archive_location_needs_a_real_archive_file() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d.zip")).unwrap();
        std::fs::create_dir(t.path().join("plain")).unwrap();
        std::fs::write(t.path().join("a.zip"), b"x").unwrap();
        let p = |n: &str| t.path().join(n).to_str().unwrap().to_string();
        assert!(!archive_location(&p("d.zip")));
        assert!(archive_location(&p("a.zip")));
        assert!(archive_location(&p("a.zip/sub")));
        assert!(!archive_location(&p("plain")));
    }

    use super::*;

    #[test]
    fn edit_new_name_validation() {
        for bad in ["", ".", "..", "a/b", "/abs", "/", "x\0y"] {
            assert!(!valid_file_name(bad), "{bad:?} must be rejected");
        }
        for ok in ["a.txt", ".hidden", "..x", "with space"] {
            assert!(valid_file_name(ok), "{ok:?} must be accepted");
        }
    }

    #[test]
    fn flush_pending_writes_latest_session_once() {
        let dir = std::env::temp_dir().join(format!("fxfm-flush-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = SessionState {
            left: Some(PanelState {
                location: "/tmp".into(),
                cursor_name: Some("x.txt".into()),
            }),
            ..SessionState::default()
        };
        let pending = Mutex::new(Some(session.clone()));
        assert!(flush_pending(&pending, &dir).unwrap());
        let back: SessionState = storage::load_json(&dir, SESSION_FILE);
        assert_eq!(back, session);
        assert!(pending.lock().unwrap().is_none());
        assert!(!flush_pending(&pending, &dir).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn url_encode_escapes_reserved() {
        assert_eq!(url_encode("/a b/č&x.txt"), "%2Fa%20b%2F%C4%8D%26x.txt");
    }
}
