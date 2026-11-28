//! Virtual file system routing: locations are either local paths or paths inside an archive
//! (`<archive file>/<inner path>`). Local locations go straight to `listing` and `ops`.
//!
//! An archive inside an archive (`outer.zip/inner.tar.gz/d`) is entered like any other: it is
//! extracted to a temp file (`nested-<n>` under the temp root, kept while the outer archive is
//! unchanged), and after every change that temp file is written back into the archive holding
//! it, up to the outermost one, each with one atomic `rewrite`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::archive::read::extract;
use crate::archive::write::{rewrite, Edit};
use crate::archive::{
    list_inner, split_location, stat_key, ArchiveError, ArchiveIndex, ArchiveKind, ArchiveLoc,
    IndexCache,
};
use crate::listing::{list_dir, sort_entries, Entry, ListError};
use crate::ops::{
    self, ConflictAnswer, ConflictInfo, ErrorAnswer, OpError, OpObserver, OpSummary, Progress,
};
use crate::remote::transfer::{Remover, Stop};
use crate::remote::{
    join_path, parent_path, parse_remote, RemoteError, RemoteFs, RemoteLoc, RemoteStat, Sessions,
};

mod transfer;

pub use transfer::{copy, move_};

#[derive(Clone, Debug, PartialEq)]
pub enum Loc {
    Local(PathBuf),
    Archive(ArchiveLoc),
    Remote(RemoteLoc),
}

pub fn parse(loc: &str) -> Loc {
    if let Some(r) = parse_remote(loc) {
        return Loc::Remote(r);
    }
    match split_location(loc) {
        Some(a) => Loc::Archive(a),
        None => Loc::Local(PathBuf::from(loc)),
    }
}

/// Shared state of the virtual file system: index cache, archive passwords and temp folders.
pub struct Ctx {
    pub cache: IndexCache,
    pub temp_root: PathBuf,
    pub sessions: Sessions,
    passwords: Mutex<HashMap<PathBuf, String>>,
    nested: Mutex<NestedState>,
    n: AtomicU64,
}

/// Archives inside archives kept as temp copies at most.
const NESTED_MAX: usize = 8;

/// An archive inside another archive, extracted to a temp file.
struct NestedSlot {
    parent_kind: ArchiveKind,
    /// `(size, mtime)` of the parent file when `file` was extracted or written back.
    parent_stamp: (u64, i64),
    /// The temp folder holding `file`.
    dir: PathBuf,
    file: PathBuf,
    used: u64,
}

/// Temp copies of nested archives, keyed by (file of the archive holding it, inner path).
#[derive(Default)]
struct NestedState {
    map: HashMap<(PathBuf, String), NestedSlot>,
    tick: u64,
}

impl NestedState {
    /// Removes the copy under `key` and every copy taken out of it; returns their folders.
    fn forget(&mut self, key: &(PathBuf, String)) -> Vec<PathBuf> {
        let Some(slot) = self.map.remove(key) else {
            return Vec::new();
        };
        let children: Vec<(PathBuf, String)> = self
            .map
            .keys()
            .filter(|(p, _)| *p == slot.file)
            .cloned()
            .collect();
        let mut dirs = vec![slot.dir];
        for c in children {
            dirs.extend(self.forget(&c));
        }
        dirs
    }
}

impl Ctx {
    pub fn new(temp_root: PathBuf) -> Ctx {
        Ctx {
            cache: IndexCache::new(),
            temp_root,
            sessions: Sessions::new(),
            passwords: Mutex::new(HashMap::new()),
            nested: Mutex::new(NestedState::default()),
            n: AtomicU64::new(0),
        }
    }

    pub fn set_password(&self, archive: &Path, pw: String) {
        self.passwords
            .lock()
            .unwrap()
            .insert(archive.to_path_buf(), pw);
    }

    pub fn password(&self, archive: &Path) -> Option<String> {
        self.passwords.lock().unwrap().get(archive).cloned()
    }

    /// Creates `temp_root/<prefix>-<n>`.
    pub fn new_temp_dir(&self, prefix: &str) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.temp_root)?;
        loop {
            let n = self.n.fetch_add(1, Ordering::Relaxed);
            let dir = self.temp_root.join(format!("{prefix}-{n}"));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Removes `dir` with everything in it, best effort. Folders extracted from an archive
    /// may have no owner permissions, so they are made accessible first.
    pub fn cleanup_dir(&self, dir: &Path) {
        fn open_up(p: &Path) {
            let Ok(md) = fs::symlink_metadata(p) else {
                return;
            };
            if !md.is_dir() {
                return;
            }
            let _ = fs::set_permissions(
                p,
                fs::Permissions::from_mode(md.permissions().mode() | 0o700),
            );
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    open_up(&e.path());
                }
            }
        }
        open_up(dir);
        let _ = fs::remove_dir_all(dir);
    }

    /// The temp copy of the archive `inner` inside `parent`; extracted again when the parent
    /// file changed since.
    fn nested_file(&self, parent: &ArchiveLoc, inner: &str) -> Result<PathBuf, ArchiveError> {
        let stamp = stat_key(&parent.archive)?;
        let key = (parent.archive.clone(), inner.to_string());
        let mut st = self.nested.lock().unwrap();
        st.tick += 1;
        let tick = st.tick;
        if let Some(slot) = st.map.get_mut(&key) {
            if slot.parent_stamp == stamp && slot.file.is_file() {
                slot.used = tick;
                return Ok(slot.file.clone());
            }
        }
        for d in st.forget(&key) {
            self.cleanup_dir(&d);
        }
        let dir = self
            .new_temp_dir("nested")
            .map_err(|e| ArchiveError::Io(e.to_string()))?;
        let file = dir.join(inner.rsplit('/').next().unwrap_or(inner));
        let pw = self.password(&parent.shown);
        let obs = FirstError::default();
        let res = extract(
            &parent.archive,
            parent.kind,
            &[inner.to_string()],
            &dir,
            pw.as_deref(),
            &obs,
        )
        .and_then(|_| match obs.0.lock().unwrap().take() {
            Some(msg) => Err(ArchiveError::Io(msg)),
            None if !file.is_file() => {
                Err(ArchiveError::Io(format!("{inner} could not be extracted")))
            }
            None => Ok(()),
        });
        if let Err(e) = res {
            self.cleanup_dir(&dir);
            return Err(e);
        }
        st.map.insert(
            key,
            NestedSlot {
                parent_kind: parent.kind,
                parent_stamp: stamp,
                dir,
                file: file.clone(),
                used: tick,
            },
        );
        while st.map.len() > NESTED_MAX {
            let Some(oldest) = st
                .map
                .iter()
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            for d in st.forget(&oldest) {
                self.cleanup_dir(&d);
            }
        }
        Ok(file)
    }

    /// After `file` was rewritten: when it is the temp copy of a nested archive, writes it
    /// back into the archive holding it, and so on up to the outermost archive. A failed
    /// write-back drops the copy, so the next access extracts it again from what the outer
    /// archive really holds. Not cancellable: the inner archive is already written.
    fn write_back(&self, file: &Path) -> Result<(), ArchiveError> {
        let mut file = file.to_path_buf();
        loop {
            let found = {
                let st = self.nested.lock().unwrap();
                st.map
                    .iter()
                    .find(|(_, s)| s.file == file)
                    .map(|(k, s)| (k.clone(), s.parent_kind))
            };
            let Some((key, kind)) = found else {
                return Ok(());
            };
            let edit = Edit::Add {
                src: file.clone(),
                inner: key.1.clone(),
            };
            let res = rewrite(&key.0, kind, &[edit], &self.cache, &NullObserver)
                .and_then(|_| stat_key(&key.0));
            let mut st = self.nested.lock().unwrap();
            match res {
                Ok(stamp) => {
                    if let Some(s) = st.map.get_mut(&key) {
                        s.parent_stamp = stamp;
                    }
                }
                Err(e) => {
                    let dirs = st.forget(&key);
                    drop(st);
                    for d in dirs {
                        self.cleanup_dir(&d);
                    }
                    return Err(e);
                }
            }
            file = key.0;
        }
    }
}

