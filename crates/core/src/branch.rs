//! Branch view (TC Ctrl+B): every file below a folder as one flat list. Folders that are
//! symbolic links are not entered (no loops), archives stay files, unreadable folders are
//! skipped and counted.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::listing::{Entry, ListError};
use crate::sync::join;

/// At most this many files are listed.
pub const BRANCH_MAX: usize = 100_000;

/// A file found below the root: its path relative to the root ("/"-separated) and its entry.
pub struct Found {
    pub rel: String,
    pub entry: Entry,
}

/// What a walk found: the files (folder by folder, names in listing order), whether the
/// `max` cap cut it short, and how many folders could not be listed.
pub struct Branch {
    pub files: Vec<Found>,
    pub truncated: bool,
    pub unreadable: usize,
}

/// Lists every file below `root` with `list` (vfs::list in the app). The root itself must be
/// listable; `cancel` stops between folders with an error.
pub fn walk(
    list: &dyn Fn(&str) -> Result<Vec<Entry>, ListError>,
    root: &str,
    max: usize,
    cancel: &AtomicBool,
) -> Result<Branch, String> {
    let mut out = Branch {
        files: Vec::new(),
        truncated: false,
        unreadable: 0,
    };
    let mut todo = vec![(String::new(), root.to_string())];
    while let Some((rel, dir)) = todo.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let entries = match list(&dir) {
            Ok(v) => v,
            Err(e) if rel.is_empty() => return Err(e.to_string()),
            Err(_) => {
                out.unreadable += 1;
                continue;
            }
        };
        let mut subdirs = Vec::new();
        for e in entries {
            let child = if rel.is_empty() {
                e.name.clone()
            } else {
                format!("{rel}/{}", e.name)
            };
            if e.is_dir {
                if !e.is_symlink {
                    subdirs.push((child, join(&dir, &e.name)));
                }
                continue;
            }
            if out.files.len() == max {
                out.truncated = true;
                return Ok(out);
            }
            out.files.push(Found {
                rel: child,
                entry: e,
            });
        }
        // Depth first, folders in listing order.
        subdirs.reverse();
        todo.extend(subdirs);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::list_dir;
    use std::fs;
    use std::path::Path;

    fn rels(b: &Branch) -> Vec<&str> {
        b.files.iter().map(|f| f.rel.as_str()).collect()
    }

    #[test]
    fn files_of_all_subfolders_no_folders_no_links() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("a/deep")).unwrap();
        fs::create_dir_all(r.join("b")).unwrap();
        fs::write(r.join("top.txt"), b"1").unwrap();
        fs::write(r.join("a/x.txt"), b"1").unwrap();
        fs::write(r.join("a/deep/y.zip"), b"1").unwrap();
        fs::write(r.join("b/z.txt"), b"1").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(r, r.join("b/loop")).unwrap();
        let list = |loc: &str| list_dir(Path::new(loc));
        let got = walk(&list, r.to_str().unwrap(), 100, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            rels(&got),
            ["top.txt", "a/x.txt", "a/deep/y.zip", "b/z.txt"]
        );
        assert!(!got.truncated);
        assert_eq!(got.unreadable, 0);
    }

    #[test]
    fn cap_cancel_and_errors() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(t.path().join(format!("f{i}")), b"1").unwrap();
        }
        let list = |loc: &str| list_dir(Path::new(loc));
        let root = t.path().to_str().unwrap();
        let capped = walk(&list, root, 3, &AtomicBool::new(false)).unwrap();
        assert_eq!(capped.files.len(), 3);
        assert!(capped.truncated);
        assert!(walk(&list, root, 10, &AtomicBool::new(true)).is_err());
        let missing = t.path().join("missing");
        assert!(walk(
            &list,
            missing.to_str().unwrap(),
            10,
            &AtomicBool::new(false)
        )
        .is_err());
        // An unreadable subfolder is skipped and counted.
        let flaky = |loc: &str| {
            if loc.ends_with("/bad") {
                Err(ListError::PermissionDenied(loc.into()))
            } else if loc == "/r" {
                Ok(vec![dir("bad"), file("ok")])
            } else {
                Ok(vec![])
            }
        };
        let got = walk(&flaky, "/r", 10, &AtomicBool::new(false)).unwrap();
        assert_eq!(rels(&got), ["ok"]);
        assert_eq!(got.unreadable, 1);
    }

    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            is_dir: false,
            is_symlink: false,
            size: 1,
            mtime: 0,
            mode: 0,
            hidden: false,
        }
    }

    fn dir(name: &str) -> Entry {
        Entry {
            is_dir: true,
            ..file(name)
        }
    }
}
