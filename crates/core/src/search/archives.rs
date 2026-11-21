//! Archive entries in the name index: lists the contents of zip, 7z, tar, tar.gz/tgz and gz files
//! without extracting them, so their inner files are searchable as `<archive>/<inner>`.
//! Nested archives are indexed as plain entries and never entered.

use std::collections::HashMap;
use std::fs::Metadata;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::scan::is_dataless;
use super::{EntryId, Index, Meta};
use crate::archive::read::read_index;
use crate::archive::{ArchiveError, ArchiveIndex, ArchiveKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveOptions {
    /// tar.gz/tgz/gz files above this many bytes are not listed (listing means inflating them).
    pub gz_limit: u64,
    /// List archives that live on network volumes.
    pub network: bool,
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        ArchiveOptions {
            gz_limit: 500 * 1024 * 1024,
            network: false,
        }
    }
}

/// Size and mtime an archive had when it was last listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveStamp {
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveOutcome {
    /// Inner entries inserted.
    Indexed(usize),
    Skipped(&'static str),
}

/// Seconds since the Unix epoch, negative before 1970, 0 when unknown.
pub(crate) fn mtime_of(md: &Metadata) -> i64 {
    md.modified()
        .ok()
        .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        })
        .unwrap_or(0)
}

pub fn stamp_of(md: &Metadata) -> ArchiveStamp {
    ArchiveStamp {
        size: md.len(),
        mtime: mtime_of(md),
    }
}

/// True when the archive must be listed again: never listed, or its size or mtime changed.
pub fn needs_reindex(old: Option<ArchiveStamp>, md: &Metadata) -> bool {
    old != Some(stamp_of(md))
}

/// Lists the archive at `path`, or says why it is skipped.
pub(crate) fn list_archive(
    path: &Path,
    kind: ArchiveKind,
    opts: &ArchiveOptions,
    on_network: bool,
    md: &Metadata,
) -> Result<ArchiveIndex, &'static str> {
    if on_network && !opts.network {
        return Err("network");
    }
    if is_dataless(md) {
        return Err("cloud");
    }
    if kind.is_stream_compressed() && md.len() > opts.gz_limit {
        return Err("too big");
    }
    match read_index(path, kind) {
        // Entries flagged `encrypted` have readable names and are indexed normally.
        Ok(ix) => Ok(ix),
        Err(ArchiveError::PasswordRequired) => Err("encrypted names"),
        Err(_) => Err("unreadable"),
    }
}

/// Replaces the children of the archive entry `archive` with the inner entries of the file at
/// `path`. On a skip the old children are left alone.
pub fn index_archive(
    index: &mut Index,
    archive: EntryId,
    path: &Path,
    opts: &ArchiveOptions,
    on_network: bool,
    md: &Metadata,
) -> ArchiveOutcome {
    let Some(kind) = path
        .file_name()
        .and_then(|n| ArchiveKind::from_name(&n.to_string_lossy()))
    else {
        return ArchiveOutcome::Skipped("unreadable");
    };
    let ix = match list_archive(path, kind, opts, on_network, md) {
        Ok(ix) => ix,
        Err(why) => return ArchiveOutcome::Skipped(why),
    };
    // The archive entry itself stays live; only its old children go.
    for c in index.children(archive) {
        index.remove(c);
    }
    let mut dirs: HashMap<&str, EntryId> = HashMap::new();
    let mut n = 0;
    // Sorted by path, so a folder is always seen before what is inside it.
    for e in &ix.entries {
        let (parent, name) = match e.path.rfind('/') {
            Some(i) => (
                dirs.get(&e.path[..i]).copied().unwrap_or(archive),
                &e.path[i + 1..],
            ),
            None => (archive, e.path.as_str()),
        };
        let id = index.insert(
            parent,
            name,
            Meta {
                is_dir: e.is_dir,
                size: if e.is_dir { 0 } else { e.size },
                mtime: e.mtime,
                in_archive: true,
            },
        );
        if e.is_dir {
            dirs.insert(e.path.as_str(), id);
        }
        n += 1;
    }
    ArchiveOutcome::Indexed(n)
}

