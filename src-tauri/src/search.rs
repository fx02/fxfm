//! Search (Alt+F7) service: owns the name index, builds it on a background thread, persists it,
//! refreshes network volumes and answers searches (index or direct walk).
//!
//! Files: `search.rs` (types, the service API, worker plumbing), `search/jobs.rs` (index-changing
//! jobs), `search/find.rs` (running searches, result items), `search/status.rs` (status),
//! `search/commands.rs` (Tauri commands), `search/live.rs` (FSEvents live updates and replay),
//! `search/tests.rs`, `search/live_tests.rs`.
//!
//! Threading and locking:
//! - `stored: RwLock<Option<Stored>>` holds the index. Searches take the read lock only, on their
//!   own thread: `search_start` itself takes no index lock (it checks readiness in the
//!   `run.complete` mirror), so it is safe on the Tauri main thread. Commands that may wait on
//!   the index lock (`forget`, `rebuild`, `refresh`, `status`, `build`, settings) run on blocking
//!   threads; `save_settings` calls `apply_settings` only after releasing its own mutexes.
//! - One worker thread runs every job in order (it first reads the mount list, which `start`
//!   never does: a hung network mount must not block app startup): load, volume scans,
//!   subtree rescans, volume removal, compaction and saves.
//! - A full volume scan builds that volume into a separate, fresh `Index` with no lock held.
//!   Then, under the read lock, the other volumes of the live index are copied into it
//!   ([`Inner::begin_merge`]); the result is saved (no lock) and swapped in under a short write
//!   lock ([`Inner::finish_swap`]). The old index is dropped after the lock is released.
//!   Memory: while a full rescan runs, the live index and the one being built coexist, a
//!   transient peak of about 2x (rare: first build has no live index; rescans are user-started,
//!   on exclude/archive changes or network refreshes). The steady state is one index (within
//!   the 250 MB budget at 3M entries). Removing a volume and compaction use the same merge.
//! - A subtree rescan ([`Inner::rescan_path`], also the entry point for Task 8's FSEvents
//!   rescans) scans the path into its own small index without locks, then takes the write lock
//!   only to drop the old subtree and graft the new one (`Index::graft`): lock time and extra
//!   memory are proportional to the subtree, never to the whole index.
//! - Writes to the live index after a merge started (`forget`, grafts, later FSEvents) are
//!   recorded in `journal` and replayed on the new index under the swap's write lock, so they
//!   are not lost. Lock order: `stored` before `journal` before `run`.
//! - Turning the index off sets and replaces the job generation's cancel flag. `save_copy` (under
//!   `save_lock`) and `finish_swap` / grafts (under the write lock) discard their work when the
//!   flag is set or no longer current, so a job in flight cannot write `index.bin` back or swap
//!   an index in after `stop`.
//! - Every save goes through `save_lock` (`store::save` uses one fixed temp file name). Saves of
//!   a new index run before its swap without any index lock; saves of the live index (hourly
//!   timer, storage move, exit) hold the read lock only while serializing into the temp file;
//!   fsync and rename follow after it is released ([`Inner::save_current`]).
//! - Priority: the worker and FSEvents threads run at normal priority and drop to Darwin
//!   background priority (`background()` guards) only around disk-scanning I/O with no lock
//!   held (scans, stat'ing events, reading archives, loading the file). Work done under an
//!   index lock (merge copies, swaps, grafts, saves) is never throttled: a throttled lock holder
//!   would stall the FSEvents writer and, behind it, every search.
//! - Each search runs on its own thread with its own cancel flag. A batch is emitted only while
//!   holding `current` and only when its search is still the current, uncancelled one, so once
//!   `search_start` (of a newer search) or `search_cancel` returns, the old id never emits again.
//! - A full scan of a volume other than the startup disk re-reads the mount table when it ends;
//!   when the volume is gone or unreadable, the result is dropped and the old entries stay.
//!
//! Compaction: full scans, volume removal and compaction jobs produce fresh, tombstone-free
//! indexes. Tombstones left in the live index by `forget` and grafts (and later FSEvents) are
//! checked after each graft and by the timer; past a quarter of the slots a compaction job
//! (a merge into a fresh index) is queued. The live index is never compacted under the write lock.

pub mod commands;
mod find;
mod jobs;
mod live;
#[cfg(test)]
mod live_tests;
mod status;
#[cfg(test)]
mod tests;

