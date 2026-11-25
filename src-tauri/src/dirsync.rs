//! Synchronize dirs: the comparison command (fxfm_core::sync). Copying runs as normal jobs.

use crate::state::AppState;
use fxfm_core::sync::{self, CompareOptions, Row};
use fxfm_core::vfs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// The running comparison's stop flag (checked between folders). A new comparison stops the
/// previous one; `sync_cancel` stops the current one.
static CURRENT: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// A fresh stop flag for a new comparison; the previous comparison is stopped.
fn new_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(old) = cur.replace(flag.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    flag
}

/// Compares the folder trees `left` and `right` (any locations the panels can list).
#[tauri::command]
pub async fn sync_compare(
    app: AppHandle,
    left: String,
    right: String,
    subdirs: bool,
    ignore_dates: bool,
    by_content: bool,
) -> Result<Vec<Row>, String> {
    let cancel = new_flag();
    let opts = CompareOptions {
        subdirs,
        ignore_dates,
        by_content,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let list = |loc: &str| vfs::list(&state.vfs, loc);
        sync::compare(
            &list,
            &left,
            &right,
            &opts,
            &cancel,
            &sync::same_local_content,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stops a running `sync_compare` (it then fails with "cancelled").
#[tauri::command]
pub fn sync_cancel() {
    if let Some(flag) = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use crate::jobs::{run_job, JobRequest};
    use fxfm_core::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
    use fxfm_core::sync::{compare, same_local_content, CompareOptions, Row, State};
    use fxfm_core::vfs::{self, Ctx};
    use std::fs::{self, File};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, SystemTime};

    /// What the UI's shared "Overwrite all" answers.
    struct OverwriteAll;
    impl OpObserver for OverwriteAll {
        fn progress(&self, _p: &Progress) {}
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            ConflictAnswer::OverwriteAll
        }
        fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
            panic!("{path}: {msg}");
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    fn put(p: &Path, data: &[u8], secs_ago: u64) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
        let t = SystemTime::now() - Duration::from_secs(secs_ago);
        File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn rows(ctx: &Ctx, l: &Path, r: &Path) -> Vec<Row> {
        let opts = CompareOptions {
            subdirs: true,
            ..Default::default()
        };
        compare(
            &|loc: &str| vfs::list(ctx, loc),
            l.to_str().unwrap(),
            r.to_str().unwrap(),
            &opts,
            &AtomicBool::new(false),
            &same_local_content,
        )
        .unwrap()
    }

    /// The jobs ui/lib.mjs `syncPlan` makes for the default directions (one per folder pair,
    /// "dir/" = into the folder).
    fn default_jobs(rows: &[Row]) -> Vec<JobRequest> {
        let mut jobs: Vec<(String, String, Vec<String>)> = Vec::new();
        for row in rows {
            let (src, src_dir, dst_dir) = match row.state {
                State::LeftOnly | State::LeftNewer => (&row.left, &row.left_dir, &row.right_dir),
                State::RightOnly | State::RightNewer => (&row.right, &row.right_dir, &row.left_dir),
                _ => continue,
            };
            let from = fxfm_core::sync::join(src_dir, &src.as_ref().unwrap().name);
            let dest = format!("{}/", dst_dir.trim_end_matches('/'));
            match jobs.iter_mut().find(|j| j.0 == *src_dir && j.1 == dest) {
                Some(j) => j.2.push(from),
                None => jobs.push((src_dir.clone(), dest, vec![from])),
            }
        }
        jobs.into_iter()
            .map(|(cwd, dest, sources)| JobRequest::Copy { sources, dest, cwd })
            .collect()
    }

    #[test]
    fn default_directions_make_both_sides_equal_without_deleting() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        put(&l.join("a.txt"), b"left is newer", 10);
        put(&r.join("a.txt"), b"old", 5000);
        put(&l.join("b.txt"), b"old", 5000);
        put(&r.join("b.txt"), b"right is newer", 10);
        put(&l.join("only-left/deep/x.txt"), b"x", 10);
        put(&r.join("sub/only-right.txt"), b"y", 10);
        fs::create_dir_all(l.join("sub")).unwrap();
        put(&l.join("same.txt"), b"s", 100);
        put(&r.join("same.txt"), b"s", 100);
        put(&l.join("clash"), b"file", 10);
        fs::create_dir_all(r.join("clash")).unwrap();

        for job in default_jobs(&rows(&ctx, &l, &r)) {
            run_job(&ctx, &job, &OverwriteAll).unwrap();
        }

        assert_eq!(fs::read(r.join("a.txt")).unwrap(), b"left is newer");
        assert_eq!(fs::read(l.join("b.txt")).unwrap(), b"right is newer");
        assert_eq!(fs::read(r.join("only-left/deep/x.txt")).unwrap(), b"x");
        assert_eq!(fs::read(l.join("sub/only-right.txt")).unwrap(), b"y");
        // The file/folder clash is left alone on both sides.
        assert_eq!(fs::read(l.join("clash")).unwrap(), b"file");
        assert!(r.join("clash").is_dir());
        let after = rows(&ctx, &l, &r);
        let left_over: Vec<_> = after
            .iter()
            .filter(|row| row.state != State::Equal)
            .map(|row| (row.rel.as_str(), row.state))
            .collect();
        assert_eq!(left_over, [("clash", State::TypeMismatch)]);
    }
}
