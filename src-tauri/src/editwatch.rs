//! F4 on a file inside an archive or on a server: watches the local copy and asks to
//! upload it back when its content changes. Tauri-free: changes are announced
//! through the `emit` closure.
//!
//! Per edited copy: a `notify` watcher on its folder (editors often save by
//! writing a new file and renaming it over the old one), a 500 ms debounce,
//! then a SHA-256 of the content compared with the last confirmed (= what the
//! archive holds). One question at a time: while a change is reported and not
//! answered with `confirm`, further saves are not reported.
//!
//! `<temp_root>/pending-edits.json` always lists every copy with its confirmed
//! hash, so a crash never loses edits: the next start reports the copies whose
//! content differs from it.

use crate::state::{temp_dir_dead, PENDING_EDITS};
use fxfm_core::remote::parse_remote;
use fxfm_core::vfs::{self, Ctx};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// `emit(id, location, name)`: the copy of `location` changed.
pub type Emit = Arc<dyn Fn(u64, &str, &str) + Send + Sync>;

pub const DEBOUNCE: Duration = Duration::from_millis(500);

type Hash = [u8; 32];

/// `(size, mtime)` of the archive entry (see `vfs::entry_stamp`).
type Stamp = Option<(u64, i64)>;

fn hash_file(path: &Path) -> std::io::Result<Hash> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h.finalize().into());
        }
        h.update(&buf[..n]);
    }
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// An entry of `pending-edits.json`: a copy and the content the archive holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingEdit {
    pub location: String,
    pub local: String,
    /// SHA-256 (hex) of the content the archive holds.
    pub confirmed_hash: String,
}

impl PendingEdit {
    /// The copy differs from what the archive holds, or cannot be read for another
    /// reason than being gone (kept to be safe). A missing copy has nothing to keep.
    fn changed(&self) -> bool {
        match hash_file(Path::new(&self.local)) {
            Ok(h) => hex(&h) != self.confirmed_hash,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    }
}

/// Writes `pending-edits.json` atomically (temp file, fsync, rename). `local` paths
/// are stored relative to `dir`, so the folder can be moved (`move_aside_own`).
fn write_manifest(dir: &Path, list: &[PendingEdit]) -> std::io::Result<()> {
    use std::io::Write;
    let stored: Vec<PendingEdit> = list
        .iter()
        .map(|p| PendingEdit {
            local: Path::new(&p.local)
                .strip_prefix(dir)
                .map_or_else(|_| p.local.clone(), |r| r.to_string_lossy().into_owned()),
            ..p.clone()
        })
        .collect();
    let json = serde_json::to_vec_pretty(&stored).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{PENDING_EDITS}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&json)?;
    fxfm_core::ops::sync_file(&f)?;
    drop(f);
    std::fs::rename(&tmp, dir.join(PENDING_EDITS))
}

/// The manifest of `dir` with `local` paths resolved against `dir`. Absolute paths
/// (first manifest version) under `<tmp>/<any folder>/` are re-rooted onto `dir`.
fn read_manifest(dir: &Path) -> Result<Vec<PendingEdit>, String> {
    let bytes = std::fs::read(dir.join(PENDING_EDITS)).map_err(|e| e.to_string())?;
    let list: Vec<PendingEdit> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(list
        .into_iter()
        .map(|p| PendingEdit {
            local: resolve_local(dir, &p.local).to_string_lossy().into_owned(),
            ..p
        })
        .collect())
}

fn resolve_local(dir: &Path, local: &str) -> PathBuf {
    let path = Path::new(local);
    if path.is_relative() {
        return dir.join(path);
    }
    let rest = dir
        .parent()
        .and_then(|tmp| path.strip_prefix(tmp).ok())
        .and_then(|r| {
            let mut c = r.components();
            c.next()?; // the old `<pid>` folder name
            Some(c.as_path().to_path_buf())
        });
    match rest {
        Some(r) if !r.as_os_str().is_empty() => dir.join(r),
        _ => path.to_path_buf(),
    }
}

struct Entry {
    location: String,
    /// The archive file the copy came from (None for a server file): an upload goes only
    /// there.
    archive: Option<PathBuf>,
    local: PathBuf,
    name: OsString,
    /// Content the archive holds (as far as fxfm knows).
    confirmed: Hash,
    /// The archive entry when the copy was extracted or last uploaded.
    stamp: Stamp,
    /// Content handed to the running upload (`upload_snapshot`).
    uploading: Option<Hash>,
    /// A change was reported; no further reports until `confirm`.
    asking: bool,
    /// When to check the content (debounce).
    due: Option<Instant>,
    _watcher: Option<RecommendedWatcher>,
}

impl Entry {
    fn record(&self) -> PendingEdit {
        PendingEdit {
            location: self.location.clone(),
            local: self.local.to_string_lossy().into_owned(),
            confirmed_hash: hex(&self.confirmed),
        }
    }
}

