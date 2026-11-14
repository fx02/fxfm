//! Archives as folders: location parsing, index and listing.

pub mod pack;
pub mod read;
#[cfg(test)]
pub(crate) mod testutil;
pub mod write;
pub(crate) mod ziptime;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::ser::SerializeStruct;
use serde::Serialize;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

use crate::listing::{sort_entries, Entry, ListError};

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    Gz,
    SevenZ,
}

impl ArchiveKind {
    /// Recognizes a supported archive by file name, case-insensitively.
    pub fn from_name(name: &str) -> Option<ArchiveKind> {
        let n = name.to_lowercase();
        if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
            Some(ArchiveKind::TarGz)
        } else if n.ends_with(".gz") {
            Some(ArchiveKind::Gz)
        } else if n.ends_with(".tar") {
            Some(ArchiveKind::Tar)
        } else if n.ends_with(".zip") {
            Some(ArchiveKind::Zip)
        } else if n.ends_with(".7z") {
            Some(ArchiveKind::SevenZ)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveLoc {
    pub archive: PathBuf,
    pub kind: ArchiveKind,
    /// Path inside the archive without leading or trailing `/`; the root is `""`.
    pub inner: String,
}

/// True for a regular file, or a symlink that points to one.
fn is_regular_file(p: &Path) -> bool {
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_symlink() => std::fs::metadata(p).is_ok_and(|m| m.is_file()),
        Ok(md) => md.is_file(),
        Err(_) => false,
    }
}

/// Splits `loc` into archive file and inner path. The first component that is an existing
/// regular file with a supported archive name is the archive.
pub fn split_location(loc: &str) -> Option<ArchiveLoc> {
    use std::path::Component;
    let comps: Vec<Component> = Path::new(loc).components().collect();
    let mut cur = PathBuf::new();
    for (i, c) in comps.iter().enumerate() {
        cur.push(c.as_os_str());
        let Component::Normal(name) = c else { continue };
        let Some(kind) = ArchiveKind::from_name(&name.to_string_lossy()) else {
            continue;
        };
        if is_regular_file(&cur) {
            let inner = comps[i + 1..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return Some(ArchiveLoc {
                archive: cur,
                kind,
                inner,
            });
        }
    }
    None
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ArchiveError {
    #[error("password required")]
    PasswordRequired,
    #[error("wrong password")]
    WrongPassword,
    #[error("{0}")]
    Unsupported(String),
    #[error("not found in archive: {0}")]
    NotFound(String),
    #[error("already exists in archive: {0}")]
    Exists(String),
    #[error("corrupt archive: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Io(String),
    #[error("operation cancelled")]
    Cancelled,
}

impl Serialize for ArchiveError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            ArchiveError::PasswordRequired => "passwordRequired",
            ArchiveError::WrongPassword => "wrongPassword",
            ArchiveError::Unsupported(_) => "unsupported",
            ArchiveError::NotFound(_) => "notFound",
            ArchiveError::Exists(_) => "exists",
            ArchiveError::Corrupt(_) => "corrupt",
            ArchiveError::Io(_) => "io",
            ArchiveError::Cancelled => "cancelled",
        };
        let mut st = s.serialize_struct("ArchiveError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub(crate) const S_IFDIR: u32 = 0o040000;
pub(crate) const S_IFREG: u32 = 0o100000;
pub(crate) const S_IFLNK: u32 = 0o120000;

#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    /// NFC, no leading or trailing `/`.
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Unix seconds.
    pub mtime: i64,
    /// Includes the file type bits, like `st_mode`.
    pub mode: u32,
    pub encrypted: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct ArchiveIndex {
    /// Sorted by `path`, including synthesized directories.
    pub entries: Vec<IndexEntry>,
}

/// Outcome of checking a raw archive entry name.
pub(crate) enum NameCheck {
    /// Safe: NFC, no leading or trailing `/`.
    Ok(String),
    /// Nothing to extract (empty or only `.` components).
    Empty,
    /// Absolute, drive-letter or `..` name that must not be exposed.
    Unsafe,
}

pub(crate) fn check_name(raw: &str) -> NameCheck {
    let bytes = raw.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if raw.starts_with('/') || drive {
        log::warn!("skipping archive entry with absolute path: {raw:?}");
        return NameCheck::Unsafe;
    }
    let mut parts = Vec::new();
    for c in raw.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                log::warn!("skipping archive entry with '..': {raw:?}");
                return NameCheck::Unsafe;
            }
            _ => parts.push(c),
        }
    }
    if parts.is_empty() {
        return NameCheck::Empty;
    }
    NameCheck::Ok(parts.join("/").nfc().collect())
}

