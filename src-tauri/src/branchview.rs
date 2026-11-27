//! Branch view (TC Ctrl+B): the files below a folder as one list (fxfm_core::branch).

use crate::state::AppState;
use fxfm_core::branch::{self, BRANCH_MAX};
use fxfm_core::sync::join;
use fxfm_core::vfs::{self, Loc};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// One listed file; the UI shows `rel` and acts on `path`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BranchItem {
    pub path: String,
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
    pub in_archive: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub items: Vec<BranchItem>,
    pub truncated: bool,
    pub unreadable: usize,
}

/// The running walk's stop flag; a new walk stops the previous one.
static CURRENT: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Every file below `location` (local folder, archive folder or server folder), at most
/// BRANCH_MAX.
#[tauri::command]
pub async fn branch_list(app: AppHandle, location: String) -> Result<BranchList, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(old) = CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(cancel.clone())
    {
        old.store(true, Ordering::Relaxed);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let in_archive = matches!(vfs::parse(&location), Loc::Archive(_));
        let found = branch::walk(
            &|loc: &str| vfs::list(&state.vfs, loc),
            &location,
            BRANCH_MAX,
            &cancel,
        )?;
        Ok(BranchList {
            items: found
                .files
                .into_iter()
                .map(|f| BranchItem {
                    path: join(&location, &f.rel),
                    size: f.entry.size,
                    mtime: f.entry.mtime,
                    in_archive,
                    rel: f.rel,
                })
                .collect(),
            truncated: found.truncated,
            unreadable: found.unreadable,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stops a running `branch_list` (it then fails with "cancelled").
#[tauri::command]
pub fn branch_cancel() {
    if let Some(flag) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}
