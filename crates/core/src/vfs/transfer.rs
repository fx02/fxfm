//! Copy and move between local folders, archives and servers.
//!
//! Archive data passes through a staging folder (`stage-<n>` under the temp root) that is
//! removed at the end in every case. Safety rules:
//! - an archive is only ever changed by one atomic `rewrite`, so it is either fully updated or
//!   byte-identical;
//! - a moved source is deleted only after its destination was written successfully. If that
//!   final delete fails, the error is returned and the data exists in both places.
//!
//! Server items pass through a staging folder too, except an upload from a local folder
//! (direct) and a move on one server (renames). Every job that touches a server works on a
//! forked connection (`RemoteFs::fork`), and a lost connection ends the job and drops the
//! session.
//!
//! Conflicts with items in an archive are resolved per top-level item: an existing folder
//! is one question, and "Overwrite" merges into it (files with the same name are replaced,
//! with "Overwrite all older" only older ones). This is simpler than the per-file questions
//! of a local copy. Conflicts on a local destination follow `ops` exactly.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use unicode_normalization::UnicodeNormalization;

use crate::archive::read::extract;
use crate::archive::write::{rewrite, Edit};
use crate::archive::{split_location, ArchiveIndex, ArchiveKind, ArchiveLoc, IndexEntry};
use crate::ops::{
    self, ConflictAnswer, ConflictInfo, ErrorAnswer, OpError, OpObserver, OpSummary, Progress,
};

use crate::remote::transfer::{scale, ProgressState, Uploader};
use crate::remote::transfer::{walk_size, Answers, Decision, Downloader, Outcome, Remover, Stop};
use crate::remote::{join_path, parent_path, parse_remote, RemoteError, RemoteFs, RemoteLoc};

use super::{
    archive_op_error, inner_loc, join_inner, parent_inner, session_for, size_in, Ctx, NullObserver,
};

pub fn copy(
    ctx: &Ctx,
    sources: &[String],
    dest: &str,
    cwd: &str,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    transfer(ctx, sources, dest, cwd, obs, false)
}

pub fn move_(
    ctx: &Ctx,
    sources: &[String],
    dest: &str,
    cwd: &str,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    transfer(ctx, sources, dest, cwd, obs, true)
}

fn transfer(
    ctx: &Ctx,
    sources: &[String],
    dest: &str,
    cwd: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    if sources.is_empty() {
        return Ok(OpSummary::default());
    }
    let dest = if dest.starts_with('/') || parse_remote(dest).is_some() {
        dest.to_string()
    } else {
        format!("{}/{dest}", cwd.trim_end_matches('/'))
    };
    let remote_src = remote_sources(sources)?;
    if remote_src.is_some() || parse_remote(&dest).is_some() {
        return server_transfer(ctx, remote_src, sources, &dest, obs, moving);
    }
    match (archive_sources(sources)?, split_location(&dest)) {
        (None, None) => {
            let paths: Vec<PathBuf> = sources.iter().map(PathBuf::from).collect();
            let d = ops::resolve_dest(&paths, &dest, Path::new("/"));
            if moving {
                ops::move_(&paths, &d, obs)
            } else {
                ops::copy(&paths, &d, obs)
            }
        }
        (Some(src), None) => archive_to_local(ctx, &src, &dest, obs, moving),
        (None, Some(a)) => {
            let (to, index) = archive_dest(ctx, a, &dest, sources.len())?;
            local_to_archive(ctx, sources, &to, &index, obs, moving)
        }
        (Some(src), Some(a)) => {
            let (to, index) = archive_dest(ctx, a, &dest, sources.len())?;
            archive_to_archive(ctx, &src, &to, &index, obs, moving)
        }
    }
}

// ---- locations ----

/// Items inside one archive, all from the same folder.
struct Sources {
    archive: PathBuf,
    kind: ArchiveKind,
    items: Vec<String>,
}

impl Sources {
    fn shown(&self, inner: &str) -> String {
        join_inner(&self.loc(), inner)
    }
    fn loc(&self) -> ArchiveLoc {
        ArchiveLoc {
            archive: self.archive.clone(),
            kind: self.kind,
            inner: String::new(),
        }
    }
}

/// `None` when all sources are local items (the archive file itself is a local item).
fn archive_sources(sources: &[String]) -> Result<Option<Sources>, OpError> {
    let locs: Vec<Option<ArchiveLoc>> = sources.iter().map(|s| inner_loc(s)).collect();
    if locs.iter().all(Option::is_none) {
        return Ok(None);
    }
    let mixed = || OpError::Io("all items must come from the same folder".into());
    let first = locs[0].clone().ok_or_else(mixed)?;
    let mut items = Vec::with_capacity(locs.len());
    for l in locs {
        let l = l.ok_or_else(mixed)?;
        if l.archive != first.archive {
            return Err(mixed());
        }
        items.push(l.inner);
    }
    Ok(Some(Sources {
        archive: first.archive,
        kind: first.kind,
        items,
    }))
}

/// Where items go inside an archive: into folder `dir`, or as the single item `as_file`.
struct ArchiveDest {
    archive: PathBuf,
    kind: ArchiveKind,
    dir: String,
    as_file: Option<String>,
}

impl ArchiveDest {
    fn target(&self, name: &str) -> String {
        match &self.as_file {
            Some(t) => t.clone(),
            None => join_rel(&self.dir, name),
        }
    }
    fn shown(&self, inner: &str) -> String {
        format!("{}/{inner}", self.archive.to_string_lossy())
    }
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

fn base_name(inner: &str) -> &str {
    inner.rsplit('/').next().unwrap_or(inner)
}

/// True if inner path `p` is `base` or below it.
fn under(p: &str, base: &str) -> bool {
    p.starts_with(base) && (p.len() == base.len() || p.as_bytes()[base.len()] == b'/')
}

/// Resolves `.` and `..` of an inner path; `None` if it leaves the archive.
fn normalize_inner(inner: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for c in inner.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(nfc(&parts.join("/")))
}

/// Like `ops::resolve_dest`, but whether the destination is a folder comes from the index.
fn archive_dest(
    ctx: &Ctx,
    a: ArchiveLoc,
    dest: &str,
    count: usize,
) -> Result<(ArchiveDest, std::sync::Arc<ArchiveIndex>), OpError> {
    let inner = normalize_inner(&a.inner)
        .ok_or_else(|| OpError::Io(format!("invalid destination '{dest}'")))?;
    let index = ctx
        .cache
        .get(&a.archive, a.kind)
        .map_err(|e| archive_op_error(e, &a.archive))?;
    let into_dir = count > 1
        || dest.ends_with('/')
        || inner.is_empty()
        || index.find(&inner).is_some_and(|e| e.is_dir);
    let (dir, as_file) = if into_dir {
        (inner, None)
    } else {
        (parent_inner(&inner).to_string(), Some(inner))
    };
    Ok((
        ArchiveDest {
            archive: a.archive,
            kind: a.kind,
            dir,
            as_file,
        },
        index,
    ))
}

/// (bytes, non-directory items) of a local item.
fn local_size(p: &Path) -> (u64, u64) {
    ops::dir_size(p, &AtomicBool::new(false)).unwrap_or((0, 1))
}

// ---- staging and observers ----

/// A staging folder, removed when dropped (also on error and unwinding).
struct Stage<'a> {
    ctx: &'a Ctx,
    dir: PathBuf,
}

impl<'a> Stage<'a> {
    fn new(ctx: &'a Ctx) -> Result<Stage<'a>, OpError> {
        let dir = ctx
            .new_temp_dir("stage")
            .map_err(|e| OpError::Io(format!("cannot create a staging folder: {e}")))?;
        Ok(Stage { ctx, dir })
    }
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        self.ctx.cleanup_dir(&self.dir);
    }
}

/// Forwards to the user's observer during one archive phase. `current` becomes
/// `<name> (<label>)`; with `total`, progress is scaled to the operation's own totals.
/// Done values never go backwards. Remembers whether an item error was reported.
struct PhaseObs<'a> {
    obs: &'a dyn OpObserver,
    label: &'static str,
    /// (bytes, files)
    total: Option<(u64, u64)>,
    last: Mutex<(u64, u64)>,
    errors: AtomicBool,
}

impl<'a> PhaseObs<'a> {
    fn new(obs: &'a dyn OpObserver, label: &'static str, total: Option<(u64, u64)>) -> Self {
        PhaseObs {
            obs,
            label,
            total,
            last: Mutex::new((0, 0)),
            errors: AtomicBool::new(false),
        }
    }
    fn had_errors(&self) -> bool {
        self.errors.load(Ordering::Relaxed)
    }
}

impl OpObserver for PhaseObs<'_> {
    fn progress(&self, p: &Progress) {
        let name = base_name(&p.current);
        let current = if name.is_empty() {
            String::new()
        } else {
            format!("{name} ({})", self.label)
        };
        let (mut db, mut df, tb, tf) = match self.total {
            Some((tb, tf)) => (
                scale(p.done_bytes, p.total_bytes, tb),
                scale(p.done_files, p.total_files, tf),
                tb,
                tf,
            ),
            None => (p.done_bytes, p.done_files, p.total_bytes, p.total_files),
        };
        {
            let mut last = self.last.lock().unwrap();
            db = db.max(last.0);
            df = df.max(last.1);
            *last = (db, df);
        }
        self.obs.progress(&Progress {
            current,
            done_bytes: db,
            total_bytes: tb,
            done_files: df,
            total_files: tf,
        });
    }
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.obs.conflict(c)
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.errors.store(true, Ordering::Relaxed);
        self.obs.error(path, msg)
    }
    fn is_cancelled(&self) -> bool {
        self.obs.is_cancelled()
    }
}

/// Forwards to the user's observer while staged items go to a local folder; staged paths
/// are shown as the archive locations they came from.
struct StageObs<'a> {
    obs: &'a dyn OpObserver,
    stage: &'a Path,
    shown: String,
}

impl StageObs<'_> {
    fn map(&self, p: &str) -> String {
        match Path::new(p).strip_prefix(self.stage) {
            Ok(rest) if rest.as_os_str().is_empty() => self.shown.clone(),
            Ok(rest) => format!("{}/{}", self.shown, rest.to_string_lossy()),
            Err(_) => p.to_string(),
        }
    }
}

impl OpObserver for StageObs<'_> {
    fn progress(&self, p: &Progress) {
        self.obs.progress(&Progress {
            current: self.map(&p.current),
            ..p.clone()
        });
    }
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.obs.conflict(&ConflictInfo {
            source: self.map(&c.source),
            ..c.clone()
        })
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.obs.error(&self.map(path), msg)
    }
    fn is_cancelled(&self) -> bool {
        self.obs.is_cancelled()
    }
}

/// (bytes, files) of the sources, from the archive's index.
fn archive_totals(ctx: &Ctx, src: &Sources) -> Result<(u64, u64), OpError> {
    let index = ctx
        .cache
        .get(&src.archive, src.kind)
        .map_err(|e| archive_op_error(e, &src.archive))?;
    let (mut bytes, mut files) = (0, 0);
    for inner in &src.items {
        let (b, f) = size_in(&index, inner)
            .ok_or_else(|| OpError::Io(format!("not found in archive: {}", src.shown(inner))))?;
        bytes += b;
        files += f;
    }
    Ok((bytes, files))
}