/// Normalizes an archive entry name to NFC with no leading or trailing `/`. Returns `None`
/// (with a warning for unsafe names) for names that must not be exposed.
pub(crate) fn normalize_name(raw: &str) -> Option<String> {
    match check_name(raw) {
        NameCheck::Ok(n) => Some(n),
        _ => None,
    }
}

impl ArchiveIndex {
    /// Builds the index from raw entries: the last duplicate wins, directories that only
    /// appear as a prefix are synthesized, the result is sorted by path.
    pub fn build(raw: Vec<IndexEntry>) -> ArchiveIndex {
        let mut by_path: HashMap<String, IndexEntry> = HashMap::with_capacity(raw.len());
        for e in raw {
            by_path.insert(e.path.clone(), e);
        }
        let mut synth: HashMap<&str, i64> = HashMap::new();
        for e in by_path.values() {
            for (i, _) in e.path.match_indices('/') {
                let anc = &e.path[..i];
                if !by_path.contains_key(anc) {
                    let m = synth.entry(anc).or_insert(e.mtime);
                    *m = (*m).max(e.mtime);
                }
            }
        }
        let synth: Vec<IndexEntry> = synth
            .into_iter()
            .map(|(p, mtime)| IndexEntry {
                path: p.to_string(),
                is_dir: true,
                is_symlink: false,
                size: 0,
                mtime,
                mode: S_IFDIR | 0o755,
                encrypted: false,
            })
            .collect();
        let mut entries: Vec<IndexEntry> = by_path.into_values().chain(synth).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        ArchiveIndex { entries }
    }

    pub(crate) fn find(&self, path: &str) -> Option<&IndexEntry> {
        self.entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.entries[i])
    }
}

const CACHE_MAX: usize = 8;

struct Slot {
    size: u64,
    mtime_ns: i64,
    index: Arc<ArchiveIndex>,
    used: u64,
}

#[derive(Default)]
struct CacheState {
    map: HashMap<PathBuf, Slot>,
    tick: u64,
}

/// Parsed archive indexes keyed by file, valid while size and mtime stay the same.
/// Keeps at most 8 archives, evicting the least recently used.
#[derive(Default)]
pub struct IndexCache {
    state: Mutex<CacheState>,
}

fn stat_key(path: &Path) -> Result<(u64, i64), ArchiveError> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path).map_err(|e| read::io_err(&e, path))?;
    Ok((md.len(), md.mtime() * 1_000_000_000 + md.mtime_nsec()))
}

impl IndexCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, path: &Path, kind: ArchiveKind) -> Result<Arc<ArchiveIndex>, ArchiveError> {
        let (size, mtime_ns) = stat_key(path)?;
        {
            let mut st = self.state.lock().unwrap();
            st.tick += 1;
            let tick = st.tick;
            if let Some(slot) = st.map.get_mut(path) {
                if slot.size == size && slot.mtime_ns == mtime_ns {
                    slot.used = tick;
                    return Ok(slot.index.clone());
                }
            }
        }
        let index = Arc::new(read::read_index(path, kind)?);
        let mut st = self.state.lock().unwrap();
        st.tick += 1;
        let used = st.tick;
        st.map.insert(
            path.to_path_buf(),
            Slot {
                size,
                mtime_ns,
                index: index.clone(),
                used,
            },
        );
        while st.map.len() > CACHE_MAX {
            let oldest = st
                .map
                .iter()
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => st.map.remove(&k),
                None => break,
            };
        }
        Ok(index)
    }

    pub fn invalidate(&self, path: &Path) {
        self.state.lock().unwrap().map.remove(path);
    }
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name == "__MACOSX"
}

