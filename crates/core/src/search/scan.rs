//! Filling the index from disk: volume scan, subtree rescan and the direct-walk fallback search.
//!
//! Names are stored exactly as `read_dir` returns them (never normalized; the index folds for
//! matching). A name that is not valid UTF-8 is stored via `to_string_lossy` (invalid bytes become
//! U+FFFD), so such entries cannot be looked up by their raw bytes.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::archives::{list_archive, mtime_of, ArchiveOptions};
use super::{fold, EntryId, Index, Meta, Query};
use crate::archive::ArchiveKind;

/// `st_flags` bit: the file's data is not on this machine (iCloud and similar placeholders).
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;
/// libc may not export the Darwin priority constants, so they are defined here:
/// `PRIO_DARWIN_THREAD` selects the calling thread and `PRIO_DARWIN_BG` is background priority.
#[cfg(target_os = "macos")]
const PRIO_DARWIN_THREAD: i32 = 3;
#[cfg(target_os = "macos")]
const PRIO_DARWIN_BG: i32 = 0x1000;

/// Hits per batch of [`walk_search`].
const BATCH_HITS: usize = 200;
const BATCH_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Absolute paths, plus bare folder names that match at any depth (for example `.Trashes`).
    pub exclude: Vec<String>,
    /// Mount points of other volumes: indexed as entries but never entered.
    pub skip_mounts: Vec<String>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub entries: usize,
    /// Folders that could not be read for lack of permission (EACCES: owner or mode bits).
    pub denied: Vec<String>,
    /// Folders macOS would not let us read (EPERM: privacy protection (TCC) or system
    /// protection). Only those in privacy locations ([`needs_full_disk_access`]) need Full Disk
    /// Access.
    pub blocked: Vec<String>,
    pub cancelled: bool,
}

/// A hit of [`walk_search`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkHit {
    pub path: String,
    pub meta: Meta,
    pub score: u8,
}

