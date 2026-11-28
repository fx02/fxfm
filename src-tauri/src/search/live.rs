//! Live updates from FSEvents and the replay of changes made while fxfm was closed.
//!
//! - [`handle_events`] is the pure core (any OS): it turns a batch of events into index changes
//!   and follow-ups. The service splits it in two: [`plan_events`] stats every path with no lock
//!   held, then [`apply_plan`] applies the whole batch under one short write lock.
//! - Event paths: a probe on macOS 15 (APFS) showed that a stream on `/` reports firmlinked paths
//!   (`/Users/...`, `/private/tmp/...`), never `/System/Volumes/Data/...`, and the same bytes
//!   (NFC or NFD, as stored) that `read_dir` returns, so no normalization is needed. The
//!   `/System/Volumes/Data` prefix is still mapped back for the startup disk, as a safeguard.
//! - Each local volume (startup disk and enabled local volumes; never network volumes) gets one
//!   stream (`crate::fsevents`, macOS only). Its callback only sends the batch into a channel; a
//!   consumer thread runs [`Inner::on_events`]. A volume whose stream cannot be
//!   created falls back to the periodic refresh used for network volumes.
//! - A full scan records the volume's events from the moment it starts ([`Inner::begin_recording`])
//!   and replays them after its swap ([`Inner::end_recording`]): the scan runs without locks, so
//!   a change it missed would otherwise be lost when its result replaces the live entries.
//! - Event ids: each applied batch stores its last id in the volume's `VolumeState` (persisted
//!   with the next save). At startup a volume with a stored id and an unchanged event-store UUID
//!   is replayed from that id (`HistoryDone` ends the replay); otherwise it is rescanned, and its
//!   stream starts from the current event id taken before that scan.

use super::*;
use std::collections::BTreeSet;

#[cfg(target_os = "macos")]
pub(crate) use crate::fsevents::Event;

/// One file system event (the macOS build uses `crate::fsevents::Event`).
#[cfg(not(target_os = "macos"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    pub path: String,
    pub flags: u32,
    pub id: u64,
}

// FSEvents event flags.
pub(crate) const MUST_SCAN_SUB_DIRS: u32 = 0x01;
pub(crate) const USER_DROPPED: u32 = 0x02;
pub(crate) const KERNEL_DROPPED: u32 = 0x04;
pub(crate) const EVENT_IDS_WRAPPED: u32 = 0x08;
pub(crate) const HISTORY_DONE: u32 = 0x10;
pub(crate) const ROOT_CHANGED: u32 = 0x20;
const MOUNT: u32 = 0x40;
const UNMOUNT: u32 = 0x80;
/// Flags after which the whole volume must be rescanned.
const RESCAN_VOLUME: u32 = USER_DROPPED | KERNEL_DROPPED | EVENT_IDS_WRAPPED | ROOT_CHANGED;

/// Distinct paths recorded during one full scan before they are folded into their ancestors at
/// the deepest level that brings them under [`RECORD_FOLDED`] (each then rescanned as a whole).
const RECORD_MAX: usize = 50_000;
/// Most distinct paths left after folding: room for later events before the next fold.
const RECORD_FOLDED: usize = RECORD_MAX / 2;

/// Prefix under which the Data volume of the startup disk is mounted.
const DATA_VOLUME: &str = "/System/Volumes/Data";

/// What the service must do after a batch was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Followup {
    /// Rescan this folder (or archive) and graft it ([`Inner::rescan_path`]).
    RescanSubtree(String),
    /// Rescan the whole volume.
    RescanVolume(String),
    /// The last event id of the batch (already stored in the volume's state).
    SetEventId(u64),
}

/// How a batch of one volume's events is filtered.
#[derive(Clone, Debug)]
pub(crate) struct EventOpts {
    pub vol_root: String,
    /// Default plus user exclusions: absolute paths and bare folder names.
    pub exclude: Vec<String>,
    /// Mount points of other volumes inside this one.
    pub skip_mounts: Vec<String>,
    /// Archive indexing is on.
    pub archives: bool,
}

