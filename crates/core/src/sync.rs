//! Synchronize directories (TC Commands → Synchronize dirs): compares two folder trees by
//! name, size and date (optionally by content) and reports one row per difference. It only
//! compares: copying is done by the normal copy jobs, and nothing is ever deleted.
//!
//! Rules:
//! - names are matched in Unicode NFC and without regard to case (macOS keeps NFD on disk and
//!   ignores case; servers usually NFC and case-sensitive); each side keeps its own spelling for
//!   its paths; two names on one side that differ only in case are a `NameClash` (never copied);
//! - only folders present on both sides are entered; a folder on one side only is one row
//!   (copied whole); a folder that is a symbolic link is never entered (no loops);
//! - dates within `DATE_SLACK` seconds count as equal (FAT, SMB and FTP round them);
//! - `.DS_Store` is left out (Finder rewrites it all the time).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::listing::{Entry, ListError};

/// Dates this close (seconds) are the same date.
pub const DATE_SLACK: i64 = 2;

/// Names never compared.
const SKIPPED: &[&str] = &[".DS_Store"];

#[derive(Clone, Copy, Debug, Default)]
pub struct CompareOptions {
    /// Enter folders present on both sides.
    pub subdirs: bool,
    /// Equal size means equal file; dates are not looked at.
    pub ignore_dates: bool,
    /// Same-size local files are compared byte by byte.
    pub by_content: bool,
}

/// One side of a row: the item's own name there and what the listing said.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Equal,
    LeftNewer,
    RightNewer,
    LeftOnly,
    RightOnly,
    /// Same date (or dates ignored) but another size or content: no side is newer.
    Differs,
    /// A file on one side, a folder on the other: never copied.
    TypeMismatch,
    /// One side has several names that differ only in case (a case-sensitive server or archive);
    /// on macOS they would land on one file, so none of them is copied.
    NameClash,
}

/// One compared name. `leftDir`/`rightDir` are the folders holding it on each side (both exist).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// Path below the compared roots, "/"-separated (the left spelling when both exist).
    pub rel: String,
    pub left_dir: String,
    pub right_dir: String,
    pub left: Option<Item>,
    pub right: Option<Item>,
    pub state: State,
}

/// `dir` + "/" + `name` for local, archive and server locations alike.
pub fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

fn item(e: &Entry) -> Item {
    Item {
        name: e.name.clone(),
        is_dir: e.is_dir,
        size: e.size,
        mtime: e.mtime,
    }
}

/// True when both files are local and byte-for-byte equal; None when that cannot be told
/// (not local, unreadable).
pub fn same_local_content(a: &str, b: &str) -> Option<bool> {
    let (a, b) = (Path::new(a), Path::new(b));
    if !a.is_absolute() || !b.is_absolute() {
        return None;
    }
    let (mut fa, mut fb) = (File::open(a).ok()?, File::open(b).ok()?);
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let na = fill(&mut fa, &mut ba).ok()?;
        let nb = fill(&mut fb, &mut bb).ok()?;
        if na != nb || ba[..na] != bb[..nb] {
            return Some(false);
        }
        if na == 0 {
            return Some(true);
        }
    }
}