struct Inner {
    entries: Mutex<HashMap<u64, Entry>>,
    wake: Condvar,
    emit: Emit,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn schedule(&self, id: u64, at: Instant) {
        if let Some(e) = self.lock().get_mut(&id) {
            e.due = Some(at);
        }
        self.wake.notify_all();
    }
}

pub struct EditWatch {
    inner: Arc<Inner>,
    next: AtomicU64,
    temp_root: PathBuf,
    /// Serializes manifest writes (each writes the latest state).
    manifest: Mutex<()>,
}

impl EditWatch {
    /// Starts the debounce thread (it ends when the `EditWatch` is dropped).
    /// The manifest goes to `temp_root/pending-edits.json`.
    pub fn new(emit: Emit, temp_root: PathBuf) -> EditWatch {
        let inner = Arc::new(Inner {
            entries: Mutex::new(HashMap::new()),
            wake: Condvar::new(),
            emit,
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("fxfm-editwatch".into())
            .spawn(move || debounce_loop(weak))
            .expect("spawn editwatch thread");
        EditWatch {
            inner,
            next: AtomicU64::new(0),
            temp_root,
            manifest: Mutex::new(()),
        }
    }

    /// Rewrites the manifest with every copy and its confirmed hash.
    fn save_manifest(&self) {
        let _g = self.manifest.lock().unwrap_or_else(|e| e.into_inner());
        let mut list: Vec<PendingEdit> = self.inner.lock().values().map(Entry::record).collect();
        list.sort_by(|a, b| a.local.cmp(&b.local));
        if let Err(e) = write_manifest(&self.temp_root, &list) {
            eprintln!("fxfm: writing {PENDING_EDITS}: {e}");
        }
    }

    /// Starts watching `local`, the copy of archive file `location` (archive entry
    /// `stamp`); its current content counts as the archive's.
    pub fn register(&self, location: String, local: PathBuf, stamp: Stamp) -> Result<u64, String> {
        let confirmed = hash_file(&local).map_err(|e| e.to_string())?;
        let name = local
            .file_name()
            .ok_or_else(|| format!("not a file: {}", local.display()))?
            .to_os_string();
        let dir = local
            .parent()
            .ok_or_else(|| format!("not a file: {}", local.display()))?
            .to_path_buf();
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let weak = Arc::downgrade(&self.inner);
        let watched = name.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let ev = match res {
                Ok(ev) => ev,
                Err(e) => {
                    eprintln!("fxfm: watching edited copy: {e}");
                    return;
                }
            };
            if !ev
                .paths
                .iter()
                .any(|p| p.file_name() == Some(watched.as_os_str()))
            {
                return;
            }
            if let Some(inner) = weak.upgrade() {
                inner.schedule(id, Instant::now() + DEBOUNCE);
            }
        })
        .map_err(|e| e.to_string())?;
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
        // The archive as it is now, so an upload never goes into another file of that path.
        let archive = if parse_remote(&location).is_some() {
            None
        } else {
            fxfm_core::archive::split_location(&location).map(|a| a.archive)
        };
        self.inner.lock().insert(
            id,
            Entry {
                location,
                archive,
                local,
                name,
                confirmed,
                stamp,
                uploading: None,
                asking: false,
                due: None,
                _watcher: Some(watcher),
            },
        );
        self.save_manifest();
        Ok(id)
    }

    /// F4 on archive file `location`: the newest copy of it when that copy has edits, or
    /// is clean and the archive entry is unchanged; otherwise a freshly extracted copy
    /// in `edit-<n>/`. Returns (id, local copy).
    pub fn open_for_edit(&self, ctx: &Ctx, location: &str) -> Result<(u64, PathBuf), String> {
        self.open_for_edit_with(ctx, location, &mut |_, _| true)
    }

