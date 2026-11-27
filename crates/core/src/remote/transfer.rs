//! Transfers with a server: download into a local folder, upload from local items, recursive
//! remove, and throttled progress.
//!
//! An upload never leaves a truncated original: a file is written to
//! `.<name>.fxfm-<pid>-<n>.part` in the target folder, gets its mode, and then replaces the
//! target. Only a symlinked target, or a folder where the `.part` cannot be created for lack
//! of permission, is written directly. The `.part` is removed on error and on Cancel.
//!
//! `Disconnected` ends the whole job (the connection is gone); other errors are per item and
//! go through `OpObserver::error`.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use filetime::FileTime;

use super::{join_path, parent_path, RemoteError, RemoteFs, RemoteStat};
pub(crate) use crate::ops::Decision;
use crate::ops::{self, ConflictInfo, ErrorAnswer, OpError, OpObserver, OpSummary, Progress};

/// `obs.progress` is called at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// Running totals of a transfer; reports them to the observer at most every 100 ms, and
/// once more at the end (`finish`).
///
/// A job that handles the same data twice (download into the staging folder, then upload
/// or pack) runs in phases: one bar, each phase filling its share of the bytes, so the user
/// sees 0 to 100 % once. Totals shown are always those of the data itself.
pub struct ProgressState {
    /// `done_bytes` and `total_bytes` count `phases` times the data.
    p: Progress,
    phases: u64,
    phase: u64,
    /// Bytes of the data (one phase).
    width: u64,
    label: Option<&'static str>,
    count_files: bool,
    last: Option<Instant>,
}

impl ProgressState {
    pub fn new(total_bytes: u64, total_files: u64) -> ProgressState {
        ProgressState {
            p: Progress {
                total_bytes,
                total_files,
                ..Progress::default()
            },
            phases: 1,
            phase: 0,
            width: total_bytes,
            label: None,
            count_files: true,
            last: None,
        }
    }

    /// The same data is handled `n` times (at least once).
    pub fn with_phases(mut self, n: u64) -> ProgressState {
        self.phases = n.max(1);
        self.p.total_bytes = self.width * self.phases;
        self
    }

    /// Starts the next phase: bytes continue from its share of the bar.
    pub fn next_phase(&mut self, label: Option<&'static str>, count_files: bool) {
        self.phase = (self.phase + 1).min(self.phases - 1);
        self.p.done_bytes = self.p.done_bytes.max(self.phase * self.width);
        self.label = label;
        self.count_files = count_files;
    }

    /// One phase of a longer job: `current` is shown as `<name> (<label>)`; `None` shows
    /// the full path.
    pub fn set_label(&mut self, label: Option<&'static str>) {
        self.label = label;
    }

    /// Whether finished items count towards `done_files`. The first of two phases over the
    /// same items (download to the staging folder) counts only bytes.
    pub fn set_count_files(&mut self, on: bool) {
        self.count_files = on;
    }

    pub fn get(&self) -> &Progress {
        &self.p
    }

    pub fn set_current(&mut self, current: &str, obs: &dyn OpObserver) {
        self.p.current = match self.label {
            Some(l) => format!("{} ({l})", current.rsplit('/').next().unwrap_or(current)),
            None => current.to_string(),
        };
        self.emit(obs);
    }

    pub fn add_bytes(&mut self, n: u64, obs: &dyn OpObserver) {
        self.p.done_bytes += n;
        self.emit(obs);
    }

    pub fn file_done(&mut self, obs: &dyn OpObserver) {
        self.advance(1, 0, obs);
    }

    /// Counts items as handled without transferring their data (skipped items too, so the
    /// bar reaches the end).
    pub fn advance(&mut self, files: u64, bytes: u64, obs: &dyn OpObserver) {
        if self.count_files {
            self.p.done_files += files;
        }
        self.p.done_bytes += bytes;
        self.emit(obs);
    }

    /// Shows the state of a phase run by other code (extracting, packing) that reports its
    /// own done/total values: `current` as given, scaled into this phase, never backwards.
    pub fn show(&mut self, p: &Progress, obs: &dyn OpObserver) {
        self.p.current = p.current.clone();
        let bytes = self.phase * self.width + scale(p.done_bytes, p.total_bytes, self.width);
        self.p.done_bytes = self.p.done_bytes.max(bytes);
        if self.count_files {
            let files = scale(p.done_files, p.total_files, self.p.total_files);
            self.p.done_files = self.p.done_files.max(files);
        }
        self.emit(obs);
    }

    /// The job is over: the bar is shown full, always (not throttled).
    pub fn finish(&mut self, obs: &dyn OpObserver) {
        self.p.done_bytes = self.p.total_bytes;
        self.p.done_files = self.p.total_files;
        self.last = Some(Instant::now());
        obs.progress(&self.clamped());
    }

    pub(crate) fn done_bytes(&self) -> u64 {
        self.p.done_bytes
    }

    /// Sets done bytes, also backwards (a failed attempt that is retried or skipped).
    pub(crate) fn set_done_bytes(&mut self, b: u64) {
        self.p.done_bytes = b;
    }