pub use status::Status;

use fxfm_core::archive::ArchiveKind;
use fxfm_core::config::SearchSettings;
#[cfg(test)]
use fxfm_core::config::SearchVolume;
use fxfm_core::search::archives::stamp_of;
use fxfm_core::search::{
    apply_change, background, default_excludes, index_archive, index_archives_under, is_dataless,
    load, meta_of, needs_full_disk_access, save, scan_into, walk_search, write_unsynced,
    ArchiveOptions, ArchiveStamp, Change as IndexChange, EntryId, Index, LoadError, Meta, Query,
    ScanOptions, Scope, Stored, VolumeState, WalkHit,
};
use live::{Live, Recording};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use crate::state::lock;

/// Sends an event (`search://batch`, `search://status`) to the main and Find files windows.
pub type Emitter = Arc<dyn Fn(&str, Value) + Send + Sync>;
/// Lists the mounted volumes (the startup disk may be in the list too).
pub type MountList = Arc<dyn Fn() -> Vec<Mounted> + Send + Sync>;

const INDEX_DIR: &str = "index";
const INDEX_FILE: &str = "index.bin";
const TMP_FILE: &str = "index.bin.tmp";
/// Seconds between saves while the index has unsaved changes. Hourly: `~/Library` changes all
/// the time, so the index is practically always dirty, and a save writes the whole file (up to
/// ~190 MB at 3M entries). Full scans save their result anyway, and exit saves what is left.
const SAVE_EVERY: i64 = 3600;
/// Refresh interval of a local volume without an event stream when network refreshes are off.
const UNWATCHED_REFRESH_HOURS: u32 = 24;
/// How often the timer thread checks for due refreshes, saves and compaction.
const TICK: Duration = Duration::from_secs(600);
/// Other names of the startup disk that must not be skipped as foreign mounts.
const STARTUP_ALIASES: [&str; 2] = ["/", "/System/Volumes/Data"];

/// A mounted volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Mounted {
    pub path: String,
    pub name: String,
    pub network: bool,
    /// `mntfrom` of a network volume.
    pub source: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchReq {
    pub query: String,
    pub under: Option<String>,
    pub archives: bool,
    pub limit: u32,
}

enum Job {
    Load,
    /// Full scan of a volume root (startup disk, another local volume or a network refresh).
    Scan(String),
    /// Rescan of one folder (a "Retry" of a denied folder).
    Subtree(String),
    /// Drop a volume that was disabled or removed in the settings.
    Remove(String),
    Compact,
    Save,
    /// Save into the current index folder, then delete the file in this old folder.
    Move(PathBuf),
    /// Archive indexing was turned off: drop the in-archive entries.
    DropArchives,
    /// An archive option changed: read these archives again.
    Rearchive(ArchiveSet),
}

/// Archives affected by an option change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArchiveSet {
    /// tar.gz/tgz/gz files (the size limit changed).
    Gz,
    /// Archives on network volumes ("also read archives on network volumes" changed).
    Network,
}

/// A write to the live index made while the worker builds a new one; replayed on it.
enum Mutation {
    Forget(String),
    Graft(Arc<Graft>),
    /// A batch of FSEvents changes.
    Events(Arc<live::Plan>),
    /// A volume's event id, stored after the subtree rescans of its batch.
    EventId(String, u64),
}

/// A rescanned folder or file, scanned into its own small index, ready to graft into the live
/// one (see [`Inner::rescan_path`]).
pub(crate) struct Graft {
    path: String,
    vol_root: String,
    /// None: the path no longer exists.
    meta: Option<Meta>,
    sub: Index,
    sub_root: EntryId,
    stamps: HashMap<String, ArchiveStamp>,
    denied: Vec<String>,
    blocked: Vec<String>,
}

/// Bookkeeping that is not the index itself.
struct Run {
    settings: SearchSettings,
    /// `<data dir>/index`.
    dir: PathBuf,
    /// Volume roots queued for or in a full scan.
    pending: HashSet<String>,
    /// Entries per volume root (without the root), as of its last scan or load.
    counts: HashMap<String, u64>,
    mounted: Vec<Mounted>,
    /// Roots of the volumes that are completely indexed (a lock-free view for `search_start`).
    complete: HashSet<String>,
    dirty: bool,
    last_save: i64,
    /// Cancel flag of the current job generation; replaced when the index is turned off.
    cancel: Arc<AtomicBool>,
    /// Events of volumes whose full scan runs, replayed after its swap.
    recording: HashMap<String, Recording>,
    /// A compaction job is queued and has not started yet.
    compact_queued: bool,
}