/// Extracts the sources into `stage`. Returns the staged path of each source (it may be
/// missing when nothing of it could be extracted) and whether any item error was reported.
fn extract_sources(
    ctx: &Ctx,
    src: &Sources,
    stage: &Path,
    obs: &dyn OpObserver,
) -> Result<(Vec<PathBuf>, bool), OpError> {
    let (bytes, files) = archive_totals(ctx, src)?;
    let pobs = PhaseObs::new(obs, "extracting", Some((bytes, files)));
    let pw = ctx.password(&src.archive);
    extract(
        &src.archive,
        src.kind,
        &src.items,
        stage,
        pw.as_deref(),
        &pobs,
    )
    .map_err(|e| archive_op_error(e, &src.archive))?;
    let staged = src.items.iter().map(|i| stage.join(base_name(i))).collect();
    Ok((staged, pobs.had_errors()))
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// The items (inner paths) that left the staging folder for `target`: gone from staging
/// and present at the target. Gone alone is not enough: a staging folder removed under
/// the job would make every item look moved.
fn transferred(
    items: &[String],
    staged: &[PathBuf],
    present: &[PathBuf],
    target: &ops::Dest,
) -> Vec<String> {
    items
        .iter()
        .zip(staged)
        .filter(|(_, p)| !exists(p))
        .filter(|(_, p)| present.contains(p))
        .filter(|(i, _)| match target {
            ops::Dest::IntoDir(d) => exists(&d.join(base_name(i))),
            ops::Dest::AsFile(p) => exists(p),
        })
        .map(|(i, _)| i.clone())
        .collect()
}

/// Deletes moved items from the source archive with one rewrite.
fn delete_from_archive(
    ctx: &Ctx,
    src: &Sources,
    inners: Vec<String>,
    obs: &dyn OpObserver,
) -> Result<(), OpError> {
    if inners.is_empty() {
        return Ok(());
    }
    let edits: Vec<Edit> = inners
        .into_iter()
        .map(|inner| Edit::Delete { inner })
        .collect();
    let pobs = PhaseObs::new(obs, "packing", None);
    rewrite(&src.archive, src.kind, &edits, &ctx.cache, &pobs)
        .map_err(|e| archive_op_error(e, &src.archive))
}

/// Last step of a move out of an archive: deletes the `transferred` items from the source
/// archive. With `withhold` (cancelled, or extraction reported an item error) nothing is
/// deleted, and the files left in the source count as skipped instead of done.
fn finish_move(
    ctx: &Ctx,
    src: &Sources,
    transferred: Vec<String>,
    withhold: bool,
    summary: &mut OpSummary,
    obs: &dyn OpObserver,
) -> Result<(), OpError> {
    if !withhold {
        return delete_from_archive(ctx, src, transferred, obs);
    }
    if transferred.is_empty() {
        return Ok(());
    }
    let index = ctx
        .cache
        .get(&src.archive, src.kind)
        .map_err(|e| archive_op_error(e, &src.archive))?;
    let left: u64 = transferred
        .iter()
        .map(|i| size_in(&index, i).map_or(1, |(_, n)| n.max(1)))
        .sum();
    summary.done = summary.done.saturating_sub(left);
    summary.skipped += left;
    Ok(())
}

// ---- archive -> local ----

fn archive_to_local(
    ctx: &Ctx,
    src: &Sources,
    dest: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let originals: Vec<PathBuf> = src.items.iter().map(PathBuf::from).collect();
    let target = ops::resolve_dest(&originals, dest, Path::new("/"));
    // Writing onto the archive being read (or a folder holding it) could replace it.
    let archive = ops::location(&src.archive);
    for inner in &src.items {
        let t = match &target {
            ops::Dest::IntoDir(d) => d.join(base_name(inner)),
            ops::Dest::AsFile(p) => p.clone(),
        };
        if archive.starts_with(ops::location(&t)) {
            return Err(OpError::IntoItself(src.shown(inner)));
        }
    }
    let stage = Stage::new(ctx)?;
    let (staged, errors) = extract_sources(ctx, src, &stage.dir, obs)?;
    let present: Vec<PathBuf> = staged.iter().filter(|p| exists(p)).cloned().collect();
    let missing = (staged.len() - present.len()) as u64;
    let sobs = StageObs {
        obs,
        stage: &stage.dir,
        shown: src.shown(parent_inner(&src.items[0])),
    };
    // Moving out of the staging folder applies the local conflict rules, and whatever was
    // not transferred (skipped, failed, cancelled) is still in the staging folder afterwards.
    // An observer cannot tell that, because "Skip all" and "Overwrite all older" skip
    // without asking.
    let mut summary = if present.is_empty() {
        OpSummary::default()
    } else {
        ops::move_(&present, &target, &sobs)?
    };
    summary.skipped += missing;
    if moving {
        let gone = transferred(&src.items, &staged, &present, &target);
        let withhold = summary.cancelled || errors;
        finish_move(ctx, src, gone, withhold, &mut summary, obs)?;
    }
    Ok(summary)
}

// ---- servers ----

/// Server items, all from one server.
struct RemoteSources {
    loc: RemoteLoc,
    paths: Vec<String>,
}

/// `None` when no source is on a server; mixing server and other items is refused.
fn remote_sources(sources: &[String]) -> Result<Option<RemoteSources>, OpError> {
    let locs: Vec<Option<RemoteLoc>> = sources.iter().map(|s| parse_remote(s)).collect();
    if locs.iter().all(Option::is_none) {
        return Ok(None);
    }
    let mixed = || OpError::Io("all items must come from the same folder".into());
    let first = locs[0].clone().ok_or_else(mixed)?;
    let mut paths = Vec::with_capacity(locs.len());
    for l in locs {
        let l = l.ok_or_else(mixed)?;
        if l.server != first.server || l.scheme != first.scheme {
            return Err(mixed());
        }
        paths.push(l.path);
    }
    Ok(Some(RemoteSources { loc: first, paths }))
}

/// The forked connection of one server for the duration of a job.
#[derive(Clone)]
struct Server<'c> {
    ctx: &'c Ctx,
    name: String,
    scheme: String,
    fs: Arc<dyn RemoteFs>,
}

impl<'c> Server<'c> {
    fn open(ctx: &'c Ctx, r: &RemoteLoc) -> Result<Server<'c>, OpError> {
        let fs = session_for(ctx, r).and_then(|s| s.fork()).map_err(|e| {
            if e.is_lost() {
                ctx.sessions.remove(&r.server);
            }
            e.into_op_error()
        })?;
        Ok(Server {
            ctx,
            name: r.server.clone(),
            scheme: r.scheme.clone(),
            fs,
        })
    }

    /// `sftp://server`, the prefix of every path shown to the user.
    fn prefix(&self) -> String {
        format!("{}://{}", self.scheme, self.name)
    }

    fn shown(&self, path: &str) -> String {
        format!("{}{path}", self.prefix())
    }

    /// Shown location of the folder holding `path` (as `StageObs` expects it).
    fn shown_parent(&self, path: &str) -> String {
        match parent_path(path) {
            "/" => self.prefix(),
            p => self.shown(p),
        }
    }

    /// Ends the job: the connection is gone (or hung), so it leaves the registry.
    fn lost(&self) -> OpError {
        self.ctx.sessions.remove(&self.name);
        RemoteError::Disconnected.into_op_error()
    }

    fn err(&self, e: RemoteError) -> OpError {
        match e {
            e if e.is_lost() => self.lost(),
            e => e.into_op_error(),
        }
    }

    fn stop(&self, s: Stop) -> OpError {
        match s {
            Stop::Lost => self.lost(),
            Stop::Cancel => OpError::Cancelled,
        }
    }

    /// (bytes, items) of `paths`, counted on the server before the transfer starts.
    fn totals(&self, paths: &[String], obs: &dyn OpObserver) -> Result<(u64, u64), OpError> {
        let mut total = (0, 0);
        for p in paths {
            match walk_size(self.fs.as_ref(), p, &|| obs.is_cancelled()) {
                Ok((b, f)) => total = (total.0 + b, total.1 + f),
                Err(RemoteError::Cancelled) => return Err(OpError::Cancelled),
                Err(e) if e.is_lost() => return Err(self.lost()),
                Err(_) => {}
            }
        }
        Ok(total)
    }

    /// Downloads `paths` into `stage` (each under its own name).
    fn download(
        &self,
        paths: &[String],
        stage: &Path,
        obs: &dyn OpObserver,
        progress: &mut ProgressState,
    ) -> Result<Outcome, OpError> {
        let out =
            Downloader::new(self.fs.as_ref(), &self.prefix(), obs, progress).run(paths, stage);
        if out.lost {
            return Err(self.lost());
        }
        Ok(out)
    }

    /// Uploads `(local item, target path)` pairs.
    fn upload(
        &self,
        items: &[(PathBuf, String)],
        obs: &dyn OpObserver,
        progress: &mut ProgressState,
    ) -> Result<Outcome, OpError> {
        let out = Uploader::new(self.fs.as_ref(), &self.prefix(), obs, progress).run(items);
        if out.lost {
            return Err(self.lost());
        }
        Ok(out)
    }

    /// Permanently removes moved sources; their data is already at the destination. As for
    /// a local move, a source left behind is an error; a Cancel here cancels the job.
    fn remove(
        &self,
        paths: Vec<String>,
        obs: &dyn OpObserver,
        summary: &mut OpSummary,
    ) -> Result<(), OpError> {
        let fs = self.fs.as_ref();
        let mut rm = Remover::new(obs, &self.prefix());
        for p in &paths {
            match rm.tree(fs, p) {
                Ok(()) => {}
                Err(Stop::Cancel) => {
                    summary.cancelled = true;
                    return Ok(());
                }
                Err(Stop::Lost) => return Err(self.lost()),
            }
        }
        for p in &paths {
            match fs.stat(p) {
                Ok(None) => {}
                Err(e) if e.is_lost() => return Err(self.lost()),
                _ => {
                    return Err(OpError::Io(format!(
                        "'{}' was copied but could not be deleted",
                        self.shown(p)
                    )))
                }
            }
        }
        Ok(())
    }
}

/// Shows a phase run by archive code (extracting, packing) on the job's one bar.
struct Share<'a, 's> {
    obs: &'a dyn OpObserver,
    state: Mutex<&'s mut ProgressState>,
}

impl<'a, 's> Share<'a, 's> {
    fn new(obs: &'a dyn OpObserver, state: &'s mut ProgressState) -> Self {
        Share {
            obs,
            state: Mutex::new(state),
        }
    }
}

impl OpObserver for Share<'_, '_> {
    fn progress(&self, p: &Progress) {
        self.state.lock().unwrap().show(p, self.obs);
    }
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.obs.conflict(c)
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.obs.error(path, msg)
    }
    fn is_cancelled(&self) -> bool {
        self.obs.is_cancelled()
    }
}

/// A bar for a job that downloads into the staging folder first and then writes the same
/// data once more (upload, pack): the download is the first of two phases.
fn staged_progress(bytes: u64, files: u64, first: &'static str) -> ProgressState {
    let mut p = ProgressState::new(bytes, files).with_phases(2);
    p.set_label(Some(first));
    p.set_count_files(false);
    p
}

/// Where items go on a server: into folder `dir`, or as the single item `as_file`.
struct RemoteDest {
    dir: String,
    as_file: Option<String>,
}

impl RemoteDest {
    fn target(&self, name: &str) -> String {
        match &self.as_file {
            Some(t) => t.clone(),
            None => join_path(&self.dir, name),
        }
    }
}

/// Like `ops::resolve_dest`, with the server telling whether the destination is a folder.
fn remote_dest(
    srv: &Server,
    r: &RemoteLoc,
    raw: &str,
    count: usize,
) -> Result<RemoteDest, OpError> {
    let fs = srv.fs.as_ref();
    let is_dir = r.path == "/"
        || match fs.stat(&r.path).map_err(|e| srv.err(e))? {
            Some(st) if st.is_dir => true,
            Some(st) if st.is_symlink => fs.list(&r.path).is_ok(),
            _ => false,
        };
    let dest = if count > 1 || raw.ends_with('/') || is_dir {
        RemoteDest {
            dir: r.path.clone(),
            as_file: None,
        }
    } else {
        RemoteDest {
            dir: parent_path(&r.path).to_string(),
            as_file: Some(r.path.clone()),
        }
    };
    Ok(dest)
}

/// As `ops`: a missing destination folder is created before the transfer.
fn prepare_remote_dest(srv: &Server, dest: &RemoteDest) -> Result<(), OpError> {
    mkdir_all(srv.fs.as_ref(), &dest.dir).map_err(|e| match e {
        e if e.is_lost() => srv.lost(),
        e => OpError::Io(format!("{}: {e}", srv.shown(&dest.dir))),
    })
}