    /// What the user sees: bytes of the data (not of all phases), done values never above
    /// the totals (sizes may change during the job).
    fn clamped(&self) -> Progress {
        Progress {
            done_bytes: self.p.done_bytes.min(self.p.total_bytes) / self.phases,
            total_bytes: self.width,
            done_files: self.p.done_files.min(self.p.total_files),
            ..self.p.clone()
        }
    }

    fn emit(&mut self, obs: &dyn OpObserver) {
        if self.last.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        obs.progress(&self.clamped());
    }
}

/// `done` of `theirs` as a share of `ours`.
pub(crate) fn scale(done: u64, theirs: u64, ours: u64) -> u64 {
    if theirs == 0 {
        return done.min(ours);
    }
    let v = u128::from(done) * u128::from(ours) / u128::from(theirs);
    (v as u64).min(ours)
}

/// Why a job stops before its end.
#[derive(Debug, PartialEq)]
pub(crate) enum Stop {
    Cancel,
    /// The connection is gone or hung (`RemoteError::is_lost`).
    Lost,
}

fn fatal(e: &RemoteError) -> Option<Stop> {
    match e {
        e if e.is_lost() => Some(Stop::Lost),
        RemoteError::Cancelled => Some(Stop::Cancel),
        _ => None,
    }
}

/// Error for a listed name that is not a plain file name.
pub const UNSAFE_NAME: &str = "the server listed an unsafe name; it was skipped";

/// A name a server listed that is not a single path component (empty, ".", "..", or with
/// "/" or NUL): following it would reach outside the folder being copied or deleted.
pub fn unsafe_name(name: &str) -> bool {
    name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0')
}

fn io_err(e: io::Error) -> RemoteError {
    RemoteError::Other(e.to_string())
}

/// Conflict and error answers remembered for the whole job.
pub(crate) struct Answers<'a> {
    pub obs: &'a dyn OpObserver,
    conflicts: ops::Conflicts,
    skip_all_errors: bool,
}