/// `rewrite`, and for an archive inside another one the write-back into the outer archives.
pub(crate) fn rewrite_in(
    ctx: &Ctx,
    archive: &Path,
    kind: ArchiveKind,
    edits: &[Edit],
    obs: &dyn OpObserver,
) -> Result<(), ArchiveError> {
    rewrite(archive, kind, edits, &ctx.cache, obs)?;
    ctx.write_back(archive)
}

/// Why `resolve` failed, and in which archive (as the user sees it).
#[derive(Debug)]
pub(crate) struct Unresolved {
    pub err: ArchiveError,
    pub archive: PathBuf,
}

impl Unresolved {
    pub(crate) fn message(&self) -> String {
        archive_error_message(&self.err, &self.archive)
    }
    pub(crate) fn op_error(self) -> OpError {
        archive_op_error(self.err, &self.archive)
    }
}

/// `split_location` that also enters archives inside archives (`outer.zip/inner.tar.gz/d`):
/// each nested archive on the way is taken out to a temp copy, and the result names the
/// innermost archive (`shown` is its path as the user sees it). With `enter`, a nested archive
/// at the very end of `loc` is entered (its root); without, it stays an item of the archive
/// holding it, as an archive file in a local folder is a local item.
pub(crate) fn resolve(ctx: &Ctx, loc: &str, enter: bool) -> Result<Option<ArchiveLoc>, Unresolved> {
    let Some(mut a) = split_location(loc) else {
        return Ok(None);
    };
    loop {
        if a.inner.is_empty() {
            return Ok(Some(a));
        }
        let fail = |err, archive: &Path| Unresolved {
            err,
            archive: archive.to_path_buf(),
        };
        let index = ctx
            .cache
            .get(&a.archive, a.kind)
            .map_err(|e| fail(e, &a.shown))?;
        let ends = a
            .inner
            .match_indices('/')
            .map(|(i, _)| i)
            .chain(enter.then_some(a.inner.len()));
        let mut nested = None;
        for end in ends {
            let p = &a.inner[..end];
            match index.find(p) {
                Some(e) if e.is_dir => continue,
                Some(e) if !e.is_symlink => {
                    let name = p.rsplit('/').next().unwrap_or(p);
                    nested = ArchiveKind::from_name(name).map(|k| (end, k));
                    break;
                }
                _ => break,
            }
        }
        let Some((end, kind)) = nested else {
            return Ok(Some(a));
        };
        let p = &a.inner[..end];
        let file = ctx.nested_file(&a, p).map_err(|e| fail(e, &a.shown))?;
        a = ArchiveLoc {
            archive: file,
            kind,
            inner: a.inner[end..].trim_start_matches('/').to_string(),
            shown: a.shown.join(p),
        };
    }
}

/// The open connection for a server location; `Disconnected` when there is none.
pub(crate) fn session_for(ctx: &Ctx, r: &RemoteLoc) -> Result<Arc<dyn RemoteFs>, RemoteError> {
    ctx.sessions.get(&r.server).ok_or(RemoteError::Disconnected)
}

/// Runs `f` on the connection of `r`. A lost connection (`Disconnected`, or `Timeout` for a
/// hung one, see `RemoteError::is_lost`) is dropped from the registry so the UI can offer to
/// reconnect.
pub(crate) fn remote_call<T>(
    ctx: &Ctx,
    r: &RemoteLoc,
    f: impl FnOnce(&dyn RemoteFs) -> Result<T, RemoteError>,
) -> Result<T, RemoteError> {
    let fs = session_for(ctx, r)?;
    let res = f(fs.as_ref());
    if res.as_ref().is_err_and(RemoteError::is_lost) {
        ctx.sessions.remove(&r.server);
    }
    res
}

/// Overwrite on conflict, skip on error, never cancel. For short operations.
pub(crate) struct NullObserver;

impl OpObserver for NullObserver {
    fn progress(&self, _p: &Progress) {}
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

/// Message text of an archive error; the UI recognizes the password prefixes.
pub(crate) fn archive_error_message(e: &ArchiveError, archive: &Path) -> String {
    match e {
        ArchiveError::PasswordRequired => format!("password required: {}", archive.display()),
        ArchiveError::WrongPassword => format!("wrong password: {}", archive.display()),
        other => other.to_string(),
    }
}

pub(crate) fn archive_op_error(e: ArchiveError, archive: &Path) -> OpError {
    match e {
        ArchiveError::Cancelled => OpError::Cancelled,
        other => OpError::Io(archive_error_message(&other, archive)),
    }
}

/// An archive location with a non-empty inner path; the archive file itself is a local item.
/// By name only: a nested archive is not entered (see `resolve_item`).
fn inner_loc(loc: &str) -> Option<ArchiveLoc> {
    split_location(loc).filter(|a| !a.inner.is_empty())
}

/// An item inside an archive (nested ones entered on the way); `None` for a local item.
pub(crate) fn resolve_item(ctx: &Ctx, loc: &str) -> Result<Option<ArchiveLoc>, Unresolved> {
    Ok(resolve(ctx, loc, false)?.filter(|a| !a.inner.is_empty()))
}

fn join_inner(a: &ArchiveLoc, inner: &str) -> String {
    if inner.is_empty() {
        a.shown.to_string_lossy().into_owned()
    } else {
        format!("{}/{inner}", a.shown.to_string_lossy())
    }
}

fn parent_inner(inner: &str) -> &str {
    inner.rsplit_once('/').map_or("", |(p, _)| p)
}

pub fn list(ctx: &Ctx, loc: &str) -> Result<Vec<Entry>, ListError> {
    match parse(loc) {
        Loc::Local(p) => list_dir(&p),
        Loc::Remote(r) => {
            let mut v =
                remote_call(ctx, &r, |fs| fs.list(&r.path)).map_err(|e| e.into_list_error(loc))?;
            sort_entries(&mut v);
            Ok(v)
        }
        Loc::Archive(_) => {
            let cannot = |e: &dyn std::fmt::Display| {
                log::warn!("cannot list {loc}: {e}");
                ListError::Io(loc.to_string())
            };
            let a = resolve(ctx, loc, true)
                .map_err(|u| cannot(&u.message()))?
                .ok_or_else(|| ListError::NotFound(loc.to_string()))?;
            let index = ctx.cache.get(&a.archive, a.kind).map_err(|e| cannot(&e))?;
            list_inner(&index, &a.inner)
        }
    }
}

/// Nearest location that exists: the closest folder inside the archive, the archive root,
/// or (when the archive is gone) what `config::restore_path` gives.
pub fn nearest(ctx: &Ctx, loc: &str, home: &Path) -> String {
    if let Some(r) = parse_remote(loc) {
        return nearest_remote(ctx, &r, loc);
    }
    let a = match resolve(ctx, loc, true) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return crate::config::restore_path(loc, home)
                .to_string_lossy()
                .into_owned()
        }
        Err(u) => return u.archive.to_string_lossy().into_owned(),
    };
    let Ok(index) = ctx.cache.get(&a.archive, a.kind) else {
        return join_inner(&a, "");
    };
    let mut inner = a.inner.as_str();
    while !inner.is_empty() && !index.find(inner).is_some_and(|e| e.is_dir) {
        inner = parent_inner(inner);
    }
    join_inner(&a, inner)
}