/// Creates folder `path` and its missing parents on the server.
fn mkdir_all(fs: &dyn RemoteFs, path: &str) -> Result<(), RemoteError> {
    if path == "/" || fs.stat(path)?.is_some() {
        return Ok(());
    }
    mkdir_all(fs, parent_path(path))?;
    match fs.mkdir(path) {
        Ok(()) | Err(RemoteError::Exists(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Forwards everything but progress: the step from the staging folder to a local
/// destination is part of the transfer already shown, not a new job.
struct NoProgress<'a>(&'a dyn OpObserver);

impl OpObserver for NoProgress<'_> {
    fn progress(&self, _p: &Progress) {}
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.0.conflict(c)
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.0.error(path, msg)
    }
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// The items of `paths` whose flags are set in both `a` and `b`.
fn picked(paths: &[String], a: &[bool], b: &[bool]) -> Vec<String> {
    paths
        .iter()
        .zip(a.iter().zip(b))
        .filter(|(_, (x, y))| **x && **y)
        .map(|(p, _)| p.clone())
        .collect()
}

/// A cancelled download: nothing reaches the destination.
fn cancelled_download(out: Outcome) -> OpSummary {
    OpSummary {
        done: 0,
        skipped: out.summary.skipped,
        cancelled: true,
    }
}

fn server_transfer(
    ctx: &Ctx,
    src: Option<RemoteSources>,
    sources: &[String],
    dest: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    match (src, parse_remote(dest)) {
        (Some(src), Some(r)) => server_to_server(ctx, &src, &r, dest, obs, moving),
        (Some(src), None) => match split_location(dest) {
            Some(a) => server_to_archive(ctx, &src, a, dest, obs, moving),
            None => server_to_local(ctx, &src, dest, obs, moving),
        },
        (None, Some(r)) => to_server(ctx, sources, &r, dest, obs, moving),
        (None, None) => unreachable!("server_transfer without a server"),
    }
}

/// Local items or archive items to a server.
fn to_server(
    ctx: &Ctx,
    sources: &[String],
    r: &RemoteLoc,
    raw: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let src = archive_sources(sources)?;
    let srv = Server::open(ctx, r)?;
    let to = remote_dest(&srv, r, raw, sources.len())?;
    prepare_remote_dest(&srv, &to)?;
    let Some(src) = src else {
        let mut items = Vec::with_capacity(sources.len());
        for s in sources {
            let p = PathBuf::from(s);
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| OpError::Io(format!("invalid source '{s}'")))?;
            items.push((p.clone(), to.target(name)));
        }
        let (b, f) = items.iter().fold((0, 0), |(b, f), (p, _)| {
            let (pb, pf) = local_size(p);
            (b + pb, f + pf)
        });
        let mut progress = ProgressState::new(b, f);
        let out = srv.upload(&items, obs, &mut progress)?;
        if moving && !out.summary.cancelled {
            let done: Vec<PathBuf> = items
                .into_iter()
                .zip(&out.complete)
                .filter(|(_, c)| **c)
                .map(|((p, _), _)| p)
                .collect();
            delete_local(&done)?;
        }
        if !out.summary.cancelled {
            progress.finish(obs);
        }
        return Ok(out.summary);
    };
    let (b, f) = archive_totals(ctx, &src)?;
    // Extracting is the first phase (its own label comes from the archive code).
    let mut progress = ProgressState::new(b, f).with_phases(2);
    progress.set_count_files(false);
    let stage = Stage::new(ctx)?;
    let (staged, errors) = extract_sources(ctx, &src, &stage.dir, &Share::new(obs, &mut progress))?;
    progress.next_phase(None, true);
    let mut items = Vec::new();
    let mut inners = Vec::new();
    for (inner, path) in src.items.iter().zip(staged) {
        if exists(&path) {
            items.push((path, to.target(base_name(inner))));
            inners.push(inner.clone());
        }
    }
    let missing = (src.items.len() - items.len()) as u64;
    let sobs = StageObs {
        obs,
        stage: &stage.dir,
        shown: src.shown(parent_inner(&src.items[0])),
    };
    let out = srv.upload(&items, &sobs, &mut progress)?;
    let mut summary = out.summary;
    summary.skipped += missing;
    if moving {
        let done = picked(&inners, &out.complete, &out.complete);
        let withhold = summary.cancelled || errors;
        finish_move(ctx, &src, done, withhold, &mut summary, obs)?;
    }
    if !summary.cancelled {
        progress.finish(obs);
    }
    Ok(summary)
}

/// The staged path of each server item.
fn staged_paths(stage: &Path, paths: &[String]) -> Vec<PathBuf> {
    paths.iter().map(|p| stage.join(base_name(p))).collect()
}

fn server_to_local(
    ctx: &Ctx,
    src: &RemoteSources,
    dest: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let srv = Server::open(ctx, &src.loc)?;
    let (b, f) = srv.totals(&src.paths, obs)?;
    let mut progress = ProgressState::new(b, f);
    let stage = Stage::new(ctx)?;
    let out = srv.download(&src.paths, &stage.dir, obs, &mut progress)?;
    if out.summary.cancelled {
        return Ok(cancelled_download(out));
    }
    let staged = staged_paths(&stage.dir, &src.paths);
    let present: Vec<PathBuf> = staged.iter().filter(|p| exists(p)).cloned().collect();
    let target = ops::resolve_dest(&staged, dest, Path::new("/"));
    let quiet = NoProgress(obs);
    let sobs = StageObs {
        obs: &quiet,
        stage: &stage.dir,
        shown: srv.shown_parent(&src.paths[0]),
    };
    // As from an archive: what was not transferred is still in the staging folder.
    let mut summary = if present.is_empty() {
        OpSummary::default()
    } else {
        ops::move_(&present, &target, &sobs)?
    };
    summary.skipped += out.summary.skipped;
    if moving && !summary.cancelled {
        let gone = transferred(&src.paths, &staged, &present, &target);
        let arrived: Vec<bool> = src.paths.iter().map(|p| gone.contains(p)).collect();
        srv.remove(
            picked(&src.paths, &arrived, &out.complete),
            obs,
            &mut summary,
        )?;
    }
    if !summary.cancelled {
        progress.finish(obs);
    }
    Ok(summary)
}

fn server_to_archive(
    ctx: &Ctx,
    src: &RemoteSources,
    a: ArchiveLoc,
    dest: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let (to, index) = archive_dest(ctx, a, dest, src.paths.len())?;
    let srv = Server::open(ctx, &src.loc)?;
    let (b, f) = srv.totals(&src.paths, obs)?;
    let mut progress = staged_progress(b, f, "downloading");
    let stage = Stage::new(ctx)?;
    let out = srv.download(&src.paths, &stage.dir, obs, &mut progress)?;
    if out.summary.cancelled {
        return Ok(cancelled_download(out));
    }
    // Packing is the second phase (its own label comes from the archive code).
    progress.next_phase(None, true);
    let mut items = Vec::new();
    let mut paths = Vec::new();
    for (p, staged) in src.paths.iter().zip(staged_paths(&stage.dir, &src.paths)) {
        if exists(&staged) {
            items.push(PackItem {
                src: staged,
                shown: srv.shown(p),
            });
            paths.push(p.clone());
        }
    }
    let (mut summary, complete) = pack(ctx, &to, &index, &items, &Share::new(obs, &mut progress))?;
    summary.skipped += out.summary.skipped;
    if moving && !summary.cancelled {
        let downloaded: Vec<bool> = src
            .paths
            .iter()
            .zip(&out.complete)
            .filter(|(p, _)| paths.contains(p))
            .map(|(_, c)| *c)
            .collect();
        srv.remove(picked(&paths, &complete, &downloaded), obs, &mut summary)?;
    }
    if !summary.cancelled {
        progress.finish(obs);
    }
    Ok(summary)
}

fn server_to_server(
    ctx: &Ctx,
    src: &RemoteSources,
    r: &RemoteLoc,
    raw: &str,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let same = src.loc.server == r.server;
    let from = Server::open(ctx, &src.loc)?;
    let to = if same {
        from.clone()
    } else {
        Server::open(ctx, r)?
    };
    let dest = remote_dest(&to, r, raw, src.paths.len())?;
    if same {
        for p in &src.paths {
            let target = dest.target(base_name(p));
            if target == *p {
                return Err(OpError::SameLocation(from.shown(p)));
            }
            if p == "/" || target.starts_with(&format!("{p}/")) {
                return Err(OpError::IntoItself(from.shown(p)));
            }
        }
    }
    prepare_remote_dest(&to, &dest)?;
    if same && moving {
        return move_on_server(&from, &src.paths, &dest, obs);
    }
    let (b, f) = from.totals(&src.paths, obs)?;
    let mut progress = staged_progress(b, f, "downloading");
    let stage = Stage::new(ctx)?;
    let out = from.download(&src.paths, &stage.dir, obs, &mut progress)?;
    if out.summary.cancelled {
        return Ok(cancelled_download(out));
    }
    progress.next_phase(None, true);
    let mut items = Vec::new();
    let mut paths = Vec::new();
    let mut downloaded = Vec::new();
    for ((p, staged), c) in src
        .paths
        .iter()
        .zip(staged_paths(&stage.dir, &src.paths))
        .zip(&out.complete)
    {
        if exists(&staged) {
            items.push((staged, dest.target(base_name(p))));
            paths.push(p.clone());
            downloaded.push(*c);
        }
    }
    let sobs = StageObs {
        obs,
        stage: &stage.dir,
        shown: from.shown_parent(&src.paths[0]),
    };
    let up = to.upload(&items, &sobs, &mut progress)?;
    let mut summary = up.summary;
    summary.skipped += out.summary.skipped;
    if moving && !summary.cancelled {
        from.remove(picked(&paths, &up.complete, &downloaded), obs, &mut summary)?;
    }
    if !summary.cancelled {
        progress.finish(obs);
    }
    Ok(summary)
}

/// A move on one server: renames, no data is transferred. An existing folder is merged
/// (as `ops` does), an existing file is a conflict and "Overwrite" replaces it atomically.
fn move_on_server(
    srv: &Server,
    paths: &[String],
    dest: &RemoteDest,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    let (b, f) = srv.totals(paths, obs)?;
    let mut m = ServerMove {
        srv,
        ans: Answers::new(obs),
        progress: ProgressState::new(b, f),
        summary: OpSummary::default(),
    };
    for p in paths {
        match m.item(p, dest.target(base_name(p))) {
            Ok(_) => {}
            Err(Stop::Cancel) => {
                m.summary.cancelled = true;
                return Ok(m.summary);
            }
            Err(s) => return Err(srv.stop(s)),
        }
    }
    m.progress.finish(obs);
    Ok(m.summary)
}

struct ServerMove<'a, 'c> {
    srv: &'a Server<'c>,
    ans: Answers<'a>,
    progress: ProgressState,
    summary: OpSummary,
}

impl ServerMove<'_, '_> {
    /// (bytes, items) under `path`, best effort.
    fn count(&self, path: &str) -> (u64, u64) {
        walk_size(self.srv.fs.as_ref(), path, &|| false).unwrap_or((0, 1))
    }

    fn moved(&mut self, target: &str) {
        let (b, f) = self.count(target);
        self.summary.done += f;
        self.progress.advance(f, b, self.ans.obs);
    }

    fn skipped(&mut self, path: &str) {
        let (b, f) = self.count(path);
        self.summary.skipped += f.max(1);
        self.progress.advance(f, b, self.ans.obs);
    }

    /// Ok(true) when all of `p` arrived at `target` (or a name the user chose instead).
    fn item(&mut self, p: &str, mut target: String) -> Result<bool, Stop> {
        let fs = self.srv.fs.as_ref();
        let shown = self.srv.shown(p);
        self.progress.set_current(&shown, self.ans.obs);
        let mut overwrite = false;
        loop {
            self.ans.check_cancel()?;
            let r = if overwrite {
                fs.replace(p, &target)
            } else {
                fs.rename(p, &target)
            };
            let e = match r {
                Ok(()) => {
                    self.moved(&target);
                    return Ok(true);
                }
                Err(e) => e,
            };
            // Retry keeps an Overwrite decision.
            if overwrite || !matches!(e, RemoteError::Exists(_)) {
                if self.ans.on_remote_error(&shown, &e)? {
                    continue;
                }
                self.skipped(p);
                return Ok(false);
            }
            let shown_target = self.srv.shown(&target);
            let (s, t) = match fs.stat(p).and_then(|s| Ok((s, fs.stat(&target)?))) {
                Ok((Some(s), Some(t))) => (s, t),
                // Gone meanwhile: rename again.
                Ok(_) => continue,
                Err(e) => {
                    if self.ans.on_remote_error(&shown, &e)? {
                        continue;
                    }
                    self.skipped(p);
                    return Ok(false);
                }
            };
            let (s_dir, t_dir) = (s.is_dir && !s.is_symlink, t.is_dir && !t.is_symlink);
            if s_dir && t_dir {
                return self.merge(p, &target);
            }
            if s_dir != t_dir {
                let msg = ops::type_clash(Path::new(&shown_target), s_dir);
                if self.ans.on_error(&shown_target, &msg)? {
                    continue;
                }
                self.skipped(p);
                return Ok(false);
            }
            let info = ConflictInfo {
                source: shown.clone(),
                target: shown_target,
                source_size: s.size,
                source_mtime: s.mtime,
                target_size: t.size,
                target_mtime: t.mtime,
            };
            match self.ans.decide(&info)? {
                Decision::Overwrite => overwrite = true,
                Decision::Skip => {
                    self.skipped(p);
                    return Ok(false);
                }
                Decision::Rename(n) => {
                    if ops::valid_name(&n) {
                        target = join_path(parent_path(&target), &n);
                    }
                }
            }
        }
    }

    /// Moves the children of folder `p` into the existing folder `target`; `p` is removed
    /// when nothing is left in it.
    fn merge(&mut self, p: &str, target: &str) -> Result<bool, Stop> {
        let fs = self.srv.fs.as_ref();
        let shown = self.srv.shown(p);
        let Some(mut children) = self.ans.attempt(&shown, || fs.list(p))? else {
            self.skipped(p);
            return Ok(false);
        };
        children.sort_by(|a, b| a.name.cmp(&b.name));
        let mut complete = true;
        for c in children {
            complete &= self.item(&join_path(p, &c.name), join_path(target, &c.name))?;
        }
        if complete && self.ans.attempt(&shown, || fs.remove_dir(p))?.is_none() {
            complete = false;
        }
        Ok(complete)
    }
}

// ---- conflicts with archive items ----

/// The user cancelled.
struct Cancel;

/// Conflict and error answers remembered for the whole operation.
struct Resolver<'a> {
    obs: &'a dyn OpObserver,
    conflicts: ops::Conflicts,
    skip_all_errors: bool,
}

impl<'a> Resolver<'a> {
    fn new(obs: &'a dyn OpObserver) -> Self {
        Resolver {
            obs,
            conflicts: ops::Conflicts::default(),
            skip_all_errors: false,
        }
    }

    /// A folder is merged, so with "Overwrite all older" it is always entered and its files
    /// are compared one by one.
    fn conflict(&mut self, info: &ConflictInfo, is_dir: bool) -> Result<Decision, Cancel> {
        let older = is_dir || info.target_mtime < info.source_mtime;
        self.conflicts
            .decide(self.obs, older, || info.clone())
            .ok_or(Cancel)
    }

    /// Whether a file inside a merged folder replaces the one in the archive.
    fn replace_file(&self, src_mtime: i64, target_mtime: i64) -> bool {
        !self.conflicts.overwrite_older() || target_mtime < src_mtime
    }