    /// `open_for_edit`, with the download's `progress(done, total)` (false stops it).
    pub fn open_for_edit_with(
        &self,
        ctx: &Ctx,
        location: &str,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(u64, PathBuf), String> {
        let newest = self
            .inner
            .lock()
            .iter()
            .filter(|(_, e)| e.location == location && e.local.is_file())
            .max_by_key(|(id, _)| **id)
            .map(|(id, e)| (*id, e.local.clone(), e.confirmed, e.stamp));
        if let Some((id, local, confirmed, stamp)) = newest {
            let dirty = hash_file(&local).is_ok_and(|h| h != confirmed);
            if dirty || vfs::entry_stamp(ctx, location)? == stamp {
                return Ok((id, local));
            }
            // Clean but stale: the archive entry changed since extraction. The old
            // copy stays registered (an editor may still have it open).
        }
        let local = vfs::materialize_with(ctx, location, "edit", progress)?;
        let registered = vfs::entry_stamp(ctx, location).and_then(|stamp| {
            make_owner_writable(&local).map_err(|e| e.to_string())?;
            self.register(location.to_string(), local.clone(), stamp)
        });
        match registered {
            Ok(id) => Ok((id, local)),
            Err(e) => {
                if let Some(d) = local.parent() {
                    ctx.cleanup_dir(d);
                }
                Err(e)
            }
        }
    }

    /// Before an upload: refuses when the archive is gone (the copy stays pending), and
    /// (`conflict: …`) when the archive entry changed since the copy was made, unless
    /// `force`; then remembers the content being uploaded.
    /// Returns (location, local).
    pub fn prepare_upload(
        &self,
        ctx: &Ctx,
        id: u64,
        force: bool,
    ) -> Result<(String, PathBuf), String> {
        let (location, name, stamp, archive) = {
            let g = self.inner.lock();
            let e = g.get(&id).ok_or("The edited copy is gone")?;
            (
                e.location.clone(),
                e.name.to_string_lossy().into_owned(),
                e.stamp,
                e.archive.clone(),
            )
        };
        vfs::check_upload_target(ctx, &location, archive.as_deref())?;
        if !force && vfs::entry_stamp(ctx, &location)? != stamp {
            let place = if parse_remote(&location).is_some() {
                "on the server"
            } else {
                "in the archive"
            };
            return Err(format!(
                "conflict: {name} was changed {place} after you opened it"
            ));
        }
        self.upload_snapshot(id)
            .ok_or_else(|| "The edited copy is gone".to_string())
    }

    /// Remembers the content being uploaded; returns (location, local).
    pub fn upload_snapshot(&self, id: u64) -> Option<(String, PathBuf)> {
        let local = self.inner.lock().get(&id)?.local.clone();
        let hash = hash_file(&local).ok()?;
        let mut g = self.inner.lock();
        let e = g.get_mut(&id)?;
        e.uploading = Some(hash);
        Some((e.location.clone(), e.local.clone()))
    }

    /// `confirm`, and after an upload the archive entry as it is now (so the next
    /// upload of this copy does not see its own write as a conflict).
    pub fn confirm_upload(&self, ctx: &Ctx, id: u64, uploaded: bool) {
        if uploaded {
            let location = self.inner.lock().get(&id).map(|e| e.location.clone());
            if let Some(loc) = location {
                let stamp = vfs::entry_stamp(ctx, &loc).ok().flatten();
                if let Some(e) = self.inner.lock().get_mut(&id) {
                    e.stamp = stamp;
                }
            }
        }
        self.confirm(id, uploaded);
    }

    /// The UI's answer to a reported change. `uploaded`: the content from
    /// `upload_snapshot` is now in the archive (a later save is reported again);
    /// otherwise the archive is unchanged and the next save asks again.
    pub fn confirm(&self, id: u64, uploaded: bool) {
        let mut g = self.inner.lock();
        let Some(e) = g.get_mut(&id) else {
            return;
        };
        e.asking = false;
        let uploading = e.uploading.take();
        if uploaded {
            if let Some(h) = uploading {
                e.confirmed = h;
            }
            // Saves during the upload were not reported: check now.
            e.due.get_or_insert_with(Instant::now);
            drop(g);
            self.inner.wake.notify_all();
            self.save_manifest();
        }
    }

    /// Stops watching the copies of locations matching `of` whose content is what the
    /// source holds (and that no upload is using), and removes their folders. Copies with
    /// changes that were not uploaded stay (and stay in the manifest).
    pub fn remove_clean_copies(&self, ctx: &Ctx, of: &dyn Fn(&str) -> bool) {
        let candidates: Vec<(u64, PathBuf, Hash)> = self
            .inner
            .lock()
            .iter()
            .filter(|(_, e)| of(&e.location))
            .map(|(id, e)| (*id, e.local.clone(), e.confirmed))
            .collect();
        let mut removed = Vec::new();
        for (id, local, confirmed) in candidates {
            // Hashing happens outside the lock; a missing copy has nothing to keep.
            let clean = match hash_file(&local) {
                Ok(h) => h == confirmed,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
            if !clean {
                continue;
            }
            let mut g = self.inner.lock();
            let still = g
                .get(&id)
                .is_some_and(|e| e.confirmed == confirmed && e.uploading.is_none());
            if still {
                let entry = g.remove(&id);
                // The watcher is dropped outside the lock (its callback takes it).
                drop(g);
                drop(entry);
                removed.push(local);
            }
        }
        if removed.is_empty() {
            return;
        }
        for local in &removed {
            if let Some(dir) = local.parent().filter(|d| d.starts_with(&self.temp_root)) {
                ctx.cleanup_dir(dir);
            }
        }
        self.save_manifest();
    }

    /// On exit: `exit_cleanup` with the pending copies, holding the manifest lock.
    pub fn exit_cleanup(&self) {
        let _g = self.manifest.lock().unwrap_or_else(|e| e.into_inner());
        exit_cleanup(&self.temp_root, &self.pending());
    }

    /// Copies whose content differs from what the archive holds.
    pub fn pending(&self) -> Vec<PendingEdit> {
        let all: Vec<PendingEdit> = self.inner.lock().values().map(Entry::record).collect();
        let mut out: Vec<PendingEdit> = all.into_iter().filter(PendingEdit::changed).collect();
        out.sort_by(|a, b| a.local.cmp(&b.local));
        out
    }
}

/// Entries extracted from an archive keep its modes; the editor must be able to save.
fn make_owner_writable(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(p)?.permissions().mode();
    if mode & 0o600 != 0o600 {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode | 0o600))?;
    }
    Ok(())
}

