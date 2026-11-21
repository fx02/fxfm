//! Multi-rename (TC Ctrl+M): renames many items at once, each within its own folder.
//!
//! Every item first goes to a temporary name and then to its new name, so names can be
//! swapped or shifted (a→b, b→c) and case-only changes work on case-insensitive volumes.
//! Nothing is ever overwritten: a new name that belongs to an item outside the batch is
//! refused for that item.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::ops::{rename_noreplace, valid_name};

/// One item to rename: its current path and its new name (no folder part).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenameItem {
    pub path: PathBuf,
    pub new_name: String,
}

/// What happened to one item: its path now (`Ok`, also when unchanged) or why not.
pub type RenameResult = Result<PathBuf, String>;

/// Case- and normalization-insensitive key of a path (APFS default, SMB, exFAT).
fn key(p: &Path) -> String {
    p.to_string_lossy().nfc().collect::<String>().to_lowercase()
}

fn temp_name(dir: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".fxfm-rename-{}-{n}.tmp", std::process::id()))
}

/// Renames `items` in order; the result has one entry per item, in the same order.
pub fn multi_rename(items: &[RenameItem]) -> Vec<RenameResult> {
    let mut out: Vec<Option<RenameResult>> = vec![None; items.len()];
    let mut targets: Vec<Option<PathBuf>> = vec![None; items.len()];

    for (i, it) in items.iter().enumerate() {
        let name = it.new_name.as_str();
        if !valid_name(name) {
            out[i] = Some(Err(format!("'{name}' is not a valid name")));
            continue;
        }
        let Some(parent) = it.path.parent() else {
            out[i] = Some(Err("no folder".into()));
            continue;
        };
        let target = parent.join(name);
        if target == it.path {
            out[i] = Some(Ok(it.path.clone()));
            continue;
        }
        if fs::symlink_metadata(&it.path).is_err() {
            out[i] = Some(Err(format!("{} no longer exists", it.path.display())));
            continue;
        }
        targets[i] = Some(target);
    }

    // Two items may not end up with the same name.
    let mut by_target: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, t) in targets.iter().enumerate() {
        if let Some(t) = t {
            by_target.entry(key(t)).or_default().push(i);
        }
    }
    for ids in by_target.values().filter(|v| v.len() > 1) {
        for &i in ids {
            let name = &items[i].new_name;
            out[i] = Some(Err(format!("more than one item would be named '{name}'")));
            targets[i] = None;
        }
    }

    // A new name that exists must belong to an item of this batch that moves away.
    let leaving: HashMap<String, usize> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_some())
        .map(|(i, _)| (key(&items[i].path), i))
        .collect();
    for i in 0..items.len() {
        let Some(t) = &targets[i] else { continue };
        if fs::symlink_metadata(t).is_ok() && !leaving.contains_key(&key(t)) {
            out[i] = Some(Err(format!("'{}' already exists", items[i].new_name)));
            targets[i] = None;
        }
    }

    // Phase 1: every item to a temporary name.
    let mut temps: Vec<Option<PathBuf>> = vec![None; items.len()];
    for i in 0..items.len() {
        if targets[i].is_none() {
            continue;
        }
        let tmp = temp_name(items[i].path.parent().unwrap_or(Path::new("/")));
        match rename_noreplace(&items[i].path, &tmp) {
            Ok(()) => temps[i] = Some(tmp),
            Err(e) => {
                out[i] = Some(Err(e.to_string()));
                targets[i] = None;
            }
        }
    }

    // Phase 2: temporary names to the new names; a failure goes back to the old name.
    for i in 0..items.len() {
        let (Some(tmp), Some(target)) = (&temps[i], &targets[i]) else {
            continue;
        };
        match rename_noreplace(tmp, target) {
            Ok(()) => out[i] = Some(Ok(target.clone())),
            Err(e) => {
                let back = rename_noreplace(tmp, &items[i].path);
                out[i] = Some(Err(match back {
                    Ok(()) => e.to_string(),
                    Err(b) => format!("{e}; left as {} ({b})", tmp.display()),
                }));
            }
        }
    }

    out.into_iter()
        .map(|r| r.unwrap_or_else(|| Err("not renamed".into())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(dir: &Path, from: &str, to: &str) -> RenameItem {
        RenameItem {
            path: dir.join(from),
            new_name: to.into(),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn touch(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn renames_and_reports_new_paths() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "a.txt", "A");
        touch(t.path(), "b.txt", "B");
        let r = multi_rename(&[
            item(t.path(), "a.txt", "1.txt"),
            item(t.path(), "b.txt", "2.txt"),
        ]);
        assert_eq!(
            r,
            vec![Ok(t.path().join("1.txt")), Ok(t.path().join("2.txt"))]
        );
        assert_eq!(names(t.path()), ["1.txt", "2.txt"]);
        assert_eq!(fs::read_to_string(t.path().join("1.txt")).unwrap(), "A");
    }

    #[test]
    fn swaps_and_shifts_names() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "a", "A");
        touch(t.path(), "b", "B");
        touch(t.path(), "c", "C");
        let r = multi_rename(&[
            item(t.path(), "a", "b"),
            item(t.path(), "b", "c"),
            item(t.path(), "c", "a"),
        ]);
        assert!(r.iter().all(|x| x.is_ok()), "{r:?}");
        assert_eq!(fs::read_to_string(t.path().join("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(t.path().join("c")).unwrap(), "B");
        assert_eq!(fs::read_to_string(t.path().join("a")).unwrap(), "C");
        assert_eq!(names(t.path()), ["a", "b", "c"]);
    }

    #[test]
    fn case_only_change_works() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "readme.md", "x");
        let r = multi_rename(&[item(t.path(), "readme.md", "README.md")]);
        assert_eq!(r, vec![Ok(t.path().join("README.md"))]);
        assert_eq!(names(t.path()), ["README.md"]);
    }

    #[test]
    fn never_overwrites_an_outside_file() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "a", "A");
        touch(t.path(), "keep", "K");
        let r = multi_rename(&[item(t.path(), "a", "keep")]);
        assert!(r[0].as_ref().unwrap_err().contains("already exists"));
        assert_eq!(fs::read_to_string(t.path().join("keep")).unwrap(), "K");
        assert_eq!(names(t.path()), ["a", "keep"]);
    }

    #[test]
    fn duplicate_and_invalid_names_are_refused() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "a", "A");
        touch(t.path(), "b", "B");
        touch(t.path(), "c", "C");
        let r = multi_rename(&[
            item(t.path(), "a", "x"),
            item(t.path(), "b", "X"),
            item(t.path(), "c", "d/e"),
        ]);
        assert!(r[0].as_ref().unwrap_err().contains("more than one"));
        assert!(r[1].as_ref().unwrap_err().contains("more than one"));
        assert!(r[2].as_ref().unwrap_err().contains("not a valid name"));
        assert_eq!(names(t.path()), ["a", "b", "c"]);
    }

    #[test]
    fn unchanged_and_missing_items() {
        let t = tempfile::tempdir().unwrap();
        touch(t.path(), "a", "A");
        let r = multi_rename(&[item(t.path(), "a", "a"), item(t.path(), "gone", "x")]);
        assert_eq!(r[0], Ok(t.path().join("a")));
        assert!(r[1].as_ref().unwrap_err().contains("no longer exists"));
    }

    #[test]
    fn items_in_different_folders() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("d1")).unwrap();
        fs::create_dir(t.path().join("d2")).unwrap();
        touch(&t.path().join("d1"), "f", "1");
        touch(&t.path().join("d2"), "f", "2");
        let r = multi_rename(&[
            item(&t.path().join("d1"), "f", "g"),
            item(&t.path().join("d2"), "f", "g"),
        ]);
        assert!(r.iter().all(|x| x.is_ok()), "{r:?}");
        assert_eq!(fs::read_to_string(t.path().join("d2/g")).unwrap(), "2");
    }
}