    /// Reports an item error; the item is skipped unless the user cancels. Retry is a skip
    /// too: the plan is made from a fixed index, so trying again gives the same answer.
    fn error(&mut self, path: &str, msg: &str) -> Result<(), Cancel> {
        if self.skip_all_errors {
            return Ok(());
        }
        match self.obs.error(path, msg) {
            ErrorAnswer::Cancel => Err(Cancel),
            ErrorAnswer::SkipAll => {
                self.skip_all_errors = true;
                Ok(())
            }
            ErrorAnswer::Retry | ErrorAnswer::Skip => Ok(()),
        }
    }
}

fn renamed_target(target: &str, new_name: &str) -> Option<String> {
    ops::valid_name(new_name).then(|| join_rel(parent_inner(target), &nfc(new_name)))
}

// ---- local -> archive ----

/// A local item to put into an archive, shown to the user as `shown`.
struct PackItem {
    src: PathBuf,
    shown: String,
}

/// Turns local items into `Add` edits, resolving conflicts with the archive's index.
struct Packer<'a> {
    res: Resolver<'a>,
    index: &'a ArchiveIndex,
    dest: &'a ArchiveDest,
    edits: Vec<Edit>,
    /// Targets of this operation, so two items never land on the same name.
    planned: Vec<String>,
    done: u64,
    bytes: u64,
    skipped: u64,
}

impl Packer<'_> {
    fn add(&mut self, src: &Path, inner: String) {
        let (b, f) = local_size(src);
        self.bytes += b;
        self.done += f;
        self.planned.push(inner.clone());
        self.edits.push(Edit::Add {
            src: src.to_path_buf(),
            inner,
        });
    }

    fn skip(&mut self, src: &Path) {
        self.skipped += local_size(src).1.max(1);
    }

    /// Ok(true) if the whole item goes into the archive.
    fn item(&mut self, it: &PackItem, mut target: String) -> Result<bool, Cancel> {
        let md = match fs::symlink_metadata(&it.src) {
            Ok(m) => m,
            Err(e) => {
                self.res.error(&it.shown, &e.to_string())?;
                self.skipped += 1;
                return Ok(false);
            }
        };
        loop {
            if self
                .planned
                .iter()
                .any(|p| under(p, &target) || under(&target, p))
            {
                let msg = format!("'{}' is already written by this operation", target);
                self.res.error(&self.dest.shown(&target), &msg)?;
                self.skip(&it.src);
                return Ok(false);
            }
            let Some(e) = self.index.find(&target) else {
                self.add(&it.src, target);
                return Ok(true);
            };
            let shown = self.dest.shown(&target);
            if e.is_dir != md.is_dir() {
                self.res
                    .error(&shown, &ops::type_clash(Path::new(&shown), md.is_dir()))?;
                self.skip(&it.src);
                return Ok(false);
            }
            let info = ConflictInfo {
                source: it.shown.clone(),
                target: shown,
                source_size: if md.is_file() { md.len() } else { 0 },
                source_mtime: md.mtime(),
                target_size: e.size,
                target_mtime: e.mtime,
            };
            match self.res.conflict(&info, md.is_dir())? {
                Decision::Overwrite if md.is_dir() => return self.merge(&it.src, &target),
                Decision::Overwrite => {
                    self.add(&it.src, target);
                    return Ok(true);
                }
                Decision::Skip => {
                    self.skip(&it.src);
                    return Ok(false);
                }
                Decision::Rename(n) => {
                    if let Some(t) = renamed_target(&target, &n) {
                        target = t;
                    }
                }
            }
        }
    }

    /// Adds the content of local folder `dir` into the existing archive folder `inner`.
    fn merge(&mut self, dir: &Path, inner: &str) -> Result<bool, Cancel> {
        let names = match fs::read_dir(dir).and_then(|rd| {
            rd.map(|e| e.map(|e| e.file_name()))
                .collect::<std::io::Result<Vec<_>>>()
        }) {
            Ok(mut n) => {
                n.sort();
                n
            }
            Err(e) => {
                self.res.error(&dir.to_string_lossy(), &e.to_string())?;
                self.skip(dir);
                return Ok(false);
            }
        };
        let mut complete = true;
        for name in names {
            let child = dir.join(&name);
            let shown_src = child.to_string_lossy().into_owned();
            let (Some(s), Ok(md)) = (name.to_str(), fs::symlink_metadata(&child)) else {
                self.res.error(&shown_src, "cannot read this item")?;
                self.skip(&child);
                complete = false;
                continue;
            };
            let t = join_rel(inner, &nfc(s));
            match self.index.find(&t) {
                None => self.add(&child, t),
                Some(e) if e.is_dir != md.is_dir() => {
                    let shown = self.dest.shown(&t);
                    self.res
                        .error(&shown, &ops::type_clash(Path::new(&shown), md.is_dir()))?;
                    self.skip(&child);
                    complete = false;
                }
                Some(_) if md.is_dir() => complete &= self.merge(&child, &t)?,
                Some(e) => {
                    if self.res.replace_file(md.mtime(), e.mtime) {
                        self.add(&child, t);
                    } else {
                        self.skip(&child);
                        complete = false;
                    }
                }
            }
        }
        Ok(complete)
    }
}

/// Writes `items` into the archive with one rewrite. Returns the summary and, per item,
/// whether all of it went in.
fn pack(
    ctx: &Ctx,
    dest: &ArchiveDest,
    index: &ArchiveIndex,
    items: &[PackItem],
    obs: &dyn OpObserver,
) -> Result<(OpSummary, Vec<bool>), OpError> {
    let mut p = Packer {
        res: Resolver::new(obs),
        index,
        dest,
        edits: Vec::new(),
        planned: Vec::new(),
        done: 0,
        bytes: 0,
        skipped: 0,
    };
    let mut complete = Vec::with_capacity(items.len());
    for it in items {
        let name = it
            .src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| OpError::Io(format!("invalid source '{}'", it.shown)))?;
        let target = dest.target(&nfc(name));
        let r = if obs.is_cancelled() {
            Err(Cancel)
        } else {
            p.item(it, target)
        };
        match r {
            Ok(c) => complete.push(c),
            Err(Cancel) => {
                let summary = OpSummary {
                    done: 0,
                    skipped: p.skipped,
                    cancelled: true,
                };
                return Ok((summary, vec![false; items.len()]));
            }
        }
    }
    if !p.edits.is_empty() {
        let pobs = PhaseObs::new(obs, "packing", Some((p.bytes, p.done)));
        rewrite(&dest.archive, dest.kind, &p.edits, &ctx.cache, &pobs)
            .map_err(|e| archive_op_error(e, &dest.archive))?;
    }
    let summary = OpSummary {
        done: p.done,
        skipped: p.skipped,
        cancelled: false,
    };
    Ok((summary, complete))
}

fn local_to_archive(
    ctx: &Ctx,
    sources: &[String],
    dest: &ArchiveDest,
    index: &ArchiveIndex,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let archive = ops::location(&dest.archive);
    for s in sources {
        if archive.starts_with(ops::location(Path::new(s))) {
            return Err(OpError::IntoItself(s.clone()));
        }
    }
    let items: Vec<PackItem> = sources
        .iter()
        .map(|s| PackItem {
            src: PathBuf::from(s),
            shown: s.clone(),
        })
        .collect();
    let (summary, complete) = pack(ctx, dest, index, &items, obs)?;
    if moving && !summary.cancelled {
        let done: Vec<PathBuf> = items
            .into_iter()
            .zip(complete)
            .filter(|(_, c)| *c)
            .map(|(it, _)| it.src)
            .collect();
        delete_local(&done)?;
    }
    Ok(summary)
}

/// Permanently deletes moved local sources; their data is already in the archive.
fn delete_local(paths: &[PathBuf]) -> Result<(), OpError> {
    if paths.is_empty() {
        return Ok(());
    }
    ops::delete(paths, false, &NullObserver)?;
    match paths.iter().find(|p| exists(p)) {
        Some(p) => Err(OpError::Io(format!(
            "'{}' was copied but could not be deleted",
            p.display()
        ))),
        None => Ok(()),
    }
}

// ---- archive -> archive ----

fn archive_to_archive(
    ctx: &Ctx,
    src: &Sources,
    dest: &ArchiveDest,
    index: &ArchiveIndex,
    obs: &dyn OpObserver,
    moving: bool,
) -> Result<OpSummary, OpError> {
    let same = ops::location(&src.archive) == ops::location(&dest.archive);
    if same {
        for inner in &src.items {
            let target = dest.target(base_name(inner));
            if target == *inner {
                return Err(OpError::SameLocation(src.shown(inner)));
            }
            if under(&target, inner) {
                return Err(OpError::IntoItself(src.shown(inner)));
            }
        }
        if moving {
            return move_in_archive(ctx, src, dest, index, obs);
        }
    }
    let stage = Stage::new(ctx)?;
    let (staged, errors) = extract_sources(ctx, src, &stage.dir, obs)?;
    let mut items = Vec::new();
    let mut inners = Vec::new();
    for (inner, path) in src.items.iter().zip(staged) {
        if exists(&path) {
            items.push(PackItem {
                src: path,
                shown: src.shown(inner),
            });
            inners.push(inner.clone());
        }
    }
    let missing = (src.items.len() - items.len()) as u64;
    let (mut summary, complete) = pack(ctx, dest, index, &items, obs)?;
    summary.skipped += missing;
    if moving {
        let done: Vec<String> = inners
            .into_iter()
            .zip(complete)
            .filter(|(_, c)| *c)
            .map(|(i, _)| i)
            .collect();
        let withhold = summary.cancelled || errors;
        finish_move(ctx, src, done, withhold, &mut summary, obs)?;
    }
    Ok(summary)
}

/// Turns a move inside one archive into `Rename` edits (and `Delete` for replaced files).
struct Mover<'a> {
    res: Resolver<'a>,
    index: &'a ArchiveIndex,
    archive: &'a Path,
    edits: Vec<Edit>,
    planned: Vec<String>,
    done: u64,
    bytes: u64,
    skipped: u64,
}

impl Mover<'_> {
    fn shown(&self, inner: &str) -> String {
        format!("{}/{inner}", self.archive.to_string_lossy())
    }

    fn size(&self, inner: &str) -> (u64, u64) {
        size_in(self.index, inner).unwrap_or((0, 1))
    }

    fn rename(&mut self, from: &str, to: String) {
        let (b, f) = self.size(from);
        self.bytes += b;
        self.done += f;
        self.planned.push(to.clone());
        self.edits.push(Edit::Rename {
            from: from.to_string(),
            to,
        });
    }

    fn skip(&mut self, inner: &str) {
        self.skipped += self.size(inner).1.max(1);
    }

    fn info(&self, s: &IndexEntry, t: &IndexEntry) -> ConflictInfo {
        ConflictInfo {
            source: self.shown(&s.path),
            target: self.shown(&t.path),
            source_size: s.size,
            source_mtime: s.mtime,
            target_size: t.size,
            target_mtime: t.mtime,
        }
    }

    fn item(&mut self, from: &str, mut target: String) -> Result<(), Cancel> {
        let Some(s) = self.index.find(from) else {
            self.res.error(&self.shown(from), "not found in archive")?;
            self.skipped += 1;
            return Ok(());
        };
        loop {
            if self
                .planned
                .iter()
                .any(|p| under(p, &target) || under(&target, p))
            {
                let msg = format!("'{target}' is already written by this operation");
                self.res.error(&self.shown(&target), &msg)?;
                self.skip(from);
                return Ok(());
            }
            let Some(t) = self.index.find(&target) else {
                self.rename(from, target);
                return Ok(());
            };
            if t.is_dir != s.is_dir {
                let shown = self.shown(&target);
                self.res
                    .error(&shown, &ops::type_clash(Path::new(&shown), s.is_dir))?;
                self.skip(from);
                return Ok(());
            }
            match self.res.conflict(&self.info(s, t), s.is_dir)? {
                Decision::Overwrite if s.is_dir => {
                    self.planned.push(target.clone());
                    if self.merge(from, &target)? {
                        self.edits.push(Edit::Delete {
                            inner: from.to_string(),
                        });
                    }
                    return Ok(());
                }
                Decision::Overwrite => {
                    self.edits.push(Edit::Delete {
                        inner: target.clone(),
                    });
                    self.rename(from, target);
                    return Ok(());
                }
                Decision::Skip => {
                    self.skip(from);
                    return Ok(());
                }
                Decision::Rename(n) => {
                    if let Some(t) = renamed_target(&target, &n) {
                        target = t;
                    }
                }
            }
        }
    }

    /// Moves the children of folder `from` into the existing folder `to`. Ok(true) when
    /// nothing is left behind, so `from` itself can go.
    fn merge(&mut self, from: &str, to: &str) -> Result<bool, Cancel> {
        let prefix = format!("{from}/");
        let children: Vec<&IndexEntry> = self
            .index
            .entries
            .iter()
            .filter(|e| e.path.starts_with(&prefix) && !e.path[prefix.len()..].contains('/'))
            .collect();
        let mut complete = true;
        for c in children {
            let t = join_rel(to, base_name(&c.path));
            match self.index.find(&t) {
                None => self.rename(&c.path, t),
                Some(te) if te.is_dir != c.is_dir => {
                    let shown = self.shown(&t);
                    self.res
                        .error(&shown, &ops::type_clash(Path::new(&shown), c.is_dir))?;
                    self.skip(&c.path);
                    complete = false;
                }
                Some(_) if c.is_dir => {
                    if self.merge(&c.path, &t)? {
                        self.edits.push(Edit::Delete {
                            inner: c.path.clone(),
                        });
                    } else {
                        complete = false;
                    }
                }
                Some(te) => {
                    if self.res.replace_file(c.mtime, te.mtime) {
                        self.edits.push(Edit::Delete { inner: t.clone() });
                        self.rename(&c.path, t);
                    } else {
                        self.skip(&c.path);
                        complete = false;
                    }
                }
            }
        }
        Ok(complete)
    }
}