/// Checks entries whose debounce ran out; reports changed content.
fn debounce_loop(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let ready: Vec<(u64, PathBuf)> = {
            let mut g = inner.lock();
            let now = Instant::now();
            match g.values().filter_map(|e| e.due).min() {
                Some(t) if t <= now => g
                    .iter_mut()
                    .filter(|(_, e)| e.due.is_some_and(|d| d <= now))
                    .map(|(id, e)| {
                        e.due = None;
                        (*id, e.local.clone())
                    })
                    .collect(),
                next => {
                    // Wakes up regularly to notice that the EditWatch is gone.
                    let wait = next.map_or(Duration::from_millis(250), |t| {
                        (t - now).min(Duration::from_millis(250))
                    });
                    drop(inner.wake.wait_timeout(g, wait));
                    continue;
                }
            }
        };
        for (id, local) in ready {
            // Hashing happens outside the lock (big files, slow disks).
            let Ok(hash) = hash_file(&local) else {
                continue;
            };
            let report = {
                let mut g = inner.lock();
                match g.get_mut(&id) {
                    // A new save during hashing: its own debounce decides.
                    Some(e) if !e.asking && e.due.is_none() && hash != e.confirmed => {
                        e.asking = true;
                        Some((e.location.clone(), e.name.to_string_lossy().into_owned()))
                    }
                    _ => None,
                }
            };
            if let Some((location, name)) = report {
                (inner.emit)(id, &location, &name);
            }
        }
    }
}