/// Lists the direct children of `inner` (`""` is the archive root), sorted with `sort_entries`.
pub fn list_inner(index: &ArchiveIndex, inner: &str) -> Result<Vec<Entry>, ListError> {
    let range: &[IndexEntry] = if inner.is_empty() {
        &index.entries
    } else {
        match index.find(inner) {
            None => return Err(ListError::NotFound(inner.to_string())),
            Some(e) if !e.is_dir => return Err(ListError::NotADirectory(inner.to_string())),
            Some(_) => {}
        }
        let prefix = format!("{inner}/");
        let lo = index
            .entries
            .partition_point(|e| e.path.as_str() < prefix.as_str());
        let hi = lo
            + index.entries[lo..]
                .iter()
                .take_while(|e| e.path.starts_with(&prefix))
                .count();
        &index.entries[lo..hi]
    };
    let skip = if inner.is_empty() { 0 } else { inner.len() + 1 };
    let mut out: Vec<Entry> = range
        .iter()
        .filter_map(|e| {
            let name = &e.path[skip..];
            if name.contains('/') {
                return None;
            }
            Some(Entry {
                hidden: is_hidden(name),
                name: name.to_string(),
                is_dir: e.is_dir,
                is_symlink: e.is_symlink,
                size: e.size,
                mtime: e.mtime,
                mode: e.mode,
            })
        })
        .collect();
    sort_entries(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    fn list(path: &Path, kind: ArchiveKind, inner: &str) -> Vec<Entry> {
        let idx = read::read_index(path, kind).unwrap();
        list_inner(&idx, inner).unwrap()
    }

    #[test]
    fn kind_from_name() {
        use ArchiveKind::*;
        assert_eq!(ArchiveKind::from_name("a.ZIP"), Some(Zip));
        assert_eq!(ArchiveKind::from_name("x.tar.gz"), Some(TarGz));
        assert_eq!(ArchiveKind::from_name("x.tgz"), Some(TarGz));
        assert_eq!(ArchiveKind::from_name("x.tar"), Some(Tar));
        assert_eq!(ArchiveKind::from_name("x.gz"), Some(Gz));
        assert_eq!(ArchiveKind::from_name("x.7z"), Some(SevenZ));
        assert_eq!(ArchiveKind::from_name("x.rar"), None);
        assert_eq!(ArchiveKind::from_name("x.bz2"), None);
    }

    #[test]
    fn split_location_finds_archive_and_inner() {
        let t = tempfile::tempdir().unwrap();
        make_zip(&t.path().join("a.zip"), &[("src/lib/x", Some(b"x"))]);
        let loc = split_location(t.path().join("a.zip/src/lib").to_str().unwrap()).unwrap();
        assert_eq!(loc.archive, t.path().join("a.zip"));
        assert_eq!(loc.kind, ArchiveKind::Zip);
        assert_eq!(loc.inner, "src/lib");
        let root = split_location(t.path().join("a.zip").to_str().unwrap()).unwrap();
        assert_eq!(root.inner, "");
    }

    #[test]
    fn split_location_ignores_directories_and_missing_files() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d.zip")).unwrap();
        let dir = format!("{}/", t.path().join("d.zip").display());
        assert_eq!(split_location(&dir), None);
        assert_eq!(
            split_location(t.path().join("nema.zip/x").to_str().unwrap()),
            None
        );
    }

    #[test]
    fn implicit_dirs_synthesized() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, &[("a/b/c.txt", Some(b"hi"))]);
        let root = list(&p, ArchiveKind::Zip, "");
        assert_eq!(names(&root), ["a"]);
        assert!(root[0].is_dir);
        assert_eq!(root[0].mode, 0o40755);
        assert_ne!(root[0].mtime, 0);
        assert_eq!(names(&list(&p, ArchiveKind::Zip, "a")), ["b"]);
        assert_eq!(names(&list(&p, ArchiveKind::Zip, "a/b")), ["c.txt"]);
    }

    #[test]
    fn list_inner_errors() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar(&p, &[("d/f.txt", Some(b"hi"))]);
        let idx = read::read_index(&p, ArchiveKind::Tar).unwrap();
        assert_eq!(
            list_inner(&idx, "nope"),
            Err(ListError::NotFound("nope".into()))
        );
        assert_eq!(
            list_inner(&idx, "d/f.txt"),
            Err(ListError::NotADirectory("d/f.txt".into()))
        );
    }

    #[test]
    fn appledouble_entries_are_hidden() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(
            &p,
            &[
                ("__MACOSX/._a", Some(b"x")),
                ("._a", Some(b"x")),
                ("a", Some(b"x")),
            ],
        );
        let v = list(&p, ArchiveKind::Zip, "");
        let hidden: Vec<(&str, bool)> = v.iter().map(|e| (e.name.as_str(), e.hidden)).collect();
        assert_eq!(hidden, [("__MACOSX", true), ("._a", true), ("a", false)]);
    }

    #[test]
    fn duplicates_last_wins() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar(&p, &[("f", Some(b"one")), ("f", Some(b"three!"))]);
        let v = list(&p, ArchiveKind::Tar, "");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].size, 6);
    }

    #[test]
    fn names_are_nfc_and_backslash_is_not_a_separator() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar(&p, &[("e\u{301}.txt", Some(b"x")), ("a\\b", Some(b"x"))]);
        let v = list(&p, ArchiveKind::Tar, "");
        assert_eq!(names(&v), ["a\\b", "\u{e9}.txt"]);
    }

    #[test]
    fn index_cache_reused_until_file_changes() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, &[("a", Some(b"x"))]);
        let cache = IndexCache::new();
        let a = cache.get(&p, ArchiveKind::Zip).unwrap();
        let b = cache.get(&p, ArchiveKind::Zip).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        make_zip(&p, &[("a", Some(b"x")), ("bb", Some(b"yy"))]);
        let c = cache.get(&p, ArchiveKind::Zip).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(c.entries.len(), 2);
        cache.invalidate(&p);
        let d = cache.get(&p, ArchiveKind::Zip).unwrap();
        assert!(!Arc::ptr_eq(&c, &d));
    }

    #[test]
    fn index_cache_keeps_at_most_eight_lru() {
        let t = tempfile::tempdir().unwrap();
        let cache = IndexCache::new();
        let paths: Vec<PathBuf> = (0..9).map(|i| t.path().join(format!("{i}.tar"))).collect();
        for p in &paths {
            make_tar(p, &[("a", Some(b"x"))]);
        }
        let first = cache.get(&paths[0], ArchiveKind::Tar).unwrap();
        for p in &paths[1..8] {
            cache.get(p, ArchiveKind::Tar).unwrap();
        }
        // Touch 0 so that 1 becomes the oldest, then overflow.
        cache.get(&paths[0], ArchiveKind::Tar).unwrap();
        cache.get(&paths[8], ArchiveKind::Tar).unwrap();
        assert_eq!(cache.state.lock().unwrap().map.len(), 8);
        assert!(Arc::ptr_eq(
            &first,
            &cache.get(&paths[0], ArchiveKind::Tar).unwrap()
        ));
        assert!(!cache.state.lock().unwrap().map.contains_key(&paths[1]));
    }

    #[test]
    fn missing_archive_file_is_not_found() {
        let cache = IndexCache::new();
        let r = cache.get(Path::new("/nonexistent-fxfm/a.zip"), ArchiveKind::Zip);
        assert!(matches!(r, Err(ArchiveError::NotFound(_))));
    }

    #[test]
    fn archive_error_serializes_kind_and_message() {
        let v = serde_json::to_value(ArchiveError::PasswordRequired).unwrap();
        assert_eq!(v["kind"], "passwordRequired");
        assert_eq!(v["message"], "password required");
        let v = serde_json::to_value(ArchiveError::Corrupt("x".into())).unwrap();
        assert_eq!(v["kind"], "corrupt");
        assert_eq!(v["message"], "corrupt archive: x");
        for (e, k) in [
            (ArchiveError::WrongPassword, "wrongPassword"),
            (ArchiveError::Unsupported("u".into()), "unsupported"),
            (ArchiveError::NotFound("n".into()), "notFound"),
            (ArchiveError::Exists("e".into()), "exists"),
            (ArchiveError::Io("i".into()), "io"),
            (ArchiveError::Cancelled, "cancelled"),
        ] {
            assert_eq!(serde_json::to_value(e).unwrap()["kind"], k);
        }
    }

    #[test]
    #[ignore]
    fn list_inner_100k_entries_is_fast() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("big.tar");
        let names: Vec<String> = (0..100_000)
            .map(|i| format!("d{}/f{i}.txt", i % 100))
            .collect();
        let items: Vec<(&str, Option<&[u8]>)> =
            names.iter().map(|n| (n.as_str(), Some(&b""[..]))).collect();
        make_tar(&p, &items);
        let start = std::time::Instant::now();
        let cache = IndexCache::new();
        let idx = cache.get(&p, ArchiveKind::Tar).unwrap();
        let v = list_inner(&idx, "d7").unwrap();
        assert_eq!(v.len(), 1000);
        assert!(start.elapsed().as_secs_f64() < 1.0, "{:?}", start.elapsed());
    }
}