/// Closest existing folder above `r.path`; the location unchanged when there is no
/// connection or it fails.
fn nearest_remote(ctx: &Ctx, r: &RemoteLoc, loc: &str) -> String {
    let mut path = r.path.clone();
    loop {
        match remote_call(ctx, r, |fs| fs.stat(&path)) {
            Err(_) => return loc.to_string(),
            Ok(Some(st)) if st.is_dir && !st.is_symlink => break,
            // A symlink counts as a folder when it resolves to one, as locally.
            Ok(Some(st)) if st.is_symlink && remote_call(ctx, r, |fs| fs.list(&path)).is_ok() => {
                break
            }
            Ok(_) if path == "/" => break,
            Ok(_) => path = parent_path(&path).to_string(),
        }
    }
    crate::remote::format_remote(&r.scheme, &r.server, &path)
}

fn remote_io(e: RemoteError) -> io::Error {
    io::Error::other(e.to_string())
}

/// (bytes, non-directory items) under `path`; symlinks count as items of size 0.
fn remote_size(
    fs: &dyn RemoteFs,
    path: &str,
    cancel: &AtomicBool,
    top: bool,
) -> Result<(u64, u64), RemoteError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(RemoteError::Cancelled);
    }
    let st = fs
        .stat(path)?
        .ok_or_else(|| RemoteError::NotFound(path.to_string()))?;
    // Only the sized location itself follows a symlink (to a folder); links inside are not
    // followed, so there are no loops.
    let follow = top && st.is_symlink && fs.list(path).is_ok();
    if st.is_symlink && !follow {
        return Ok((0, 1));
    }
    if !st.is_dir && !follow {
        return Ok((st.size, 1));
    }
    let mut total = (0, 0);
    for e in fs.list(path)? {
        let (b, n) = remote_size(fs, &join_path(path, &e.name), cancel, false)?;
        total = (total.0 + b, total.1 + n);
    }
    Ok(total)
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

fn size_in(index: &ArchiveIndex, inner: &str) -> Option<(u64, u64)> {
    let size = |e: &crate::archive::IndexEntry| if e.is_symlink { 0 } else { e.size };
    if inner.is_empty() {
        return Some(
            index
                .entries
                .iter()
                .filter(|e| !e.is_dir)
                .fold((0, 0), |(b, n), e| (b + size(e), n + 1)),
        );
    }
    let root = index.find(inner)?;
    if !root.is_dir {
        return Some((size(root), 1));
    }
    let prefix = format!("{inner}/");
    Some(
        index
            .entries
            .iter()
            .filter(|e| e.path.starts_with(&prefix) && !e.is_dir)
            .fold((0, 0), |(b, n), e| (b + size(e), n + 1)),
    )
}

/// (bytes, non-directory items) under `loc`.
pub fn dir_size(ctx: &Ctx, loc: &str, cancel: &AtomicBool) -> io::Result<(u64, u64)> {
    if let Some(r) = parse_remote(loc) {
        return remote_call(ctx, &r, |fs| remote_size(fs, &r.path, cancel, true)).map_err(
            |e| match e {
                RemoteError::Cancelled => cancelled(),
                e => remote_io(e),
            },
        );
    }
    let item = resolve_item(ctx, loc).map_err(|u| io::Error::other(u.message()))?;
    match item {
        None => ops::dir_size(Path::new(loc), cancel),
        Some(a) => {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            let index = ctx
                .cache
                .get(&a.archive, a.kind)
                .map_err(|e| io::Error::other(archive_error_message(&e, &a.shown)))?;
            size_in(&index, &a.inner)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("not found: {loc}")))
        }
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if ops::valid_name(name) {
        Ok(())
    } else {
        Err(format!("invalid name '{name}'"))
    }
}

pub fn mkdir(ctx: &Ctx, parent: &str, name: &str) -> Result<String, String> {
    if let Some(r) = parse_remote(parent) {
        check_name(name)?;
        let path = join_path(&r.path, name);
        remote_call(ctx, &r, |fs| fs.mkdir(&path)).map_err(|e| e.to_string())?;
        return Ok(crate::remote::format_remote(&r.scheme, &r.server, &path));
    }
    match resolve(ctx, parent, true).map_err(|u| u.message())? {
        None => ops::mkdir(Path::new(parent), name)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| e.to_string()),
        Some(a) => {
            check_name(name)?;
            let inner = if a.inner.is_empty() {
                name.to_string()
            } else {
                format!("{}/{name}", a.inner)
            };
            rewrite_in(
                ctx,
                &a.archive,
                a.kind,
                &[Edit::Mkdir {
                    inner: inner.clone(),
                }],
                &NullObserver,
            )
            .map_err(|e| archive_error_message(&e, &a.shown))?;
            Ok(join_inner(&a, &inner))
        }
    }
}

pub fn rename(ctx: &Ctx, loc: &str, new_name: &str) -> Result<String, String> {
    if let Some(r) = parse_remote(loc) {
        check_name(new_name)?;
        let to = join_path(parent_path(&r.path), new_name);
        remote_call(ctx, &r, |fs| fs.rename(&r.path, &to)).map_err(|e| e.to_string())?;
        return Ok(crate::remote::format_remote(&r.scheme, &r.server, &to));
    }
    match resolve_item(ctx, loc).map_err(|u| u.message())? {
        None => ops::rename(Path::new(loc), new_name)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| e.to_string()),
        Some(a) => {
            check_name(new_name)?;
            let parent = parent_inner(&a.inner);
            let to = if parent.is_empty() {
                new_name.to_string()
            } else {
                format!("{parent}/{new_name}")
            };
            rewrite_in(
                ctx,
                &a.archive,
                a.kind,
                &[Edit::Rename {
                    from: a.inner.clone(),
                    to: to.clone(),
                }],
                &NullObserver,
            )
            .map_err(|e| archive_error_message(&e, &a.shown))?;
            Ok(join_inner(&a, &to))
        }
    }
}

/// Deletes `locs` (all from one folder). In an archive deletion is permanent and done with a
/// single rewrite per archive; `to_trash` is ignored there.
pub fn delete(
    ctx: &Ctx,
    locs: &[String],
    to_trash: bool,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    if locs.iter().any(|l| parse_remote(l).is_some()) {
        return delete_remote(ctx, locs, obs);
    }
    let parsed: Vec<Option<ArchiveLoc>> = locs
        .iter()
        .map(|l| resolve_item(ctx, l))
        .collect::<Result<_, _>>()
        .map_err(Unresolved::op_error)?;
    if parsed.iter().all(Option::is_none) {
        let paths: Vec<PathBuf> = locs.iter().map(PathBuf::from).collect();
        return ops::delete(&paths, to_trash, obs);
    }
    let mut groups: Vec<(PathBuf, Vec<ArchiveLoc>)> = Vec::new();
    for a in parsed.iter().flatten() {
        match groups.iter_mut().find(|(p, _)| *p == a.archive) {
            Some((_, v)) => v.push(a.clone()),
            None => groups.push((a.archive.clone(), vec![a.clone()])),
        }
    }
    let mut summary = OpSummary::default();
    for (archive, items) in groups {
        if obs.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let shown = &items[0].shown;
        let index = ctx
            .cache
            .get(&archive, items[0].kind)
            .map_err(|e| archive_op_error(e, shown))?;
        let count: u64 = items
            .iter()
            .filter_map(|a| size_in(&index, &a.inner))
            .map(|(_, n)| n)
            .sum();
        let edits: Vec<Edit> = items
            .iter()
            .map(|a| Edit::Delete {
                inner: a.inner.clone(),
            })
            .collect();
        rewrite_in(ctx, &archive, items[0].kind, &edits, obs)
            .map_err(|e| archive_op_error(e, shown))?;
        summary.done += count;
    }
    Ok(summary)
}