impl EventOpts {
    /// Whether `path` (below the volume root) is excluded: below an absolute exclusion that lies
    /// inside this volume (so `/Volumes` excludes nothing on `/Volumes/Disk`), below another
    /// volume's mount point, or with an excluded folder name at any depth below the root.
    pub(crate) fn excluded(&self, path: &str) -> bool {
        let root = self.vol_root.as_str();
        let rel = if root == "/" {
            path.strip_prefix('/')
        } else {
            path.strip_prefix(root).map(|r| r.trim_start_matches('/'))
        }
        .unwrap_or("");
        self.exclude.iter().any(|x| {
            if x.starts_with('/') {
                x != root && is_under(x, root) && is_under(path, x)
            } else {
                rel.split('/').any(|part| part == x)
            }
        }) || self.skip_mounts.iter().any(|m| is_under(path, m))
    }
}

/// An event path as the index spells it: no trailing slash and, for the startup disk, the
/// firmlinked path instead of `/System/Volumes/Data/...`.
pub(crate) fn index_path(path: &str, vol_root: &str) -> String {
    let path = trim(path);
    if vol_root == "/" {
        if path == DATA_VOLUME {
            return "/".to_string();
        }
        if let Some(rest) = path.strip_prefix(DATA_VOLUME) {
            if rest.starts_with('/') {
                return rest.to_string();
            }
        }
    }
    path
}

/// The event id to replay a volume from at startup: its stored id, when it has one, was fully
/// scanned, and its event store is still the same (`uuid_now` = the device's current UUID).
/// None: the volume must be rescanned.
pub(crate) fn replay_from(v: &VolumeState, uuid_now: Option<&str>) -> Option<u64> {
    let same = matches!((v.uuid.as_deref(), uuid_now), (Some(a), Some(b)) if a == b);
    (same && v.complete && v.event_id > 0).then_some(v.event_id)
}

/// One changed path with its state on disk (stat taken before the write lock).
#[derive(Debug)]
pub(crate) struct Change {
    path: String,
    meta: Option<Meta>,
    /// Stamp of an archive file (only when archive indexing is on).
    stamp: Option<ArchiveStamp>,
}

/// A batch of one volume, ready to apply.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    root: String,
    archives: bool,
    /// Sorted by path, so a parent comes before its children.
    changes: Vec<Change>,
    /// Folders to rescan (none below another).
    rescans: Vec<String>,
    rescan_volume: bool,
    last_id: Option<u64>,
}

/// Filters, deduplicates and stats a batch (no index access, no lock).
pub(crate) fn plan_events(events: &[Event], opts: &EventOpts) -> Plan {
    let root = opts.vol_root.as_str();
    let mut plan = Plan {
        root: root.to_string(),
        archives: opts.archives,
        ..Plan::default()
    };
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut rescans: BTreeSet<String> = BTreeSet::new();
    for e in events {
        if e.id > 0 {
            plan.last_id = Some(plan.last_id.map_or(e.id, |l| l.max(e.id)));
        }
        if e.flags & RESCAN_VOLUME != 0 {
            plan.rescan_volume = true;
            continue;
        }
        if e.flags & (HISTORY_DONE | MOUNT | UNMOUNT) != 0 || e.path.is_empty() {
            continue;
        }
        let path = index_path(&e.path, root);
        if !is_under(&path, root) || opts.excluded(&path) {
            continue;
        }
        if e.flags & MUST_SCAN_SUB_DIRS != 0 {
            if path == root {
                plan.rescan_volume = true;
            } else {
                rescans.insert(path);
            }
        } else {
            paths.insert(path);
        }
    }
    if plan.rescan_volume {
        return plan;
    }
    plan.rescans = outermost(rescans);
    for path in paths {
        if plan.rescans.iter().any(|r| is_under(&path, r)) {
            continue; // the rescan covers it
        }
        let md = std::fs::symlink_metadata(&path).ok();
        let stamp = match &md {
            Some(md)
                if opts.archives
                    && md.is_file()
                    && ArchiveKind::from_name(&last_name(&path)).is_some() =>
            {
                Some(stamp_of(md))
            }
            _ => None,
        };
        plan.changes.push(Change {
            meta: md.as_ref().map(meta_of),
            path,
            stamp,
        });
    }
    plan
}

/// The paths of a sorted set that are not below another one of them.
fn outermost(sorted: BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in sorted {
        if !out.iter().any(|o| is_under(&p, o)) {
            out.push(p);
        }
    }
    out
}

