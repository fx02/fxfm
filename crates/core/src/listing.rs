//! Local directory listing.

use std::fs;
use std::io;
use std::path::Path;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Unix seconds.
    pub mtime: i64,
    pub mode: u32,
    pub hidden: bool,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ListError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("i/o error: {0}")]
    Io(String),
}

impl Serialize for ListError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (kind, path) = match self {
            ListError::NotFound(p) => ("notFound", p),
            ListError::PermissionDenied(p) => ("permissionDenied", p),
            ListError::NotADirectory(p) => ("notADirectory", p),
            ListError::Io(p) => ("io", p),
        };
        let mut st = s.serialize_struct("ListError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("path", path)?;
        st.end()
    }
}

fn map_err(e: &io::Error, path: &Path) -> ListError {
    let p = path.to_string_lossy().into_owned();
    match e.kind() {
        io::ErrorKind::NotFound => ListError::NotFound(p),
        io::ErrorKind::PermissionDenied => ListError::PermissionDenied(p),
        io::ErrorKind::NotADirectory => ListError::NotADirectory(p),
        _ if e.raw_os_error() == Some(libc::ENOTDIR) => ListError::NotADirectory(p),
        _ => ListError::Io(p),
    }
}

#[cfg(target_os = "macos")]
fn flag_hidden(md: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    md.st_flags() & libc::UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn flag_hidden(_md: &fs::Metadata) -> bool {
    false
}

fn mtime_of(md: &fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    md.mtime()
}

fn mode_of(md: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    md.mode()
}

/// Lists `path` (without `.` and `..`), sorted with `sort_entries`.
pub fn list_dir(path: &Path) -> Result<Vec<Entry>, ListError> {
    let rd = fs::read_dir(path).map_err(|e| map_err(&e, path))?;
    let mut out = Vec::new();
    for de in rd {
        let de = de.map_err(|e| map_err(&e, path))?;
        let name = de.file_name().to_string_lossy().into_owned();
        let Ok(lmd) = de.metadata() else {
            // Vanished or unreadable entry: show it as an empty file.
            out.push(Entry {
                hidden: name.starts_with('.'),
                name,
                is_dir: false,
                is_symlink: false,
                size: 0,
                mtime: 0,
                mode: 0,
            });
            continue;
        };
        let is_symlink = lmd.file_type().is_symlink();
        // Symlinks report their target's attributes; broken ones stay as the link itself
        // with size 0 and are not directories.
        let (md, broken) = if is_symlink {
            match fs::metadata(de.path()) {
                Ok(t) => (t, false),
                Err(_) => (lmd.clone(), true),
            }
        } else {
            (lmd.clone(), false)
        };
        out.push(Entry {
            hidden: name.starts_with('.') || flag_hidden(&lmd),
            is_dir: !broken && md.is_dir(),
            is_symlink,
            size: if broken { 0 } else { md.len() },
            mtime: mtime_of(&md),
            mode: mode_of(&md),
            name,
        });
    }
    sort_entries(&mut out);
    Ok(out)
}

/// Directories first; within a group by `name.nfc().to_lowercase()`, ties by original name.
pub fn sort_entries(v: &mut [Entry]) {
    v.sort_by_cached_key(|e| {
        (
            !e.is_dir,
            e.name.nfc().collect::<String>().to_lowercase(),
            e.name.clone(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir, File};
    use std::time::Instant;

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn list_dir_sorts_dirs_first() {
        let d = tempfile::tempdir().unwrap();
        create_dir(d.path().join("b")).unwrap();
        create_dir(d.path().join("A")).unwrap();
        File::create(d.path().join("c.txt")).unwrap();
        File::create(d.path().join(".hidden")).unwrap();
        let v = list_dir(d.path()).unwrap();
        assert_eq!(names(&v), ["A", "b", ".hidden", "c.txt"]);
        assert!(v[2].hidden);
        assert!(!v[3].hidden);
    }

    #[test]
    fn sort_is_case_insensitive_and_nfc() {
        let mk = |n: &str| Entry {
            name: n.into(),
            is_dir: false,
            is_symlink: false,
            size: 0,
            mtime: 0,
            mode: 0,
            hidden: false,
        };
        let mut v = vec![mk("dom"), mk("Cesta"), mk("c\u{30C}aj"), mk("Ante")];
        sort_entries(&mut v);
        // "dom" discriminates NFC: without .nfc() the NFD name would sort before it.
        assert_eq!(names(&v), ["Ante", "Cesta", "dom", "c\u{30C}aj"]);
        let mut w = vec![
            mk("\u{10D}aj"),
            mk("dom"),
            mk("Cesta"),
            mk("c\u{30C}aj"),
            mk("Ante"),
        ];
        sort_entries(&mut w);
        // NFC and NFD spellings of the same name sort together.
        assert_eq!(names(&w)[..2], ["Ante", "Cesta"]);
        assert_eq!(names(&w)[2], "dom");
        assert_eq!(names(&w)[3..], ["c\u{30C}aj", "\u{10D}aj"]);
    }

    #[test]
    fn list_dir_reports_symlinks() {
        let d = tempfile::tempdir().unwrap();
        create_dir(d.path().join("real")).unwrap();
        std::os::unix::fs::symlink(d.path().join("real"), d.path().join("ln")).unwrap();
        std::os::unix::fs::symlink(d.path().join("nope"), d.path().join("broken")).unwrap();
        let v = list_dir(d.path()).unwrap();
        let ln = v.iter().find(|e| e.name == "ln").unwrap();
        assert!(ln.is_dir && ln.is_symlink);
        let br = v.iter().find(|e| e.name == "broken").unwrap();
        assert!(!br.is_dir && br.is_symlink && br.size == 0);
    }

    #[test]
    fn list_dir_permission_denied_is_typed_error() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("locked");
        create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
        let r = list_dir(&p);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            r,
            Err(ListError::PermissionDenied(
                p.to_string_lossy().into_owned()
            ))
        );
    }

    #[test]
    fn list_dir_missing_is_not_found() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("missing");
        assert!(matches!(list_dir(&p), Err(ListError::NotFound(_))));
    }

    #[test]
    fn list_dir_on_file_is_not_a_directory() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        File::create(&p).unwrap();
        assert!(matches!(list_dir(&p), Err(ListError::NotADirectory(_))));
    }

    #[test]
    fn list_error_serializes_kind_and_path() {
        let j = serde_json::to_value(ListError::PermissionDenied("/x".into())).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"kind": "permissionDenied", "path": "/x"})
        );
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore)]
    fn list_dir_50k_entries_is_fast() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..50_000 {
            File::create(d.path().join(format!("f{i:05}.txt"))).unwrap();
        }
        let t = Instant::now();
        let v = list_dir(d.path()).unwrap();
        let el = t.elapsed();
        assert_eq!(v.len(), 50_000);
        let limit = if cfg!(debug_assertions) { 5 } else { 1 };
        assert!(el.as_secs() < limit, "took {el:?}");
    }
}