/// On exit: without pending edits `temp_root` goes away; otherwise
/// `pending-edits.json` is rewritten with just them and only the folders holding
/// them stay (everything else in `temp_root` is removed).
pub fn exit_cleanup(temp_root: &Path, pending: &[PendingEdit]) {
    let ctx = Ctx::new(temp_root.to_path_buf());
    let keep: Vec<&PendingEdit> = pending
        .iter()
        .filter(|p| Path::new(&p.local).starts_with(temp_root))
        .collect();
    if keep.is_empty() {
        ctx.cleanup_dir(temp_root);
        return;
    }
    let list: Vec<PendingEdit> = keep.iter().map(|p| (*p).clone()).collect();
    if let Err(e) = write_manifest(temp_root, &list) {
        // The manifest written while editing still lists every copy; removing
        // nothing keeps them all for the next start.
        eprintln!("fxfm: writing {PENDING_EDITS}: {e}");
        return;
    }
    let Ok(rd) = std::fs::read_dir(temp_root) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if e.file_name() == PENDING_EDITS
            || keep.iter().any(|p| Path::new(&p.local).starts_with(&path))
        {
            continue;
        }
        if path.is_dir() {
            ctx.cleanup_dir(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Edits of dead processes: for each dead `<tmp>/<pid>` (or `<pid>.prev-<n>`) folder
/// with a manifest, the copies that differ from their confirmed hash. Folders where
/// nothing changed are removed; returns the reported edits and the folders they are in.
pub fn collect_pending(
    tmp: &Path,
    alive: &dyn Fn(u32) -> bool,
) -> (Vec<PendingEdit>, Vec<PathBuf>) {
    let mut edits = Vec::new();
    let mut dirs = Vec::new();
    let Ok(rd) = std::fs::read_dir(tmp) else {
        return (edits, dirs);
    };
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| temp_dir_dead(n, alive))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .filter(|d| d.join(PENDING_EDITS).is_file())
        .collect();
    found.sort();
    let ctx = Ctx::new(tmp.to_path_buf());
    for dir in found {
        let list = match read_manifest(&dir) {
            Ok(list) => list,
            Err(e) => {
                // Kept: nothing can be told about its copies.
                eprintln!(
                    "fxfm: unreadable {}: {e}",
                    dir.join(PENDING_EDITS).display()
                );
                continue;
            }
        };
        let changed: Vec<PendingEdit> = list.into_iter().filter(PendingEdit::changed).collect();
        if changed.is_empty() {
            ctx.cleanup_dir(&dir);
        } else {
            edits.extend(changed);
            dirs.push(dir);
        }
    }
    (edits, dirs)
}

/// Discard of the previous sessions' not uploaded edits (`collect_pending`): moves the
/// folder of each copy to the Trash with `trash`, then removes each session folder in
/// `dirs` whose copies all went. Nothing is deleted permanently that could hold an edit:
/// a folder with a copy that could not be trashed is kept and reported at the next start,
/// and copies outside `dirs` are not touched.
pub fn discard_pending(
    edits: &[PendingEdit],
    dirs: &[PathBuf],
    trash: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut failed = Vec::new();
    for dir in dirs {
        let mut all = true;
        let mut folders: Vec<&Path> = Vec::new();
        for e in edits {
            match Path::new(&e.local).parent() {
                Some(p) if p.starts_with(dir) && p != dir.as_path() => folders.push(p),
                // Not in a folder of its own: the session folder must stay.
                Some(p) if p == dir.as_path() => all = false,
                _ => {}
            }
        }
        folders.sort();
        folders.dedup();
        for folder in folders {
            if folder.symlink_metadata().is_err() {
                continue;
            }
            if let Err(e) = trash(folder) {
                eprintln!("fxfm: trash {}: {e}", folder.display());
                failed.push(format!("{} ({e})", folder.display()));
                all = false;
            }
        }
        if all {
            if let Some(tmp) = dir.parent() {
                Ctx::new(tmp.to_path_buf()).cleanup_dir(dir);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Could not move to the Trash (kept for the next start): {}",
            failed.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{run_job, JobRequest};
    use fxfm_core::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
    use std::sync::Mutex;
    use std::time::Instant;

    const LOC: &str = "/a.zip/d/x.txt";

    struct Fixture {
        _t: tempfile::TempDir,
        /// `<tmp>`; `root` is `<tmp>/999999` (a dead pid for the tests).
        tmp: PathBuf,
        root: PathBuf,
        watch: EditWatch,
        emits: Arc<Mutex<Vec<(u64, String, String)>>>,
    }

    fn fixture() -> Fixture {
        let t = tempfile::tempdir().unwrap();
        // Canonical: FSEvents reports /private/var/... for /var/...
        let tmp = t.path().canonicalize().unwrap().join("tmp");
        let root = tmp.join("999999");
        std::fs::create_dir_all(root.join("edit-0")).unwrap();
        std::fs::write(root.join("edit-0").join("x.txt"), b"original").unwrap();
        let emits: Arc<Mutex<Vec<(u64, String, String)>>> = Arc::default();
        let e = emits.clone();
        let watch = EditWatch::new(
            Arc::new(move |id, loc: &str, name: &str| {
                e.lock().unwrap().push((id, loc.into(), name.into()));
            }),
            root.clone(),
        );
        Fixture {
            _t: t,
            tmp,
            root,
            watch,
            emits,
        }
    }

    impl Fixture {
        fn local(&self) -> PathBuf {
            self.root.join("edit-0").join("x.txt")
        }
        fn register(&self) -> u64 {
            let id = self.watch.register(LOC.into(), self.local(), None).unwrap();
            // Let the watcher settle before the first write.
            std::thread::sleep(Duration::from_millis(150));
            id
        }
        fn count(&self) -> usize {
            self.emits.lock().unwrap().len()
        }
        /// Waits up to `limit` for at least `n` emits.
        fn wait_for(&self, n: usize, limit: Duration) -> bool {
            let end = Instant::now() + limit;
            while Instant::now() < end {
                if self.count() >= n {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            self.count() >= n
        }
        fn manifest(&self) -> Vec<PendingEdit> {
            read_manifest(&self.root).unwrap()
        }
    }

    const LIMIT: Duration = Duration::from_millis(1500);
    /// Longer than the debounce: no further emit may come within it.
    const QUIET: Duration = Duration::from_millis(900);

    fn hash_of(data: &[u8]) -> String {
        hex(&Sha256::digest(data).into())
    }

    #[test]
    fn debounce_coalesces_saves() {
        let f = fixture();
        let id = f.register();
        std::fs::write(f.local(), b"one").unwrap();
        // Less than the debounce: without a restart the check would run 200 ms
        // after the second save.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(f.local(), b"two").unwrap();
        let second = Instant::now();
        while second.elapsed() < Duration::from_millis(350) {
            assert_eq!(
                f.count(),
                0,
                "reported before the debounce of the last save ran out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(f.wait_for(1, LIMIT), "no change reported");
        std::thread::sleep(QUIET);
        let emits = f.emits.lock().unwrap().clone();
        assert_eq!(emits, vec![(id, LOC.into(), "x.txt".into())]);
        // The reported change is the second content.
        f.watch.upload_snapshot(id).unwrap();
        f.watch.confirm(id, true);
        assert_eq!(f.manifest()[0].confirmed_hash, hash_of(b"two"));
    }

    #[test]
    fn unchanged_content_is_not_reported() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"original").unwrap();
        assert!(!f.wait_for(1, LIMIT), "same content was reported");
        assert!(f.watch.pending().is_empty());
    }

    #[test]
    fn declined_change_asks_again() {
        let f = fixture();
        let id = f.register();
        std::fs::write(f.local(), b"one").unwrap();
        assert!(f.wait_for(1, LIMIT));
        // While the question is open, more saves are not reported.
        std::fs::write(f.local(), b"two").unwrap();
        assert!(!f.wait_for(2, QUIET));
        f.watch.confirm(id, false);
        assert!(
            !f.wait_for(2, QUIET),
            "declining must not ask again by itself"
        );
        std::fs::write(f.local(), b"three").unwrap();
        assert!(f.wait_for(2, LIMIT), "next change after No must ask again");
        assert_eq!(f.watch.pending().len(), 1);
    }

    /// Editor replaces the file by rename (atomic save).
    #[test]
    fn save_by_rename_is_seen() {
        let f = fixture();
        f.register();
        let tmp = f.root.join("edit-0").join(".x.txt.swp");
        std::fs::write(&tmp, b"renamed").unwrap();
        std::fs::rename(&tmp, f.local()).unwrap();
        assert!(f.wait_for(1, LIMIT));
    }

    #[test]
    fn uploaded_change_is_confirmed() {
        let f = fixture();
        let id = f.register();
        std::fs::write(f.local(), b"one").unwrap();
        assert!(f.wait_for(1, LIMIT));
        let (loc, local) = f.watch.upload_snapshot(id).unwrap();
        assert_eq!((loc.as_str(), local), (LOC, f.local()));
        f.watch.confirm(id, true);
        assert!(!f.wait_for(2, QUIET));
        assert!(f.watch.pending().is_empty());
    }

    #[test]
    fn save_during_upload_asks_again() {
        let f = fixture();
        let id = f.register();
        std::fs::write(f.local(), b"one").unwrap();
        assert!(f.wait_for(1, LIMIT));
        f.watch.upload_snapshot(id).unwrap();
        // Saved while the upload job runs: "two" is not in the archive. Its debounce
        // runs out while the question is still open, so only the recheck in
        // `confirm` can report it.
        std::fs::write(f.local(), b"two").unwrap();
        assert!(!f.wait_for(2, QUIET));
        f.watch.confirm(id, true);
        assert!(
            f.wait_for(2, LIMIT),
            "content newer than the upload must be reported"
        );
        assert_eq!(f.watch.pending().len(), 1);
    }

    #[test]
    fn manifest_always_lists_copies_with_confirmed_hash() {
        let f = fixture();
        let id = f.register();
        let entry = |h: &[u8]| PendingEdit {
            location: LOC.into(),
            local: f.local().to_string_lossy().into_owned(),
            confirmed_hash: hash_of(h),
        };
        assert_eq!(f.manifest(), vec![entry(b"original")]);
        std::fs::write(f.local(), b"one").unwrap();
        f.watch.upload_snapshot(id).unwrap();
        f.watch.confirm(id, true);
        assert_eq!(f.manifest(), vec![entry(b"one")]);
    }

    #[test]
    fn crash_keeps_changed_copies() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        // The process dies here: no exit_cleanup.
        crate::state::remove_stale_temp(&f.tmp, &|_| false);
        assert!(f.local().exists(), "a crash must not lose the edit");
        let (edits, dirs) = collect_pending(&f.tmp, &|_| false);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].location, LOC);
        assert_eq!(edits[0].local, f.local().to_string_lossy());
        assert_eq!(dirs, vec![f.root.clone()]);
        assert!(f.local().exists());
    }

    #[test]
    fn manifest_stores_paths_relative_to_temp_root() {
        let f = fixture();
        f.register();
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(f.root.join(PENDING_EDITS)).unwrap()).unwrap();
        assert_eq!(raw[0]["local"], "edit-0/x.txt");
        assert_eq!(f.manifest()[0].local, f.local().to_string_lossy());
    }

    /// A later process with the same pid moves the crashed folder aside.
    #[test]
    fn reused_pid_edits_survive_move_aside() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        // Crash; a new process gets pid 999999 again (alive).
        crate::state::move_aside_own(&f.tmp, 999999);
        crate::state::remove_stale_temp(&f.tmp, &|_| true);
        let moved = f.tmp.join("999999.prev-0");
        let local = moved.join("edit-0").join("x.txt");
        assert_eq!(std::fs::read(&local).unwrap(), b"changed");
        let (edits, dirs) = collect_pending(&f.tmp, &|_| true);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].local, local.to_string_lossy());
        assert_eq!(dirs, vec![moved]);
        assert!(local.exists());
    }

    /// Manifests written with absolute paths (first version) are re-rooted too.
    #[test]
    fn absolute_manifest_paths_are_rerooted() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        let old = serde_json::json!([{
            "location": LOC,
            "local": f.local().to_string_lossy(),
            "confirmedHash": hash_of(b"original"),
        }]);
        std::fs::write(f.root.join(PENDING_EDITS), old.to_string()).unwrap();
        crate::state::move_aside_own(&f.tmp, 999999);
        let (edits, _) = collect_pending(&f.tmp, &|_| true);
        assert_eq!(edits.len(), 1);
        let want = f.tmp.join("999999.prev-0").join("edit-0").join("x.txt");
        assert_eq!(edits[0].local, want.to_string_lossy());
    }

    #[test]
    fn unreadable_copy_keeps_the_folder() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        std::fs::set_permissions(f.local(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = std::fs::File::open(f.local()).is_err(); // false when run as root
        let (edits, dirs) = collect_pending(&f.tmp, &|_| false);
        std::fs::set_permissions(f.local(), std::fs::Permissions::from_mode(0o644)).unwrap();
        if unreadable {
            assert_eq!(edits.len(), 1);
            assert_eq!(dirs, vec![f.root.clone()]);
        }
        assert!(f.local().exists());
    }

    #[test]
    fn crash_without_changes_is_cleaned_up() {
        let f = fixture();
        f.register();
        let (edits, dirs) = collect_pending(&f.tmp, &|_| false);
        assert!(edits.is_empty());
        assert!(dirs.is_empty());
        assert!(!f.root.exists());
    }

    #[test]
    fn live_process_is_left_alone() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        let (edits, _) = collect_pending(&f.tmp, &|pid| pid == 999999);
        assert!(edits.is_empty());
        assert!(f.local().exists());
    }

    #[test]
    fn pending_edits_survive_cleanup() {
        let f = fixture();
        let id = f.register();
        std::fs::create_dir_all(f.root.join("view-1")).unwrap();
        std::fs::write(f.root.join("view-1").join("v.txt"), b"v").unwrap();
        std::fs::create_dir_all(f.root.join("edit-2")).unwrap();
        std::fs::write(f.local(), b"changed").unwrap();
        assert!(f.wait_for(1, LIMIT));

        let pending = f.watch.pending();
        assert_eq!(
            pending,
            vec![PendingEdit {
                location: LOC.into(),
                local: f.local().to_string_lossy().into_owned(),
                confirmed_hash: hash_of(b"original"),
            }]
        );
        exit_cleanup(&f.root, &pending);
        assert_eq!(std::fs::read(f.local()).unwrap(), b"changed");
        assert!(!f.root.join("view-1").exists());
        assert!(!f.root.join("edit-2").exists());
        assert_eq!(f.manifest(), pending);
        let (edits, _) = collect_pending(&f.tmp, &|_| false);
        assert_eq!(edits, pending);

        // Uploaded: nothing pending, the whole temp root goes.
        f.watch.upload_snapshot(id).unwrap();
        f.watch.confirm(id, true);
        let pending = f.watch.pending();
        assert!(pending.is_empty());
        exit_cleanup(&f.root, &pending);
        assert!(!f.root.exists());
    }

    #[test]
    fn discard_trashes_copy_folders_then_removes_the_session_folder() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        std::fs::create_dir_all(f.root.join("view-1")).unwrap();
        let (mut edits, dirs) = collect_pending(&f.tmp, &|_| false);
        assert_eq!(edits.len(), 1);
        // A copy outside the session folders is never touched.
        let outside = tempfile::tempdir().unwrap();
        let stray = outside.path().join("edit-9").join("y.txt");
        std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
        std::fs::write(&stray, b"y").unwrap();
        edits.push(PendingEdit {
            location: LOC.into(),
            local: stray.to_string_lossy().into_owned(),
            confirmed_hash: hash_of(b"y0"),
        });
        let trashed = Mutex::new(Vec::new());
        let trash = |p: &Path| {
            trashed.lock().unwrap().push(p.to_path_buf());
            std::fs::remove_dir_all(p).map_err(|e| e.to_string())
        };
        discard_pending(&edits, &dirs, &trash).unwrap();
        assert_eq!(*trashed.lock().unwrap(), vec![f.root.join("edit-0")]);
        assert!(!f.root.exists());
        assert!(stray.exists());
    }

    #[test]
    fn discard_keeps_everything_when_trash_fails() {
        let f = fixture();
        f.register();
        std::fs::write(f.local(), b"changed").unwrap();
        let (edits, dirs) = collect_pending(&f.tmp, &|_| false);
        let trash = |_: &Path| Err("Cannot move to Trash".to_string());
        let e = discard_pending(&edits, &dirs, &trash).unwrap_err();
        assert!(e.contains("edit-0"), "{e}");
        assert_eq!(std::fs::read(f.local()).unwrap(), b"changed");
        // Shown again at the next start.
        let (again, _) = collect_pending(&f.tmp, &|_| false);
        assert_eq!(again, edits);
    }

    // ------------------------------------------------ archive entry changes

    fn write_tar(path: &Path, data: &[u8], mtime: u64) {
        let mut b = tar::Builder::new(std::fs::File::create(path).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o444);
        h.set_mtime(mtime);
        h.set_cksum();
        b.append_data(&mut h, "x.txt", data).unwrap();
        b.finish().unwrap();
    }

    struct Quiet;
    impl OpObserver for Quiet {
        fn progress(&self, _p: &Progress) {}
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            ConflictAnswer::Cancel
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            ErrorAnswer::Cancel
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    struct Arch {
        f: Fixture,
        ctx: Ctx,
        tar: PathBuf,
        loc: String,
    }

    fn arch() -> Arch {
        let f = fixture();
        let ctx = Ctx::new(f.root.clone());
        let tar = f.tmp.join("a.tar");
        write_tar(&tar, b"old", 1_000_000);
        let loc = format!("{}/x.txt", tar.display());
        Arch { f, ctx, tar, loc }
    }

    impl Arch {
        fn archive_content(&self) -> Vec<u8> {
            std::fs::read(vfs::materialize(&self.ctx, &self.loc, "view").unwrap()).unwrap()
        }
    }

    #[test]
    fn clean_copy_of_changed_entry_is_extracted_again() {
        let a = arch();
        let (id1, p1) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        assert_eq!(std::fs::read(&p1).unwrap(), b"old");
        // Read-only in the archive, writable for the editor.
        assert!(!std::fs::metadata(&p1).unwrap().permissions().readonly());
        assert_eq!(
            a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap(),
            (id1, p1.clone())
        );
        write_tar(&a.tar, b"theirs", 2_000_000);
        let (id2, p2) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        assert_ne!(id2, id1);
        assert_ne!(p2, p1);
        assert_eq!(std::fs::read(&p2).unwrap(), b"theirs");
    }

    #[test]
    fn dirty_copy_is_kept_when_entry_changed() {
        let a = arch();
        let (id1, p1) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        std::fs::write(&p1, b"mine").unwrap();
        write_tar(&a.tar, b"theirs", 2_000_000);
        assert_eq!(
            a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap(),
            (id1, p1.clone())
        );
        assert_eq!(std::fs::read(&p1).unwrap(), b"mine");
    }

    #[test]
    fn upload_over_changed_entry_needs_force() {
        let a = arch();
        let (id, p) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        std::fs::write(&p, b"mine").unwrap();
        write_tar(&a.tar, b"theirs", 2_000_000);
        let e = a.f.watch.prepare_upload(&a.ctx, id, false).unwrap_err();
        assert_eq!(
            e,
            "conflict: x.txt was changed in the archive after you opened it"
        );
        assert_eq!(a.archive_content(), b"theirs");

        let (target, local) = a.f.watch.prepare_upload(&a.ctx, id, true).unwrap();
        let req = JobRequest::Upload {
            local: local.to_string_lossy().into_owned(),
            target,
        };
        run_job(&a.ctx, &req, &Quiet).unwrap();
        a.f.watch.confirm_upload(&a.ctx, id, true);
        assert_eq!(a.archive_content(), b"mine");
        assert!(a.f.watch.pending().is_empty());
        // Its own upload is not a conflict for the next one.
        std::fs::write(&p, b"mine 2").unwrap();
        assert!(a.f.watch.prepare_upload(&a.ctx, id, false).is_ok());
    }

    #[test]
    fn upload_of_unchanged_entry_needs_no_force() {
        let a = arch();
        let (id, p) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        std::fs::write(&p, b"mine").unwrap();
        assert!(a.f.watch.prepare_upload(&a.ctx, id, false).is_ok());
    }

    #[test]
    fn upload_after_entry_vanished_is_a_conflict() {
        let a = arch();
        let (id, p) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
        std::fs::write(&p, b"mine").unwrap();
        let mut b = tar::Builder::new(std::fs::File::create(&a.tar).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "other.txt", &b"o"[..]).unwrap();
        b.finish().unwrap();
        let e = a.f.watch.prepare_upload(&a.ctx, id, false).unwrap_err();
        assert!(e.starts_with("conflict: "), "{e}");
    }

    #[test]
    fn upload_after_archive_is_gone_keeps_the_copy() {
        for how in ["deleted", "renamed", "folder"] {
            let a = arch();
            let (id, p) = a.f.watch.open_for_edit(&a.ctx, &a.loc).unwrap();
            std::fs::write(&p, b"mine").unwrap();
            match how {
                "deleted" => std::fs::remove_file(&a.tar).unwrap(),
                "renamed" => std::fs::rename(&a.tar, a.f.tmp.join("b.tar")).unwrap(),
                _ => {
                    // A folder now has the archive's name.
                    std::fs::remove_file(&a.tar).unwrap();
                    std::fs::create_dir(&a.tar).unwrap();
                }
            }
            let want = format!(
                "The archive {} no longer exists; your edited copy is kept",
                a.tar.display()
            );
            for force in [false, true] {
                let e = a.f.watch.prepare_upload(&a.ctx, id, force).unwrap_err();
                assert_eq!(e, want, "{how} force {force}");
            }
            a.f.watch.confirm_upload(&a.ctx, id, false);
            assert_eq!(a.f.watch.pending().len(), 1, "{how}");
            assert_eq!(std::fs::read(&p).unwrap(), b"mine");
            assert!(!a.tar.join("x.txt").exists(), "{how}");
        }
    }
}