/// Applies a planned batch to `s` (the caller holds the write lock). The volume root itself is
/// never removed or replaced by an event. Stores the batch's last event id in the volume state.
pub(crate) fn apply_plan(s: &mut Stored, plan: &Plan) -> Vec<Followup> {
    let root = plan.root.as_str();
    let mut out = Vec::new();
    if plan.rescan_volume {
        // Events were lost: the volume owes a full rescan. Its id must not move past them, and
        // it must not be replayed at the next start even if a save happens before the rescan
        // ends. `uuid = None` is the persisted marker (only the volume's full scan sets it
        // again), so later batches cannot make it replayable.
        if let Some(v) = s.volumes.iter_mut().find(|v| v.root == root) {
            v.event_id = 0;
            v.uuid = None;
        }
        out.push(Followup::RescanVolume(root.to_string()));
        return out;
    } else {
        let mut rescans: BTreeSet<String> = plan.rescans.iter().cloned().collect();
        for c in &plan.changes {
            if c.path == root {
                continue;
            }
            match apply_change(&mut s.index, &c.path, c.meta) {
                IndexChange::NewDir(_) => {
                    rescans.insert(c.path.clone());
                }
                IndexChange::Removed => s.stamps.retain(|k, _| !is_under(k, &c.path)),
                IndexChange::Upserted(_) => {
                    if plan.archives
                        && c.stamp.is_some()
                        && s.stamps.get(&c.path).copied() != c.stamp
                    {
                        rescans.insert(c.path.clone());
                    }
                }
                IndexChange::Unknown => {}
            }
        }
        out.extend(outermost(rescans).into_iter().map(Followup::RescanSubtree));
    }
    if let Some(id) = plan.last_id {
        // With subtree rescans owed, the caller stores the id only after they ran
        // ([`Inner::on_events`]); otherwise a save in between would skip them at the next start.
        if out.is_empty() {
            set_event_id(s, root, id);
        }
        out.push(Followup::SetEventId(id));
    }
    out
}

/// Moves the stored event id of the volume `root` forward to `id`.
pub(crate) fn set_event_id(s: &mut Stored, root: &str, id: u64) {
    if let Some(v) = s.volumes.iter_mut().find(|v| v.root == root) {
        v.event_id = v.event_id.max(id);
    }
}

/// Applies one volume's batch to `s`: [`plan_events`] then [`apply_plan`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn handle_events(s: &mut Stored, events: &[Event], opts: &EventOpts) -> Vec<Followup> {
    let plan = plan_events(events, opts);
    apply_plan(s, &plan)
}

/// Events recorded for a volume while its full scan runs.
#[derive(Default)]
pub(crate) struct Recording {
    /// Path to the OR of its event flags.
    paths: HashMap<String, u32>,
    last_id: u64,
    /// Too many paths: everything deeper than this many folders below the root is folded
    /// into its ancestor at this depth.
    depth: Option<usize>,
}

impl Recording {
    fn add(&mut self, root: &str, events: &[Event]) {
        for e in events {
            self.last_id = self.last_id.max(e.id);
            let (path, flags) = match self.depth {
                Some(d) => fold_at(&e.path, root, d, e.flags),
                None => (e.path.clone(), e.flags),
            };
            *self.paths.entry(path).or_default() |= flags;
        }
        // At depth 1 nothing folds further (a second fold would only cost time).
        if self.paths.len() > RECORD_MAX && self.depth != Some(1) {
            let depth = fold_depth(self.paths.keys(), root, self.depth);
            self.depth = Some(depth);
            let old = std::mem::take(&mut self.paths);
            for (p, f) in old {
                let (path, flags) = fold_at(&p, root, depth, f);
                *self.paths.entry(path).or_default() |= flags;
            }
        }
    }

    fn into_events(self) -> Vec<Event> {
        let id = self.last_id;
        self.paths
            .into_iter()
            .map(|(path, flags)| Event { path, flags, id })
            .collect()
    }
}