/// Reads until `buf` is full or the file ends; returns the byte count.
fn fill(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// State of a name present on both sides.
fn both_state(
    l: &Item,
    r: &Item,
    lpath: &str,
    rpath: &str,
    opts: &CompareOptions,
    same_content: &dyn Fn(&str, &str) -> Option<bool>,
) -> State {
    if l.is_dir != r.is_dir {
        return State::TypeMismatch;
    }
    if l.is_dir {
        return State::Equal;
    }
    let dt = l.mtime - r.mtime;
    let same_date = opts.ignore_dates || dt.abs() <= DATE_SLACK;
    if l.size == r.size {
        // Equal size: the content (when asked and readable) decides over the date.
        if opts.by_content {
            match same_content(lpath, rpath) {
                Some(true) => return State::Equal,
                Some(false) if same_date => return State::Differs,
                _ => {}
            }
        }
        if same_date {
            return State::Equal;
        }
    } else if same_date {
        return State::Differs;
    }
    if dt > 0 {
        State::LeftNewer
    } else {
        State::RightNewer
    }
}

/// Compares the trees under `left` and `right`. `list` lists one folder (vfs::list in the app);
/// `cancel` stops between folders. Rows come in tree order (a folder's own rows before those of
/// its subfolders); equal names are included as `State::Equal`.
pub fn compare(
    list: &dyn Fn(&str) -> Result<Vec<Entry>, ListError>,
    left: &str,
    right: &str,
    opts: &CompareOptions,
    cancel: &AtomicBool,
    same_content: &dyn Fn(&str, &str) -> Option<bool>,
) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    let mut todo = vec![(String::new(), left.to_string(), right.to_string())];
    while let Some((rel, ldir, rdir)) = todo.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let ls = list(&ldir).map_err(|e| e.to_string())?;
        let rs = list(&rdir).map_err(|e| e.to_string())?;
        type Side<'a> = Vec<&'a Entry>;
        let mut by_name: BTreeMap<String, (Side, Side)> = BTreeMap::new();
        let key = |name: &str| name.nfc().collect::<String>().to_lowercase();
        for e in ls.iter().filter(|e| !SKIPPED.contains(&e.name.as_str())) {
            by_name.entry(key(&e.name)).or_default().0.push(e);
        }
        for e in rs.iter().filter(|e| !SKIPPED.contains(&e.name.as_str())) {
            by_name.entry(key(&e.name)).or_default().1.push(e);
        }
        let mut subdirs = Vec::new();
        let rel_of = |name: &str| {
            if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            }
        };
        for (_, (l, r)) in by_name {
            if l.len() > 1 || r.len() > 1 {
                // Every item of the clash is listed, none is copied or entered.
                for (left, right) in l
                    .iter()
                    .map(|e| (Some(*e), None))
                    .chain(r.iter().map(|e| (None, Some(*e))))
                {
                    let name = left
                        .or(right)
                        .map(|e: &Entry| e.name.as_str())
                        .unwrap_or("");
                    rows.push(Row {
                        rel: rel_of(name),
                        left_dir: ldir.clone(),
                        right_dir: rdir.clone(),
                        left: left.map(item),
                        right: right.map(item),
                        state: State::NameClash,
                    });
                }
                continue;
            }
            let (l, r) = (l.first().copied(), r.first().copied());
            let Some(first) = l.or(r) else { continue };
            let row_rel = rel_of(&first.name);
            let state = match (l, r) {
                (Some(l), Some(r)) => both_state(
                    &item(l),
                    &item(r),
                    &join(&ldir, &l.name),
                    &join(&rdir, &r.name),
                    opts,
                    same_content,
                ),
                (Some(_), None) => State::LeftOnly,
                _ => State::RightOnly,
            };
            if let (Some(l), Some(r)) = (l, r) {
                // Only real folders on both sides are entered.
                if opts.subdirs && l.is_dir && r.is_dir && !l.is_symlink && !r.is_symlink {
                    subdirs.push((row_rel.clone(), join(&ldir, &l.name), join(&rdir, &r.name)));
                }
            }
            rows.push(Row {
                rel: row_rel,
                left_dir: ldir.clone(),
                right_dir: rdir.clone(),
                left: l.map(item),
                right: r.map(item),
                state,
            });
        }
        // Depth first, in name order.
        subdirs.reverse();
        todo.extend(subdirs);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::list_dir;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn touch(p: &Path, data: &[u8], secs_ago: u64) {
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(p, data).unwrap();
        let t = SystemTime::now() - Duration::from_secs(secs_ago);
        File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn run(l: &Path, r: &Path, opts: CompareOptions) -> Vec<(String, State)> {
        let list = |loc: &str| list_dir(Path::new(loc));
        compare(
            &list,
            l.to_str().unwrap(),
            r.to_str().unwrap(),
            &opts,
            &AtomicBool::new(false),
            &same_local_content,
        )
        .unwrap()
        .into_iter()
        .map(|row| (row.rel, row.state))
        .collect()
    }

    const SUB: CompareOptions = CompareOptions {
        subdirs: true,
        ignore_dates: false,
        by_content: false,
    };

    #[test]
    fn states_by_size_and_date() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        touch(&l.join("same.txt"), b"abc", 100);
        touch(&r.join("same.txt"), b"abc", 101); // within the slack
        touch(&l.join("newer.txt"), b"new", 10);
        touch(&r.join("newer.txt"), b"old", 1000);
        touch(&l.join("older.txt"), b"old", 1000);
        touch(&r.join("older.txt"), b"new!", 10);
        touch(&l.join("size.txt"), b"a", 50);
        touch(&r.join("size.txt"), b"ab", 50);
        touch(&l.join("only-left.txt"), b"x", 5);
        touch(&r.join("only-right.txt"), b"x", 5);
        touch(&l.join(".DS_Store"), b"x", 5);
        let got = run(&l, &r, SUB);
        assert_eq!(
            got,
            [
                ("newer.txt".into(), State::LeftNewer),
                ("older.txt".into(), State::RightNewer),
                ("only-left.txt".into(), State::LeftOnly),
                ("only-right.txt".into(), State::RightOnly),
                ("same.txt".into(), State::Equal),
                ("size.txt".into(), State::Differs),
            ]
        );
    }

    #[test]
    fn folders_shared_are_entered_one_sided_are_one_row() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        touch(&l.join("d/inner.txt"), b"1", 5);
        fs::create_dir_all(r.join("d")).unwrap();
        touch(&l.join("lonely/deep/x.txt"), b"1", 5);
        touch(&l.join("clash"), b"file", 5);
        fs::create_dir_all(r.join("clash")).unwrap();
        let got = run(&l, &r, SUB);
        assert_eq!(
            got,
            [
                ("clash".into(), State::TypeMismatch),
                ("d".into(), State::Equal),
                ("lonely".into(), State::LeftOnly),
                ("d/inner.txt".into(), State::LeftOnly),
            ]
        );
        // Without subfolders the shared folder is not entered.
        let flat = run(&l, &r, CompareOptions::default());
        assert!(flat.iter().all(|(rel, _)| !rel.contains('/')));
    }

    #[test]
    fn content_and_ignored_dates() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        touch(&l.join("touched.txt"), b"same", 10);
        touch(&r.join("touched.txt"), b"same", 5000);
        touch(&l.join("edited.txt"), b"aaaa", 100);
        touch(&r.join("edited.txt"), b"bbbb", 100);
        let content = CompareOptions {
            by_content: true,
            ..SUB
        };
        assert_eq!(
            run(&l, &r, content),
            [
                ("edited.txt".into(), State::Differs),
                ("touched.txt".into(), State::Equal),
            ]
        );
        let dates_off = CompareOptions {
            ignore_dates: true,
            ..SUB
        };
        assert_eq!(
            run(&l, &r, dates_off),
            [
                ("edited.txt".into(), State::Equal),
                ("touched.txt".into(), State::Equal),
            ]
        );
    }

    #[test]
    fn nfc_and_nfd_names_match_and_keep_their_spelling() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        fs::create_dir_all(&l).unwrap();
        fs::create_dir_all(&r).unwrap();
        let rows = compare(
            &|loc: &str| {
                let name = if loc.ends_with('L') {
                    "c\u{30c}.txt"
                } else {
                    "\u{10d}.txt"
                };
                Ok(vec![Entry {
                    name: name.into(),
                    is_dir: false,
                    is_symlink: false,
                    size: 1,
                    mtime: 0,
                    mode: 0,
                    hidden: false,
                }])
            },
            l.to_str().unwrap(),
            r.to_str().unwrap(),
            &SUB,
            &AtomicBool::new(false),
            &|_, _| None,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, State::Equal);
        assert_eq!(rows[0].left.as_ref().unwrap().name, "c\u{30c}.txt");
        assert_eq!(rows[0].right.as_ref().unwrap().name, "\u{10d}.txt");
    }

    fn fake(
        names_l: &'static [&'static str],
        names_r: &'static [&'static str],
    ) -> Vec<(String, State)> {
        let entry = |n: &str| Entry {
            name: n.into(),
            is_dir: false,
            is_symlink: false,
            size: 1,
            mtime: 0,
            mode: 0,
            hidden: false,
        };
        compare(
            &|loc: &str| {
                Ok((if loc == "/L" { names_l } else { names_r })
                    .iter()
                    .map(|n| entry(n))
                    .collect())
            },
            "/L",
            "/R",
            &SUB,
            &AtomicBool::new(false),
            &|_, _| None,
        )
        .unwrap()
        .into_iter()
        .map(|row| (row.rel, row.state))
        .collect()
    }

    #[test]
    fn case_only_differences_are_one_row_and_clashes_are_never_copied() {
        // Local "readme" and a server's "README" are the same file on macOS: one row.
        assert_eq!(
            fake(&["readme"], &["README"]),
            [("readme".into(), State::Equal)]
        );
        // A case-sensitive side with both names: listed, never copied.
        assert_eq!(
            fake(&["a.txt"], &["A.txt", "a.txt"]),
            [
                ("a.txt".into(), State::NameClash),
                ("A.txt".into(), State::NameClash),
                ("a.txt".into(), State::NameClash),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_not_entered() {
        let t = tempfile::tempdir().unwrap();
        let (l, r) = (t.path().join("L"), t.path().join("R"));
        touch(&l.join("real/x.txt"), b"1", 5);
        fs::create_dir_all(r.join("loop")).unwrap();
        std::os::unix::fs::symlink(&l, l.join("loop")).unwrap();
        let got = run(&l, &r, SUB);
        assert!(
            got.iter().all(|(rel, _)| !rel.starts_with("loop/")),
            "{got:?}"
        );
    }

    #[test]
    fn cancel_and_list_errors_stop() {
        let t = tempfile::tempdir().unwrap();
        let list = |loc: &str| list_dir(Path::new(loc));
        let l = t.path().to_str().unwrap();
        let stop = AtomicBool::new(true);
        let none = |_: &str, _: &str| None;
        assert!(compare(&list, l, l, &SUB, &stop, &none).is_err());
        let missing = t.path().join("missing");
        let go = AtomicBool::new(false);
        assert!(compare(&list, l, missing.to_str().unwrap(), &SUB, &go, &none).is_err());
    }

    #[test]
    fn content_compare_needs_local_files() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        fs::write(&a, vec![7u8; 200_000]).unwrap();
        fs::write(&b, vec![7u8; 200_000]).unwrap();
        let mut other = vec![7u8; 200_000];
        other[199_999] = 8;
        fs::write(&c, other).unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_string();
        assert_eq!(same_local_content(&s(&a), &s(&b)), Some(true));
        assert_eq!(same_local_content(&s(&a), &s(&c)), Some(false));
        assert_eq!(same_local_content("sftp://x/a", &s(&a)), None);
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("sftp://srv/", "a"), "sftp://srv/a");
    }
}