/// Permanent recursive delete on servers; `obs.error` is asked per item.
fn delete_remote(ctx: &Ctx, locs: &[String], obs: &dyn OpObserver) -> Result<OpSummary, OpError> {
    let mut rm = Remover::new(obs, "");
    for loc in locs {
        let res = match parse_remote(loc) {
            None => rm.fail(loc, "not a server location"),
            Some(r) => {
                let fs = session_for(ctx, &r).map_err(RemoteError::into_op_error)?;
                rm.shown = format!("{}://{}", r.scheme, r.server);
                rm.tree(fs.as_ref(), &r.path)
            }
        };
        match res {
            Ok(()) => {}
            Err(Stop::Cancel) => {
                rm.summary.cancelled = true;
                break;
            }
            Err(Stop::Lost) => {
                if let Some(r) = parse_remote(loc) {
                    ctx.sessions.remove(&r.server);
                }
                return Err(RemoteError::Disconnected.into_op_error());
            }
        }
    }
    Ok(rm.summary)
}

/// Folder to use as working directory: the folder holding the archive for a location inside
/// one, the user's home for a location on a server, otherwise the location itself.
pub fn local_dir_for(loc: &str, home: &Path) -> PathBuf {
    if parse_remote(loc).is_some() {
        return home.to_path_buf();
    }
    match split_location(loc) {
        Some(a) => a
            .archive
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
        None => PathBuf::from(loc),
    }
}

/// `(size, mtime)` of the file entry `loc` inside an archive or on a server, as it is now;
/// None when `loc` is neither or the entry is gone.
pub fn entry_stamp(ctx: &Ctx, loc: &str) -> Result<Option<(u64, i64)>, String> {
    if let Some(r) = parse_remote(loc) {
        let st = stat_following(ctx, &r).map_err(|e| e.to_string())?;
        return Ok(st.filter(|s| !s.is_dir).map(|s| (s.size, s.mtime)));
    }
    let Some(a) = resolve_item(ctx, loc).map_err(|u| u.message())? else {
        return Ok(None);
    };
    let index = ctx
        .cache
        .get(&a.archive, a.kind)
        .map_err(|e| archive_error_message(&e, &a.shown))?;
    Ok(index
        .find(&a.inner)
        .filter(|e| !e.is_dir)
        .map(|e| (e.size, e.mtime)))
}

/// Before writing an edited copy back to `loc` (a file inside an archive): the archive the
/// location names must still be there, and be `expected` (the archive the copy came from)
/// when given. Otherwise (renamed, moved, deleted, replaced by a folder) the copy would land
/// in a new folder named like the archive. For a server file the connection must still be
/// open. A folder whose name looks like an archive (`backup.zip/real.tar`) is fine: the
/// archive is the first real archive file on the path.
pub fn check_upload_target(ctx: &Ctx, loc: &str, expected: Option<&Path>) -> Result<(), String> {
    if let Some(r) = parse_remote(loc) {
        return match ctx.sessions.get(&r.server) {
            Some(_) => Ok(()),
            None => Err(format!(
                "Not connected to {}; your edited copy is kept",
                r.server
            )),
        };
    }
    use std::path::Component;
    // The archive is the first component with an archive name, whether or not it exists.
    let mut named = PathBuf::new();
    let mut found = false;
    for c in Path::new(loc).components() {
        named.push(c.as_os_str());
        if let Component::Normal(n) = c {
            if crate::archive::ArchiveKind::from_name(&n.to_string_lossy()).is_some() {
                found = true;
                break;
            }
        }
    }
    let shown = if found {
        named.display().to_string()
    } else {
        loc.to_string()
    };
    let gone = |what: String| {
        Err(format!(
            "The archive {what} no longer exists; your edited copy is kept"
        ))
    };
    match (inner_loc(loc), expected) {
        (Some(a), Some(exp)) if a.archive == exp => Ok(()),
        (Some(_), Some(exp)) => gone(exp.display().to_string()),
        (Some(_), None) => Ok(()),
        (None, Some(exp)) => gone(exp.display().to_string()),
        (None, None) => gone(shown),
    }
}

/// Local path for `loc`: a local location is returned unchanged; a file inside an archive is
/// extracted into a new `ctx.temp_root/<prefix>-<n>` folder (removed by the caller).
pub fn materialize(ctx: &Ctx, loc: &str, prefix: &str) -> Result<PathBuf, String> {
    materialize_with(ctx, loc, prefix, &mut |_, _| true)
}

/// Refusal when a download was stopped through `progress`.
pub const DOWNLOAD_CANCELLED: &str = "Download cancelled";