/// A move inside one archive: renames in one rewrite, nothing is extracted.
fn move_in_archive(
    ctx: &Ctx,
    src: &Sources,
    dest: &ArchiveDest,
    index: &ArchiveIndex,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    let mut m = Mover {
        res: Resolver::new(obs),
        index,
        archive: &src.archive,
        edits: Vec::new(),
        planned: Vec::new(),
        done: 0,
        bytes: 0,
        skipped: 0,
    };
    for inner in &src.items {
        let r = if obs.is_cancelled() {
            Err(Cancel)
        } else {
            m.item(inner, dest.target(base_name(inner)))
        };
        if r.is_err() {
            return Ok(OpSummary {
                done: 0,
                skipped: m.skipped,
                cancelled: true,
            });
        }
    }
    if !m.edits.is_empty() {
        let pobs = PhaseObs::new(obs, "packing", Some((m.bytes, m.done)));
        rewrite(&src.archive, src.kind, &m.edits, &ctx.cache, &pobs)
            .map_err(|e| archive_op_error(e, &src.archive))?;
    }
    Ok(OpSummary {
        done: m.done,
        skipped: m.skipped,
        cancelled: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::read::extract;
    use crate::archive::testutil::*;
    use crate::archive::ArchiveKind;
    use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, Progress};
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    #[test]
    fn transferred_needs_the_target() {
        let t = tempfile::tempdir().unwrap();
        let stage = t.path().join("stage");
        let out = t.path().join("out");
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&out).unwrap();
        let items = vec!["d/a.txt".to_string(), "d/b.txt".into(), "d/c.txt".into()];
        let staged: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|n| stage.join(n))
            .collect();
        // a: moved; b: still staged (skipped); c: gone from staging but not at the target
        // (the staging folder was removed under the job).
        fs::write(out.join("a.txt"), b"a").unwrap();
        fs::write(&staged[1], b"b").unwrap();
        let into = ops::Dest::IntoDir(out.clone());
        assert_eq!(transferred(&items, &staged, &staged, &into), ["d/a.txt"]);
        let as_file = ops::Dest::AsFile(out.join("a.txt"));
        assert_eq!(
            transferred(&items[..1], &staged[..1], &staged[..1], &as_file),
            ["d/a.txt"]
        );
        let missing = ops::Dest::AsFile(out.join("z.txt"));
        assert!(transferred(&items[..1], &staged[..1], &staged[..1], &missing).is_empty());
    }

    /// Answers conflicts and errors from queues (errors default to Skip) and records every
    /// call. With `cancel_on`, it cancels once a progress `current` contains that text.
    #[derive(Default)]
    struct RecordingObserver {
        answers: Mutex<VecDeque<ConflictAnswer>>,
        conflicts: Mutex<Vec<ConflictInfo>>,
        error_answers: Mutex<VecDeque<ErrorAnswer>>,
        errors: Mutex<Vec<(String, String)>>,
        progress: Mutex<Vec<Progress>>,
        cancel_on: Option<&'static str>,
        cancelled: AtomicBool,
    }

    impl RecordingObserver {
        fn answering(answers: Vec<ConflictAnswer>) -> Self {
            RecordingObserver {
                answers: Mutex::new(answers.into()),
                ..Default::default()
            }
        }
        fn errors_answering(mut self, answers: Vec<ErrorAnswer>) -> Self {
            self.error_answers = Mutex::new(answers.into());
            self
        }
        fn cancelling_on(mut self, text: &'static str) -> Self {
            self.cancel_on = Some(text);
            self
        }
        fn conflicts(&self) -> Vec<ConflictInfo> {
            self.conflicts.lock().unwrap().clone()
        }
        fn errors(&self) -> Vec<(String, String)> {
            self.errors.lock().unwrap().clone()
        }
        fn progress(&self) -> Vec<Progress> {
            self.progress.lock().unwrap().clone()
        }
    }

    impl OpObserver for RecordingObserver {
        fn progress(&self, p: &Progress) {
            if self.cancel_on.is_some_and(|t| p.current.contains(t)) {
                self.cancelled.store(true, Ordering::SeqCst);
            }
            self.progress.lock().unwrap().push(p.clone());
        }
        fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
            self.conflicts.lock().unwrap().push(c.clone());
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected conflict() call")
        }
        fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
            self.errors
                .lock()
                .unwrap()
                .push((path.to_string(), msg.to_string()));
            self.error_answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ErrorAnswer::Skip)
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }

    fn ctx(t: &Path) -> Ctx {
        Ctx::new(t.join("tmp-root"))
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn zip_with_d(t: &Path) {
        make_zip(
            &t.join("a.zip"),
            &[
                ("a.txt", Some(b"aaa")),
                ("d/b.txt", Some(b"bbbb")),
                ("d/e/c.txt", Some(b"cc")),
            ],
        );
    }

    /// All paths in the archive's index (including folders).
    fn paths(c: &Ctx, archive: &Path, kind: ArchiveKind) -> Vec<String> {
        c.cache
            .get(archive, kind)
            .unwrap()
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect()
    }

    fn read_entry(archive: &Path, kind: ArchiveKind, inner: &str) -> Vec<u8> {
        let out = tempfile::tempdir().unwrap();
        let obs = RecordingObserver::default();
        extract(archive, kind, &[inner.to_string()], out.path(), None, &obs).unwrap();
        let name = inner.rsplit('/').next().unwrap();
        fs::read(out.path().join(name)).unwrap()
    }

    fn no_stage_dirs(c: &Ctx) -> bool {
        match fs::read_dir(&c.temp_root) {
            Err(_) => true,
            Ok(rd) => rd
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with("stage-")),
        }
    }

    #[test]
    fn copy_archive_to_local() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        fs::create_dir(t.path().join("out")).unwrap();
        let obs = RecordingObserver::default();
        let r = copy(
            &c,
            &[format!("{tp}/a.zip/d")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 2);
        assert_eq!(fs::read(t.path().join("out/d/b.txt")).unwrap(), b"bbbb");
        assert_eq!(fs::read(t.path().join("out/d/e/c.txt")).unwrap(), b"cc");
        assert!(c.n.load(Ordering::Relaxed) > 0, "a staging folder was used");
        assert!(no_stage_dirs(&c));
        // The archive is unchanged.
        assert!(paths(&c, &t.path().join("a.zip"), ArchiveKind::Zip).contains(&"d/b.txt".into()));
    }

    #[test]
    fn copy_archive_to_local_relative_dest_and_as_file() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        let obs = RecordingObserver::default();
        copy(&c, &[format!("{tp}/a.zip/a.txt")], "renamed.txt", &tp, &obs).unwrap();
        assert_eq!(fs::read(t.path().join("renamed.txt")).unwrap(), b"aaa");
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn copy_archive_to_local_conflict_uses_ops_rules() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        fs::create_dir(t.path().join("out")).unwrap();
        fs::write(t.path().join("out/a.txt"), b"old").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let r = copy(
            &c,
            &[format!("{tp}/a.zip/a.txt")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.skipped, 1);
        assert_eq!(r.done, 0);
        assert_eq!(fs::read(t.path().join("out/a.txt")).unwrap(), b"old");
        // The conflict shows where the item comes from, not the staging folder.
        let cf = obs.conflicts();
        assert_eq!(cf.len(), 1);
        assert_eq!(cf[0].source, format!("{tp}/a.zip/a.txt"));
        assert_eq!(cf[0].target, format!("{tp}/out/a.txt"));
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn copy_local_into_archive_with_conflict() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("x.txt", Some(b"old")), ("k.txt", Some(b"k"))]);
        fs::create_dir(t.path().join("src")).unwrap();
        fs::write(t.path().join("src/x.txt"), b"new content").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Rename {
            new_name: "x (2).txt".into(),
        }]);
        let r = copy(
            &c,
            &[format!("{tp}/src/x.txt")],
            &format!("{tp}/a.zip"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 1);
        let cf = obs.conflicts();
        assert_eq!(cf.len(), 1);
        assert_eq!(cf[0].target_size, 3);
        assert_eq!(cf[0].source_size, 11);
        assert_eq!(cf[0].target, format!("{tp}/a.zip/x.txt"));
        let p = paths(&c, &a, ArchiveKind::Zip);
        assert!(
            p.contains(&"x.txt".into()) && p.contains(&"x (2).txt".into()),
            "{p:?}"
        );
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "x.txt"), b"old");
        assert_eq!(
            read_entry(&a, ArchiveKind::Zip, "x (2).txt"),
            b"new content"
        );
        assert!(t.path().join("src/x.txt").exists());
    }

    #[test]
    fn copy_local_dir_into_archive_merges_existing_dir() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let a = t.path().join("a.zip");
        fs::create_dir_all(t.path().join("src/d")).unwrap();
        fs::write(t.path().join("src/d/b.txt"), b"BBB").unwrap();
        fs::write(t.path().join("src/d/n.txt"), b"N").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = copy(
            &c,
            &[format!("{tp}/src/d")],
            &format!("{tp}/a.zip"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 2);
        assert_eq!(obs.conflicts().len(), 1, "one question for the folder");
        let p = paths(&c, &a, ArchiveKind::Zip);
        assert!(
            p.contains(&"d/e/c.txt".into()),
            "existing content kept: {p:?}"
        );
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "d/b.txt"), b"BBB");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "d/n.txt"), b"N");
    }

    #[test]
    fn move_local_into_archive_deletes_source_after_success() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let a = t.path().join("a.zip");
        fs::create_dir_all(t.path().join("src/m")).unwrap();
        fs::write(t.path().join("src/m/f.txt"), b"F").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = move_(
            &c,
            &[format!("{tp}/src/m")],
            &format!("{tp}/a.zip/d"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 1);
        assert!(!t.path().join("src/m").exists());
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "d/m/f.txt"), b"F");
    }

    #[test]
    fn move_local_into_archive_keeps_source_on_failure() {
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipped: running as root ignores folder permissions");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let dir = t.path().join("arch");
        fs::create_dir(&dir).unwrap();
        let a = dir.join("a.zip");
        make_zip(&a, &[("k.txt", Some(b"k"))]);
        let before = fs::read(&a).unwrap();
        fs::write(t.path().join("f.txt"), b"F").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = move_(&c, &[format!("{tp}/f.txt")], &s(&a), &tp, &obs);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(r, Err(OpError::Io(_))), "{r:?}");
        assert_eq!(fs::read(t.path().join("f.txt")).unwrap(), b"F");
        assert_eq!(fs::read(&a).unwrap(), before);
    }

    #[test]
    fn move_archive_to_local_skipped_not_deleted() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        fs::create_dir(t.path().join("out")).unwrap();
        fs::write(t.path().join("out/a.txt"), b"old").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/a.txt"), format!("{tp}/a.zip/k.txt")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!((r.done, r.skipped), (1, 1));
        assert_eq!(fs::read(t.path().join("out/a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(t.path().join("out/k.txt")).unwrap(), b"kk");
        assert_eq!(paths(&c, &a, ArchiveKind::Zip), vec!["a.txt"]);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn move_archive_to_local_skip_all_not_deleted() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(
            &a,
            &[
                ("a.txt", Some(b"a")),
                ("b.txt", Some(b"b")),
                ("k.txt", Some(b"k")),
            ],
        );
        fs::create_dir(t.path().join("out")).unwrap();
        fs::write(t.path().join("out/a.txt"), b"old").unwrap();
        fs::write(t.path().join("out/b.txt"), b"old").unwrap();
        let c = ctx(t.path());
        // Only the first conflict asks; the second is skipped silently.
        let obs = RecordingObserver::answering(vec![ConflictAnswer::SkipAll]);
        let srcs: Vec<String> = ["a.txt", "b.txt", "k.txt"]
            .iter()
            .map(|n| format!("{tp}/a.zip/{n}"))
            .collect();
        let r = move_(&c, &srcs, &format!("{tp}/out"), &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 2));
        assert_eq!(paths(&c, &a, ArchiveKind::Zip), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn move_within_same_archive_is_rename() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(
            &a,
            &[
                ("d/b.txt", Some(b"bbbb")),
                ("d/z.txt", Some(b"z")),
                ("e/", None),
            ],
        );
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/d/b.txt")],
            &format!("{tp}/a.zip/e"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 1);
        let p = paths(&c, &a, ArchiveKind::Zip);
        assert!(
            p.contains(&"e/b.txt".into()) && !p.contains(&"d/b.txt".into()),
            "{p:?}"
        );
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "e/b.txt"), b"bbbb");
        assert_eq!(c.n.load(Ordering::Relaxed), 0, "no staging for a rename");
    }

    #[test]
    fn move_within_same_archive_merges_folders() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(
            &a,
            &[
                ("d/x/1.txt", Some(b"new1")),
                ("d/x/2.txt", Some(b"two")),
                ("e/x/1.txt", Some(b"old1")),
                ("e/x/3.txt", Some(b"three")),
            ],
        );
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/d/x")],
            &format!("{tp}/a.zip/e"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 2);
        let p = paths(&c, &a, ArchiveKind::Zip);
        assert!(!p.iter().any(|p| p.starts_with("d/x")), "{p:?}");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "e/x/1.txt"), b"new1");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "e/x/2.txt"), b"two");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "e/x/3.txt"), b"three");
    }

    #[test]
    fn copy_between_archives() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let b = t.path().join("b.tgz");
        make_tgz(&b, &[("z.txt", Some(b"z"))]);
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = copy(&c, &[format!("{tp}/a.zip/d")], &s(&b), &tp, &obs).unwrap();
        assert_eq!(r.done, 2);
        let p = paths(&c, &b, ArchiveKind::TarGz);
        assert!(p.contains(&"z.txt".into()), "{p:?}");
        assert_eq!(read_entry(&b, ArchiveKind::TarGz, "d/b.txt"), b"bbbb");
        assert_eq!(read_entry(&b, ArchiveKind::TarGz, "d/e/c.txt"), b"cc");
        assert!(paths(&c, &t.path().join("a.zip"), ArchiveKind::Zip).contains(&"d/b.txt".into()));
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn move_between_archives_deletes_source() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let a = t.path().join("a.zip");
        let b = t.path().join("b.tar");
        make_tar(&b, &[("z.txt", Some(b"z"))]);
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/d")],
            &format!("{tp}/b.tar"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 2);
        assert_eq!(read_entry(&b, ArchiveKind::Tar, "d/e/c.txt"), b"cc");
        assert_eq!(paths(&c, &a, ArchiveKind::Zip), vec!["a.txt"]);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn copy_into_itself_in_archive_refused() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let src = format!("{tp}/a.zip/d");
        let r = copy(
            &c,
            std::slice::from_ref(&src),
            &format!("{tp}/a.zip/d/x"),
            &tp,
            &obs,
        );
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(&c, std::slice::from_ref(&src), &src, &tp, &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = move_(&c, &[format!("{tp}/a.zip/d/b.txt")], &src, &tp, &obs);
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        // A local folder into an archive inside it.
        fs::create_dir(t.path().join("w")).unwrap();
        make_zip(&t.path().join("w/in.zip"), &[("k", Some(b"k"))]);
        let r = copy(
            &c,
            &[format!("{tp}/w")],
            &format!("{tp}/w/in.zip"),
            &tp,
            &obs,
        );
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn password_required_maps_to_io_prefix() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("enc.zip");
        make_zip_aes(&a, &[("s.txt", Some(b"secret"))], "pw");
        fs::create_dir(t.path().join("out")).unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let src = vec![format!("{tp}/enc.zip/s.txt")];
        match copy(&c, &src, &format!("{tp}/out"), &tp, &obs) {
            Err(OpError::Io(m)) => assert!(m.starts_with("password required: "), "{m}"),
            r => panic!("{r:?}"),
        }
        assert!(no_stage_dirs(&c));
        c.set_password(&a, "pw".into());
        copy(&c, &src, &format!("{tp}/out"), &tp, &obs).unwrap();
        assert_eq!(fs::read(t.path().join("out/s.txt")).unwrap(), b"secret");
    }

    #[test]
    fn progress_names_phases() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let b = t.path().join("b.zip");
        make_zip(&b, &[("z.txt", Some(b"z"))]);
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        copy(&c, &[format!("{tp}/a.zip/d")], &s(&b), &tp, &obs).unwrap();
        let p = obs.progress();
        let ext: Vec<&Progress> = p
            .iter()
            .filter(|p| p.current.ends_with(" (extracting)"))
            .collect();
        let pack: Vec<&Progress> = p
            .iter()
            .filter(|p| p.current.ends_with(" (packing)"))
            .collect();
        assert!(!ext.is_empty() && !pack.is_empty(), "{p:?}");
        assert!(
            ext.iter().any(|p| p.current == "b.txt (extracting)"),
            "{ext:?}"
        );
        for phase in [&ext, &pack] {
            assert!(phase.iter().all(|p| p.total_bytes == 6), "{phase:?}");
            assert!(phase.windows(2).all(|w| w[0].done_bytes <= w[1].done_bytes));
        }
        assert!(obs.errors().is_empty());
    }

    #[test]
    fn local_to_local_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        fs::write(t.path().join("f"), b"1").unwrap();
        fs::create_dir(t.path().join("o")).unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let r = move_(&c, &[format!("{tp}/f")], "o", &tp, &obs).unwrap();
        assert_eq!(r.done, 1);
        assert!(t.path().join("o/f").exists() && !t.path().join("f").exists());
        let r = move_(&c, &[format!("{tp}/o/f")], &format!("{tp}/o"), &tp, &obs);
        assert!(matches!(r, Err(OpError::SameLocation(_))));
        // The archive file itself is a local item.
        zip_with_d(t.path());
        copy(&c, &[format!("{tp}/a.zip")], &format!("{tp}/o"), &tp, &obs).unwrap();
        assert_eq!(
            fs::read(t.path().join("o/a.zip")).unwrap(),
            fs::read(t.path().join("a.zip")).unwrap()
        );
    }

    fn is_root() -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    fn chmod(p: &Path, mode: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn move_local_into_archive_skipped_item_survives() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("x.txt", Some(b"old"))]);
        fs::create_dir(t.path().join("src")).unwrap();
        fs::write(t.path().join("src/x.txt"), b"new").unwrap();
        fs::write(t.path().join("src/y.txt"), b"y").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let srcs = vec![format!("{tp}/src/x.txt"), format!("{tp}/src/y.txt")];
        let r = move_(&c, &srcs, &s(&a), &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 1));
        assert!(t.path().join("src/x.txt").exists());
        assert!(!t.path().join("src/y.txt").exists());
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "x.txt"), b"old");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "y.txt"), b"y");
    }

    #[test]
    fn move_local_into_archive_incomplete_merge_keeps_source() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        zip_with_d(t.path());
        let a = t.path().join("a.zip");
        fs::create_dir_all(t.path().join("src/d")).unwrap();
        fs::write(t.path().join("src/d/b.txt"), b"B").unwrap();
        // A file where the archive has the folder d/e: a type clash.
        fs::write(t.path().join("src/d/e"), b"E").unwrap();
        fs::write(t.path().join("src/m.txt"), b"M").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let srcs = vec![format!("{tp}/src/d"), format!("{tp}/src/m.txt")];
        let r = move_(&c, &srcs, &s(&a), &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (2, 1));
        let errs = obs.errors();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].1.contains("Cannot overwrite directory"), "{errs:?}");
        assert_eq!(fs::read(t.path().join("src/d/b.txt")).unwrap(), b"B");
        assert!(t.path().join("src/d/e").exists());
        assert!(!t.path().join("src/m.txt").exists());
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "d/b.txt"), b"B");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "d/e/c.txt"), b"cc");
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "m.txt"), b"M");
    }

    #[test]
    fn move_between_archives_skip_and_incomplete_merge() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(
            &a,
            &[
                ("a.txt", Some(b"aaa")),
                ("k.txt", Some(b"kk")),
                ("d/b.txt", Some(b"bbbb")),
                ("d/e/c.txt", Some(b"cc")),
            ],
        );
        let b = t.path().join("b.tar");
        make_tar(&b, &[("a.txt", Some(b"old")), ("d/e", Some(b"file"))]);
        let c = ctx(t.path());
        let obs =
            RecordingObserver::answering(vec![ConflictAnswer::Skip, ConflictAnswer::Overwrite]);
        let srcs: Vec<String> = ["a.txt", "d", "k.txt"]
            .iter()
            .map(|n| format!("{tp}/a.zip/{n}"))
            .collect();
        let r = move_(&c, &srcs, &s(&b), &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (2, 2));
        assert_eq!(obs.errors().len(), 1);
        assert_eq!(
            paths(&c, &a, ArchiveKind::Zip),
            vec!["a.txt", "d", "d/b.txt", "d/e", "d/e/c.txt"]
        );
        assert_eq!(read_entry(&b, ArchiveKind::Tar, "a.txt"), b"old");
        assert_eq!(read_entry(&b, ArchiveKind::Tar, "d/b.txt"), b"bbbb");
        assert_eq!(read_entry(&b, ArchiveKind::Tar, "d/e"), b"file");
        assert_eq!(read_entry(&b, ArchiveKind::Tar, "k.txt"), b"kk");
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn move_out_of_archive_with_rename_deletes_the_source() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        fs::create_dir(t.path().join("out")).unwrap();
        fs::write(t.path().join("out/a.txt"), b"old").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Rename {
            new_name: "a2.txt".into(),
        }]);
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/a.txt")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(fs::read(t.path().join("out/a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(t.path().join("out/a2.txt")).unwrap(), b"aaa");
        assert_eq!(paths(&c, &a, ArchiveKind::Zip), vec!["k.txt"]);
    }

    #[test]
    fn move_between_archives_rename() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        let b = t.path().join("b.zip");
        make_zip(&b, &[("a.txt", Some(b"old"))]);
        let c = ctx(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Rename {
            new_name: "a2.txt".into(),
        }]);
        let r = move_(&c, &[format!("{tp}/a.zip/a.txt")], &s(&b), &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(paths(&c, &a, ArchiveKind::Zip), vec!["k.txt"]);
        assert_eq!(read_entry(&b, ArchiveKind::Zip, "a.txt"), b"old");
        assert_eq!(read_entry(&b, ArchiveKind::Zip, "a2.txt"), b"aaa");
    }

    /// d/b.txt plus an unsafe name inside d that extraction reports and skips.
    fn zip_with_unsafe(a: &Path) {
        make_zip(
            a,
            &[
                ("d/b.txt", Some(b"bbbb")),
                ("d/../../evil", Some(b"x")),
                ("k", Some(b"k")),
            ],
        );
    }

    #[test]
    fn move_with_extraction_error_deletes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        zip_with_unsafe(&a);
        let before = fs::read(&a).unwrap();
        fs::create_dir(t.path().join("out")).unwrap();
        let c = ctx(t.path());
        // To a local folder.
        let obs = RecordingObserver::default();
        let r = move_(
            &c,
            &[format!("{tp}/a.zip/d")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert!(!obs.errors().is_empty());
        assert_eq!((r.done, r.skipped, r.cancelled), (0, 1, false));
        assert_eq!(fs::read(t.path().join("out/d/b.txt")).unwrap(), b"bbbb");
        assert_eq!(fs::read(&a).unwrap(), before);
        // To another archive.
        let b = t.path().join("b.zip");
        make_zip(&b, &[("z", Some(b"z"))]);
        let obs = RecordingObserver::default();
        let r = move_(&c, &[format!("{tp}/a.zip/d")], &s(&b), &tp, &obs).unwrap();
        assert!(!obs.errors().is_empty());
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(read_entry(&b, ArchiveKind::Zip, "d/b.txt"), b"bbbb");
        assert_eq!(fs::read(&a).unwrap(), before);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn cancel_at_conflict_writes_and_deletes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("x.txt", Some(b"old")), ("a.txt", Some(b"aaa"))]);
        let b = t.path().join("b.zip");
        make_zip(&b, &[("a.txt", Some(b"bold"))]);
        let (abefore, bbefore) = (fs::read(&a).unwrap(), fs::read(&b).unwrap());
        fs::write(t.path().join("x.txt"), b"new").unwrap();
        fs::write(t.path().join("y.txt"), b"y").unwrap();
        let c = ctx(t.path());
        // Local into an archive.
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Cancel]);
        let srcs = vec![format!("{tp}/y.txt"), format!("{tp}/x.txt")];
        let r = move_(&c, &srcs, &s(&a), &tp, &obs).unwrap();
        assert!(r.cancelled);
        assert_eq!(fs::read(&a).unwrap(), abefore);
        assert!(t.path().join("x.txt").exists() && t.path().join("y.txt").exists());
        // Archive into another archive.
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Cancel]);
        let r = move_(&c, &[format!("{tp}/a.zip/a.txt")], &s(&b), &tp, &obs).unwrap();
        assert!(r.cancelled);
        assert_eq!(fs::read(&a).unwrap(), abefore);
        assert_eq!(fs::read(&b).unwrap(), bbefore);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn cancel_while_packing_writes_and_deletes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        let b = t.path().join("b.zip");
        make_zip(&b, &[("z", Some(b"z"))]);
        let (abefore, bbefore) = (fs::read(&a).unwrap(), fs::read(&b).unwrap());
        fs::write(t.path().join("f1"), b"1").unwrap();
        fs::write(t.path().join("f2"), b"2").unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default().cancelling_on("(packing)");
        let srcs = vec![format!("{tp}/f1"), format!("{tp}/f2")];
        let r = move_(&c, &srcs, &s(&b), &tp, &obs);
        assert_eq!(r, Err(OpError::Cancelled));
        assert_eq!(fs::read(&b).unwrap(), bbefore);
        assert!(t.path().join("f1").exists() && t.path().join("f2").exists());
        let obs = RecordingObserver::default().cancelling_on("(packing)");
        let srcs = vec![format!("{tp}/a.zip/a.txt"), format!("{tp}/a.zip/k.txt")];
        let r = move_(&c, &srcs, &s(&b), &tp, &obs);
        assert_eq!(r, Err(OpError::Cancelled));
        assert_eq!(fs::read(&a).unwrap(), abefore);
        assert_eq!(fs::read(&b).unwrap(), bbefore);
        // Cancel while extracting.
        let obs = RecordingObserver::default().cancelling_on("(extracting)");
        let r = move_(&c, &srcs, &s(&b), &tp, &obs);
        assert_eq!(r, Err(OpError::Cancelled));
        assert_eq!(fs::read(&a).unwrap(), abefore);
        assert_eq!(fs::read(&b).unwrap(), bbefore);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn cancel_while_moving_out_keeps_archive_and_counts_skipped() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        let before = fs::read(&a).unwrap();
        fs::create_dir(t.path().join("out")).unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default().cancelling_on("/a.zip/a.txt");
        let srcs = vec![format!("{tp}/a.zip/a.txt"), format!("{tp}/a.zip/k.txt")];
        let r = move_(&c, &srcs, &format!("{tp}/out"), &tp, &obs).unwrap();
        assert!(r.cancelled);
        // a.txt reached the target but stays in the archive too.
        assert_eq!(fs::read(t.path().join("out/a.txt")).unwrap(), b"aaa");
        assert!(!t.path().join("out/k.txt").exists());
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(fs::read(&a).unwrap(), before);
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn failed_source_delete_returns_error_and_keeps_both() {
        if is_root() {
            eprintln!("skipped: running as root ignores folder permissions");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let dir = t.path().join("ro");
        fs::create_dir(&dir).unwrap();
        let a = dir.join("a.zip");
        make_zip(&a, &[("a.txt", Some(b"aaa")), ("k.txt", Some(b"kk"))]);
        fs::write(dir.join("f.txt"), b"F").unwrap();
        let before = fs::read(&a).unwrap();
        let b = t.path().join("b.zip");
        make_zip(&b, &[("z", Some(b"z"))]);
        fs::create_dir(t.path().join("out")).unwrap();
        let ds = s(&dir);
        chmod(&dir, 0o500);
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        // Archive -> local.
        let r1 = move_(
            &c,
            &[format!("{ds}/a.zip/k.txt")],
            &format!("{tp}/out"),
            &tp,
            &obs,
        );
        // Archive -> archive.
        let r2 = move_(&c, &[format!("{ds}/a.zip/a.txt")], &s(&b), &tp, &obs);
        // Local -> archive.
        let r3 = move_(&c, &[format!("{ds}/f.txt")], &s(&b), &tp, &obs);
        chmod(&dir, 0o755);
        for r in [&r1, &r2, &r3] {
            assert!(matches!(r, Err(OpError::Io(_))), "{r:?}");
        }
        assert_eq!(fs::read(t.path().join("out/k.txt")).unwrap(), b"kk");
        assert_eq!(read_entry(&b, ArchiveKind::Zip, "a.txt"), b"aaa");
        assert_eq!(read_entry(&b, ArchiveKind::Zip, "f.txt"), b"F");
        assert_eq!(fs::read(&a).unwrap(), before);
        assert_eq!(fs::read(dir.join("f.txt")).unwrap(), b"F");
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn moving_out_onto_the_archive_itself_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("sub/a.zip", Some(b"inner")), ("sub/x", Some(b"x"))]);
        let before = fs::read(&a).unwrap();
        let c = ctx(t.path());
        let obs = RecordingObserver::default();
        let src = vec![format!("{tp}/a.zip/sub/a.zip")];
        let r = move_(&c, &src, &tp, &tp, &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(&c, &src, &tp, &tp, &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        assert!(obs.conflicts().is_empty());
        assert_eq!(fs::read(&a).unwrap(), before);
        assert_eq!(c.n.load(Ordering::Relaxed), 0, "refused before staging");
    }

    #[test]
    fn error_answers_cancel_and_skip_all_in_archive_plan() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let a = t.path().join("a.zip");
        make_zip(&a, &[("p/", None), ("q/", None), ("n.txt", Some(b"n"))]);
        let before = fs::read(&a).unwrap();
        // Local files where the archive has folders: two type clashes.
        fs::create_dir(t.path().join("src")).unwrap();
        for n in ["p", "q", "n2.txt"] {
            fs::write(t.path().join("src").join(n), n).unwrap();
        }
        let srcs: Vec<String> = ["p", "q", "n2.txt"]
            .iter()
            .map(|n| format!("{tp}/src/{n}"))
            .collect();
        let c = ctx(t.path());
        let obs = RecordingObserver::default().errors_answering(vec![ErrorAnswer::Cancel]);
        let r = move_(&c, &srcs, &s(&a), &tp, &obs).unwrap();
        assert!(r.cancelled);
        assert_eq!(fs::read(&a).unwrap(), before);
        assert!(srcs.iter().all(|p| Path::new(p).exists()));
        let obs = RecordingObserver::default().errors_answering(vec![ErrorAnswer::SkipAll]);
        let r = move_(&c, &srcs, &s(&a), &tp, &obs).unwrap();
        assert_eq!(obs.errors().len(), 1, "SkipAll stops asking");
        assert_eq!((r.done, r.skipped), (1, 2));
        assert!(Path::new(&srcs[0]).exists() && Path::new(&srcs[1]).exists());
        assert!(!Path::new(&srcs[2]).exists());
        assert_eq!(read_entry(&a, ArchiveKind::Zip, "n2.txt"), b"n2.txt");
    }

    // ---- servers ----

    use crate::remote::testutil::MemFs;
    use crate::remote::RemoteFs;
    use std::sync::Arc;

    fn remote_ctx(t: &Path) -> (Ctx, MemFs) {
        let c = ctx(t);
        let m = MemFs::new();
        c.sessions.insert("m", Arc::new(m.clone()));
        (c, m)
    }

    fn mode(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    fn no_part(m: &MemFs) -> bool {
        m.paths().iter().all(|p| !p.ends_with(".part"))
    }

    /// `d/{a.txt (0600), sub (0750)/b.txt (0644)}` under `t`.
    fn local_tree(t: &Path) -> PathBuf {
        let d = t.join("d");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("a.txt"), b"aaa").unwrap();
        fs::write(d.join("sub/b.txt"), b"bbbb").unwrap();
        fs::set_permissions(d.join("a.txt"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(d.join("sub/b.txt"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(d.join("sub"), fs::Permissions::from_mode(0o750)).unwrap();
        d
    }

    #[test]
    fn upload_new_tree() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_dir("/up");
        let d = local_tree(t.path());
        let obs = RecordingObserver::default();
        let r = copy(&c, &[s(&d)], "sftp://m/up", &s(t.path()), &obs).unwrap();
        assert_eq!((r.done, r.skipped, r.cancelled), (2, 0, false));
        assert_eq!(m.data("/up/d/a.txt").unwrap(), b"aaa");
        assert_eq!(m.data("/up/d/sub/b.txt").unwrap(), b"bbbb");
        assert_eq!(m.mode_of("/up/d/a.txt"), Some(0o600));
        assert_eq!(m.mode_of("/up/d/sub/b.txt"), Some(0o644));
        assert_eq!(m.mode_of("/up/d/sub"), Some(0o750));
        assert!(no_part(&m), "{:?}", m.paths());
        assert!(obs.conflicts().is_empty() && obs.errors().is_empty());
        assert_eq!(m.calls("fork"), 1, "one forked connection for the job");
        // Copy and move leave the local source alone / delete it.
        assert!(d.join("a.txt").exists());
        // A missing destination folder is created, as locally.
        let r = move_(&c, &[s(&d)], "sftp://m/up2/", &s(t.path()), &obs).unwrap();
        assert_eq!((r.done, r.skipped), (2, 0));
        assert_eq!(m.data("/up2/d/sub/b.txt").unwrap(), b"bbbb");
        assert!(!d.exists(), "a moved source is deleted after the upload");
    }

    #[test]
    fn upload_conflict_answers() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/up/a.txt", b"old", 0o644, 5);
        let a = t.path().join("a.txt");
        fs::write(&a, b"new").unwrap();
        let tp = s(t.path());
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(m.data("/up/a.txt").unwrap(), b"old");
        let cf = obs.conflicts();
        assert_eq!(cf.len(), 1);
        assert_eq!(cf[0].target, "sftp://m/up/a.txt");
        assert_eq!((cf[0].target_size, cf[0].target_mtime), (3, 5));

        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(m.data("/up/a.txt").unwrap(), b"new");

        fs::write(&a, b"newer").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Rename {
            new_name: "a(2).txt".into(),
        }]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(m.data("/up/a.txt").unwrap(), b"new");
        assert_eq!(m.data("/up/a(2).txt").unwrap(), b"newer");

        // Overwrite all older: the server file (mtime 5) is older than the local one.
        let obs = RecordingObserver::answering(vec![ConflictAnswer::OverwriteAllOlder]);
        let r = copy(&c, &[s(&a)], "sftp://m/up/a(2).txt", &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        m.put_file("/up/a.txt", b"future", 0o644, i64::MAX / 2);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::OverwriteAllOlder]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &tp, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(m.data("/up/a.txt").unwrap(), b"future");

        // A local folder where the server has a file: the ops type clash text.
        fs::create_dir(t.path().join("x")).unwrap();
        m.put_file("/up/x", b"f", 0o644, 0);
        let obs = RecordingObserver::default();
        let r = copy(&c, &[s(&t.path().join("x"))], "sftp://m/up", &tp, &obs).unwrap();
        assert_eq!(r.skipped, 1);
        assert_eq!(
            obs.errors(),
            [(
                "sftp://m/up/x".to_string(),
                "Cannot overwrite file 'sftp://m/up/x' with a directory".to_string()
            )]
        );
        assert!(no_part(&m));
    }

    #[test]
    fn upload_replace_keeps_target_mode() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/up/run.sh", b"old", 0o755, 0);
        let a = t.path().join("run.sh");
        fs::write(&a, b"new").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        copy(&c, &[s(&a)], "sftp://m/up", &s(t.path()), &obs).unwrap();
        assert_eq!(m.data("/up/run.sh").unwrap(), b"new");
        assert_eq!(m.mode_of("/up/run.sh"), Some(0o755));
        assert!(m.calls("replace") >= 1, "replaced via a .part file");
        assert!(no_part(&m));
    }

    #[test]
    fn upload_interrupted_leaves_target_intact() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        let old: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
        m.put_file("/up/big.bin", &old, 0o644, 0);
        let a = t.path().join("big.bin");
        fs::write(&a, vec![7u8; 1024 * 1024]).unwrap();
        m.fail_after_bytes(10);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        match copy(&c, &[s(&a)], "sftp://m/up", &s(t.path()), &obs) {
            Err(OpError::Io(msg)) => assert!(msg.starts_with("connection lost"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(m.data("/up/big.bin").unwrap() == old, "target unchanged");
        assert!(no_part(&m), "{:?}", m.paths());
        assert!(c.sessions.get("m").is_none(), "the session was dropped");
    }

    #[test]
    fn upload_over_symlink_writes_through() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/up/real.txt", b"old", 0o640, 0);
        m.put_symlink("/up/a.txt", "real.txt", 0o777, 0);
        let a = t.path().join("a.txt");
        fs::write(&a, b"new").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &s(t.path()), &obs).unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(m.read_link("/up/a.txt").unwrap(), "real.txt");
        assert_eq!(m.data("/up/real.txt").unwrap(), b"new");
        assert_eq!(m.mode_of("/up/real.txt"), Some(0o640));
        assert!(no_part(&m));
    }

    #[test]
    fn upload_without_dir_write_permission_writes_directly() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/up/a.txt", b"old", 0o644, 0);
        m.set_no_create("/up");
        let a = t.path().join("a.txt");
        fs::write(&a, b"new content").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = copy(&c, &[s(&a)], "sftp://m/up", &s(t.path()), &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(m.data("/up/a.txt").unwrap(), b"new content");
        assert!(obs.errors().is_empty());
        assert!(no_part(&m));
        // A new file in that folder cannot be created at all: an item error.
        let b = t.path().join("b.txt");
        fs::write(&b, b"b").unwrap();
        let obs = RecordingObserver::default();
        let r = copy(&c, &[s(&b)], "sftp://m/up", &s(t.path()), &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(obs.errors().len(), 1);
        assert!(!m.exists("/up/b.txt"));
    }

    #[test]
    fn download_to_local_with_conflicts() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/d/a.txt", b"A", 0o600, 1_000_000);
        m.put_file("/d/b.txt", b"B", 0o644, 1_000_000);
        m.put_file("/d/sub/c.txt", b"C", 0o640, 1_000_000);
        m.put_symlink("/d/l", "a.txt", 0o777, 0);
        let out = t.path().join("out");
        fs::create_dir_all(out.join("d")).unwrap();
        fs::write(out.join("d/a.txt"), b"local a").unwrap();
        fs::write(out.join("d/b.txt"), b"local b").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::SkipAll]);
        let r = copy(&c, &["sftp://m/d".to_string()], &s(&out), "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped, r.cancelled), (2, 2, false));
        assert_eq!(obs.conflicts().len(), 1, "SkipAll stops asking");
        assert_eq!(obs.conflicts()[0].source, "sftp://m/d/a.txt");
        assert_eq!(fs::read(out.join("d/a.txt")).unwrap(), b"local a");
        assert_eq!(fs::read(out.join("d/b.txt")).unwrap(), b"local b");
        assert_eq!(fs::read(out.join("d/sub/c.txt")).unwrap(), b"C");
        assert_eq!(mode(&out.join("d/sub/c.txt")), 0o640);
        let md = fs::metadata(out.join("d/sub/c.txt")).unwrap();
        assert_eq!(md.mtime(), 1_000_000);
        assert_eq!(
            fs::read_link(out.join("d/l")).unwrap(),
            PathBuf::from("a.txt")
        );
        assert!(no_stage_dirs(&c));
        assert!(m.exists("/d/a.txt"), "copy keeps the source");
    }

    #[test]
    fn move_remote_to_local_deletes_source_after_success() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/d/a.txt", b"A", 0o644, 0);
        m.put_file("/d/b.txt", b"B", 0o644, 0);
        m.put_file("/d/sub/c.txt", b"C", 0o644, 0);
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("b.txt"), b"local b").unwrap();
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let srcs: Vec<String> = ["a.txt", "b.txt", "sub"]
            .iter()
            .map(|n| format!("sftp://m/d/{n}"))
            .collect();
        let r = move_(&c, &srcs, &s(&out), "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped), (2, 1));
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"A");
        assert_eq!(fs::read(out.join("b.txt")).unwrap(), b"local b");
        assert_eq!(fs::read(out.join("sub/c.txt")).unwrap(), b"C");
        assert!(!m.exists("/d/a.txt") && !m.exists("/d/sub"));
        assert_eq!(m.data("/d/b.txt").unwrap(), b"B", "skipped item stays");
        assert!(no_stage_dirs(&c));
    }

    #[test]
    fn move_within_same_server_renames() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/a/x/f.txt", b"f", 0o644, 0);
        m.put_dir("/b");
        let obs = RecordingObserver::default();
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(m.calls("read_to"), 0);
        assert_eq!(m.data("/b/x/f.txt").unwrap(), b"f");
        assert!(!m.exists("/a/x"));
        // An existing target: Skip leaves both, Overwrite replaces it.
        let t2 = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t2.path());
        m.put_file("/a/x", b"new", 0o644, 0);
        m.put_file("/b/x", b"old", 0o644, 0);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(m.data("/b/x").unwrap(), b"old");
        assert!(m.exists("/a/x"));
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(m.data("/b/x").unwrap(), b"new");
        assert!(!m.exists("/a/x"));
        assert_eq!(m.calls("read_to"), 0);
        // Into itself and onto itself are refused.
        m.put_dir("/a/y");
        let r = move_(
            &c,
            &["sftp://m/a/y".to_string()],
            "sftp://m/a/y/",
            "/",
            &obs,
        );
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(&c, &["sftp://m/a/y".to_string()], "sftp://m/a/", "/", &obs);
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
    }

    #[test]
    fn cancel_during_upload_removes_part() {
        /// Cancels once the first file is on the server and a `.part` of the second exists.
        struct CancelOnSecond(MemFs);
        impl OpObserver for CancelOnSecond {
            fn progress(&self, _p: &Progress) {}
            fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
                ConflictAnswer::Cancel
            }
            fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
                ErrorAnswer::Cancel
            }
            fn is_cancelled(&self) -> bool {
                self.0.exists("/up/d/a.txt") && !no_part(&self.0)
            }
        }
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_dir("/up");
        let d = t.path().join("d");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("a.txt"), b"a").unwrap();
        fs::write(d.join("b.bin"), vec![1u8; 100_000]).unwrap();
        let obs = CancelOnSecond(m.clone());
        let r = move_(&c, &[s(&d)], "sftp://m/up", "/", &obs).unwrap();
        assert!(r.cancelled);
        assert!(no_part(&m), "{:?}", m.paths());
        assert!(!m.exists("/up/d/b.bin"));
        assert!(d.join("a.txt").exists(), "a cancelled move deletes nothing");
    }

    #[test]
    fn archive_to_remote_and_back() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let (c, m) = remote_ctx(t.path());
        m.put_dir("/x");
        let tp = s(t.path());
        let obs = RecordingObserver::default();
        let r = copy(&c, &[format!("{tp}/a.zip/d")], "sftp://m/x", &tp, &obs).unwrap();
        assert_eq!(r.done, 2);
        assert_eq!(m.data("/x/d/b.txt").unwrap(), b"bbbb");
        assert_eq!(m.data("/x/d/e/c.txt").unwrap(), b"cc");
        fs::create_dir(t.path().join("out")).unwrap();
        let r = copy(
            &c,
            &["sftp://m/x/d".to_string()],
            &format!("{tp}/out"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 2);
        assert_eq!(fs::read(t.path().join("out/d/b.txt")).unwrap(), b"bbbb");
        assert_eq!(fs::read(t.path().join("out/d/e/c.txt")).unwrap(), b"cc");
        // And into an archive: server -> archive.
        let r = copy(
            &c,
            &["sftp://m/x/d/b.txt".to_string()],
            &format!("{tp}/a.zip/"),
            &tp,
            &obs,
        )
        .unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(
            read_entry(&t.path().join("a.zip"), ArchiveKind::Zip, "b.txt"),
            b"bbbb"
        );
        assert!(no_stage_dirs(&c) && no_part(&m));
    }

    #[test]
    fn mixed_local_and_remote_sources_refused() {
        let t = tempfile::tempdir().unwrap();
        let (c, _m) = remote_ctx(t.path());
        let obs = RecordingObserver::default();
        let r = copy(
            &c,
            &["sftp://m/a".to_string(), s(t.path())],
            &s(t.path()),
            "/",
            &obs,
        );
        assert_eq!(
            r,
            Err(OpError::Io(
                "all items must come from the same folder".into()
            ))
        );
    }

    #[test]
    fn move_on_server_merges_folders() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/a/x/f", b"new", 0o644, 10);
        m.put_file("/a/x/sub/g", b"g", 0o644, 10);
        m.put_file("/b/x/f", b"old", 0o644, 1);
        m.put_file("/b/x/keep", b"keep", 0o644, 1);
        m.put_file("/b/x/sub/h", b"h", 0o644, 1);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped, r.cancelled), (2, 0, false));
        assert_eq!(
            obs.conflicts().len(),
            1,
            "asked for the file, not the folder"
        );
        assert_eq!(obs.conflicts()[0].target, "sftp://m/b/x/f");
        assert_eq!(m.data("/b/x/keep").unwrap(), b"keep");
        assert_eq!(m.data("/b/x/f").unwrap(), b"new");
        assert_eq!(m.data("/b/x/sub/g").unwrap(), b"g");
        assert_eq!(m.data("/b/x/sub/h").unwrap(), b"h");
        assert!(!m.exists("/a/x"), "an emptied source folder is removed");
        assert_eq!(m.calls("read_to"), 0);
        // A skipped file keeps its source folder.
        m.put_file("/a/y/f", b"new", 0o644, 0);
        m.put_file("/b/y/f", b"old", 0o644, 0);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Skip]);
        let r = move_(&c, &["sftp://m/a/y".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert!(m.exists("/a/y/f") && m.data("/b/y/f").unwrap() == b"old");
    }

    #[test]
    fn move_on_server_overwrite_file_is_atomic() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/a/x", b"new", 0o644, 0);
        m.put_file("/b/x", b"old", 0o644, 0);
        m.fail_next("replace", RemoteError::Disconnected);
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs);
        assert!(
            matches!(&r, Err(OpError::Io(msg)) if msg.starts_with("connection lost")),
            "{r:?}"
        );
        assert_eq!(m.data("/b/x").unwrap(), b"old", "old target intact");
        assert_eq!(m.data("/a/x").unwrap(), b"new");
        c.sessions.insert("m", Arc::new(m.clone()));
        let obs = RecordingObserver::answering(vec![ConflictAnswer::Overwrite]);
        let r = move_(&c, &["sftp://m/a/x".to_string()], "sftp://m/b/", "/", &obs).unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(m.data("/b/x").unwrap(), b"new");
        assert!(!m.exists("/a/x"));
    }

    #[test]
    fn job_timeout_drops_session_like_disconnected() {
        // A hung server ("server not responding") ends the job as a lost connection: the
        // session leaves the registry (so `connections://changed` names it), no item error box.
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/a/f", b"data", 0o644, 0);
        m.fail_next("read_to", RemoteError::Timeout);
        let obs = RecordingObserver::answering(vec![]);
        let dest = s(t.path());
        let r = copy(&c, &["sftp://m/a/f".to_string()], &dest, &dest, &obs);
        assert!(
            matches!(&r, Err(OpError::Io(msg)) if msg.starts_with("connection lost")),
            "{r:?}"
        );
        assert!(obs.errors().is_empty(), "{:?}", obs.errors());
        assert!(c.sessions.get("m").is_none());
    }

    #[test]
    fn move_from_server_reports_source_delete_outcome() {
        /// Cancels once both files arrived locally (the delete phase).
        struct CancelAfter(PathBuf);
        impl OpObserver for CancelAfter {
            fn progress(&self, _p: &Progress) {}
            fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
                ConflictAnswer::Cancel
            }
            fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
                ErrorAnswer::Skip
            }
            fn is_cancelled(&self) -> bool {
                self.0.exists()
            }
        }
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/d/a.txt", b"A", 0o644, 0);
        m.put_file("/d/b.txt", b"B", 0o644, 0);
        let out = t.path().join("out");
        fs::create_dir(&out).unwrap();
        let srcs = vec!["sftp://m/d/a.txt".to_string(), "sftp://m/d/b.txt".into()];
        let obs = CancelAfter(out.join("b.txt"));
        let r = move_(&c, &srcs, &s(&out), "/", &obs).unwrap();
        assert!(r.cancelled, "{r:?}");
        assert!(m.exists("/d/a.txt") && m.exists("/d/b.txt"));
        // A source that cannot be deleted is reported, as for a local move.
        fs::remove_file(out.join("a.txt")).unwrap();
        fs::remove_file(out.join("b.txt")).unwrap();
        m.fail_next(
            "remove_file",
            RemoteError::PermissionDenied("/d/a.txt".into()),
        );
        let obs = RecordingObserver::default();
        let r = move_(&c, &srcs[..1], &s(&out), "/", &obs);
        assert_eq!(
            r,
            Err(OpError::Io(
                "'sftp://m/d/a.txt' was copied but could not be deleted".into()
            ))
        );
        assert!(m.exists("/d/a.txt") && out.join("a.txt").exists());
    }

    /// One bar from 0 to 100 %: fixed totals, done never goes back, ends at the totals.
    fn assert_one_bar(obs: &RecordingObserver) {
        let p = obs.progress();
        assert!(!p.is_empty());
        let (tb, tf) = (p[0].total_bytes, p[0].total_files);
        assert!(tb > 0 && tf > 0, "{p:?}");
        for w in p.windows(2) {
            assert_eq!((w[1].total_bytes, w[1].total_files), (tb, tf), "{p:?}");
            assert!(w[1].done_bytes >= w[0].done_bytes, "{p:?}");
            assert!(w[1].done_files >= w[0].done_files, "{p:?}");
        }
        let last = p.last().unwrap();
        assert_eq!((last.done_bytes, last.done_files), (tb, tf), "{p:?}");
    }

    #[test]
    fn transfer_through_stage_shows_one_progress_run() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        let n = MemFs::new();
        c.sessions.insert("n", Arc::new(n.clone()));
        m.put_file("/d/a.txt", b"aaaa", 0o644, 0);
        m.put_file("/d/b.txt", b"bb", 0o644, 0);
        n.put_dir("/up");
        let obs = RecordingObserver::default();
        copy(&c, &["sftp://m/d".to_string()], "sftp://n/up", "/", &obs).unwrap();
        assert_eq!(n.data("/up/d/a.txt").unwrap(), b"aaaa");
        assert_one_bar(&obs);
        let obs = RecordingObserver::default();
        copy(&c, &["sftp://m/d".to_string()], "sftp://m/e", "/", &obs).unwrap();
        assert_eq!(m.data("/e/b.txt").unwrap(), b"bb");
        assert_one_bar(&obs);
        zip_with_d(t.path());
        let obs = RecordingObserver::default();
        let zip = format!("{}/a.zip/n/", s(t.path()));
        copy(&c, &["sftp://m/d".to_string()], &zip, "/", &obs).unwrap();
        assert_one_bar(&obs);
        let obs = RecordingObserver::default();
        copy(
            &c,
            &[format!("{}/a.zip/d", s(t.path()))],
            "sftp://n/up2/",
            "/",
            &obs,
        )
        .unwrap();
        assert_one_bar(&obs);
        let obs = RecordingObserver::default();
        copy(
            &c,
            &["sftp://m/d".to_string()],
            &s(&t.path().join("out")),
            "/",
            &obs,
        )
        .unwrap();
        assert_one_bar(&obs);
    }
}