pub fn default_excludes() -> Vec<String> {
    [
        "/System",
        "/private/var",
        "/private/tmp",
        "/dev",
        "/Volumes",
        "/cores",
        "/Volumes/com.apple.TimeMachine.localsnapshots",
        ".Spotlight-V100",
        ".fseventsd",
        ".Trashes",
        ".DocumentRevisions-V100",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

thread_local! {
    /// Nesting depth of [`background`] guards on this thread.
    static BACKGROUND: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Sets the calling thread's Darwin background state (CPU and I/O throttling); a no-op off
/// macOS.
#[allow(unused_variables)]
fn set_background(on: bool) {
    #[cfg(target_os = "macos")]
    // SAFETY: plain syscall with constant arguments; it only affects the calling thread.
    unsafe {
        libc::setpriority(
            PRIO_DARWIN_THREAD as _,
            0,
            if on { PRIO_DARWIN_BG } else { 0 },
        );
    }
}

/// Runs the calling thread at background priority until the guard drops (guards nest).
///
/// Only disk-scanning I/O runs in the background: never hold an index lock inside a guard. A
/// throttled thread holding the read lock would make the FSEvents writer wait, and (std
/// `RwLock` queues new readers behind a waiting writer) every search behind it.
pub fn background() -> Background {
    BACKGROUND.with(|d| {
        if d.get() == 0 {
            set_background(true);
        }
        d.set(d.get() + 1);
    });
    Background(std::marker::PhantomData)
}

/// Guard of [`background`]; not `Send` (the priority belongs to the thread).
pub struct Background(std::marker::PhantomData<*const ()>);

impl Drop for Background {
    fn drop(&mut self) {
        BACKGROUND.with(|d| {
            d.set(d.get() - 1);
            if d.get() == 0 {
                set_background(false);
            }
        });
    }
}

/// Whether a [`background`] guard is active on this thread.
pub fn in_background() -> bool {
    BACKGROUND.with(|d| d.get() > 0)
}

/// True for a dataless (cloud placeholder) directory; always false off macOS.
#[allow(unused_variables)]
pub fn is_dataless(md: &Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        md.st_flags() & SF_DATALESS != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn name_of(n: &OsStr) -> String {
    n.to_string_lossy().into_owned()
}

/// Index metadata of a file on disk (`symlink_metadata`). The one helper for every scan,
/// rescan and event, so all agree (a pre-1970 mtime is negative).
pub fn meta_of(md: &Metadata) -> Meta {
    let mtime = mtime_of(md);
    let is_dir = md.file_type().is_dir();
    Meta {
        is_dir,
        size: if is_dir { 0 } else { md.len() },
        mtime,
        in_archive: false,
    }
}

/// Why a folder could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// EACCES: the owner or mode bits forbid it (Full Disk Access does not help).
    Permissions,
    /// EPERM: "operation not permitted", macOS privacy protection (TCC) or system protection.
    Blocked,
}

/// Classifies a `read_dir` error; None when it is not about access.
pub fn denial(e: &std::io::Error) -> Option<Denial> {
    if e.raw_os_error() == Some(libc::EPERM) {
        Some(Denial::Blocked)
    } else if e.kind() == std::io::ErrorKind::PermissionDenied {
        Some(Denial::Permissions)
    } else {
        None
    }
}

/// Whether a folder that was blocked (EPERM) is one Full Disk Access would open: privacy
/// protection (TCC) covers the home folder (Desktop, Documents, Downloads, Library/Mail,
/// Library/Messages, ...) and removable or network volumes under `/Volumes`. Blocked system
/// folders elsewhere (for example `/Library/Caches/com.apple.aned`, SIP) stay unreadable even
/// with Full Disk Access, so they must not raise the hint.
pub fn needs_full_disk_access(path: &str, home: &str) -> bool {
    let under = |base: &str| {
        base.len() > 1
            && path
                .strip_prefix(base)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    under(home.trim_end_matches('/')) || path.starts_with("/Volumes/")
}

/// Whether `name` (found at `path`) is excluded by a bare name or an absolute path.
fn excluded(exclude: &[String], name: &str, path: &str) -> bool {
    exclude.iter().any(|x| {
        if x.starts_with('/') {
            x == path
        } else {
            x == name
        }
    })
}

fn join(dir: &Path, name: &OsStr) -> PathBuf {
    dir.join(name)
}

/// Scans `root_path` breadth-first into `index` under `root`. Never follows symlinks. This is a
/// full volume scan: afterwards it shrinks the index to its tight size. (Subtree rescans scan
/// into their own small index and graft it, so they never shrink the whole index.)
pub fn scan_into(
    index: &mut Index,
    root: EntryId,
    root_path: &Path,
    opts: &ScanOptions,
) -> ScanReport {
    let report = scan_tree(index, root, root_path, opts);
    index.shrink_to_fit();
    report
}

/// The scan itself, without the final shrink.
fn scan_tree(index: &mut Index, root: EntryId, root_path: &Path, opts: &ScanOptions) -> ScanReport {
    let mut report = ScanReport::default();
    let mut queue: VecDeque<(EntryId, PathBuf)> = VecDeque::new();
    queue.push_back((root, root_path.to_path_buf()));
    while let Some((id, dir)) = queue.pop_front() {
        if opts.cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                match denial(&e) {
                    Some(Denial::Permissions) => {
                        report.denied.push(dir.to_string_lossy().into_owned())
                    }
                    Some(Denial::Blocked) => {
                        report.blocked.push(dir.to_string_lossy().into_owned())
                    }
                    None => {}
                }
                continue;
            }
        };
        for ent in rd.flatten() {
            let fname = ent.file_name();
            let name = name_of(&fname);
            let path = join(&dir, &fname);
            let path_s = path.to_string_lossy();
            if excluded(&opts.exclude, &name, &path_s) {
                continue;
            }
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let meta = meta_of(&md);
            let child = index.insert(id, &name, meta);
            report.entries += 1;
            if meta.is_dir
                && !opts.skip_mounts.iter().any(|m| m.as_str() == path_s)
                && !is_dataless(&md)
            {
                queue.push_back((child, path));
            }
        }
    }
    if !report.cancelled {
        // Cancellation can also be set while the last folder was read.
        report.cancelled = opts.cancel.load(Ordering::Relaxed);
    }
    report
}

/// Removes the subtree at `path` and scans it again, without shrinking the index. A path that is not indexed is added when
/// its parent is; when neither is, nothing happens. Test-only: the service rescans a subtree
/// into its own small index and grafts it (`Index::graft`).
#[cfg(test)]
fn rescan_subtree(index: &mut Index, path: &str, opts: &ScanOptions) -> ScanReport {
    let mut report = ScanReport::default();
    if let Some(id) = index.lookup(path) {
        if index.root_of(id) == id {
            // A volume root: keep it, rebuild everything below it.
            for c in index.children(id) {
                index.remove(c);
            }
            return scan_tree(index, id, Path::new(path), opts);
        }
    }
    let trimmed = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let Some(slash) = trimmed.rfind('/') else {
        return report;
    };
    let parent_path = if slash == 0 { "/" } else { &trimmed[..slash] };
    let name = &trimmed[slash + 1..];
    if name.is_empty() {
        return report;
    }
    let Some(parent) = index.lookup(parent_path) else {
        return report;
    };
    if let Some(old) = index.lookup(trimmed) {
        index.remove(old);
    }
    // The parent was not part of the removed subtree, so it is still live.
    let Ok(md) = std::fs::symlink_metadata(trimmed) else {
        return report;
    };
    let meta = meta_of(&md);
    let id = index.insert(parent, name, meta);
    report.entries += 1;
    if meta.is_dir && !is_dataless(&md) {
        let sub = scan_tree(index, id, Path::new(trimmed), opts);
        report.entries += sub.entries;
        report.denied = sub.denied;
        report.blocked = sub.blocked;
        report.cancelled = sub.cancelled;
    }
    report
}

/// Direct-walk fallback search below `root` (the root itself is never a hit), with the default
/// exclusions. Calls `on_batch` every 200 hits or every 250 ms. With `archives`, the inner entries
/// of every archive met on the way are searched too (hit paths are `<archive>/<inner>`); a walk
/// cannot tell network volumes apart, so every archive is treated as local.
pub fn walk_search(
    root: &Path,
    q: &Query,
    archives: Option<&ArchiveOptions>,
    cancel: &AtomicBool,
    on_batch: &mut dyn FnMut(Vec<WalkHit>),
) {
    if q.is_empty() {
        return;
    }
    let exclude = default_excludes();
    let mut batch: Vec<WalkHit> = Vec::new();
    let mut last = Instant::now();
    let mut stack = vec![root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break 'walk;
            }
            let fname = ent.file_name();
            let name = name_of(&fname);
            let path = join(&dir, &fname);
            let path_s = path.to_string_lossy().into_owned();
            if excluded(&exclude, &name, &path_s) {
                continue;
            }
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let meta = meta_of(&md);
            if let Some(score) = q.score(&fold(&name)) {
                batch.push(WalkHit {
                    path: path_s,
                    meta,
                    score,
                });
            }
            if let (Some(ao), false) = (archives, meta.is_dir) {
                if let Some(kind) = ArchiveKind::from_name(&name) {
                    if let Ok(ix) = list_archive(&path, kind, ao, false, &md) {
                        for e in &ix.entries {
                            let base = e.path.rsplit('/').next().unwrap_or(&e.path);
                            if let Some(score) = q.score(&fold(base)) {
                                batch.push(WalkHit {
                                    path: format!("{}/{}", path.to_string_lossy(), e.path),
                                    meta: Meta {
                                        is_dir: e.is_dir,
                                        size: if e.is_dir { 0 } else { e.size },
                                        mtime: e.mtime,
                                        in_archive: true,
                                    },
                                    score,
                                });
                            }
                        }
                    }
                }
            }
            if batch.len() >= BATCH_HITS || (!batch.is_empty() && last.elapsed() >= BATCH_EVERY) {
                on_batch(std::mem::take(&mut batch));
                last = Instant::now();
            }
            if meta.is_dir && !is_dataless(&md) {
                stack.push(path);
            }
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn opts() -> ScanOptions {
        ScanOptions {
            exclude: default_excludes(),
            skip_mounts: vec![],
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn scan(tmp: &Path, o: &ScanOptions) -> (Index, ScanReport) {
        let mut ix = Index::new();
        let root = ix.add_root(tmp.to_str().unwrap());
        let r = scan_into(&mut ix, root, tmp, o);
        (ix, r)
    }

    fn p(tmp: &Path, rel: &str) -> String {
        format!("{}/{}", tmp.to_str().unwrap(), rel)
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn scans_tree_with_hidden() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("a/.hidden"));
        touch(&t.path().join("a/b/c.txt"));
        let (ix, r) = scan(t.path(), &opts());
        assert_eq!(r.entries, 4);
        assert!(!r.cancelled);
        for rel in ["a", "a/.hidden", "a/b", "a/b/c.txt"] {
            assert!(ix.lookup(&p(t.path(), rel)).is_some(), "{rel}");
        }
        let c = ix.lookup(&p(t.path(), "a/b/c.txt")).unwrap();
        assert_eq!(ix.meta(c).size, 1);
        assert!(ix.meta(ix.lookup(&p(t.path(), "a/b")).unwrap()).is_dir);
    }

    #[test]
    fn excludes_by_name_and_path() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("x/.Trashes/f"));
        touch(&t.path().join("skip/f"));
        touch(&t.path().join("keep/f"));
        let mut o = opts();
        o.exclude.push(p(t.path(), "skip"));
        let (ix, _) = scan(t.path(), &o);
        assert!(ix.lookup(&p(t.path(), "x/.Trashes/f")).is_none());
        assert!(ix.lookup(&p(t.path(), "x/.Trashes")).is_none());
        assert!(ix.lookup(&p(t.path(), "skip/f")).is_none());
        assert!(ix.lookup(&p(t.path(), "keep/f")).is_some());
        assert!(ix.lookup(&p(t.path(), "x")).is_some());
    }

    #[test]
    fn skip_mounts() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("vol/f"));
        let mut o = opts();
        o.skip_mounts.push(p(t.path(), "vol"));
        let (ix, _) = scan(t.path(), &o);
        assert!(ix.lookup(&p(t.path(), "vol")).is_some());
        assert!(ix.lookup(&p(t.path(), "vol/f")).is_none());
    }

    #[test]
    fn symlink_not_followed() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("a/f"));
        std::os::unix::fs::symlink(t.path().join("a"), t.path().join("a/l")).unwrap();
        let (ix, r) = scan(t.path(), &opts());
        assert_eq!(r.entries, 3);
        let l = ix.lookup(&p(t.path(), "a/l")).unwrap();
        assert!(!ix.meta(l).is_dir);
        assert!(ix.lookup(&p(t.path(), "a/l/f")).is_none());
    }

    #[test]
    fn denied_folder_counted() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("ok/f"));
        let locked = t.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let _g = Restore(locked.clone());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let (ix, r) = scan(t.path(), &opts());
        assert_eq!(r.denied, vec![p(t.path(), "locked")]);
        assert!(r.blocked.is_empty(), "EACCES is not a privacy block");
        assert!(ix.lookup(&p(t.path(), "locked")).is_some());
        assert!(ix.lookup(&p(t.path(), "ok/f")).is_some());
    }

    #[test]
    fn background_guards_nest() {
        assert!(!in_background());
        {
            let _a = background();
            assert!(in_background());
            {
                let _b = background();
                assert!(in_background());
            }
            assert!(
                in_background(),
                "inner guard must not restore normal priority"
            );
        }
        assert!(!in_background());
    }

    #[test]
    fn denial_kinds() {
        use std::io::Error;
        assert_eq!(
            denial(&Error::from_raw_os_error(libc::EPERM)),
            Some(Denial::Blocked)
        );
        assert_eq!(
            denial(&Error::from_raw_os_error(libc::EACCES)),
            Some(Denial::Permissions)
        );
        assert_eq!(denial(&Error::from_raw_os_error(libc::ENOENT)), None);
    }

    #[test]
    fn full_disk_access_only_for_privacy_locations() {
        let home = "/Users/me";
        // Privacy-protected (TCC) places: inside the home folder or on other volumes.
        assert!(needs_full_disk_access("/Users/me/Desktop", home));
        assert!(needs_full_disk_access("/Users/me/Library/Mail", home));
        assert!(needs_full_disk_access("/Volumes/USB/DCIM", home));
        // Root-only or system-protected folders stay unreadable even with Full Disk Access.
        assert!(!needs_full_disk_access(
            "/Library/Caches/com.apple.aned",
            home
        ));
        assert!(!needs_full_disk_access("/usr/sbin/authserver", home));
        assert!(!needs_full_disk_access("/private/etc/cups/certs", home));
        assert!(!needs_full_disk_access("/Users/meme/Desktop", home));
        assert!(!needs_full_disk_access("/Users/me", "/"));
    }

    #[test]
    fn nfd_name_round_trip() {
        use unicode_normalization::UnicodeNormalization;
        let t = tempfile::tempdir().unwrap();
        let nfd: String = "pročitaj.txt".nfd().collect();
        touch(&t.path().join(&nfd));
        let (ix, _) = scan(t.path(), &opts());
        let full = p(t.path(), &nfd);
        let id = ix.lookup(&full).expect("lookup by on-disk name");
        assert_eq!(ix.path(id), full);
        let hits = ix.search(
            &Query::parse("pročitaj"),
            &crate::search::Scope::Everywhere { archives: false },
            10,
            &AtomicBool::new(false),
        );
        assert_eq!(hits.1, 1);
    }

    #[test]
    fn cancel_stops() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("a/f"));
        let o = opts();
        o.cancel.store(true, Ordering::Relaxed);
        let (_, r) = scan(t.path(), &o);
        assert!(r.cancelled);
    }

    #[test]
    fn rescan_subtree_picks_up_changes() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("d/old.txt"));
        touch(&t.path().join("d/sub/x"));
        let (mut ix, _) = scan(t.path(), &opts());
        fs::remove_file(t.path().join("d/old.txt")).unwrap();
        touch(&t.path().join("d/new.txt"));
        touch(&t.path().join("d/sub/y"));
        let r = rescan_subtree(&mut ix, &p(t.path(), "d"), &opts());
        assert_eq!(r.entries, 5);
        assert!(ix.lookup(&p(t.path(), "d/old.txt")).is_none());
        for rel in ["d/new.txt", "d/sub/x", "d/sub/y"] {
            assert!(ix.lookup(&p(t.path(), rel)).is_some(), "{rel}");
        }
        // Not indexed, but its parent is: added and scanned.
        touch(&t.path().join("fresh/f"));
        rescan_subtree(&mut ix, &p(t.path(), "fresh"), &opts());
        assert!(ix.lookup(&p(t.path(), "fresh/f")).is_some());
        // Neither it nor its parent is indexed.
        let r = rescan_subtree(&mut ix, "/nowhere/at/all", &opts());
        assert_eq!(r, ScanReport::default());
        // Gone from disk: removed.
        fs::remove_dir_all(t.path().join("fresh")).unwrap();
        rescan_subtree(&mut ix, &p(t.path(), "fresh"), &opts());
        assert!(ix.lookup(&p(t.path(), "fresh")).is_none());
    }

    #[test]
    fn rescan_subtree_does_not_shrink() {
        let t = tempfile::tempdir().unwrap();
        touch(&t.path().join("d/f"));
        let (mut ix, _) = scan(t.path(), &opts());
        ix.entries.reserve_exact(10_000);
        ix.names.reserve_exact(100_000);
        ix.roots.reserve_exact(100);
        let before = ix.heap_bytes();
        rescan_subtree(&mut ix, &p(t.path(), "d"), &opts());
        rescan_subtree(&mut ix, &p(t.path(), "d"), &opts());
        assert!(ix.heap_bytes() >= before);
        // A full scan does shrink.
        let root = ix.add_root(t.path().to_str().unwrap());
        scan_into(&mut ix, root, t.path(), &opts());
        assert!(ix.heap_bytes() < before);
    }

    #[test]
    fn walk_search_batches() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..450 {
            touch(&t.path().join(format!("d{}/match{i}.txt", i % 3)));
        }
        touch(&t.path().join("other.txt"));
        let q = Query::parse("match");
        let cancel = AtomicBool::new(false);
        let mut sizes = vec![];
        walk_search(t.path(), &q, None, &cancel, &mut |b| sizes.push(b.len()));
        assert_eq!(sizes.iter().sum::<usize>(), 450);
        assert!(sizes.iter().all(|&n| n <= 200));

        let mut total = 0;
        walk_search(t.path(), &q, None, &cancel, &mut |b| {
            total += b.len();
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(total, 200);
    }
}