/// The ancestor of `path` `depth` (≥ 1) folders below `root`, and whether `path` was deeper
/// (or `path` itself, false, when it is not that deep or not below `root`).
fn ancestor_at(path: &str, root: &str, depth: usize) -> (String, bool) {
    let p = index_path(path, root);
    let rest = if root == "/" {
        p.strip_prefix('/')
    } else {
        p.strip_prefix(root).and_then(|r| r.strip_prefix('/'))
    };
    let Some(rest) = rest.filter(|r| !r.is_empty()) else {
        return (p, false);
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() <= depth {
        return (p, false);
    }
    let kept = parts[..depth].join("/");
    let out = if root == "/" {
        format!("/{kept}")
    } else {
        format!("{root}/{kept}")
    };
    (out, true)
}

/// A recorded path and its flags folded to `depth`: a cut path must rescan its whole subtree.
fn fold_at(path: &str, root: &str, depth: usize, flags: u32) -> (String, u32) {
    match ancestor_at(path, root, depth) {
        (p, true) => (p, flags | MUST_SCAN_SUB_DIRS),
        (p, false) => (p, flags),
    }
}

/// The deepest fold depth (below `below`, when already folded) at which `paths` come down to
/// at most [`RECORD_FOLDED`] distinct paths; 1 (top-level folders) at the least.
fn fold_depth<'a>(
    paths: impl Iterator<Item = &'a String> + Clone,
    root: &str,
    below: Option<usize>,
) -> usize {
    let deepest = paths
        .clone()
        .map(|p| p.matches('/').count())
        .max()
        .unwrap_or(1);
    let mut d = below.map_or(deepest, |b| b.saturating_sub(1)).max(1);
    while d > 1 {
        let distinct: std::collections::HashSet<String> =
            paths.clone().map(|p| ancestor_at(p, root, d).0).collect();
        if distinct.len() <= RECORD_FOLDED {
            break;
        }
        d -= 1;
    }
    d
}

/// Event streams and their consumer.
#[derive(Default)]
pub(crate) struct Live {
    /// Set by `start_live`; streams send `(volume root, batch)` here.
    tx: Option<Sender<(String, Vec<Event>)>>,
    #[cfg(target_os = "macos")]
    streams: HashMap<String, crate::fsevents::Stream>,
    /// Linux: inotify watchers by volume root (for the startup disk: the home folder).
    #[cfg(target_os = "linux")]
    watchers: HashMap<String, notify::RecommendedWatcher>,
    /// Local volumes whose stream could not be created: refreshed by the timer instead.
    pub(crate) unwatched: HashSet<String>,
}

impl Service {
    /// Starts the FSEvents consumer thread; streams are then started for every local indexed
    /// volume (call before [`Service::start`]). Not called in tests: they feed fake events.
    pub fn start_live(&self) {
        let (tx, rx) = channel::<(String, Vec<Event>)>();
        lock(&self.0.live).tx = Some(tx);
        let inner = self.0.clone();
        std::thread::Builder::new()
            .name("search-fsevents".into())
            .spawn(move || {
                for (root, events) in rx {
                    inner.on_events(&root, events);
                }
            })
            .expect("spawn search fsevents");
    }
}

impl Inner {
    /// Handles one batch of a volume's events: records it when the volume's full scan runs,
    /// stats its paths (no lock), applies it under one write lock (journaled during a merge),
    /// then rescans new or dirty folders and queues full rescans.
    pub(crate) fn on_events(&self, root: &str, events: Vec<Event>) {
        let (opts, cancel) = {
            let mut run = lock(&self.run);
            if run.settings.index != Some(true) {
                return;
            }
            if let Some(r) = run.recording.get_mut(root) {
                r.add(root, &events);
            }
            let opts = EventOpts {
                vol_root: root.to_string(),
                exclude: excludes(&run.settings),
                skip_mounts: skip_mounts(root, &self.root, &run.mounted),
                archives: run.settings.archives,
            };
            (opts, run.cancel.clone())
        };
        let plan = {
            // Stat'ing the paths is disk I/O: background priority, but not while locked.
            let _bg = background();
            Arc::new(plan_events(&events, &opts))
        };
        let followups = {
            let mut w = self.write();
            if !self.still_current(&cancel) {
                return;
            }
            let Some(s) = w.as_mut() else {
                return;
            };
            if !s.volumes.iter().any(|v| v.root == root) {
                return; // not indexed yet: its scan's recording replays the batch
            }
            let f = apply_plan(s, &plan);
            let compact = needs_compaction(s.index.dead(), s.index.len());
            if let Some(j) = lock(&self.journal).as_mut() {
                j.push(Mutation::Events(plan.clone()));
            }
            lock(&self.run).dirty = true;
            drop(w);
            if compact {
                self.queue_compact();
            }
            f
        };
        let owed = followups
            .iter()
            .any(|f| matches!(f, Followup::RescanSubtree(_)));
        for f in followups {
            match f {
                Followup::RescanSubtree(p) => self.rescan_path(&p),
                Followup::RescanVolume(r) => self.enqueue_scan(&r),
                // Already stored by `apply_plan` unless subtree rescans were owed; those ran now.
                Followup::SetEventId(id) if owed => self.store_event_id(root, id),
                Followup::SetEventId(_) => {}
            }
        }
    }