/// `materialize`; for a server file `progress(done, total)` hears the bytes so far and stops
/// the download (`DOWNLOAD_CANCELLED`) when it returns false.
pub fn materialize_with(
    ctx: &Ctx,
    loc: &str,
    prefix: &str,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<PathBuf, String> {
    if let Some(r) = parse_remote(loc) {
        return materialize_remote(ctx, &r, prefix, progress);
    }
    let Some(a) = resolve_item(ctx, loc).map_err(|u| u.message())? else {
        return Ok(PathBuf::from(loc));
    };
    let fail = |e: ArchiveError| archive_error_message(&e, &a.shown);
    let index = ctx.cache.get(&a.archive, a.kind).map_err(fail)?;
    match index.find(&a.inner) {
        None => return Err(fail(ArchiveError::NotFound(a.inner.clone()))),
        Some(e) if e.is_dir => return Err("Only files can be opened from an archive".into()),
        Some(_) => {}
    }
    let dir = ctx.new_temp_dir(prefix).map_err(|e| e.to_string())?;
    let pw = ctx.password(&a.shown);
    let obs = FirstError::default();
    let res = extract(
        &a.archive,
        a.kind,
        std::slice::from_ref(&a.inner),
        &dir,
        pw.as_deref(),
        &obs,
    );
    let path = dir.join(a.inner.rsplit('/').next().unwrap_or(&a.inner));
    // A skipped entry (hard link, unsafe link, write error) leaves no file.
    let res = res
        .map_err(fail)
        .and_then(|_| match obs.0.lock().unwrap().take() {
            Some(msg) => Err(msg),
            None if fs::symlink_metadata(&path).is_err() => {
                Err(format!("{} could not be extracted", a.inner))
            }
            None => Ok(path),
        });
    if res.is_err() {
        ctx.cleanup_dir(&dir);
    }
    res
}

/// Collapses `.` and `..` in an absolute server path.
fn normalize_remote(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `stat` of a server path with symlinks followed (up to 8 hops), so a symlink shows what it
/// points to. Where the link cannot be read (FTP), the link's own values are kept.
fn stat_following(ctx: &Ctx, r: &RemoteLoc) -> Result<Option<RemoteStat>, RemoteError> {
    let mut path = r.path.clone();
    let mut st = remote_call(ctx, r, |fs| fs.stat(&path))?;
    for _ in 0..8 {
        let Some(cur) = st.as_ref().filter(|s| s.is_symlink) else {
            return Ok(st);
        };
        let target = match remote_call(ctx, r, |fs| fs.read_link(&path)) {
            Ok(t) => t,
            Err(RemoteError::Unsupported(_)) => return Ok(Some(cur.clone())),
            Err(e) => return Err(e),
        };
        path = if target.starts_with('/') {
            normalize_remote(&target)
        } else {
            normalize_remote(&join_path(parent_path(&path), &target))
        };
        match remote_call(ctx, r, |fs| fs.stat(&path))? {
            // A dangling link keeps its own values.
            None => return Ok(st),
            next => st = next,
        }
    }
    Ok(st)
}

/// Downloads the server file `r` into a new `<prefix>-<n>` temp folder (removed by the caller
/// on success, here on any error). A symlink is followed: the copy holds the content.
fn materialize_remote(
    ctx: &Ctx,
    r: &RemoteLoc,
    prefix: &str,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<PathBuf, String> {
    let st = stat_following(ctx, r).map_err(|e| e.to_string())?;
    let total = match st {
        None => return Err(RemoteError::NotFound(r.path.clone()).to_string()),
        Some(s) if s.is_dir => return Err("Only files can be opened from a server".into()),
        Some(s) => s.size,
    };
    let mut done = 0u64;
    let mut stopped = false;
    let dir = ctx.new_temp_dir(prefix).map_err(|e| e.to_string())?;
    let name = r.path.rsplit('/').next().unwrap_or(&r.path);
    let path = dir.join(name);
    let res = fs::File::create(&path)
        .map_err(|e| e.to_string())
        .and_then(|mut f| {
            remote_call(ctx, r, |fs| {
                fs.read_to(&r.path, &mut f, &mut |n| {
                    done += n;
                    let go = progress(done, total);
                    stopped |= !go;
                    go
                })
            })
            .map_err(|e| {
                if stopped {
                    DOWNLOAD_CANCELLED.to_string()
                } else {
                    e.to_string()
                }
            })
        });
    match res {
        Ok(()) => Ok(path),
        Err(e) => {
            ctx.cleanup_dir(&dir);
            Err(e)
        }
    }
}

/// Records the first item error; the item is skipped.
#[derive(Default)]
struct FirstError(Mutex<Option<String>>);

impl OpObserver for FirstError {
    fn progress(&self, _p: &Progress) {}
    fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
        ConflictAnswer::Overwrite
    }
    fn error(&self, _path: &str, msg: &str) -> ErrorAnswer {
        self.0
            .lock()
            .unwrap()
            .get_or_insert_with(|| msg.to_string());
        ErrorAnswer::Skip
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::testutil::*;
    use crate::archive::ArchiveKind;

    fn ctx(t: &Path) -> Ctx {
        Ctx::new(t.join("tmp-root"))
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn names(v: &[Entry]) -> Vec<String> {
        v.iter().map(|e| e.name.clone()).collect()
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

    use crate::listing::ListError;
    use crate::remote::testutil::MemFs;
    use crate::remote::RemoteError;

    fn remote_ctx(t: &Path) -> (Ctx, MemFs) {
        let c = ctx(t);
        let m = MemFs::new();
        c.sessions.insert("m", Arc::new(m.clone()));
        (c, m)
    }

    #[test]
    fn vfs_routes_remote() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/d/b.txt", b"bbbb", 0o644, 5);
        m.put_file("/d/.hidden", b"h", 0o600, 5);
        m.put_file("/d/sub/c.txt", b"cc", 0o644, 5);
        let v = list(&c, "sftp://m/d").unwrap();
        assert_eq!(names(&v), vec!["sub", ".hidden", "b.txt"]);
        assert!(v.iter().find(|e| e.name == ".hidden").unwrap().hidden);
        assert_eq!(mkdir(&c, "sftp://m/d", "n").unwrap(), "sftp://m/d/n");
        assert_eq!(rename(&c, "sftp://m/d/n", "k").unwrap(), "sftp://m/d/k");
        assert!(m.exists("/d/k") && !m.exists("/d/n"));
        assert!(mkdir(&c, "sftp://m/d", "a/b").is_err());
        assert_eq!(nearest(&c, "sftp://m/d/nema/x", t.path()), "sftp://m/d");
        assert_eq!(nearest(&c, "sftp://m/zzz/x", t.path()), "sftp://m/");
        let cancel = AtomicBool::new(false);
        assert_eq!(dir_size(&c, "sftp://m/d", &cancel).unwrap(), (7, 3));
        cancel.store(true, Ordering::Relaxed);
        assert!(dir_size(&c, "sftp://m/d", &cancel).is_err());
        let r = delete(&c, &["sftp://m/d".to_string()], true, &NullObserver).unwrap();
        assert_eq!(r.done, 3);
        assert!(!m.exists("/d") && !m.exists("/d/sub/c.txt"));
    }

    #[test]
    fn remote_symlinked_folder_counts_as_folder() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_dir("/srv/www");
        m.put_file("/srv/www/a.txt", b"aaa", 0o644, 0);
        m.put_file("/srv/www/sub/b.txt", b"bb", 0o644, 0);
        m.put_symlink("/srv/www/loop", "/srv/www", 0o777, 0);
        m.put_dir("/var");
        m.put_symlink("/var/www", "/srv/www", 0o777, 0);
        assert_eq!(
            nearest(&c, "sftp://m/var/www/gone", t.path()),
            "sftp://m/var/www"
        );
        let cancel = AtomicBool::new(false);
        // The top-level symlink is followed; links inside the walk are not.
        assert_eq!(dir_size(&c, "sftp://m/var/www", &cancel).unwrap(), (5, 3));
    }

    #[test]
    fn vfs_remote_without_session() {
        let t = tempfile::tempdir().unwrap();
        let c = ctx(t.path());
        match list(&c, "sftp://nema/") {
            Err(ListError::Io(m)) => assert!(m.contains("connection lost"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(nearest(&c, "sftp://nema/a/b", t.path()), "sftp://nema/a/b");
    }

    #[test]
    fn disconnected_drops_session() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.fail_next("list", RemoteError::Disconnected);
        assert!(list(&c, "sftp://m/").is_err());
        assert!(c.sessions.get("m").is_none());
    }

    #[test]
    fn timeout_drops_session() {
        // A hung connection is treated as lost: Reconnect must really reconnect.
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.fail_next("list", RemoteError::Timeout);
        match list(&c, "sftp://m/") {
            Err(ListError::Io(msg)) => assert!(msg.ends_with("server not responding"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(c.sessions.get("m").is_none());
    }

    #[test]
    fn remote_delete_reports_item_errors_and_skips() {
        struct Skipper(Mutex<Vec<String>>);
        impl OpObserver for Skipper {
            fn progress(&self, _p: &Progress) {}
            fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
                ConflictAnswer::Overwrite
            }
            fn error(&self, path: &str, _msg: &str) -> ErrorAnswer {
                self.0.lock().unwrap().push(path.to_string());
                ErrorAnswer::Skip
            }
            fn is_cancelled(&self) -> bool {
                false
            }
        }
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/d/a", b"a", 0o644, 0);
        let obs = Skipper(Mutex::new(Vec::new()));
        // Nothing to remove under a missing path is not an error.
        let r = delete(&c, &["sftp://m/nema".to_string()], false, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 0));
        m.fail_next("remove_file", RemoteError::PermissionDenied("/d/a".into()));
        let r = delete(&c, &["sftp://m/d".to_string()], false, &obs).unwrap();
        assert_eq!((r.done, r.skipped), (0, 1));
        assert_eq!(obs.0.lock().unwrap().as_slice(), ["sftp://m/d/a"]);
        assert!(m.exists("/d/a") && m.exists("/d"));
    }

    #[test]
    fn list_routes_local_and_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let local = list(&c, &s(t.path())).unwrap();
        assert!(names(&local).contains(&"a.zip".to_string()));
        let inner = list(&c, &format!("{}/a.zip/d", s(t.path()))).unwrap();
        assert!(names(&inner).contains(&"b.txt".to_string()));
    }

    #[test]
    fn nearest_inside_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let home = t.path();
        let tp = s(t.path());
        assert_eq!(
            nearest(&c, &format!("{tp}/a.zip/d/nema"), home),
            format!("{tp}/a.zip/d")
        );
        assert_eq!(
            nearest(&c, &format!("{tp}/a.zip/nema/x"), home),
            format!("{tp}/a.zip")
        );
        assert_eq!(nearest(&c, &format!("{tp}/obrisano.zip/x"), home), tp);
    }

    #[test]
    fn mkdir_rename_delete_in_archive() {
        let t = tempfile::tempdir().unwrap();
        make_zip(
            &t.path().join("a.zip"),
            &[("a.txt", Some(b"a")), ("k.txt", Some(b"k"))],
        );
        let c = ctx(t.path());
        let tp = s(t.path());
        let n = mkdir(&c, &format!("{tp}/a.zip"), "n").unwrap();
        assert_eq!(n, format!("{tp}/a.zip/n"));
        let m = rename(&c, &n, "m").unwrap();
        assert_eq!(m, format!("{tp}/a.zip/m"));
        let r = delete(&c, &[m, format!("{tp}/a.zip/a.txt")], false, &NullObserver).unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(
            names(&list(&c, &format!("{tp}/a.zip")).unwrap()),
            vec!["k.txt"]
        );
    }

    #[test]
    fn delete_folder_counts_files_in_subtree() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        let r = delete(&c, &[format!("{tp}/a.zip/d")], true, &NullObserver).unwrap();
        assert_eq!(r.done, 2);
    }

    #[test]
    fn gz_mkdir_is_refused_with_message() {
        let t = tempfile::tempdir().unwrap();
        build(
            ArchiveKind::Gz,
            &t.path().join("x.gz"),
            &[("x.txt", Some(b"hello"))],
        );
        let c = ctx(t.path());
        let e = mkdir(&c, &format!("{}/x.gz", s(t.path())), "n").unwrap_err();
        assert!(e.contains("Not possible in a .gz file"), "{e}");
    }

    #[test]
    fn dir_size_in_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        let no = AtomicBool::new(false);
        assert_eq!(dir_size(&c, &format!("{tp}/a.zip/d"), &no).unwrap(), (6, 2));
        let own = dir_size(&c, &format!("{tp}/a.zip"), &no).unwrap();
        assert_eq!(own, ops::dir_size(&t.path().join("a.zip"), &no).unwrap());
        assert_eq!(own.1, 1);
        assert_eq!(
            dir_size(&c, &format!("{tp}/a.zip/a.txt"), &no).unwrap(),
            (3, 1)
        );
        assert_eq!(dir_size(&c, &tp, &no).unwrap().1, 1);
    }

    #[test]
    fn local_paths_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let c = ctx(t.path());
        let tp = s(t.path());
        assert_eq!(mkdir(&c, &tp, "x/y").unwrap(), format!("{tp}/x/y"));
        assert!(mkdir(&c, &tp, "x/y").is_err());
        fs::write(t.path().join("f"), b"1").unwrap();
        assert_eq!(
            rename(&c, &format!("{tp}/f"), "g").unwrap(),
            format!("{tp}/g")
        );
        assert!(rename(&c, &format!("{tp}/g"), "a/b").is_err());
        let r = delete(&c, &[format!("{tp}/g")], false, &NullObserver).unwrap();
        assert_eq!(r.done, 1);
        assert!(!t.path().join("g").exists());
    }

    #[test]
    fn local_dir_for_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let tp = s(t.path());
        let home = Path::new("/home/x");
        assert_eq!(local_dir_for(&format!("{tp}/a.zip/d"), home), t.path());
        assert_eq!(local_dir_for(&tp, home), t.path());
        assert_eq!(local_dir_for("sftp://m/var", home), home);
    }

    #[test]
    fn temp_dirs_and_cleanup_handle_locked_folders() {
        let t = tempfile::tempdir().unwrap();
        let c = ctx(t.path());
        let a = c.new_temp_dir("stage").unwrap();
        let b = c.new_temp_dir("stage").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(&c.temp_root) && a.is_dir());
        let locked = a.join("x/y");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("f"), b"1").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(a.join("x"), fs::Permissions::from_mode(0o500)).unwrap();
        c.cleanup_dir(&a);
        assert!(!a.exists());
        assert!(b.exists());
    }

    #[test]
    fn passwords_are_remembered_per_archive() {
        let c = Ctx::new(PathBuf::from("/nonexistent"));
        assert_eq!(c.password(Path::new("/a.zip")), None);
        c.set_password(Path::new("/a.zip"), "pw".into());
        assert_eq!(c.password(Path::new("/a.zip")).as_deref(), Some("pw"));
        assert_eq!(c.password(Path::new("/b.zip")), None);
    }

    #[test]
    fn upload_target_must_be_inside_an_existing_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let tp = s(t.path());
        let c = ctx(t.path());
        let a_zip = t.path().join("a.zip");
        assert_eq!(
            check_upload_target(&c, &format!("{tp}/a.zip/d/b.txt"), Some(&a_zip)),
            Ok(())
        );
        let gone = |loc: &str, archive: &str| {
            let exp = PathBuf::from(archive);
            let exp = (archive != format!("{tp}/x.txt")).then_some(exp);
            assert_eq!(
                check_upload_target(&c, loc, exp.as_deref()),
                Err(format!(
                    "The archive {archive} no longer exists; your edited copy is kept"
                ))
            );
        };
        // The archive file itself is not a target inside it.
        gone(&format!("{tp}/a.zip"), &format!("{tp}/a.zip"));
        fs::rename(t.path().join("a.zip"), t.path().join("b.zip")).unwrap();
        gone(&format!("{tp}/a.zip/d/b.txt"), &format!("{tp}/a.zip"));
        // A folder named like the archive, holding another archive.
        fs::create_dir_all(t.path().join("a.zip/d")).unwrap();
        fs::rename(t.path().join("b.zip"), t.path().join("a.zip/d/c.zip")).unwrap();
        gone(
            &format!("{tp}/a.zip/d/c.zip/d/b.txt"),
            &format!("{tp}/a.zip"),
        );
        gone(&format!("{tp}/x.txt"), &format!("{tp}/x.txt"));
    }

    #[test]
    fn upload_into_an_archive_inside_a_folder_named_like_an_archive() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("backup.zip")).unwrap();
        let real = t.path().join("backup.zip/real.tar");
        let mut b = tar::Builder::new(fs::File::create(&real).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(1);
        b.append_data(&mut h, "f.txt", &b"x"[..]).unwrap();
        b.into_inner().unwrap();
        let c = ctx(t.path());
        let loc = format!("{}/f.txt", s(&real));
        assert_eq!(check_upload_target(&c, &loc, Some(&real)), Ok(()));
        assert_eq!(check_upload_target(&c, &loc, None), Ok(()));
    }

    #[test]
    fn materialize_fails_when_the_file_is_not_extracted() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        let mut b = tar::Builder::new(fs::File::create(&p).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_mode(0o644);
        h.set_size(4);
        b.append_data(&mut h, "f", &b"data"[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Link);
        h.set_mode(0o644);
        h.set_size(0);
        b.append_link(&mut h, "h", "f").unwrap();
        b.into_inner().unwrap().flush().unwrap();
        let c = ctx(t.path());
        let e = materialize(&c, &format!("{}/h", s(&p)), "view").unwrap_err();
        assert_eq!(e, "unsupported entry type in archive: h");
        // Its temp folder is gone.
        assert_eq!(fs::read_dir(&c.temp_root).map_or(0, |d| d.count()), 0);
    }

    #[test]
    fn materialize_local_and_archive() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        assert_eq!(
            materialize(&c, &format!("{tp}/x.txt"), "view").unwrap(),
            t.path().join("x.txt")
        );
        // the archive file itself is a local item
        assert_eq!(
            materialize(&c, &format!("{tp}/a.zip"), "view").unwrap(),
            t.path().join("a.zip")
        );
        let p = materialize(&c, &format!("{tp}/a.zip/d/b.txt"), "view").unwrap();
        assert!(p.starts_with(c.temp_root.join("view-0")), "{p:?}");
        assert_eq!(fs::read(&p).unwrap(), b"bbbb");
        let e = materialize(&c, &format!("{tp}/a.zip/d"), "view").unwrap_err();
        assert_eq!(e, "Only files can be opened from an archive");
    }

    #[test]
    fn entry_stamp_of_archive_file() {
        let t = tempfile::tempdir().unwrap();
        zip_with_d(t.path());
        let c = ctx(t.path());
        let tp = s(t.path());
        let (size, _) = entry_stamp(&c, &format!("{tp}/a.zip/d/b.txt"))
            .unwrap()
            .unwrap();
        assert_eq!(size, 4);
        assert_eq!(
            entry_stamp(&c, &format!("{tp}/a.zip/d/nope")).unwrap(),
            None
        );
        assert_eq!(entry_stamp(&c, &format!("{tp}/x.txt")).unwrap(), None);
    }

    #[test]
    fn materialize_password_errors() {
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("p.zip");
        make_zip_aes(&z, &[("s.txt", Some(b"secret"))], "pw");
        let c = ctx(t.path());
        let loc = format!("{}/s.txt", s(&z));
        assert_eq!(
            materialize(&c, &loc, "view").unwrap_err(),
            format!("password required: {}", z.display())
        );
        c.set_password(&z, "bad".into());
        assert_eq!(
            materialize(&c, &loc, "view").unwrap_err(),
            format!("wrong password: {}", z.display())
        );
        c.set_password(&z, "pw".into());
        let p = materialize(&c, &loc, "view").unwrap();
        assert_eq!(fs::read(p).unwrap(), b"secret");
    }

    #[test]
    fn error_mapping() {
        let p = Path::new("/a.zip");
        assert_eq!(
            archive_op_error(ArchiveError::PasswordRequired, p),
            OpError::Io("password required: /a.zip".into())
        );
        assert_eq!(
            archive_op_error(ArchiveError::WrongPassword, p),
            OpError::Io("wrong password: /a.zip".into())
        );
        assert_eq!(
            archive_op_error(ArchiveError::Cancelled, p),
            OpError::Cancelled
        );
    }

    #[test]
    fn materialize_remote_file() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/x/f.php", b"<?php echo 1;", 0o644, 5);
        m.put_dir("/x/d");
        let p = materialize(&c, "sftp://m/x/f.php", "view").unwrap();
        assert_eq!(p.file_name().unwrap(), "f.php");
        assert!(p.parent().unwrap().starts_with(t.path().join("tmp-root")));
        assert_eq!(fs::read(&p).unwrap(), b"<?php echo 1;");
        let e = materialize(&c, "sftp://m/x/d", "view").unwrap_err();
        assert_eq!(e, "Only files can be opened from a server");
        assert!(materialize(&c, "sftp://m/x/none", "view").is_err());
        // Failures leave no temp folder behind.
        let left = fs::read_dir(t.path().join("tmp-root")).unwrap().count();
        assert_eq!(left, 1);
        m.fail_next("read_to", RemoteError::Disconnected);
        assert!(materialize(&c, "sftp://m/x/f.php", "view").is_err());
        assert_eq!(fs::read_dir(t.path().join("tmp-root")).unwrap().count(), 1);
        assert!(c.sessions.get("m").is_none());
    }

    #[test]
    fn entry_stamp_remote_changes_after_write() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/x/f", b"one", 0o644, 5);
        let a = entry_stamp(&c, "sftp://m/x/f").unwrap().unwrap();
        m.put_file("/x/f", b"three", 0o644, 9);
        let b = entry_stamp(&c, "sftp://m/x/f").unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(entry_stamp(&c, "sftp://m/x/none").unwrap(), None);
    }

    #[test]
    fn check_upload_target_needs_session() {
        let t = tempfile::tempdir().unwrap();
        let (c, _m) = remote_ctx(t.path());
        assert_eq!(check_upload_target(&c, "sftp://m/x/f", None), Ok(()));
        let e = check_upload_target(&c, "sftp://gone/x/f", None).unwrap_err();
        assert!(e.contains("your edited copy is kept"), "{e}");
        assert!(e.contains("Not connected to gone"), "{e}");
    }

    #[test]
    fn entry_stamp_follows_symlink_to_changed_target() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/x/real", b"one", 0o644, 5);
        m.put_symlink("/x/link", "real", 0o777, 5);
        let a = entry_stamp(&c, "sftp://m/x/link").unwrap().unwrap();
        assert_eq!(a, (3, 5));
        m.put_file("/x/real", b"three", 0o644, 9);
        let b = entry_stamp(&c, "sftp://m/x/link").unwrap().unwrap();
        assert_eq!(b, (5, 9));
    }

    #[test]
    fn remote_symlinked_folder_enter_and_delete() {
        // Enter on a link to a folder: open is refused with the text the UI recognizes, the
        // link lists as the folder, and deleting the link never touches the target.
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_file("/srv/www/a.txt", b"aaa", 0o644, 0);
        m.put_file("/srv/www/sub/b.txt", b"bb", 0o644, 0);
        m.put_dir("/var");
        m.put_symlink("/var/html", "/srv/www", 0o777, 0);
        let e = materialize(&c, "sftp://m/var/html", "view").unwrap_err();
        assert_eq!(e, "Only files can be opened from a server");
        let names: Vec<String> = list(&c, "sftp://m/var/html")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["sub".to_string(), "a.txt".to_string()]);
        let r = delete(&c, &["sftp://m/var/html".to_string()], true, &NullObserver).unwrap();
        assert_eq!(r.done, 1);
        assert!(!m.exists("/var/html"), "the link is gone");
        assert_eq!(m.data("/srv/www/a.txt").unwrap(), b"aaa");
        assert_eq!(m.data("/srv/www/sub/b.txt").unwrap(), b"bb");
    }

    #[test]
    fn materialize_symlink_to_folder_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let (c, m) = remote_ctx(t.path());
        m.put_dir("/x/d");
        m.put_symlink("/x/ld", "d", 0o777, 5);
        let e = materialize(&c, "sftp://m/x/ld", "view").unwrap_err();
        assert_eq!(e, "Only files can be opened from a server");
        assert_eq!(
            fs::read_dir(t.path().join("tmp-root"))
                .map(|r| r.count())
                .unwrap_or(0),
            0
        );
    }

    /// Names in `loc`, sorted, read with a context of its own (so from disk).
    fn fresh_names(t: &Path, loc: &str) -> Vec<String> {
        let c = Ctx::new(t.join("tmp-fresh"));
        let mut v = names(&list(&c, loc).unwrap());
        v.sort();
        v
    }

    #[test]
    fn nested_archive_browse_and_edit() {
        for (outer, inner) in [
            ("o.zip", "i.tar.gz"),
            ("o.tar", "i.7z"),
            ("o.7z", "i.zip"),
            ("o.tgz", "i.tar"),
        ] {
            let t = tempfile::tempdir().unwrap();
            let tp = s(t.path());
            make_nested(
                t.path(),
                outer,
                inner,
                &[("d/a.txt", Some(b"aaa")), ("b.txt", Some(b"bb"))],
            );
            let n = format!("{tp}/{outer}/docs/{inner}");
            let c = ctx(t.path());
            assert_eq!(
                names(&list(&c, &n).unwrap()),
                ["d", "b.txt"],
                "{outer}/{inner}"
            );
            assert_eq!(names(&list(&c, &format!("{n}/d")).unwrap()), ["a.txt"]);
            let p = materialize(&c, &format!("{n}/d/a.txt"), "view").unwrap();
            assert_eq!(fs::read(&p).unwrap(), b"aaa");
            // The nested archive itself stays an item of the outer one.
            let no = AtomicBool::new(false);
            assert_eq!(dir_size(&c, &n, &no).unwrap().1, 1);
            assert_eq!(dir_size(&c, &format!("{n}/d"), &no).unwrap(), (3, 1));
            assert_eq!(
                nearest(&c, &format!("{n}/d/gone"), t.path()),
                format!("{n}/d")
            );
            assert!(entry_stamp(&c, &format!("{n}/b.txt")).unwrap().is_some());
            assert_eq!(mkdir(&c, &n, "m").unwrap(), format!("{n}/m"));
            assert_eq!(
                rename(&c, &format!("{n}/b.txt"), "c.txt").unwrap(),
                format!("{n}/c.txt")
            );
            let r = delete(&c, &[format!("{n}/d/a.txt")], false, &NullObserver).unwrap();
            assert_eq!(r.done, 1);
            // Written back into the outer archive file.
            let got = fresh_names(t.path(), &n);
            assert!(
                got.contains(&"c.txt".into()) && got.contains(&"m".into()),
                "{got:?}"
            );
            assert!(!got.contains(&"b.txt".into()), "{got:?}");
            let d = Ctx::new(t.path().join("tmp-fresh"));
            assert!(list(&d, &format!("{n}/d")).map_or(true, |v| v.is_empty()));
            assert_eq!(
                fresh_names(t.path(), &format!("{tp}/{outer}/docs")),
                [inner, "x.txt"]
            );
        }
    }

    #[test]
    fn archive_two_levels_deep() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let mid = tempfile::tempdir().unwrap();
        make_nested(mid.path(), "b.tar", "c.zip", &[("f.txt", Some(b"f"))]);
        let data = fs::read(mid.path().join("b.tar")).unwrap();
        make_zip(&t.path().join("a.zip"), &[("x/b.tar", Some(&data))]);
        let loc = format!("{tp}/a.zip/x/b.tar/docs/c.zip");
        let c = ctx(t.path());
        assert_eq!(names(&list(&c, &loc).unwrap()), ["f.txt"]);
        let first = resolve(&c, &loc, true).unwrap().unwrap();
        assert!(first.is_nested());
        assert_eq!(first.shown, PathBuf::from(&loc));
        assert_eq!(first.inner, "");
        // The copies are reused while the outer archive is unchanged.
        assert_eq!(resolve(&c, &loc, true).unwrap().unwrap(), first);
        rename(&c, &format!("{loc}/f.txt"), "g.txt").unwrap();
        assert_eq!(fresh_names(t.path(), &loc), ["g.txt"]);
        assert_eq!(
            fresh_names(t.path(), &format!("{tp}/a.zip/x/b.tar/docs")),
            ["c.zip", "x.txt"]
        );
        // Changed by another program: taken out again.
        let other = tempfile::tempdir().unwrap();
        make_nested(other.path(), "b.tar", "c.zip", &[("new.txt", Some(b"n"))]);
        let data = fs::read(other.path().join("b.tar")).unwrap();
        make_zip(
            &t.path().join("a.zip"),
            &[("x/b.tar", Some(&data)), ("y", Some(b"y"))],
        );
        assert_eq!(names(&list(&c, &loc).unwrap()), ["new.txt"]);
    }

    #[test]
    fn failed_write_back_leaves_the_outer_archive_and_drops_the_copy() {
        // Root ignores the read-only folder (the write-back would succeed): nothing to show.
        // SAFETY: geteuid cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("ro");
        fs::create_dir(&dir).unwrap();
        make_nested(&dir, "o.zip", "i.zip", &[("b.txt", Some(b"b"))]);
        let n = format!("{}/o.zip/docs/i.zip", s(&dir));
        let c = ctx(t.path());
        assert_eq!(names(&list(&c, &n).unwrap()), ["b.txt"]);
        // The outer archive cannot be rewritten: its folder is read-only.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let res = rename(&c, &format!("{n}/b.txt"), "c.txt");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(res.is_err(), "{res:?}");
        assert_eq!(names(&list(&c, &n).unwrap()), ["b.txt"]);
        assert_eq!(fresh_names(t.path(), &n), ["b.txt"]);
    }

    #[test]
    fn nested_archive_passwords_are_per_shown_path() {
        let t = tempfile::tempdir().unwrap();
        let tp = s(t.path());
        let src = tempfile::tempdir().unwrap();
        make_zip_aes(
            &src.path().join("i.zip"),
            &[("s.txt", Some(b"secret"))],
            "pw",
        );
        let data = fs::read(src.path().join("i.zip")).unwrap();
        make_zip(&t.path().join("o.zip"), &[("i.zip", Some(&data))]);
        let n = format!("{tp}/o.zip/i.zip");
        let c = ctx(t.path());
        assert_eq!(names(&list(&c, &n).unwrap()), ["s.txt"]);
        let loc = format!("{n}/s.txt");
        assert_eq!(
            materialize(&c, &loc, "view").unwrap_err(),
            format!("password required: {n}")
        );
        c.set_password(Path::new(&n), "pw".into());
        let p = materialize(&c, &loc, "view").unwrap();
        assert_eq!(fs::read(p).unwrap(), b"secret");
        // An encrypted outer archive asks for its own password first.
        make_zip_aes(&t.path().join("p.zip"), &[("i.zip", Some(&data))], "outer");
        let n = format!("{tp}/p.zip/i.zip");
        assert!(matches!(list(&c, &n), Err(ListError::Io(_))));
        assert_eq!(
            materialize(&c, &format!("{n}/s.txt"), "view").unwrap_err(),
            format!("password required: {tp}/p.zip")
        );
    }
}