impl<'a> Answers<'a> {
    pub fn new(obs: &'a dyn OpObserver) -> Answers<'a> {
        Answers {
            obs,
            conflicts: ops::Conflicts::default(),
            skip_all_errors: false,
        }
    }

    pub fn check_cancel(&self) -> Result<(), Stop> {
        if self.obs.is_cancelled() {
            Err(Stop::Cancel)
        } else {
            Ok(())
        }
    }

    /// Asks about an item error: Ok(true) = retry, Ok(false) = skip.
    pub fn on_error(&mut self, path: &str, msg: &str) -> Result<bool, Stop> {
        if self.skip_all_errors {
            return Ok(false);
        }
        match self.obs.error(path, msg) {
            ErrorAnswer::Retry => Ok(true),
            ErrorAnswer::Skip => Ok(false),
            ErrorAnswer::SkipAll => {
                self.skip_all_errors = true;
                Ok(false)
            }
            ErrorAnswer::Cancel => Err(Stop::Cancel),
        }
    }

    /// Like `on_error` for a `RemoteError`; a lost connection and `Cancelled` stop the job.
    pub fn on_remote_error(&mut self, path: &str, e: &RemoteError) -> Result<bool, Stop> {
        match fatal(e) {
            Some(stop) => Err(stop),
            None => self.on_error(path, &e.to_string()),
        }
    }

    /// Runs `f` until it succeeds or the user skips (`None`).
    pub fn attempt<T>(
        &mut self,
        path: &str,
        mut f: impl FnMut() -> Result<T, RemoteError>,
    ) -> Result<Option<T>, Stop> {
        loop {
            self.check_cancel()?;
            match f() {
                Ok(v) => return Ok(Some(v)),
                Err(e) => {
                    if !self.on_remote_error(path, &e)? {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Resolves a conflict as `ops` does, including "Overwrite all older" by mtime.
    pub fn decide(&mut self, info: &ConflictInfo) -> Result<Decision, Stop> {
        let older = info.target_mtime < info.source_mtime;
        self.conflicts
            .decide(self.obs, older, || info.clone())
            .ok_or(Stop::Cancel)
    }
}

/// Result of a transfer: per item whether all of it arrived, and whether the connection
/// was lost (the summary then covers what happened before).
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub summary: OpSummary,
    pub complete: Vec<bool>,
    pub lost: bool,
}

impl Outcome {
    fn stopped(&mut self, s: Stop) {
        match s {
            Stop::Cancel => self.summary.cancelled = true,
            Stop::Lost => self.lost = true,
        }
    }
}

fn lost_error() -> OpError {
    RemoteError::Disconnected.into_op_error()
}

/// (bytes, non-directory items) under `path`; symlinks are items of size 0 and are not
/// followed.
pub fn walk_size(
    fs: &dyn RemoteFs,
    path: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(u64, u64), RemoteError> {
    if cancelled() {
        return Err(RemoteError::Cancelled);
    }
    let st = fs
        .stat(path)?
        .ok_or_else(|| RemoteError::NotFound(path.to_string()))?;
    if st.is_symlink {
        return Ok((0, 1));
    }
    if !st.is_dir {
        return Ok((st.size, 1));
    }
    let mut total = (0, 0);
    for e in fs.list(path)? {
        let (b, n) = walk_size(fs, &join_path(path, &e.name), cancelled)?;
        total = (total.0 + b, total.1 + n);
    }
    Ok(total)
}

// ---- download ----

/// Downloads server items into a local folder. `shown` prefixes server paths in messages.
pub(crate) struct Downloader<'a, 'p> {
    fs: &'a dyn RemoteFs,
    ans: Answers<'a>,
    progress: &'p mut ProgressState,
    shown: String,
    summary: OpSummary,
}

impl<'a, 'p> Downloader<'a, 'p> {
    pub fn new(
        fs: &'a dyn RemoteFs,
        shown: &str,
        obs: &'a dyn OpObserver,
        progress: &'p mut ProgressState,
    ) -> Self {
        Downloader {
            fs,
            ans: Answers::new(obs),
            progress,
            shown: shown.to_string(),
            summary: OpSummary::default(),
        }
    }

    fn shown(&self, path: &str) -> String {
        format!("{}{path}", self.shown)
    }

    /// Downloads each item into `to_dir` under its own name.
    pub fn run(mut self, items: &[String], to_dir: &Path) -> Outcome {
        let mut out = Outcome::default();
        for it in items {
            let name = it.rsplit('/').next().unwrap_or(it);
            match self.item(it, &to_dir.join(name)) {
                Ok(c) => out.complete.push(c),
                Err(s) => {
                    out.stopped(s);
                    break;
                }
            }
        }
        out.complete.resize(items.len(), false);
        let (cancelled, lost) = (out.summary.cancelled, out.lost);
        out.summary = self.summary;
        out.summary.cancelled = cancelled;
        out.lost = lost;
        out
    }

    fn skip(&mut self, path: &str, st: Option<&RemoteStat>) {
        let (b, f) = match st {
            Some(s) if s.is_symlink => (0, 1),
            Some(s) if !s.is_dir => (s.size, 1),
            _ => walk_size(self.fs, path, &|| false).unwrap_or((0, 1)),
        };
        self.summary.skipped += f.max(1);
        self.progress.advance(f, b, self.ans.obs);
    }

    /// Ok(true) when all of `src` arrived at `dst`.
    fn item(&mut self, src: &str, dst: &Path) -> Result<bool, Stop> {
        self.ans.check_cancel()?;
        let shown = self.shown(src);
        self.progress.set_current(&shown, self.ans.obs);
        let fs = self.fs;
        let found = self.ans.attempt(&shown, || {
            fs.stat(src)?
                .ok_or_else(|| RemoteError::NotFound(src.to_string()))
        })?;
        let Some(st) = found else {
            self.skip(src, None);
            return Ok(false);
        };
        if st.is_symlink {
            // A server without symlinks (FTP) gives the target's content instead.
            let made = self.ans.attempt(&shown, || match fs.read_link(src) {
                Ok(t) => symlink(&t, dst).map(|_| true).map_err(io_err),
                Err(RemoteError::Unsupported(_)) => Ok(false),
                Err(e) => Err(e),
            })?;
            return match made {
                Some(true) => {
                    self.summary.done += 1;
                    self.progress.file_done(self.ans.obs);
                    Ok(true)
                }
                Some(false) => self.file(src, &st, dst),
                None => {
                    self.skip(src, Some(&st));
                    Ok(false)
                }
            };
        }
        if st.is_dir {
            self.dir(src, &st, dst)
        } else {
            self.file(src, &st, dst)
        }
    }

    fn dir(&mut self, src: &str, st: &RemoteStat, dst: &Path) -> Result<bool, Stop> {
        let shown = self.shown(src);
        let created = self.ans.attempt(&shown, || match fs::create_dir(dst) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && dst.is_dir() => Ok(false),
            Err(e) => Err(io_err(e)),
        })?;
        let Some(created) = created else {
            self.skip(src, Some(st));
            return Ok(false);
        };
        let fs = self.fs;
        let Some(mut children) = self.ans.attempt(&shown, || fs.list(src))? else {
            self.skip(src, Some(st));
            return Ok(false);
        };
        children.sort_by(|a, b| a.name.cmp(&b.name));
        let mut complete = true;
        for e in children {
            // A listed name with "/" (or "..") would write outside `dst`: never followed.
            if unsafe_name(&e.name) {
                self.ans
                    .on_error(&format!("{shown}/{}", e.name), UNSAFE_NAME)?;
                self.summary.skipped += 1;
                complete = false;
                continue;
            }
            complete &= self.item(&join_path(src, &e.name), &dst.join(&e.name))?;
        }
        if created {
            set_local_attrs(dst, st.mode & 0o1777, st.mtime);
        }
        Ok(complete)
    }

    fn file(&mut self, src: &str, st: &RemoteStat, dst: &Path) -> Result<bool, Stop> {
        let shown = self.shown(src);
        // Downloads go into an empty staging folder: something already there is another item
        // whose name differs only in case (a case-sensitive server, a disk that ignores case).
        // Writing over it would lose that item, and a move would delete both originals.
        if fs::symlink_metadata(dst).is_ok() {
            self.ans
                .on_error(&shown, crate::archive::read::SAME_FILE_NAME)?;
            self.skip(src, Some(st));
            return Ok(false);
        }
        loop {
            self.ans.check_cancel()?;
            let base = self.progress.done_bytes();
            let (fs, obs) = (self.fs, self.ans.obs);
            let progress = &mut *self.progress;
            let r = File::create(dst).map_err(io_err).and_then(|mut f| {
                fs.read_to(src, &mut f, &mut |n| {
                    progress.add_bytes(n, obs);
                    !obs.is_cancelled()
                })
            });
            match r {
                Ok(()) => {
                    set_local_attrs(dst, st.mode & 0o777, st.mtime);
                    self.summary.done += 1;
                    self.progress.file_done(obs);
                    return Ok(true);
                }
                Err(e) => {
                    let _ = fs::remove_file(dst);
                    self.progress.set_done_bytes(base);
                    if !self.ans.on_remote_error(&shown, &e)? {
                        self.skip(src, Some(st));
                        return Ok(false);
                    }
                }
            }
        }
    }
}

/// Mode (when the server gave one) and mtime of a downloaded item, best effort.
fn set_local_attrs(p: &Path, mode: u32, mtime: i64) {
    if mode != 0 {
        if let Err(e) = fs::set_permissions(p, fs::Permissions::from_mode(mode)) {
            log::warn!("cannot set the mode of {}: {e}", p.display());
        }
    }
    if let Err(e) = filetime::set_symlink_file_times(
        p,
        FileTime::from_unix_time(mtime, 0),
        FileTime::from_unix_time(mtime, 0),
    ) {
        log::warn!("cannot set the time of {}: {e}", p.display());
    }
}

/// Downloads `items` (server paths) into `to_dir`: folders are created, symlinks stay
/// symlinks (or become the target's content where the server has none), mode and mtime are
/// set. Item errors go through `obs.error`.
pub fn download_tree(
    fs: &dyn RemoteFs,
    items: &[String],
    to_dir: &Path,
    obs: &dyn OpObserver,
    progress: &mut ProgressState,
) -> Result<(), OpError> {
    let out = Downloader::new(fs, "", obs, progress).run(items, to_dir);
    if out.lost {
        Err(lost_error())
    } else if out.summary.cancelled {
        Err(OpError::Cancelled)
    } else {
        Ok(())
    }
}

// ---- upload ----

static PART_N: AtomicU64 = AtomicU64::new(0);

/// `.<name>.fxfm-<pid>-<n>.part` next to `target`.
fn part_path(target: &str) -> String {
    let name = target.rsplit('/').next().unwrap_or(target);
    let n = PART_N.fetch_add(1, Ordering::Relaxed);
    join_path(
        parent_path(target),
        &format!(".{name}.fxfm-{}-{n}.part", std::process::id()),
    )
}

/// Uploads local items to a server. `shown` prefixes server paths in messages.
pub(crate) struct Uploader<'a, 'p> {
    fs: &'a dyn RemoteFs,
    ans: Answers<'a>,
    progress: &'p mut ProgressState,
    shown: String,
    summary: OpSummary,
}

impl<'a, 'p> Uploader<'a, 'p> {
    pub fn new(
        fs: &'a dyn RemoteFs,
        shown: &str,
        obs: &'a dyn OpObserver,
        progress: &'p mut ProgressState,
    ) -> Self {
        Uploader {
            fs,
            ans: Answers::new(obs),
            progress,
            shown: shown.to_string(),
            summary: OpSummary::default(),
        }
    }

    fn shown(&self, path: &str) -> String {
        format!("{}{path}", self.shown)
    }

    /// Uploads each `(local item, server target path)`.
    pub fn run(mut self, items: &[(PathBuf, String)]) -> Outcome {
        let mut out = Outcome::default();
        for (src, target) in items {
            match self.item(src, target.clone()) {
                Ok(c) => out.complete.push(c),
                Err(s) => {
                    out.stopped(s);
                    break;
                }
            }
        }
        out.complete.resize(items.len(), false);
        let (cancelled, lost) = (out.summary.cancelled, out.lost);
        out.summary = self.summary;
        out.summary.cancelled = cancelled;
        out.lost = lost;
        out
    }

    fn skip(&mut self, src: &Path) {
        let (b, f) =
            ops::dir_size(src, &std::sync::atomic::AtomicBool::new(false)).unwrap_or((0, 1));
        self.summary.skipped += f.max(1);
        self.progress.advance(f, b, self.ans.obs);
    }

    fn item(&mut self, src: &Path, target: String) -> Result<bool, Stop> {
        self.ans.check_cancel()?;
        let shown_src = src.to_string_lossy().into_owned();
        self.progress.set_current(&shown_src, self.ans.obs);
        let Some(md) = self
            .ans
            .attempt(&shown_src, || fs::symlink_metadata(src).map_err(io_err))?
        else {
            self.summary.skipped += 1;
            self.progress.advance(1, 0, self.ans.obs);
            return Ok(false);
        };
        if md.is_dir() {
            self.dir(src, &md, &target)
        } else {
            self.leaf(src, &md, target)
        }
    }

    fn dir(&mut self, src: &Path, md: &fs::Metadata, target: &str) -> Result<bool, Stop> {
        let shown = self.shown(target);
        let fs = self.fs;
        let mut created = false;
        loop {
            let Some(st) = self.ans.attempt(&shown, || fs.stat(target))? else {
                self.skip(src);
                return Ok(false);
            };
            match st {
                // An existing folder (or a link to one) is merged without asking.
                Some(s) if s.is_dir || (s.is_symlink && fs.list(target).is_ok()) => break,
                Some(_) => {
                    let msg = ops::type_clash(Path::new(&shown), true);
                    if self.ans.on_error(&shown, &msg)? {
                        continue;
                    }
                    self.skip(src);
                    return Ok(false);
                }
                None => match fs.mkdir(target) {
                    Ok(()) => {
                        created = true;
                        break;
                    }
                    Err(RemoteError::Exists(_)) => continue,
                    Err(e) => {
                        if self.ans.on_remote_error(&shown, &e)? {
                            continue;
                        }
                        self.skip(src);
                        return Ok(false);
                    }
                },
            }
        }
        let shown_src = src.to_string_lossy().into_owned();
        let Some(names) = self.ans.attempt(&shown_src, || {
            let mut v = fs::read_dir(src)
                .and_then(|rd| {
                    rd.map(|e| e.map(|e| e.file_name()))
                        .collect::<io::Result<Vec<_>>>()
                })
                .map_err(io_err)?;
            v.sort();
            Ok(v)
        })?
        else {
            self.skip(src);
            return Ok(false);
        };
        let mut complete = true;
        for name in names {
            let child = src.join(&name);
            match name.to_str() {
                Some(n) => complete &= self.item(&child, join_path(target, n))?,
                None => {
                    let shown = child.to_string_lossy().into_owned();
                    self.ans.on_error(&shown, "the name is not valid UTF-8")?;
                    self.skip(&child);
                    complete = false;
                }
            }
        }
        if created {
            // After the children: a read-only mode would prevent creating them.
            self.set_mode(target, md.mode() & 0o7777)?;
        }
        Ok(complete)
    }

    /// Sets a mode, best effort; a server without modes is fine. Skipped entirely when
    /// the backend's uploads do not set modes (FTP).
    fn set_mode(&self, path: &str, mode: u32) -> Result<(), Stop> {
        if !self.fs.upload_sets_mode() {
            return Ok(());
        }
        match self.fs.set_mode(path, mode) {
            Ok(()) | Err(RemoteError::Unsupported(_)) => Ok(()),
            Err(e) => match fatal(&e) {
                Some(s) => Err(s),
                None => {
                    log::warn!("cannot set the mode of {}: {e}", self.shown(path));
                    Ok(())
                }
            },
        }
    }

    /// A file or symlink. Conflicts are asked as in `ops`; Retry keeps the decision.
    fn leaf(&mut self, src: &Path, md: &fs::Metadata, mut target: String) -> Result<bool, Stop> {
        let size = if md.is_file() { md.len() } else { 0 };
        let shown_src = src.to_string_lossy().into_owned();
        let fs = self.fs;
        let mut overwrite = false;
        loop {
            self.ans.check_cancel()?;
            let shown = self.shown(&target);
            let Some(existing) = self.ans.attempt(&shown, || fs.stat(&target))? else {
                self.summary.skipped += 1;
                self.progress.advance(1, size, self.ans.obs);
                return Ok(false);
            };
            if let Some(st) = &existing {
                if st.is_dir {
                    let msg = ops::type_clash(Path::new(&shown), false);
                    if self.ans.on_error(&shown, &msg)? {
                        continue;
                    }
                    self.summary.skipped += 1;
                    self.progress.advance(1, size, self.ans.obs);
                    return Ok(false);
                }
                if !overwrite {
                    let info = ConflictInfo {
                        source: shown_src.clone(),
                        target: shown.clone(),
                        source_size: size,
                        source_mtime: md.mtime(),
                        target_size: st.size,
                        target_mtime: st.mtime,
                    };
                    match self.ans.decide(&info)? {
                        Decision::Overwrite => overwrite = true,
                        Decision::Skip => {
                            self.summary.skipped += 1;
                            self.progress.advance(1, size, self.ans.obs);
                            return Ok(false);
                        }
                        Decision::Rename(n) => {
                            // An invalid name simply asks again for the same target.
                            if ops::valid_name(&n) {
                                target = join_path(parent_path(&target), &n);
                            }
                            continue;
                        }
                    }
                }
            }
            let base = self.progress.done_bytes();
            match self.write(src, md, &target, existing.as_ref()) {
                Ok(()) => {
                    self.progress.set_done_bytes(base + size);
                    self.summary.done += 1;
                    self.progress.file_done(self.ans.obs);
                    return Ok(true);
                }
                Err(e) => {
                    self.progress.set_done_bytes(base);
                    if !self.ans.on_remote_error(&shown, &e)? {
                        self.summary.skipped += 1;
                        self.progress.advance(1, size, self.ans.obs);
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// Writes one file or symlink to `target` (whose current state is `existing`).
    fn write(
        &mut self,
        src: &Path,
        md: &fs::Metadata,
        target: &str,
        existing: Option<&RemoteStat>,
    ) -> Result<(), RemoteError> {
        if md.file_type().is_symlink() {
            let link = fs::read_link(src).map_err(io_err)?;
            if let Some(link) = link.to_str() {
                let part = part_path(target);
                match self.fs.symlink(link, &part) {
                    Ok(()) => return self.put_part(&part, target, existing),
                    // No symlinks on this server: upload what the link points to.
                    Err(RemoteError::Unsupported(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        if existing.is_some_and(|s| s.is_symlink) {
            return self.send(src, target);
        }
        let part = part_path(target);
        match self.send(src, &part) {
            Ok(()) => {}
            Err(RemoteError::PermissionDenied(_))
                if existing.is_some() && matches!(self.fs.stat(&part), Ok(None)) =>
            {
                // The folder does not allow new files, but the target may be writable.
                return self.send(src, target);
            }
            Err(e) => {
                self.drop_part(&part);
                return Err(e);
            }
        }
        let mode = existing
            .map(|s| s.mode & 0o7777)
            .filter(|m| *m != 0)
            .unwrap_or(md.mode() & 0o7777);
        if let Err(s) = self.set_mode(&part, mode) {
            self.drop_part(&part);
            return Err(match s {
                Stop::Cancel => RemoteError::Cancelled,
                Stop::Lost => RemoteError::Disconnected,
            });
        }
        self.put_part(&part, target, existing)
    }

    /// Moves a finished `.part` onto `target`.
    fn put_part(
        &self,
        part: &str,
        target: &str,
        existing: Option<&RemoteStat>,
    ) -> Result<(), RemoteError> {
        let r = if existing.is_some() {
            self.fs.replace(part, target)
        } else {
            self.fs.rename(part, target)
        };
        if let Err(e) = r {
            // A replace that removed the target and then failed leaves the upload as the only
            // copy: the .part stays (and the error says where).
            if existing.is_some() && matches!(self.fs.stat(target), Ok(None)) {
                return Err(RemoteError::Other(format!(
                    "{target}: {e}; the uploaded file is kept as {part}"
                )));
            }
            self.drop_part(part);
            return Err(e);
        }
        Ok(())
    }

    fn drop_part(&self, part: &str) {
        let _ = self.fs.remove_file(part);
    }

    /// Streams the content of local `src` into server file `path` (create or truncate).
    fn send(&mut self, src: &Path, path: &str) -> Result<(), RemoteError> {
        let mut f = File::open(src).map_err(io_err)?;
        let obs = self.ans.obs;
        let progress = &mut *self.progress;
        self.fs.write_from(path, &mut f, &mut |n| {
            progress.add_bytes(n, obs);
            !obs.is_cancelled()
        })
    }
}

/// Uploads local `sources` into server folder `to_dir` (each keeps its name). Existing
/// folders are merged; existing files are a conflict for `obs`.
pub fn upload_tree(
    fs: &dyn RemoteFs,
    sources: &[PathBuf],
    to_dir: &str,
    obs: &dyn OpObserver,
    progress: &mut ProgressState,
) -> Result<OpSummary, OpError> {
    let mut items = Vec::with_capacity(sources.len());
    for s in sources {
        let name = s
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| OpError::Io(format!("invalid source '{}'", s.display())))?;
        items.push((s.clone(), join_path(to_dir, name)));
    }
    let out = Uploader::new(fs, "", obs, progress).run(&items);
    if out.lost {
        return Err(lost_error());
    }
    Ok(out.summary)
}

// ---- remove ----

/// Permanent recursive delete on a server; `obs.error` is asked per item. `shown` prefixes
/// server paths in messages.
pub(crate) struct Remover<'a> {
    ans: Answers<'a>,
    pub shown: String,
    pub summary: OpSummary,
}

impl<'a> Remover<'a> {
    pub fn new(obs: &'a dyn OpObserver, shown: &str) -> Remover<'a> {
        Remover {
            ans: Answers::new(obs),
            shown: shown.to_string(),
            summary: OpSummary::default(),
        }
    }

    /// Reports an item that cannot be removed; Err when the user cancels.
    pub fn fail(&mut self, path: &str, msg: &str) -> Result<(), Stop> {
        self.ans.on_error(path, msg)?;
        self.summary.skipped += 1;
        Ok(())
    }

    /// Removes `path` with everything in it. A missing path is not an error.
    pub fn tree(&mut self, fs: &dyn RemoteFs, path: &str) -> Result<(), Stop> {
        let shown = format!("{}{path}", self.shown);
        let Some(st) = self.ans.attempt(&shown, || fs.stat(path))? else {
            self.summary.skipped += 1;
            return Ok(());
        };
        let Some(st) = st else {
            return Ok(());
        };
        if st.is_dir && !st.is_symlink {
            let Some(children) = self.ans.attempt(&shown, || fs.list(path))? else {
                self.summary.skipped += 1;
                return Ok(());
            };
            let before = self.summary.skipped;
            for e in children {
                // Never delete through a name that points elsewhere.
                if unsafe_name(&e.name) {
                    self.fail(&format!("{shown}/{}", e.name), UNSAFE_NAME)?;
                    continue;
                }
                self.tree(fs, &join_path(path, &e.name))?;
            }
            // A folder with skipped content cannot be removed.
            if self.summary.skipped == before
                && self.ans.attempt(&shown, || fs.remove_dir(path))?.is_none()
            {
                self.summary.skipped += 1;
            }
        } else if self.ans.attempt(&shown, || fs.remove_file(path))?.is_some() {
            self.summary.done += 1;
        } else {
            self.summary.skipped += 1;
        }
        Ok(())
    }
}

/// Permanently removes `path` on the server; returns the number of removed files.
pub fn remove_tree(fs: &dyn RemoteFs, path: &str, obs: &dyn OpObserver) -> Result<u64, OpError> {
    let mut rm = Remover::new(obs, "");
    match rm.tree(fs, path) {
        Ok(()) => Ok(rm.summary.done),
        Err(Stop::Cancel) => Err(OpError::Cancelled),
        Err(Stop::Lost) => Err(lost_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::ConflictAnswer;
    use crate::remote::testutil::MemFs;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Count(Mutex<Vec<Progress>>);

    impl OpObserver for Count {
        fn progress(&self, p: &Progress) {
            self.0.lock().unwrap().push(p.clone());
        }
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            ConflictAnswer::Overwrite
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            ErrorAnswer::Skip
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn transfer_progress_is_throttled() {
        let obs = Count::default();
        let mut p = ProgressState::new(1000, 10);
        for _ in 0..500 {
            p.add_bytes(1, &obs);
        }
        assert_eq!(obs.0.lock().unwrap().len(), 1);
        std::thread::sleep(PROGRESS_EVERY + Duration::from_millis(20));
        p.add_bytes(1, &obs);
        let v = obs.0.lock().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[1].done_bytes, v[1].total_bytes), (501, 1000));
    }

    /// MemFs whose listing of "/s" also has a name that points outside the folder.
    struct Evil(MemFs);
    impl RemoteFs for Evil {
        fn display(&self) -> String {
            self.0.display()
        }
        fn home(&self) -> Result<String, RemoteError> {
            self.0.home()
        }
        fn list(&self, path: &str) -> Result<Vec<crate::listing::Entry>, RemoteError> {
            let mut v = self.0.list(path)?;
            if path == "/s" {
                let mut bad = v[0].clone();
                bad.name = "../evil.txt".into();
                v.push(bad);
            }
            Ok(v)
        }
        fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
            self.0.stat(path)
        }
        fn read_link(&self, path: &str) -> Result<String, RemoteError> {
            self.0.read_link(path)
        }
        fn read_to(
            &self,
            path: &str,
            out: &mut dyn std::io::Write,
            p: &mut dyn FnMut(u64) -> bool,
        ) -> Result<(), RemoteError> {
            self.0.read_to(path, out, p)
        }
        fn write_from(
            &self,
            path: &str,
            input: &mut dyn std::io::Read,
            p: &mut dyn FnMut(u64) -> bool,
        ) -> Result<(), RemoteError> {
            self.0.write_from(path, input, p)
        }
        fn mkdir(&self, path: &str) -> Result<(), RemoteError> {
            self.0.mkdir(path)
        }
        fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
            self.0.remove_file(path)
        }
        fn remove_dir(&self, path: &str) -> Result<(), RemoteError> {
            self.0.remove_dir(path)
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
            self.0.rename(from, to)
        }
        fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
            self.0.replace(from, to)
        }
        fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError> {
            self.0.symlink(target, link)
        }
        fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError> {
            self.0.set_mode(path, mode)
        }
        fn fork(&self) -> Result<std::sync::Arc<dyn RemoteFs>, RemoteError> {
            self.0.fork()
        }
        fn close(&self) {}
    }

    /// A name with "/" from a listing is reported and skipped: nothing is written next to the
    /// target folder, and deleting the folder leaves it (and the real file) in place.
    #[test]
    fn unsafe_listed_names_never_reach_outside() {
        assert!(unsafe_name("../x") && unsafe_name("a/b") && unsafe_name("..") && unsafe_name(""));
        assert!(!unsafe_name("..hidden") && !unsafe_name("a b.txt"));
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let m = MemFs::new();
        m.put_file("/s/a.txt", b"aaa", 0o644, 0);
        m.put_file("/evil.txt", b"evil", 0o644, 0);
        let evil = Evil(m.clone());
        let obs = Count::default();
        let mut p = ProgressState::new(3, 1);
        let _ = download_tree(&evil, &["/s".to_string()], &out, &obs, &mut p);
        assert_eq!(fs::read(out.join("s/a.txt")).unwrap(), b"aaa");
        assert!(!out.join("evil.txt").exists());
        assert!(!t.path().join("evil.txt").exists());
        let _ = remove_tree(&evil, "/s", &obs);
        assert!(m.exists("/evil.txt"), "the file outside the folder stays");
        assert!(
            m.exists("/s"),
            "a folder with a skipped name is not removed"
        );
    }

    /// A case-sensitive server with `A.txt` and `a.txt`: on a disk that ignores case the second
    /// is reported and skipped (not written over the first), so the folder counts as incomplete.
    #[test]
    fn names_differing_only_in_case_are_not_written_over_each_other() {
        let probe = tempfile::tempdir().unwrap();
        fs::write(probe.path().join("X"), b"").unwrap();
        if !probe.path().join("x").exists() {
            return; // this disk keeps both
        }
        let t = tempfile::tempdir().unwrap();
        let m = MemFs::new();
        m.put_file("/s/A.txt", b"upper", 0o644, 0);
        m.put_file("/s/a.txt", b"lower", 0o644, 0);
        let obs = Count::default();
        let mut p = ProgressState::new(10, 2);
        let out = Downloader::new(&m, "", &obs, &mut p).run(&["/s".to_string()], t.path());
        assert_eq!(fs::read(t.path().join("s/A.txt")).unwrap(), b"upper");
        assert_eq!((out.summary.done, out.summary.skipped), (1, 1));
        assert!(
            !out.complete.iter().all(|c| *c),
            "the folder is not complete"
        );
    }

    #[test]
    fn transfer_public_api_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let m = MemFs::new();
        m.put_file("/s/a.txt", b"aaa", 0o640, 1_000_000);
        m.put_file("/s/sub/b.txt", b"bb", 0o644, 0);
        let obs = Count::default();
        let mut p = ProgressState::new(5, 2);
        download_tree(&m, &["/s".to_string()], t.path(), &obs, &mut p).unwrap();
        assert_eq!(fs::read(t.path().join("s/a.txt")).unwrap(), b"aaa");
        assert_eq!(p.get().done_files, 2);
        m.put_dir("/up");
        let mut p = ProgressState::new(5, 2);
        let r = upload_tree(&m, &[t.path().join("s")], "/up", &obs, &mut p).unwrap();
        assert_eq!((r.done, r.skipped), (2, 0));
        assert_eq!(m.data("/up/s/sub/b.txt").unwrap(), b"bb");
        assert_eq!(m.mode_of("/up/s/a.txt"), Some(0o640));
        assert_eq!(walk_size(&m, "/up", &|| false).unwrap(), (5, 2));
        assert_eq!(remove_tree(&m, "/up/s", &obs).unwrap(), 2);
        assert!(!m.exists("/up/s"));
        m.put_file("/x", b"x", 0o644, 0);
        m.fail_next("remove_file", RemoteError::Disconnected);
        let r = remove_tree(&m, "/x", &obs);
        assert!(matches!(r, Err(OpError::Io(ref s)) if s.starts_with("connection lost")));
    }

    #[test]
    fn memfs_fail_after_bytes_is_exact_and_one_shot() {
        let m = MemFs::new();
        m.fail_after_bytes(3);
        m.write_from("/a", &mut &b"abc"[..], &mut |_| true).unwrap();
        assert_eq!(m.data("/a").unwrap(), b"abc");
        assert_eq!(
            m.write_from("/b", &mut &b"abcd"[..], &mut |_| true),
            Err(RemoteError::Disconnected)
        );
        assert_eq!(m.data("/b").unwrap(), b"abc");
        m.write_from("/c", &mut &b"abcdef"[..], &mut |_| true)
            .unwrap();
        assert_eq!(m.data("/c").unwrap(), b"abcdef");
    }

    #[test]
    fn upload_without_modes_never_calls_set_mode() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("s")).unwrap();
        fs::write(t.path().join("s/a.txt"), b"a").unwrap();
        let m = MemFs::new();
        m.put_dir("/up");
        m.put_file("/up/s/a.txt", b"old", 0o600, 0);
        m.set_upload_sets_mode(false);
        let obs = Count::default();
        let mut p = ProgressState::new(1, 1);
        let r = upload_tree(&m, &[t.path().join("s")], "/up", &obs, &mut p).unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(m.data("/up/s/a.txt").unwrap(), b"a");
        fs::write(t.path().join("s/b.txt"), b"b").unwrap();
        m.put_dir("/up2");
        let mut p = ProgressState::new(2, 2);
        upload_tree(&m, &[t.path().join("s")], "/up2", &obs, &mut p).unwrap();
        assert_eq!(m.calls("set_mode"), 0);
    }

    #[test]
    fn transfer_part_names() {
        let p = part_path("/up/a.txt");
        let pid = std::process::id();
        assert!(p.starts_with(&format!("/up/.a.txt.fxfm-{pid}-")), "{p}");
        assert!(p.ends_with(".part"));
        assert_ne!(part_path("/up/a.txt"), part_path("/up/a.txt"));
    }
}