    /// Stores an event id after the rescans of its batch ran (journaled during a merge).
    ///
    /// Residual window (accepted): ids only move forward (`set_event_id` takes the max), and
    /// batches of one volume are not serialized against the end-of-scan replay
    /// ([`Inner::end_recording`], on the worker). While a live batch is still running its
    /// subtree rescans here, the replay can store a larger id; a save in that moment followed by
    /// a crash before those rescans finish would replay from past the batch at the next start
    /// and miss its subtrees until they change again. Closing it would need per-volume ordering
    /// of the two threads for a gap of one batch's rescans.
    fn store_event_id(&self, root: &str, id: u64) {
        let mut w = self.write();
        let Some(s) = w.as_mut() else {
            return;
        };
        set_event_id(s, root, id);
        if let Some(j) = lock(&self.journal).as_mut() {
            j.push(Mutation::EventId(root.to_string(), id));
        }
        lock(&self.run).dirty = true;
    }

    /// A full scan of `root` starts: record its events from now on (and make sure its stream
    /// runs). Returns the event id the scan's result is current as of.
    pub(crate) fn begin_recording(&self, root: &str) -> u64 {
        let since = current_event_id();
        lock(&self.run)
            .recording
            .insert(root.to_string(), Recording::default());
        self.watch(root, since);
        since
    }

    /// The scan of `root` ended (swapped in or dropped): stop recording and replay what it may
    /// have missed.
    pub(crate) fn end_recording(&self, root: &str) {
        let rec = lock(&self.run).recording.remove(root);
        if let Some(rec) = rec {
            if !rec.paths.is_empty() {
                self.on_events(root, rec.into_events());
            }
        }
    }

    /// Queues one compaction; further requests are dropped until that job runs.
    pub(crate) fn queue_compact(&self) {
        let first = !std::mem::replace(&mut lock(&self.run).compact_queued, true);
        if first {
            #[cfg(test)]
            self.compact_sends.fetch_add(1, Ordering::Relaxed);
            self.send(Job::Compact);
        }
    }

    /// After a load: start a stream for every local indexed volume that can replay from its
    /// stored event id; rescan the others (their scan starts their stream).
    pub(crate) fn start_streams(&self) {
        if !cfg!(target_os = "macos") || lock(&self.live).tx.is_none() {
            return;
        }
        let vols: Vec<VolumeState> = match self.read().as_ref() {
            Some(s) => s.volumes.clone(),
            None => return,
        };
        let enabled = self.enabled_mounted();
        for v in vols {
            if v.network || (v.root != self.root && !enabled.contains(&v.root)) {
                continue;
            }
            if self.watching(&v.root) {
                continue; // already streaming, or refreshed by the timer
            }
            match replay_from(&v, volume_uuid(&v.root).as_deref()) {
                Some(since) => self.watch(&v.root, since),
                None => self.enqueue_scan(&v.root),
            }
        }
    }