pub struct Inner {
    root: String,
    emit: Emitter,
    mounts: MountList,
    stored: RwLock<Option<Stored>>,
    journal: Mutex<Option<Vec<Mutation>>>,
    run: Mutex<Run>,
    save_lock: Mutex<()>,
    jobs: Mutex<Option<Sender<Job>>>,
    current: Mutex<Option<(u64, Arc<AtomicBool>)>>,
    next_id: AtomicU64,
    /// FSEvents streams (lock order: never taken while holding `stored`).
    live: Mutex<Live>,
    #[cfg(test)]
    compact_sends: AtomicUsize,
    /// Runs once in a full scan after the volume was read, before its merge.
    #[cfg(test)]
    scan_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[derive(Clone)]
pub struct Service(Arc<Inner>);

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True when a volume last refreshed at `refreshed` is due again (`hours` = 0: manual only).
pub fn due_for_refresh(refreshed: i64, now: i64, hours: u32) -> bool {
    hours > 0 && now - refreshed >= i64::from(hours) * 3600
}

/// True when tombstones exceed a quarter of all slots.
pub fn needs_compaction(dead: usize, live: usize) -> bool {
    dead * 4 > dead + live
}

/// The mount points a scan of `scan_root` must not enter: every other volume, except the
/// startup disk's own names.
pub fn skip_mounts(scan_root: &str, startup: &str, mounted: &[Mounted]) -> Vec<String> {
    mounted
        .iter()
        .map(|m| m.path.clone())
        .filter(|p| p != scan_root && p != startup && !STARTUP_ALIASES.contains(&p.as_str()))
        .collect()
}

/// `path` is `base` or lies below it.
fn is_under(path: &str, base: &str) -> bool {
    if base == "/" {
        return path.starts_with('/');
    }
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn trim(path: &str) -> String {
    if path.len() > 1 {
        path.trim_end_matches('/').to_string()
    } else {
        path.to_string()
    }
}

fn last_name(path: &str) -> String {
    match path.rsplit('/').find(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => path.to_string(),
    }
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn excludes(s: &SearchSettings) -> Vec<String> {
    let mut v = default_excludes();
    v.extend(s.exclude.iter().cloned());
    v
}

fn archive_options(s: &SearchSettings) -> ArchiveOptions {
    ArchiveOptions {
        gz_limit: s.gz_limit_mb.saturating_mul(1024 * 1024),
        network: s.network_archives,
    }
}

/// Every mounted volume except the home folder, with network volumes marked from the mount table.
pub fn system_mounts() -> Vec<Mounted> {
    use fxfm_core::volumes::{list_volumes, VolumeKind};
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let net = crate::smb::network_mounts();
    let mut out: Vec<Mounted> = Vec::new();
    for v in list_volumes(&home) {
        if v.kind == VolumeKind::Home {
            continue;
        }
        let n = net.iter().find(|(p, _, _)| *p == v.path);
        out.push(Mounted {
            network: v.kind == VolumeKind::Network || n.is_some(),
            source: n.map(|x| x.2.clone()),
            path: v.path,
            name: v.name,
        });
    }
    for (p, _, from) in net {
        if !out.iter().any(|m| m.path == p) {
            out.push(Mounted {
                name: last_name(&p),
                path: p,
                network: true,
                source: Some(from),
            });
        }
    }
    out
}

impl Service {
    /// `root` is the startup disk ("/" in the app), `data_dir` the settings folder (the index
    /// lives in `<data_dir>/index`). Nothing runs until [`Service::start`].
    pub fn new(root: PathBuf, data_dir: PathBuf, emit: Emitter, mounts: MountList) -> Service {
        Service(Arc::new(Inner {
            root: trim(&root.to_string_lossy()),
            emit,
            mounts,
            stored: RwLock::new(None),
            journal: Mutex::new(None),
            run: Mutex::new(Run {
                settings: SearchSettings::default(),
                dir: data_dir.join(INDEX_DIR),
                pending: HashSet::new(),
                counts: HashMap::new(),
                mounted: Vec::new(),
                complete: HashSet::new(),
                dirty: false,
                last_save: now(),
                cancel: Arc::default(),
                recording: HashMap::new(),
                compact_queued: false,
            }),
            save_lock: Mutex::new(()),
            jobs: Mutex::new(None),
            current: Mutex::new(None),
            next_id: AtomicU64::new(0),
            live: Mutex::new(Live::default()),
            #[cfg(test)]
            compact_sends: AtomicUsize::new(0),
            #[cfg(test)]
            scan_hook: Mutex::new(None),
        }))
    }

    /// Starts the worker thread and, when the index is on, loads (or builds) it.
    pub fn start(&self, settings: &SearchSettings) {
        let (tx, rx) = channel::<Job>();
        *lock(&self.0.jobs) = Some(tx);
        let inner = self.0.clone();
        std::thread::Builder::new()
            .name("search-index".into())
            .spawn(move || {
                // The mount list is read here, never in `start` (Tauri setup on the main thread):
                // a hung network mount can block it, and the window must still appear.
                inner.refresh_mounts();
                for job in rx {
                    inner.run_job(job);
                    inner.emit_status();
                }
            })
            .expect("spawn search worker");
        lock(&self.0.run).settings = settings.clone();
        if settings.index == Some(true) {
            self.0.send(Job::Load);
        }
    }

    /// Starts the timer thread: network refreshes, periodic saves and compaction.
    pub fn start_ticker(&self) {
        let svc = self.clone();
        std::thread::Builder::new()
            .name("search-timer".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                svc.tick(now());
            })
            .expect("spawn search timer");
    }

    /// One timer step at `now` (Unix seconds).
    pub fn tick(&self, now: i64) {
        let inner = &self.0;
        inner.refresh_mounts();
        let (on, hours, dirty, last_save) = {
            let run = lock(&inner.run);
            (
                run.settings.index == Some(true),
                run.settings.network_refresh_hours,
                run.dirty,
                run.last_save,
            )
        };
        if !on {
            return;
        }
        inner.queue_missing();
        // Network volumes, and local ones whose event stream could not be created.
        let unwatched = lock(&inner.live).unwatched.clone();
        let (due, compact) = {
            let g = inner.read();
            let Some(s) = g.as_ref() else {
                return;
            };
            let due: Vec<String> = s
                .volumes
                .iter()
                .filter(|v| {
                    // An unwatched local volume has no other way to stay current: with network
                    // refreshes off (0 h) it is still refreshed daily.
                    let hours = if v.network {
                        hours
                    } else if unwatched.contains(&v.root) {
                        if hours == 0 {
                            UNWATCHED_REFRESH_HOURS
                        } else {
                            hours
                        }
                    } else {
                        return false;
                    };
                    v.complete && due_for_refresh(v.refreshed, now, hours)
                })
                .map(|v| v.root.clone())
                .collect();
            (due, needs_compaction(s.index.dead(), s.index.len()))
        };
        let enabled = inner.enabled_mounted();
        for root in due
            .into_iter()
            .filter(|r| *r == inner.root || enabled.contains(r))
        {
            inner.enqueue_scan(&root);
        }
        if dirty && now - last_save >= SAVE_EVERY {
            inner.send(Job::Save);
        }
        if compact {
            inner.queue_compact();
        }
    }

    /// The mounted volumes changed: re-read them and scan enabled volumes that are not indexed.
    pub fn volumes_changed(&self) {
        self.0.refresh_mounts();
        if lock(&self.0.run).settings.index == Some(true) {
            self.0.queue_missing();
        }
        self.0.rewatch();
        self.0.emit_status();
    }

    /// Settings changed (the caller saved them): start or stop, add or drop volumes.
    pub fn apply_settings(&self, new: &SearchSettings) {
        let inner = &self.0;
        let old = std::mem::replace(&mut lock(&inner.run).settings, new.clone());
        let was = old.index == Some(true);
        let on = new.index == Some(true);
        if was && !on {
            inner.stop(new.index == Some(false));
        } else if !was && on {
            inner.send(Job::Load);
        } else if on {
            let mounted = lock(&inner.run).mounted.clone();
            let enabled = |s: &SearchSettings| -> HashSet<String> {
                s.volumes
                    .iter()
                    .filter(|v| v.enabled)
                    .map(|v| trim(&v.path))
                    .collect()
            };
            let (before, after) = (enabled(&old), enabled(new));
            if old.exclude != new.exclude {
                // Rescan where an added or removed exclusion lies: its enabled volume (the
                // deepest that holds it), else the startup disk.
                let (o, n): (HashSet<String>, HashSet<String>) = (
                    old.exclude.iter().map(|e| trim(e)).collect(),
                    new.exclude.iter().map(|e| trim(e)).collect(),
                );
                let vols: Vec<&String> = before
                    .intersection(&after)
                    .filter(|v| mounted.iter().any(|m| &&m.path == v))
                    .collect();
                let mut targets: HashSet<String> = HashSet::new();
                for changed in o.symmetric_difference(&n) {
                    let on = vols
                        .iter()
                        .filter(|v| is_under(changed, v))
                        .max_by_key(|v| v.len());
                    targets.insert(on.map_or_else(|| inner.root.clone(), |v| (*v).clone()));
                }
                for t in targets {
                    inner.enqueue_scan(&t);
                }
            }
            for gone in before.difference(&after) {
                inner.send(Job::Remove(gone.clone()));
            }
            for added in after.difference(&before) {
                if mounted.iter().any(|m| &m.path == added) {
                    inner.enqueue_scan(added);
                }
            }
            // Archive jobs run on the worker after any scan already queued or running (which
            // read the old settings), so they always have the last word.
            if new.archives && !old.archives {
                // A rescan lists the archives of every indexed volume.
                inner.enqueue_scan(&inner.root);
                for v in after
                    .iter()
                    .filter(|v| mounted.iter().any(|m| &m.path == *v))
                {
                    inner.enqueue_scan(v);
                }
            } else if old.archives && !new.archives {
                inner.send(Job::DropArchives);
            } else if new.archives {
                if old.gz_limit_mb != new.gz_limit_mb {
                    inner.send(Job::Rearchive(ArchiveSet::Gz));
                }
                if old.network_archives != new.network_archives {
                    inner.send(Job::Rearchive(ArchiveSet::Network));
                }
            }
        }
        inner.emit_status();
    }

    /// `search_build`: turn the index on (the caller saved `index = Some(true)`).
    pub fn build(&self) {
        let mut s = lock(&self.0.run).settings.clone();
        s.index = Some(true);
        self.apply_settings(&s);
    }

    /// The settings folder moved: save the index there and delete the old file.
    pub fn set_data_dir(&self, data_dir: &Path) {
        let new = data_dir.join(INDEX_DIR);
        let old = std::mem::replace(&mut lock(&self.0.run).dir, new.clone());
        if old != new {
            self.0.send(Job::Move(old));
        }
    }

    /// On exit: stop scanning and save unsaved changes.
    pub fn shutdown(&self) {
        let dirty = {
            let run = lock(&self.0.run);
            run.cancel.store(true, Ordering::Relaxed);
            run.dirty && run.settings.index == Some(true)
        };
        lock(&self.0.jobs).take();
        if dirty {
            self.0.save_current();
        }
    }

    pub fn status(&self) -> Status {
        self.0.refresh_mounts();
        self.0.status()
    }

    pub fn search_start(&self, req: SearchReq) -> Result<u64, String> {
        let inner = self.0.clone();
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut cur = lock(&inner.current);
            if let Some((_, c)) = cur.take() {
                c.store(true, Ordering::Relaxed);
            }
            *cur = Some((id, cancel.clone()));
        }
        // No index lock here: this runs on the caller's (UI) thread.
        let (settings, mounted, ready) = {
            let run = lock(&inner.run);
            let on = run.settings.index == Some(true);
            (
                run.settings.clone(),
                run.mounted.clone(),
                on && run.complete.contains(&inner.root),
            )
        };
        let under = req.under.as_deref().map(trim);
        if under.is_none() && !ready {
            lock(&inner.current).take();
            return Err("no index".into());
        }
        let q = Query::parse(&req.query);
        let archives = req.archives && settings.archives;
        let limit = req.limit.max(1) as usize;
        std::thread::spawn(move || {
            if inner.index_covers(under.as_deref(), &settings, &mounted) {
                inner.index_search(id, &cancel, &q, under, archives, limit, &mounted);
            } else if let Some(under) = under {
                let ao = archives.then(|| archive_options(&settings));
                inner.walk(id, &cancel, &q, &under, ao, limit);
            } else {
                // The index went away after the check above.
                inner.emit_batch(id, &cancel, Vec::new(), 0, true);
            }
        });
        Ok(id)
    }

    pub fn search_cancel(&self, id: u64) {
        let mut cur = lock(&self.0.current);
        if cur.as_ref().is_some_and(|(c, _)| *c == id) {
            if let Some((_, flag)) = cur.take() {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Drops `path` (a stale result) from the index. Volume roots are kept.
    pub fn forget(&self, path: &str) {
        let path = trim(path);
        let inner = &self.0;
        let mut g = inner.write();
        let Some(s) = g.as_mut() else {
            return;
        };
        forget_in(s, &path);
        lock(&inner.run).dirty = true;
        if let Some(j) = lock(&inner.journal).as_mut() {
            j.push(Mutation::Forget(path));
        }
    }

    /// None: rescan everything; a volume root: rescan that volume; another path: that folder.
    pub fn rebuild(&self, path: Option<String>) {
        let inner = &self.0;
        if lock(&inner.run).settings.index != Some(true) {
            return;
        }
        match path.map(|p| trim(&p)) {
            None => {
                inner.enqueue_scan(&inner.root);
                for v in inner.enabled_mounted() {
                    inner.enqueue_scan(&v);
                }
            }
            Some(p) if p == inner.root || inner.enabled_mounted().contains(&p) => {
                inner.enqueue_scan(&p);
            }
            Some(p) => inner.send(Job::Subtree(p)),
        }
        inner.emit_status();
    }

    /// Rescans one network (or other enabled) volume now; other paths are ignored.
    pub fn refresh(&self, path: &str) {
        let p = trim(path);
        if p != self.0.root && !self.0.enabled_mounted().contains(&p) {
            return;
        }
        if lock(&self.0.run).settings.index == Some(true) {
            self.0.enqueue_scan(&p);
            self.0.emit_status();
        }
    }
}

impl Inner {
    fn read(&self) -> RwLockReadGuard<'_, Option<Stored>> {
        self.stored.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Option<Stored>> {
        self.stored.write().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, job: Job) {
        if let Some(tx) = lock(&self.jobs).as_ref() {
            let _ = tx.send(job);
        }
    }

    fn refresh_mounts(&self) {
        let m = (self.mounts)();
        lock(&self.run).mounted = m;
    }

    fn emit_status(&self) {
        (self.emit)("search://status", json!(self.status()));
    }

    /// Queues a full scan of `root` unless one is already queued or running.
    fn enqueue_scan(&self, root: &str) {
        if lock(&self.run).pending.insert(root.to_string()) {
            self.send(Job::Scan(root.to_string()));
        }
    }

    /// Enabled volumes (other than the startup disk) that are mounted now.
    fn enabled_mounted(&self) -> HashSet<String> {
        let run = lock(&self.run);
        run.settings
            .volumes
            .iter()
            .filter(|v| v.enabled)
            .map(|v| trim(&v.path))
            .filter(|p| run.mounted.iter().any(|m| &m.path == p) && *p != self.root)
            .collect()
    }

    /// Queues scans of the startup disk and enabled mounted volumes that are not complete.
    fn queue_missing(&self) {
        let complete: HashSet<String> = match self.read().as_ref() {
            Some(s) => s
                .volumes
                .iter()
                .filter(|v| v.complete)
                .map(|v| v.root.clone())
                .collect(),
            None => HashSet::new(),
        };
        if !complete.contains(&self.root) {
            self.enqueue_scan(&self.root);
        }
        for v in self.enabled_mounted() {
            if !complete.contains(&v) {
                self.enqueue_scan(&v);
            }
        }
    }

    /// Turns the index off: cancels scans, frees the index, deletes the file when `delete`.
    fn stop(&self, delete: bool) {
        let dir = {
            let mut run = lock(&self.run);
            run.cancel.store(true, Ordering::Relaxed);
            run.cancel = Arc::default();
            run.pending.clear();
            run.counts.clear();
            run.complete.clear();
            run.recording.clear();
            run.compact_queued = false;
            run.dirty = false;
            run.dir.clone()
        };
        self.unwatch(None);
        let old = {
            let mut g = self.write();
            lock(&self.journal).take();
            g.take()
        };
        drop(old);
        if delete {
            let _s = lock(&self.save_lock);
            let _ = std::fs::remove_file(dir.join(INDEX_FILE));
        }
    }

    fn run_job(&self, job: Job) {
        if lock(&self.run).settings.index != Some(true) {
            return;
        }
        match job {
            Job::Load => self.load(),
            Job::Scan(root) => self.scan_volume(&root),
            Job::Subtree(path) => self.rescan_path(&path),
            Job::DropArchives => self.drop_archives(),
            Job::Rearchive(set) => self.reread_archives(set),
            Job::Remove(root) => self.remove_volume(&root),
            Job::Compact => {
                // Copying every volume into a fresh index drops the tombstones.
                let cancel = {
                    let mut run = lock(&self.run);
                    run.compact_queued = false;
                    run.cancel.clone()
                };
                let s = self.begin_merge(Stored::default(), None);
                if self.finish_swap(s, &cancel) {
                    lock(&self.run).dirty = true;
                }
            }
            Job::Save => {
                self.save_current();
            }
            Job::Move(old) => {
                // No live index yet (first build): nothing to save here, and the build saves
                // its result into the new folder when it finishes. The old file is useless
                // either way. (A missing old folder, set aside by the storage move, is fine:
                // `storage::migrate` drops the index from it.)
                let saved = self.save_current();
                if saved || self.read().is_none() {
                    let _ = std::fs::remove_file(old.join(INDEX_FILE));
                    let _ = std::fs::remove_file(old.join(TMP_FILE));
                    let _ = std::fs::remove_dir(&old);
                }
            }
        }
    }

    /// True while a job started with `cancel` may still change the index: not cancelled and the
    /// index not turned off (and maybe on again) since.
    pub(crate) fn still_current(&self, cancel: &Arc<AtomicBool>) -> bool {
        let run = lock(&self.run);
        !cancel.load(Ordering::Relaxed)
            && Arc::ptr_eq(cancel, &run.cancel)
            && run.settings.index == Some(true)
    }

    /// Copies every volume of the live index except `skip` into `base` (which must not hold
    /// those volumes), with their states and archive stamps; starts the journal. Runs under the
    /// read lock: searches go on, writers wait for the copy.
    pub(crate) fn begin_merge(&self, base: Stored, skip: Option<&str>) -> Stored {
        self.begin_merge_with(base, skip, true)
    }

    /// [`Inner::begin_merge`]; without `archives` the entries inside archives are left out.
    pub(crate) fn begin_merge_with(
        &self,
        mut base: Stored,
        skip: Option<&str>,
        archives: bool,
    ) -> Stored {
        let g = self.read();
        *lock(&self.journal) = Some(Vec::new());
        if let Some(live) = g.as_ref() {
            for v in live
                .volumes
                .iter()
                .filter(|v| Some(v.root.as_str()) != skip)
            {
                if let Some(id) = live.index.lookup(&v.root) {
                    if live.index.parent(id).is_none() {
                        if archives {
                            base.index.copy_root(&live.index, id);
                        } else {
                            base.index.copy_root_without_archives(&live.index, id);
                        }
                    }
                }
                base.volumes.push(v.clone());
            }
            base.stamps.extend(
                live.stamps
                    .iter()
                    .filter(|(k, _)| skip.is_none_or(|r| !is_under(k, r)))
                    .map(|(k, v)| (k.clone(), *v)),
            );
        }
        base
    }

    /// Drops a copy without swapping it in.
    fn abort_copy(&self) {
        lock(&self.journal).take();
    }

    /// Replays the journal on `new` and swaps it in; the old index is dropped after the lock.
    /// Discards `new` (returns false) when its job was cancelled or the index turned off; that is
    /// checked under the write lock, so a concurrent `stop` cannot be undone.
    pub(crate) fn finish_swap(&self, mut new: Stored, cancel: &Arc<AtomicBool>) -> bool {
        let old = {
            let mut g = self.write();
            let muts = lock(&self.journal).take().unwrap_or_default();
            if !self.still_current(cancel) {
                drop(g);
                return false;
            }
            if !muts.is_empty() {
                lock(&self.run).dirty = true;
            }
            for m in muts {
                match m {
                    Mutation::Forget(p) => forget_in(&mut new, &p),
                    Mutation::Graft(gr) => {
                        apply_graft(&mut new, &gr);
                    }
                    Mutation::Events(plan) => {
                        live::apply_plan(&mut new, &plan);
                    }
                    Mutation::EventId(root, id) => live::set_event_id(&mut new, &root, id),
                }
            }
            lock(&self.run).complete = complete_roots(&new);
            g.replace(new)
        };
        drop(old);
        true
    }

    /// Saves a new index that is not shared yet (no index lock needed). Returns false, without
    /// writing, when its job was cancelled or the index turned off (`stop` deletes the file under
    /// `save_lock` after cancelling, so it cannot be written back).
    pub(crate) fn save_copy(&self, s: &Stored, cancel: &Arc<AtomicBool>) -> bool {
        let _s = lock(&self.save_lock);
        if !self.still_current(cancel) {
            return false;
        }
        let dir = lock(&self.run).dir.clone();
        match save(&dir, s) {
            Ok(()) => {
                let mut run = lock(&self.run);
                run.dirty = false;
                run.last_save = now();
            }
            Err(e) => eprintln!("fxfm: saving the search index: {e}"),
        }
        true
    }

    /// Saves the live index. Only serializing into the temp file (page cache, no fsync) runs
    /// under the read lock, at normal priority; fsync and rename run after it is released, so a
    /// waiting FSEvents writer (and the searches queued behind it) never waits for the disk
    /// flush. No in-memory copy of the file is made (it would add up to ~190 MB at 3M entries).
    fn save_current(&self) -> bool {
        let _s = lock(&self.save_lock);
        let dir = {
            let mut run = lock(&self.run);
            run.dirty = false;
            run.dir.clone()
        };
        let pending = {
            let g = self.read();
            let Some(s) = g.as_ref() else {
                return false;
            };
            write_unsynced(&dir, s)
        };
        match pending.and_then(|p| p.commit()) {
            Ok(()) => {
                lock(&self.run).last_save = now();
                true
            }
            Err(e) => {
                lock(&self.run).dirty = true;
                eprintln!("fxfm: saving the search index: {e}");
                false
            }
        }
    }

    /// Leaves `root` pending unless the index was turned off meanwhile (that cleared `pending`).
    fn done_pending(&self, root: &str, cancel: &Arc<AtomicBool>) {
        let mut run = lock(&self.run);
        if Arc::ptr_eq(cancel, &run.cancel) {
            run.pending.remove(root);
        }
    }
}

/// Removes `path` (never a volume root) from `s`.
fn forget_in(s: &mut Stored, path: &str) {
    if let Some(id) = s.index.lookup(path) {
        if s.index.parent(id).is_some() {
            s.index.remove(id);
        }
    }
}

/// Roots of the completely indexed volumes of `s`.
fn complete_roots(s: &Stored) -> HashSet<String> {
    s.volumes
        .iter()
        .filter(|v| v.complete)
        .map(|v| v.root.clone())
        .collect()
}

/// Puts a rescanned path into `s`: drops the old entry and subtree, grafts the new one under its
/// (indexed) parent, and replaces the archive stamps and denied folders below the path. Returns
/// the change in the number of entries.
fn apply_graft(s: &mut Stored, g: &Graft) -> i64 {
    let old = s
        .index
        .lookup(&g.path)
        .filter(|&id| s.index.parent(id).is_some());
    let before = old.map_or(0, |id| s.index.count_under(id)) as i64;
    let after = match g.meta {
        None => {
            if let Some(id) = old {
                s.index.remove(id);
            }
            0
        }
        Some(meta) => {
            let Some(parent) = s.index.lookup(&parent_of(&g.path)) else {
                return 0;
            };
            let id = s
                .index
                .graft(parent, &last_name(&g.path), meta, &g.sub, g.sub_root);
            s.index.count_under(id) as i64
        }
    };
    s.stamps.retain(|k, _| !is_under(k, &g.path));
    s.stamps
        .extend(g.stamps.iter().map(|(k, v)| (k.clone(), *v)));
    if let Some(v) = s.volumes.iter_mut().find(|v| v.root == g.vol_root) {
        v.denied.retain(|d| !is_under(d, &g.path));
        v.denied.extend(g.denied.iter().cloned());
        v.blocked.retain(|d| !is_under(d, &g.path));
        v.blocked.extend(g.blocked.iter().cloned());
    }
    after - before
}
