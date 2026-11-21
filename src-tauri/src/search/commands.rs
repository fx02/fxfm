//! Tauri commands of the search service (see `search.rs` for the JSON shapes).

use super::{SearchReq, Status};
use crate::state::{lock, AppState};
use fxfm_core::config::SETTINGS_FILE;
use fxfm_core::storage;
use tauri::{AppHandle, Manager, State};

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_status(app: AppHandle) -> Result<Status, String> {
    blocking(move || app.state::<AppState>().search.status()).await
}

/// Turns the index on (saves `search.index = true`) and starts the startup-disk scan.
#[tauri::command]
pub async fn search_build(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        {
            let data = lock(&state.data);
            let mut cur = lock(&state.settings);
            let mut next = cur.clone();
            next.search.index = Some(true);
            storage::save_json(&data.dir, SETTINGS_FILE, &next).map_err(|e| e.to_string())?;
            *cur = next;
        }
        state.search.build();
        Ok(())
    })
    .await?
}

/// Starts a search and returns its id; results arrive as `search://batch` events. Takes no
/// index lock (the search thread does), so it is safe on the main thread.
#[tauri::command]
pub fn search_start(state: State<'_, AppState>, req: SearchReq) -> Result<u64, String> {
    state.search.search_start(req)
}

/// Only takes the short `current` mutex.
#[tauri::command]
pub fn search_cancel(state: State<'_, AppState>, id: u64) {
    state.search.search_cancel(id);
}

// The commands below may wait on the index lock (a save or a merge holds it for a moment),
// so they run on a blocking thread, never on the main thread.

#[tauri::command]
pub async fn search_forget(app: AppHandle, path: String) -> Result<(), String> {
    blocking(move || app.state::<AppState>().search.forget(&path)).await
}

#[tauri::command]
pub async fn search_rebuild(app: AppHandle, path: Option<String>) -> Result<(), String> {
    blocking(move || app.state::<AppState>().search.rebuild(path)).await
}

#[tauri::command]
pub async fn search_refresh(app: AppHandle, path: String) -> Result<(), String> {
    blocking(move || app.state::<AppState>().search.refresh(&path)).await
}