    /// Starts the stream of a local volume from `since` unless it runs already (no-op without
    /// `start_live`, for network volumes and off macOS).
    #[allow(unused_variables)]
    pub(crate) fn watch(&self, root: &str, since: u64) {
        let network = lock(&self.run)
            .mounted
            .iter()
            .any(|m| m.path == root && m.network);
        if network && root != self.root {
            return;
        }
        let mut live = lock(&self.live);
        let Some(tx) = live.tx.clone() else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            if live.streams.contains_key(root) {
                return;
            }
            let r = root.to_string();
            let tx = Mutex::new(tx);
            match crate::fsevents::Stream::start(
                &[root.to_string()],
                since,
                crate::fsevents::LATENCY,
                move |ev| {
                    let _ = lock(&tx).send((r.clone(), ev));
                },
            ) {
                Ok(s) => {
                    live.unwatched.remove(root);
                    live.streams.insert(root.to_string(), s);
                }
                Err(e) => {
                    eprintln!("fxfm: search: no live updates for {root} ({e}); refreshed by timer");
                    live.unwatched.insert(root.to_string());
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            if live.watchers.contains_key(root) {
                return;
            }
            // The rest of the startup disk (and a volume without a watcher) gets the timer.
            live.unwatched.insert(root.to_string());
            drop(live);
            // Registering a recursive inotify watch walks the tree: no lock held meanwhile.
            match linux_watcher(root, &self.root, tx) {
                Ok(w) => {
                    let mut live = lock(&self.live);
                    if !live.watchers.contains_key(root) {
                        if root != self.root {
                            live.unwatched.remove(root);
                        }
                        live.watchers.insert(root.to_string(), w);
                    }
                }
                Err(e) => {
                    eprintln!("fxfm: search: no live updates for {root} ({e}); refreshed by timer")
                }
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            drop(tx);
            live.unwatched.insert(root.to_string());
        }
    }

    /// Whether `root` has a stream or is known to need the timer instead.
    fn watching(&self, root: &str) -> bool {
        let live = lock(&self.live);
        #[cfg(target_os = "macos")]
        if live.streams.contains_key(root) {
            return true;
        }
        #[cfg(target_os = "linux")]
        if live.watchers.contains_key(root) {
            return true;
        }
        live.unwatched.contains(root)
    }

    /// Stops the stream of `root` (None: every stream).
    pub(crate) fn unwatch(&self, root: Option<&str>) {
        let mut live = lock(&self.live);
        #[cfg(target_os = "macos")]
        let dropped: Vec<crate::fsevents::Stream> = {
            let keys: Vec<String> = live
                .streams
                .keys()
                .filter(|k| root.is_none_or(|r| r == k.as_str()))
                .cloned()
                .collect();
            keys.iter().filter_map(|k| live.streams.remove(k)).collect()
        };
        #[cfg(target_os = "linux")]
        let dropped_watchers: Vec<notify::RecommendedWatcher> = {
            let keys: Vec<String> = live
                .watchers
                .keys()
                .filter(|k| root.is_none_or(|r| r == k.as_str()))
                .cloned()
                .collect();
            keys.iter()
                .filter_map(|k| live.watchers.remove(k))
                .collect()
        };
        match root {
            Some(r) => {
                live.unwatched.remove(r);
            }
            None => live.unwatched.clear(),
        }
        drop(live);
        #[cfg(target_os = "linux")]
        drop(dropped_watchers);
        // Dropping a stream waits for a running callback; that callback never takes `live`.
        #[cfg(target_os = "macos")]
        drop(dropped);
    }

    /// Mounts changed: drop streams of volumes that are gone, start the others.
    pub(crate) fn rewatch(&self) {
        if lock(&self.live).tx.is_none() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let enabled = self.enabled_mounted();
            let gone: Vec<String> = lock(&self.live)
                .streams
                .keys()
                .filter(|k| **k != self.root && !enabled.contains(*k))
                .cloned()
                .collect();
            for r in gone {
                self.unwatch(Some(&r));
            }
        }
        #[cfg(target_os = "linux")]
        {
            let enabled = self.enabled_mounted();
            let gone: Vec<String> = lock(&self.live)
                .watchers
                .keys()
                .filter(|k| **k != self.root && !enabled.contains(*k))
                .cloned()
                .collect();
            for r in gone {
                self.unwatch(Some(&r));
            }
        }
        if lock(&self.run).settings.index == Some(true) {
            self.start_streams();
        }
    }
}

/// Linux live updates: a recursive inotify watcher on `root` (the home folder when `root` is
/// the startup disk `startup`). Its events go, gathered for about a second like FSEvents'
/// latency, as one batch per volume into `tx`; a queue overflow asks for a rescan of the
/// watched folder.
#[cfg(target_os = "linux")]
fn linux_watcher(
    root: &str,
    startup: &str,
    tx: Sender<(String, Vec<Event>)>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher;
    let target = if root == startup {
        std::env::var("HOME").unwrap_or_else(|_| root.to_string())
    } else {
        root.to_string()
    };
    let (raw_tx, raw_rx) = channel::<Vec<Event>>();
    let watched = target.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let batch = match res {
            Ok(ev) => {
                let flags = if ev.need_rescan() {
                    MUST_SCAN_SUB_DIRS
                } else {
                    0
                };
                ev.paths
                    .iter()
                    .map(|p| Event {
                        path: p.to_string_lossy().into_owned(),
                        flags,
                        id: 0,
                    })
                    .collect()
            }
            Err(_) => vec![Event {
                path: watched.clone(),
                flags: MUST_SCAN_SUB_DIRS,
                id: 0,
            }],
        };
        let _ = raw_tx.send(batch);
    })?;
    w.watch(Path::new(&target), notify::RecursiveMode::Recursive)?;
    let r = root.to_string();
    std::thread::Builder::new()
        .name("search-inotify".into())
        .spawn(move || {
            // Ends when the watcher (and with it `raw_tx`) is dropped.
            while let Ok(first) = raw_rx.recv() {
                let mut batch = first;
                let until = std::time::Instant::now() + Duration::from_secs(1);
                while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
                    match raw_rx.recv_timeout(left) {
                        Ok(more) => batch.extend(more),
                        Err(_) => break,
                    }
                }
                if tx.send((r.clone(), batch)).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;
    Ok(w)
}

/// The current FSEvents id (0 off macOS).
fn current_event_id() -> u64 {
    #[cfg(target_os = "macos")]
    {
        crate::fsevents::current_event_id()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// The event-store UUID of the device holding `root` (None off macOS).
pub(crate) fn volume_uuid(root: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        crate::fsevents::path_uuid(root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root;
        None
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;

    fn ev(path: String) -> Event {
        Event {
            path,
            flags: 0x10000,
            id: 1,
        }
    }

    #[test]
    fn ancestor_at_cuts_below_the_root() {
        assert_eq!(ancestor_at("/v/a/b/c", "/v", 2), ("/v/a/b".into(), true));
        assert_eq!(ancestor_at("/v/a/b", "/v", 2), ("/v/a/b".into(), false));
        assert_eq!(ancestor_at("/a/b/c", "/", 1), ("/a".into(), true));
        assert_eq!(ancestor_at("/other/x", "/v", 1), ("/other/x".into(), false));
    }

    #[test]
    fn too_many_paths_fold_to_the_deepest_level_that_fits() {
        let mut r = Recording::default();
        // 60k files in 600 folders two levels down: folding to those folders fits.
        let events: Vec<Event> = (0..60_000)
            .map(|i| ev(format!("/v/Users/u{}/f{}/file{i}", i % 3, i % 600)))
            .collect();
        r.add("/v", &events);
        assert_eq!(r.depth, Some(3));
        assert_eq!(r.paths.len(), 600);
        assert!(r.paths.keys().all(|p| p.starts_with("/v/Users/u")));
        assert!(r.paths.values().all(|f| f & MUST_SCAN_SUB_DIRS != 0));
        // Later events fold to the same depth; shallow ones stay as they are.
        r.add(
            "/v",
            &[ev("/v/Users/u1/new/deep/file".into()), ev("/v/top".into())],
        );
        assert!(r.paths.contains_key("/v/Users/u1/new"));
        assert_eq!(r.paths["/v/top"] & MUST_SCAN_SUB_DIRS, 0);
    }

    #[test]
    fn a_second_overflow_folds_shallower() {
        let mut r = Recording::default();
        let wide: Vec<Event> = (0..50_001).map(|i| ev(format!("/v/d{i}/x/y"))).collect();
        r.add("/v", &wide);
        // 50k top-level folders: only depth 1 is left, which still holds them all.
        assert_eq!(r.depth, Some(1));
        assert_eq!(r.paths.len(), 50_001);
    }
}