/// Lists every changed archive below `root` (found in the index, not on disk). Returns how many
/// were read again. `stamps` is keyed by the archive's full path.
pub fn index_archives_under(
    index: &mut Index,
    root: EntryId,
    opts: &ArchiveOptions,
    on_network: bool,
    stamps: &mut HashMap<String, ArchiveStamp>,
    cancel: &AtomicBool,
) -> usize {
    let mut found: Vec<EntryId> = Vec::new();
    let mut stack = index.children(root);
    while let Some(id) = stack.pop() {
        let m = index.meta(id);
        if m.in_archive {
            continue;
        }
        if m.is_dir {
            stack.extend(index.children(id));
        } else if ArchiveKind::from_name(index.name(id)).is_some() {
            found.push(id);
        }
    }
    let mut reread = 0;
    for id in found {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let path = index.path(id);
        let Ok(md) = std::fs::metadata(&path) else {
            continue;
        };
        if !needs_reindex(stamps.get(&path).copied(), &md) {
            continue;
        }
        // Stamp skipped archives too, so they are not retried until they change.
        let _ = index_archive(index, id, Path::new(&path), opts, on_network, &md);
        stamps.insert(path, stamp_of(&md));
        reread += 1;
    }
    reread
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::testutil::{make_7z, make_tar, make_tgz, make_zip};
    use crate::search::scan::{default_excludes, scan_into, walk_search, ScanOptions};
    use crate::search::{Query, Scope};
    use std::sync::Arc;

    fn scan(tmp: &Path) -> (Index, EntryId) {
        let mut ix = Index::new();
        let root = ix.add_root(tmp.to_str().unwrap());
        let o = ScanOptions {
            exclude: default_excludes(),
            skip_mounts: vec![],
            cancel: Arc::new(AtomicBool::new(false)),
        };
        scan_into(&mut ix, root, tmp, &o);
        (ix, root)
    }

    fn find(ix: &Index, q: &str, archives: bool) -> Vec<String> {
        let (ids, _) = ix.search(
            &Query::parse(q),
            &Scope::Everywhere { archives },
            100,
            &AtomicBool::new(false),
        );
        ids.into_iter().map(|i| ix.path(i)).collect()
    }

    fn run(ix: &mut Index, root: EntryId, st: &mut HashMap<String, ArchiveStamp>) -> usize {
        index_archives_under(
            ix,
            root,
            &ArchiveOptions::default(),
            false,
            st,
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn zip_entries_searchable() {
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("SOW.zip");
        make_zip(&z, &[("docs/scope-of-work.txt", Some(b"hi"))]);
        let (mut ix, root) = scan(t.path());
        run(&mut ix, root, &mut HashMap::new());
        let want = format!(
            "{}/SOW.zip/docs/scope-of-work.txt",
            t.path().to_str().unwrap()
        );
        assert_eq!(find(&ix, "scope-of-work", true), vec![want.clone()]);
        assert!(find(&ix, "scope-of-work", false).is_empty());
        let id = ix.lookup(&want).unwrap();
        assert!(ix.meta(id).in_archive);
        assert_eq!(ix.meta(id).size, 2);
        let d = ix
            .lookup(&format!("{}/SOW.zip/docs", t.path().to_str().unwrap()))
            .unwrap();
        assert!(ix.meta(d).is_dir && ix.meta(d).in_archive);
    }

    #[test]
    fn other_kinds_listed() {
        let t = tempfile::tempdir().unwrap();
        make_tar(&t.path().join("a.tar"), &[("x/one.txt", Some(b"1"))]);
        make_tgz(&t.path().join("b.tgz"), &[("y/two.txt", Some(b"2"))]);
        make_7z(&t.path().join("c.7z"), &[("z/three.txt", Some(b"3"))]);
        let (mut ix, root) = scan(t.path());
        assert_eq!(run(&mut ix, root, &mut HashMap::new()), 3);
        for q in ["one.txt", "two.txt", "three.txt"] {
            assert_eq!(find(&ix, q, true).len(), 1, "{q}");
        }
    }

    #[test]
    fn unchanged_archive_not_reread() {
        let t = tempfile::tempdir().unwrap();
        make_zip(&t.path().join("a.zip"), &[("f.txt", Some(b"1"))]);
        let (mut ix, root) = scan(t.path());
        let mut st = HashMap::new();
        assert_eq!(run(&mut ix, root, &mut st), 1);
        assert_eq!(run(&mut ix, root, &mut st), 0);
    }

    #[test]
    fn changed_archive_reread() {
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("a.zip");
        make_zip(&z, &[("old-name.txt", Some(b"1"))]);
        let (mut ix, root) = scan(t.path());
        let mut st = HashMap::new();
        run(&mut ix, root, &mut st);
        assert_eq!(find(&ix, "old-name", true).len(), 1);
        make_zip(&z, &[("new-name.txt", Some(b"1234"))]);
        assert_eq!(run(&mut ix, root, &mut st), 1);
        assert!(find(&ix, "old-name", true).is_empty());
        assert_eq!(find(&ix, "new-name", true).len(), 1);
    }

    #[test]
    fn tgz_over_limit_skipped() {
        let t = tempfile::tempdir().unwrap();
        let g = t.path().join("a.tgz");
        make_tgz(&g, &[("f.txt", Some(b"1"))]);
        let (mut ix, _) = scan(t.path());
        let id = ix.lookup(g.to_str().unwrap()).unwrap();
        let md = std::fs::metadata(&g).unwrap();
        let o = ArchiveOptions {
            gz_limit: 10,
            network: false,
        };
        assert_eq!(
            index_archive(&mut ix, id, &g, &o, false, &md),
            ArchiveOutcome::Skipped("too big")
        );
        assert_eq!(find(&ix, "a.tgz", true).len(), 1);
        assert!(find(&ix, "f.txt", true).is_empty());
    }

    #[test]
    fn network_off_skips() {
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("a.zip");
        make_zip(&z, &[("f.txt", Some(b"1"))]);
        let (mut ix, _) = scan(t.path());
        let id = ix.lookup(z.to_str().unwrap()).unwrap();
        let md = std::fs::metadata(&z).unwrap();
        let mut o = ArchiveOptions::default();
        assert_eq!(
            index_archive(&mut ix, id, &z, &o, true, &md),
            ArchiveOutcome::Skipped("network")
        );
        o.network = true;
        assert_eq!(
            index_archive(&mut ix, id, &z, &o, true, &md),
            ArchiveOutcome::Indexed(1)
        );
    }

    #[test]
    fn encrypted_content_with_readable_names_indexed() {
        let t = tempfile::tempdir().unwrap();
        crate::archive::testutil::make_zip_aes(
            &t.path().join("s.zip"),
            &[("secret/plan.txt", Some(b"data"))],
            "pw",
        );
        let (mut ix, root) = scan(t.path());
        assert_eq!(run(&mut ix, root, &mut HashMap::new()), 1);
        assert_eq!(find(&ix, "plan.txt", true).len(), 1);
        let q = Query::parse("plan.txt");
        let mut hits = vec![];
        walk_search(
            t.path(),
            &q,
            Some(&ArchiveOptions::default()),
            &AtomicBool::new(false),
            &mut |b| hits.extend(b),
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn nested_not_entered() {
        let t = tempfile::tempdir().unwrap();
        let inner = t.path().join("inner.zip");
        make_zip(&inner, &[("a.txt", Some(b"1"))]);
        let bytes = std::fs::read(&inner).unwrap();
        std::fs::remove_file(&inner).unwrap();
        make_zip(&t.path().join("outer.zip"), &[("inner.zip", Some(&bytes))]);
        let (mut ix, root) = scan(t.path());
        run(&mut ix, root, &mut HashMap::new());
        assert_eq!(find(&ix, "inner.zip", true).len(), 1);
        assert!(find(&ix, "a.txt", true).is_empty());
    }

    #[test]
    fn corrupt_archive_unreadable() {
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("x.zip");
        std::fs::write(&z, b"garbage bytes, not a zip").unwrap();
        let (mut ix, _) = scan(t.path());
        let id = ix.lookup(z.to_str().unwrap()).unwrap();
        let md = std::fs::metadata(&z).unwrap();
        assert_eq!(
            index_archive(&mut ix, id, &z, &ArchiveOptions::default(), false, &md),
            ArchiveOutcome::Skipped("unreadable")
        );
    }

    #[test]
    fn walk_search_lists_archive_entries() {
        let t = tempfile::tempdir().unwrap();
        make_zip(
            &t.path().join("SOW.zip"),
            &[("docs/scope-of-work.txt", Some(b"hi"))],
        );
        let q = Query::parse("scope-of-work");
        let cancel = AtomicBool::new(false);
        let mut hits = vec![];
        walk_search(
            t.path(),
            &q,
            Some(&ArchiveOptions::default()),
            &cancel,
            &mut |b| hits.extend(b),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].path,
            format!(
                "{}/SOW.zip/docs/scope-of-work.txt",
                t.path().to_str().unwrap()
            )
        );
        assert!(hits[0].meta.in_archive);
        let mut none = vec![];
        walk_search(t.path(), &q, None, &cancel, &mut |b| none.extend(b));
        assert!(none.is_empty());
    }
}
