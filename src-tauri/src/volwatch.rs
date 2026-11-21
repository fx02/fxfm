//! Polls mounted volumes and emits `volumes://changed` when the set of paths changes.

use fxfm_core::volumes::{list_volumes, Volume};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const INTERVAL: Duration = Duration::from_secs(2);

fn paths(v: &[Volume]) -> BTreeSet<String> {
    v.iter().map(|x| x.path.clone()).collect()
}

pub fn start(app: AppHandle, home: PathBuf) {
    std::thread::spawn(move || {
        let mut known = paths(&list_volumes(&home));
        loop {
            std::thread::sleep(INTERVAL);
            let now = list_volumes(&home);
            let set = paths(&now);
            if set != known {
                known = set;
                if let Err(e) = app.emit("volumes://changed", now) {
                    eprintln!("fxfm: emit volumes://changed: {e}");
                }
                // Enabled search volumes that were just mounted get scanned.
                app.state::<crate::state::AppState>()
                    .search
                    .volumes_changed();
            }
        }
    });
}
