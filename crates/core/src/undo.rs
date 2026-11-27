//! Undo of the last local file operation (Ctrl+Z): items moved or put in the Trash go back to
//! where they were, a renamed item gets its old name, a new empty folder is removed. Nothing is
//! ever overwritten: an item whose old place is taken again stays where it is (and is reported).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ops::{self, ConflictAnswer, ConflictInfo, Dest, ErrorAnswer, OpObserver, Progress};

/// What Undo does: move each `(now, was)` back (in reverse order), then remove `remove_dirs`
/// when they are still empty.
#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UndoPlan {
    #[serde(default)]
    pub moves: Vec<(String, String)>,
    #[serde(default)]
    pub remove_dirs: Vec<String>,
}

#[derive(Serialize, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub done: usize,
    /// One line per item that could not be put back.
    pub failed: Vec<String>,
}

/// Never asks: a conflict or an error leaves the item where it is.
struct Quiet;
impl OpObserver for Quiet {
    fn progress(&self, _p: &Progress) {}
    fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
        ConflictAnswer::Skip
    }
    fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
        ErrorAnswer::Skip
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Moves `now` back to `was`; another volume (EXDEV) copies and deletes like F6.
fn put_back(now: &Path, was: &Path) -> Result<(), String> {
    if !exists(now) {
        return Err(format!("{}: no longer there", now.display()));
    }
    if exists(was) {
        return Err(format!("{}: something new is there now", was.display()));
    }
    if let Some(parent) = was.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    match fs::rename(now, was) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            ops::move_(
                &[now.to_path_buf()],
                &Dest::AsFile(was.to_path_buf()),
                &Quiet,
            )
            .map_err(|e| e.to_string())?;
            if exists(was) && !exists(now) {
                Ok(())
            } else {
                Err(format!("{}: could not be moved back", now.display()))
            }
        }
        Err(e) => Err(format!("{}: {e}", now.display())),
    }
}

pub fn undo(plan: &UndoPlan) -> UndoResult {
    let mut out = UndoResult::default();
    for (now, was) in plan.moves.iter().rev() {
        match put_back(Path::new(now), Path::new(was)) {
            Ok(()) => out.done += 1,
            Err(e) => out.failed.push(e),
        }
    }
    for dir in &plan.remove_dirs {
        let p = PathBuf::from(dir);
        // remove_dir refuses a folder that is not empty: what was put in it stays.
        match fs::remove_dir(&p) {
            Ok(()) => out.done += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => out.failed.push(format!("{}: {e}", p.display())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::OpSummary;

    fn moves(s: &OpSummary) -> UndoPlan {
        UndoPlan {
            moves: s.undo.clone(),
            remove_dirs: vec![],
        }
    }

    #[test]
    fn a_move_goes_back_and_a_taken_place_is_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        fs::create_dir_all(a.join("d")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("x.txt"), b"x").unwrap();
        fs::write(a.join("d/y.txt"), b"y").unwrap();
        fs::write(a.join("z.txt"), b"z").unwrap();
        fs::write(b.join("z.txt"), b"old z").unwrap(); // replaced: not undoable
        struct Overwrite;
        impl OpObserver for Overwrite {
            fn progress(&self, _p: &Progress) {}
            fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
                ConflictAnswer::Overwrite
            }
            fn error(&self, _p: &str, _m: &str) -> ErrorAnswer {
                ErrorAnswer::Skip
            }
            fn is_cancelled(&self) -> bool {
                false
            }
        }
        let srcs = vec![a.join("x.txt"), a.join("d"), a.join("z.txt")];
        let s = ops::move_(&srcs, &Dest::IntoDir(b.clone()), &Overwrite).unwrap();
        assert_eq!(s.undo.len(), 2, "{:?}", s.undo);
        // Something new appears where x.txt was: that one stays.
        fs::write(a.join("x.txt"), b"new").unwrap();
        let r = undo(&moves(&s));
        assert_eq!(r.done, 1);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(fs::read(a.join("d/y.txt")).unwrap(), b"y");
        assert_eq!(fs::read(a.join("x.txt")).unwrap(), b"new");
        assert_eq!(fs::read(b.join("x.txt")).unwrap(), b"x");
        assert_eq!(fs::read(b.join("z.txt")).unwrap(), b"z");
    }

    #[test]
    fn a_new_folder_is_removed_only_while_empty() {
        let t = tempfile::tempdir().unwrap();
        let (e, f) = (t.path().join("empty"), t.path().join("full"));
        fs::create_dir(&e).unwrap();
        fs::create_dir(&f).unwrap();
        fs::write(f.join("keep"), b"k").unwrap();
        let r = undo(&UndoPlan {
            moves: vec![],
            remove_dirs: vec![e.to_string_lossy().into(), f.to_string_lossy().into()],
        });
        assert_eq!(r.done, 1);
        assert_eq!(r.failed.len(), 1);
        assert!(!e.exists() && f.join("keep").exists());
    }

    /// Uses the real Trash (as the ops Trash test does): the file goes there and comes back.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "touches the user's Trash"]
    fn a_trashed_file_comes_back() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("fxfm-undo-test.txt");
        fs::write(&f, b"keep me").unwrap();
        let s = ops::delete(std::slice::from_ref(&f), true, &Quiet).unwrap();
        assert!(!f.exists());
        assert_eq!(s.undo.len(), 1, "{:?}", s.undo);
        let r = undo(&moves(&s));
        assert_eq!(
            r,
            UndoResult {
                done: 1,
                failed: vec![]
            }
        );
        assert_eq!(fs::read(&f).unwrap(), b"keep me");
    }
}
