//! File operations: copy, move, delete, mkdir, rename, dir_size.
//!
//! Safety rules: an existing target is only ever replaced atomically (data is
//! written to a temp file in the target directory and renamed over it), an
//! interrupted copy removes only what this operation created, and sources of a
//! cross-device move are deleted only after their copy completed.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use filetime::FileTime;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

const CHUNK: usize = 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub current: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub total_files: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    pub source: String,
    pub target: String,
    pub source_size: u64,
    /// Unix seconds.
    pub source_mtime: i64,
    pub target_size: u64,
    /// Unix seconds.
    pub target_mtime: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConflictAnswer {
    Overwrite,
    OverwriteAll,
    Skip,
    SkipAll,
    OverwriteAllOlder,
    Rename { new_name: String },
    Cancel,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ErrorAnswer {
    Retry,
    Skip,
    SkipAll,
    Cancel,
}

/// Callbacks of a running operation. Called from the operation's thread.
pub trait OpObserver: Send + Sync {
    fn progress(&self, p: &Progress);
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer;
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer;
    fn is_cancelled(&self) -> bool;
}

/// `done`/`skipped` count non-directory items (files and symlinks).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpSummary {
    pub done: u64,
    pub skipped: u64,
    pub cancelled: bool,
}

/// Errors that refuse an operation before it starts. Errors on individual
/// items go through [`OpObserver::error`] instead. Serialized as
/// `{"kind": "intoItself"|"sameLocation"|"exists"|"io"|"cancelled", "message": <Display>}`.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum OpError {
    #[error("cannot copy or move '{0}' into itself")]
    IntoItself(String),
    #[error("'{0}' is already in the target location")]
    SameLocation(String),
    /// The target of an operation that never replaces without asking (packing) exists.
    #[error("'{0}' already exists")]
    Exists(String),
    #[error("{0}")]
    Io(String),
    #[error("operation cancelled")]
    Cancelled,
}

impl Serialize for OpError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            OpError::IntoItself(_) => "intoItself",
            OpError::SameLocation(_) => "sameLocation",
            OpError::Io(_) => "io",
            OpError::Exists(_) => "exists",
            OpError::Cancelled => "cancelled",
        };
        let mut st = s.serialize_struct("OpError", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dest {
    /// Items keep their names inside this directory.
    IntoDir(PathBuf),
    /// A single item is copied/moved under this path.
    AsFile(PathBuf),
}

pub fn resolve_dest(sources: &[PathBuf], dest: &str, cwd: &Path) -> Dest {
    let path = if Path::new(dest).is_absolute() {
        PathBuf::from(dest)
    } else {
        cwd.join(dest)
    };
    if dest.ends_with('/') || path.is_dir() || sources.len() > 1 {
        Dest::IntoDir(path)
    } else {
        Dest::AsFile(path)
    }
}

pub fn copy(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError> {
    let dest = &resolve_parent_dirs(dest);
    let targets = validate(sources, dest, false)?;
    let mut ctx = Ctx::start(obs, sources, dest)?;
    for (src, dst) in sources.iter().zip(&targets) {
        if ctx.copy_item(src, dst).is_err() {
            ctx.summary.cancelled = true;
            break;
        }
    }
    Ok(ctx.summary)
}

pub fn move_(sources: &[PathBuf], dest: &Dest, obs: &dyn OpObserver) -> Result<OpSummary, OpError> {
    move_impl(sources, dest, obs, false)
}

/// `force_copy` treats every rename as cross-device (test seam for EXDEV).
fn move_impl(
    sources: &[PathBuf],
    dest: &Dest,
    obs: &dyn OpObserver,
    force_copy: bool,
) -> Result<OpSummary, OpError> {
    let dest = &resolve_parent_dirs(dest);
    let targets = validate(sources, dest, true)?;
    let mut ctx = Ctx::start(obs, sources, dest)?;
    ctx.moving = true;
    ctx.force_copy = force_copy;
    let mut stopped = false;
    for (src, dst) in sources.iter().zip(&targets) {
        if ctx.move_item(src, dst).is_err() {
            stopped = true;
            break;
        }
    }
    // Copied sources are deleted only when the copy phase ran to the end.
    if stopped || ctx.delete_copied_sources().is_err() {
        ctx.summary.cancelled = true;
    }
    Ok(ctx.summary)
}

pub fn delete(
    paths: &[PathBuf],
    to_trash: bool,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    let mut ctx = Ctx::new(obs, paths)?;
    ctx.emit();
    for p in paths {
        let r = if to_trash {
            ctx.trash_item(p)
        } else {
            ctx.delete_tree(p).map(|_| ())
        };
        if r.is_err() {
            ctx.summary.cancelled = true;
            break;
        }
    }
    Ok(ctx.summary)
}

/// Trash backend. On macOS NSFileManager instead of the default Finder
/// AppleScript: no Automation prompt, no Finder alerts, and failures (read-only
/// volume, network share) come back at once.
fn trash_context() -> trash::TrashContext {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx
}

/// Moves `p` to the Trash, as F8 does.
pub fn trash(p: &Path) -> Result<(), trash::Error> {
    trash_context().delete(p)
}

/// Creates `parent/name`; `name` may contain `/` to create a chain.
pub fn mkdir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let mut path = parent.to_path_buf();
    let mut any = false;
    for c in Path::new(name).components() {
        match c {
            Component::Normal(n) => {
                path.push(n);
                any = true;
            }
            Component::CurDir => {}
            _ => return Err(invalid_name(name)),
        }
    }
    if !any {
        return Err(invalid_name(name));
    }
    if fs::symlink_metadata(&path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("'{}' already exists", path.display()),
        ));
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Renames `path` within its directory. Never replaces an existing item.
pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    if !valid_name(new_name) {
        return Err(invalid_name(new_name));
    }
    let parent = path.parent().ok_or_else(|| invalid_name(new_name))?;
    let target = parent.join(new_name);
    let smd = fs::symlink_metadata(path)?;
    if let Ok(tmd) = fs::symlink_metadata(&target) {
        if !is_alias(path, &target, same_file(&smd, &tmd)) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("'{}' already exists", target.display()),
            ));
        }
        // Same file under another spelling (case-insensitive volume).
        fs::rename(path, &target)?;
        return Ok(target);
    }
    rename_noreplace(path, &target)?;
    Ok(target)
}

/// Total bytes of regular files and number of non-directory items under
/// `path`. Symlinks count as items of 0 bytes and are not followed.
/// Unreadable subdirectories are skipped. Returns `Interrupted` when cancelled.
pub fn dir_size(path: &Path, cancel: &AtomicBool) -> io::Result<(u64, u64)> {
    walk_size(path, &|| cancel.load(Ordering::Relaxed))
}

fn walk_size(path: &Path, cancel: &dyn Fn() -> bool) -> io::Result<(u64, u64)> {
    if cancel() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let md = fs::symlink_metadata(path)?;
    if !md.is_dir() {
        return Ok((if md.is_file() { md.len() } else { 0 }, 1));
    }
    let (mut bytes, mut files) = (0, 0);
    for entry in fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        match walk_size(&entry.path(), cancel) {
            Ok((b, f)) => {
                bytes += b;
                files += f;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
            Err(_) => {}
        }
    }
    Ok((bytes, files))
}

/// (files, bytes) of a tree, best effort.
fn count_tree(path: &Path) -> (u64, u64) {
    walk_size(path, &|| false)
        .map(|(b, f)| (f, b))
        .unwrap_or((0, 0))
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

fn invalid_name(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid name '{name}'"),
    )
}

fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether `b` is merely another spelling of item `a` (case or Unicode
/// normalization variant on an insensitive volume). The inode alone is not
/// enough: on FAT all empty files share one file number.
fn is_alias(a: &Path, b: &Path, same_inode: bool) -> bool {
    same_inode && alias_decision(&location(a), &location(b))
}

/// Same canonical parent and names equal after NFC + lowercase.
fn alias_decision(la: &Path, lb: &Path) -> bool {
    let fold = |s: &std::ffi::OsStr| s.to_string_lossy().nfc().collect::<String>().to_lowercase();
    la.parent() == lb.parent()
        && match (la.file_name(), lb.file_name()) {
            (Some(x), Some(y)) => fold(x) == fold(y),
            _ => false,
        }
}

fn mtime_of(md: &Metadata) -> FileTime {
    FileTime::from_last_modification_time(md)
}

fn is_exdev(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::EXDEV)
}

/// Canonical absolute path of `p`. Relative paths are taken against the
/// current directory; the missing tail is normalized lexically (`.`/`..`).
fn canon_lenient(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let comps: Vec<Component> = abs.components().collect();
    for i in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..i].iter().collect();
        if let Ok(base) = prefix.canonicalize() {
            return push_lexical(base, &comps[i..]);
        }
    }
    push_lexical(PathBuf::new(), &comps)
}

fn push_lexical(mut base: PathBuf, comps: &[Component]) -> PathBuf {
    for c in comps {
        match c {
            Component::ParentDir => {
                base.pop();
            }
            Component::CurDir => {}
            c => base.push(c),
        }
    }
    base
}

/// Location of the item itself (a final symlink is not followed).
pub(crate) fn location(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => canon_lenient(parent).join(name),
        _ => canon_lenient(&abs),
    }
}

fn validate(sources: &[PathBuf], dest: &Dest, moving: bool) -> Result<Vec<PathBuf>, OpError> {
    if matches!(dest, Dest::AsFile(_)) && sources.len() > 1 {
        return Err(OpError::Io(
            "multiple items need a directory as destination".into(),
        ));
    }
    let mut targets = Vec::with_capacity(sources.len());
    for src in sources {
        let shown = src.to_string_lossy().into_owned();
        let name = src
            .file_name()
            .ok_or_else(|| OpError::Io(format!("invalid source '{shown}'")))?;
        let target = match dest {
            Dest::IntoDir(d) => d.join(name),
            Dest::AsFile(p) => p.clone(),
        };
        let (ls, lt) = (location(src), location(&target));
        if ls == lt {
            return Err(OpError::SameLocation(shown));
        }
        let smd = fs::symlink_metadata(src).ok();
        if let (Some(s), Ok(t)) = (&smd, fs::symlink_metadata(&target)) {
            // Moving to another spelling of the same name is a plain rename.
            if !moving && is_alias(src, &target, same_file(s, &t)) {
                return Err(OpError::SameLocation(shown));
            }
        }
        if smd.as_ref().is_some_and(|m| m.is_dir()) && lt.starts_with(&ls) {
            return Err(OpError::IntoItself(shown));
        }
        targets.push(target);
    }
    Ok(targets)
}

/// A destination with `..` is replaced by its canonical form, so that
/// creating it does not create the directories the `..` steps out of.
fn resolve_parent_dirs(dest: &Dest) -> Dest {
    let has_parent = |p: &Path| p.components().any(|c| c == Component::ParentDir);
    match dest {
        Dest::IntoDir(d) if has_parent(d) => Dest::IntoDir(canon_lenient(d)),
        Dest::AsFile(p) if has_parent(p) => Dest::AsFile(location(p)),
        d => d.clone(),
    }
}

fn prepare_dest(dest: &Dest) -> Result<(), OpError> {
    let dir = match dest {
        Dest::IntoDir(d) => d.as_path(),
        Dest::AsFile(p) => match p.parent() {
            Some(d) => d,
            None => return Ok(()),
        },
    };
    fs::create_dir_all(dir).map_err(|e| OpError::Io(format!("{}: {e}", dir.display())))
}

/// Flushes `f` to stable storage as far as its file system allows. Rust's `sync_all` is
/// `F_FULLFSYNC` on macOS, which SMB shares (and some other file systems) reject with
/// ENOTSUP; then a plain `fsync` is used, and a file system without either is taken as
/// flushed (the data was written and the close reports write errors).
pub fn sync_file(f: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    sync_with(
        || f.sync_all(),
        || {
            // SAFETY: the descriptor belongs to `f`, which outlives this call.
            if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        },
    )
}

/// `sync_file`'s fallback order: `full`, then `plain` when the file system does not
/// support `full`, then success when it supports neither.
fn sync_with(
    full: impl FnOnce() -> io::Result<()>,
    plain: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let unsupported = |e: &io::Error| {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOTTY) | Some(libc::ENOSYS)
        )
    };
    match full() {
        Err(e) if unsupported(&e) => match plain() {
            Err(e) if unsupported(&e) => Ok(()),
            r => r,
        },
        r => r,
    }
}

/// Rename that fails with `AlreadyExists` instead of replacing `to`.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let f = CString::new(from.as_os_str().as_bytes())?;
        let t = CString::new(to.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        // SAFETY: both arguments are valid NUL-terminated strings.
        let rc = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        // SAFETY: both arguments are valid NUL-terminated strings.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                f.as_ptr(),
                libc::AT_FDCWD,
                t.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            // Not supported by this file system: fall back below.
            Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS) => {}
            _ => return Err(e),
        }
    }
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("'{}' already exists", to.display()),
        ));
    }
    fs::rename(from, to)
}

fn temp_path(dir: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".fxfm-{}-{n}.tmp", std::process::id()))
}

/// Creates a new temp item next to `dst` via `make`, retrying on name clashes.
fn with_temp<T>(
    dst: &Path,
    mut make: impl FnMut(&Path) -> io::Result<T>,
) -> io::Result<(PathBuf, T)> {
    let dir = dst.parent().unwrap_or(Path::new("."));
    let mut tries = 0;
    loop {
        let tmp = temp_path(dir);
        match make(&tmp) {
            Ok(v) => return Ok((tmp, v)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
            Err(e) => return Err(e),
        }
    }
}

fn read_dir_sorted(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

pub(crate) fn type_clash(dst: &Path, src_is_dir: bool) -> String {
    if src_is_dir {
        format!("Cannot overwrite file '{}' with a directory", dst.display())
    } else {
        format!("Cannot overwrite directory '{}' with a file", dst.display())
    }
}

/// The user cancelled.
struct Stop;

/// How a conflict is resolved.
pub(crate) enum Decision {
    Overwrite,
    Skip,
    Rename(String),
}

/// Conflict answers remembered for the whole operation (the "all" answers). Shared by
/// local, archive and server transfers.
#[derive(Default)]
pub(crate) struct Conflicts {
    overwrite_all: bool,
    skip_all: bool,
    overwrite_older: bool,
}

impl Conflicts {
    /// Resolves one conflict; `older` tells whether the target is older than the source
    /// (for "Overwrite all older"), and `info` is built only when the user is asked.
    /// `None` when the user cancels.
    pub(crate) fn decide(
        &mut self,
        obs: &dyn OpObserver,
        older: bool,
        info: impl FnOnce() -> ConflictInfo,
    ) -> Option<Decision> {
        let by_age = || {
            if older {
                Decision::Overwrite
            } else {
                Decision::Skip
            }
        };
        if self.overwrite_all {
            return Some(Decision::Overwrite);
        }
        if self.skip_all {
            return Some(Decision::Skip);
        }
        if self.overwrite_older {
            return Some(by_age());
        }
        Some(match obs.conflict(&info()) {
            ConflictAnswer::Overwrite => Decision::Overwrite,
            ConflictAnswer::OverwriteAll => {
                self.overwrite_all = true;
                Decision::Overwrite
            }
            ConflictAnswer::Skip => Decision::Skip,
            ConflictAnswer::SkipAll => {
                self.skip_all = true;
                Decision::Skip
            }
            ConflictAnswer::OverwriteAllOlder => {
                self.overwrite_older = true;
                by_age()
            }
            ConflictAnswer::Rename { new_name } => Decision::Rename(new_name),
            ConflictAnswer::Cancel => return None,
        })
    }
}

enum LeafError {
    Stop,
    Io(PathBuf, io::Error),
}

struct Ctx<'a> {
    obs: &'a dyn OpObserver,
    progress: Progress,
    summary: OpSummary,
    conflicts: Conflicts,
    skip_all_errors: bool,
    moving: bool,
    force_copy: bool,
    /// Sources copied completely during a cross-device move, to delete at the end.
    copied: Vec<PathBuf>,
    /// (dev, ino) of target directories of this operation; never used as a
    /// source, so a missed into-itself case cannot recurse forever.
    target_dirs: HashSet<(u64, u64)>,
}

impl<'a> Ctx<'a> {
    fn new(obs: &'a dyn OpObserver, sources: &[PathBuf]) -> Result<Self, OpError> {
        let mut progress = Progress::default();
        let cancel = || obs.is_cancelled();
        for s in sources {
            match walk_size(s, &cancel) {
                Ok((b, f)) => {
                    progress.total_bytes += b;
                    progress.total_files += f;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(OpError::Cancelled),
                Err(_) => {}
            }
        }
        Ok(Ctx {
            obs,
            progress,
            summary: OpSummary::default(),
            conflicts: Conflicts::default(),
            skip_all_errors: false,
            moving: false,
            force_copy: false,
            copied: Vec::new(),
            target_dirs: HashSet::new(),
        })
    }

    fn start(obs: &'a dyn OpObserver, sources: &[PathBuf], dest: &Dest) -> Result<Self, OpError> {
        let ctx = Ctx::new(obs, sources)?;
        prepare_dest(dest)?;
        ctx.emit();
        Ok(ctx)
    }

    fn emit(&self) {
        self.obs.progress(&self.progress);
    }

    fn check_cancel(&self) -> Result<(), Stop> {
        if self.obs.is_cancelled() {
            Err(Stop)
        } else {
            Ok(())
        }
    }

    fn set_current(&mut self, p: &Path) {
        self.progress.current = p.to_string_lossy().into_owned();
    }

    /// Ok(true) = retry, Ok(false) = skip.
    fn on_error(&mut self, path: &Path, msg: &str) -> Result<bool, Stop> {
        if self.skip_all_errors {
            return Ok(false);
        }
        match self.obs.error(&path.to_string_lossy(), msg) {
            ErrorAnswer::Retry => Ok(true),
            ErrorAnswer::Skip => Ok(false),
            ErrorAnswer::SkipAll => {
                self.skip_all_errors = true;
                Ok(false)
            }
            ErrorAnswer::Cancel => Err(Stop),
        }
    }

    /// Runs `f` until it succeeds or the user skips (`None`).
    fn attempt<T>(
        &mut self,
        path: &Path,
        mut f: impl FnMut() -> io::Result<T>,
    ) -> Result<Option<T>, Stop> {
        loop {
            match f() {
                Ok(v) => return Ok(Some(v)),
                Err(e) => {
                    if !self.on_error(path, &e.to_string())? {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Records a skipped item covering `files` files of `bytes` bytes.
    fn skip_item(&mut self, files: u64, bytes: u64) {
        self.summary.skipped += files.max(1);
        self.progress.done_files += files;
        self.progress.done_bytes += bytes;
        self.emit();
    }

    fn skip_tree(&mut self, path: &Path) {
        let (f, b) = count_tree(path);
        self.skip_item(f, b);
    }

    fn file_done(&mut self, bytes_already_counted: bool, bytes: u64) {
        self.summary.done += 1;
        self.progress.done_files += 1;
        if !bytes_already_counted {
            self.progress.done_bytes += bytes;
        }
        self.emit();
    }

    fn decide(
        &mut self,
        src: &Path,
        smd: &Metadata,
        dst: &Path,
        dmd: &Metadata,
    ) -> Result<Decision, Stop> {
        let older = mtime_of(dmd) < mtime_of(smd);
        self.conflicts
            .decide(self.obs, older, || ConflictInfo {
                source: src.to_string_lossy().into_owned(),
                target: dst.to_string_lossy().into_owned(),
                source_size: smd.len(),
                source_mtime: smd.mtime(),
                target_size: dmd.len(),
                target_mtime: dmd.mtime(),
            })
            .ok_or(Stop)
    }

    /// Resolves a conflict of a non-directory `src` at `dst`. Returns the final
    /// target and whether it replaces an existing item, or `None` if skipped.
    fn resolve_leaf_target(
        &mut self,
        src: &Path,
        smd: &Metadata,
        mut dst: PathBuf,
    ) -> Result<Option<(PathBuf, bool)>, Stop> {
        loop {
            let dmd = match fs::symlink_metadata(&dst) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Some((dst, false))),
                Err(e) => {
                    if self.on_error(&dst, &e.to_string())? {
                        continue;
                    }
                    return Ok(None);
                }
            };
            if dmd.is_dir() {
                if self.on_error(&dst, &type_clash(&dst, false))? {
                    continue;
                }
                return Ok(None);
            }
            if is_alias(src, &dst, same_file(smd, &dmd)) {
                if self.on_error(&dst, "Source and target are the same file")? {
                    continue;
                }
                return Ok(None);
            }
            match self.decide(src, smd, &dst, &dmd)? {
                Decision::Overwrite => return Ok(Some((dst, true))),
                Decision::Skip => return Ok(None),
                Decision::Rename(n) => {
                    // An invalid name simply asks again for the same target.
                    if valid_name(&n) {
                        dst.set_file_name(n);
                    }
                }
            }
        }
    }

    /// Copies `src` to `dst`. Ok(true) if the item was copied completely.
    fn copy_item(&mut self, src: &Path, dst: &Path) -> Result<bool, Stop> {
        self.check_cancel()?;
        self.set_current(src);
        let Some(smd) = self.attempt(src, || fs::symlink_metadata(src))? else {
            self.skip_item(1, 0);
            return Ok(false);
        };
        if smd.is_dir() {
            self.copy_dir(src, &smd, dst)
        } else {
            self.copy_leaf(src, &smd, dst.to_path_buf(), None)
        }
    }

    fn copy_dir(&mut self, src: &Path, smd: &Metadata, dst: &Path) -> Result<bool, Stop> {
        while self.target_dirs.contains(&(smd.dev(), smd.ino())) {
            if !self.on_error(src, "Cannot copy a directory into itself")? {
                self.skip_item(0, 0);
                return Ok(false);
            }
        }
        let mut created = false;
        loop {
            match fs::symlink_metadata(dst) {
                Ok(dmd) if dmd.is_dir() => {
                    // merge into the existing directory
                    self.target_dirs.insert((dmd.dev(), dmd.ino()));
                    break;
                }
                Ok(_) => {
                    if self.on_error(dst, &type_clash(dst, true))? {
                        continue;
                    }
                    self.skip_tree(src);
                    return Ok(false);
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(dst) {
                    Ok(()) => {
                        created = true;
                        if let Ok(dmd) = fs::symlink_metadata(dst) {
                            self.target_dirs.insert((dmd.dev(), dmd.ino()));
                        }
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        if self.on_error(dst, &e.to_string())? {
                            continue;
                        }
                        self.skip_tree(src);
                        return Ok(false);
                    }
                },
                Err(e) => {
                    if self.on_error(dst, &e.to_string())? {
                        continue;
                    }
                    self.skip_tree(src);
                    return Ok(false);
                }
            }
        }
        let start = self.copied.len();
        let complete = match self.attempt(src, || read_dir_sorted(src))? {
            Some(names) => {
                let mut complete = true;
                for name in names {
                    complete &= self.copy_item(&src.join(&name), &dst.join(&name))?;
                }
                complete
            }
            None => {
                self.skip_tree(src);
                false
            }
        };
        if created {
            // After the children: they change the mtime, and a read-only mode
            // would prevent creating them.
            let mode = smd.mode() & 0o1777;
            if let Err(e) = fs::set_permissions(dst, fs::Permissions::from_mode(mode))
                .and_then(|_| filetime::set_file_mtime(dst, mtime_of(smd)))
            {
                log::warn!("cannot set attributes of {}: {e}", dst.display());
            }
        }
        if complete && self.moving {
            self.copied.truncate(start);
            self.copied.push(src.to_path_buf());
        }
        Ok(complete)
    }

    /// Copies a non-directory item. `decided` is `Some(overwrite)` when the
    /// conflict at `dst` was already resolved by the caller.
    fn copy_leaf(
        &mut self,
        src: &Path,
        smd: &Metadata,
        dst: PathBuf,
        mut decided: Option<bool>,
    ) -> Result<bool, Stop> {
        let size = if smd.is_file() { smd.len() } else { 0 };
        let mut dst = dst;
        let mut races = 0;
        loop {
            let (target, overwrite) = match decided.take() {
                Some(overwrite) => (dst.clone(), overwrite),
                None => match self.resolve_leaf_target(src, smd, dst.clone())? {
                    Some(t) => t,
                    None => {
                        self.skip_item(1, size);
                        return Ok(false);
                    }
                },
            };
            let base = self.progress.done_bytes;
            match self.write_leaf(src, smd, &target, overwrite) {
                Ok(()) => {
                    // Align with the counted size in case the file changed meanwhile.
                    self.progress.done_bytes = base + size;
                    self.file_done(true, 0);
                    if self.moving {
                        self.copied.push(src.to_path_buf());
                    }
                    return Ok(true);
                }
                Err(LeafError::Stop) => return Err(Stop),
                Err(LeafError::Io(path, e)) => {
                    self.progress.done_bytes = base;
                    if e.kind() == io::ErrorKind::AlreadyExists && !overwrite && races < 3 {
                        // Target appeared meanwhile: resolve the conflict again.
                        races += 1;
                        dst = target;
                        continue;
                    }
                    if self.on_error(&path, &e.to_string())? {
                        // Retry keeps the conflict decision.
                        dst = target;
                        decided = Some(overwrite);
                        continue;
                    }
                    self.skip_item(1, size);
                    return Ok(false);
                }
            }
        }
    }

    /// Writes one non-directory item. With `overwrite`, the data goes to a temp
    /// file that replaces `dst` only once complete.
    fn write_leaf(
        &mut self,
        src: &Path,
        smd: &Metadata,
        dst: &Path,
        overwrite: bool,
    ) -> Result<(), LeafError> {
        let io_err = |p: &Path| {
            let p = p.to_path_buf();
            move |e: io::Error| LeafError::Io(p, e)
        };
        if smd.file_type().is_symlink() {
            let link = fs::read_link(src).map_err(io_err(src))?;
            let make = |p: &Path| {
                symlink(&link, p)?;
                let t = FileTime::from_last_modification_time(smd);
                if let Err(e) = filetime::set_symlink_file_times(p, t, t) {
                    log::warn!("cannot set times of {}: {e}", p.display());
                }
                Ok(())
            };
            if !overwrite {
                return make(dst).map_err(io_err(dst));
            }
            let (tmp, ()) = with_temp(dst, make).map_err(io_err(dst))?;
            return fs::rename(&tmp, dst).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                LeafError::Io(dst.to_path_buf(), e)
            });
        }
        if !smd.is_file() {
            return Err(LeafError::Io(
                src.to_path_buf(),
                io::Error::new(io::ErrorKind::Unsupported, "Unsupported file type"),
            ));
        }
        let mut input = File::open(src).map_err(io_err(src))?;
        let imd = input.metadata().map_err(io_err(src))?;
        let create = |p: &Path| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(p)
        };
        let (out_path, mut out) = if overwrite {
            with_temp(dst, create).map_err(io_err(dst))?
        } else {
            (dst.to_path_buf(), create(dst).map_err(io_err(dst))?)
        };
        // From here on, `out_path` is ours and is removed on any failure.
        let result = self.pump(src, &mut input, &mut out, &out_path, &imd, overwrite);
        let result = result.and_then(|()| {
            if overwrite {
                fs::rename(&out_path, dst).map_err(io_err(dst))
            } else {
                Ok(())
            }
        });
        if result.is_err() {
            drop(out);
            let _ = fs::remove_file(&out_path);
        }
        result
    }

    fn pump(
        &mut self,
        src: &Path,
        input: &mut File,
        out: &mut File,
        out_path: &Path,
        imd: &Metadata,
        overwrite: bool,
    ) -> Result<(), LeafError> {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(LeafError::Io(src.to_path_buf(), e)),
            };
            out.write_all(&buf[..n])
                .map_err(|e| LeafError::Io(out_path.to_path_buf(), e))?;
            self.progress.done_bytes += n as u64;
            self.emit();
            if self.obs.is_cancelled() {
                return Err(LeafError::Stop);
            }
        }
        // Some file systems (e.g. SMB, exFAT) reject attributes; the data is
        // what matters, so these failures are only logged.
        let atime = FileTime::from_last_access_time(imd);
        if let Err(e) = out
            .set_permissions(fs::Permissions::from_mode(imd.mode() & 0o1777))
            .and_then(|()| filetime::set_file_handle_times(out, Some(atime), Some(mtime_of(imd))))
        {
            log::warn!("cannot set attributes of {}: {e}", out_path.display());
        }
        if self.moving || overwrite {
            // The copy replaces the target, or the source is deleted later:
            // make sure the data is on disk first.
            sync_file(out).map_err(|e| LeafError::Io(out_path.to_path_buf(), e))?;
        }
        Ok(())
    }

    /// Moves `src` to `dst`. Ok(true) if nothing of `src` was left behind
    /// (except items already scheduled in `copied`).
    fn move_item(&mut self, src: &Path, dst: &Path) -> Result<bool, Stop> {
        self.check_cancel()?;
        self.set_current(src);
        let Some(smd) = self.attempt(src, || fs::symlink_metadata(src))? else {
            self.skip_item(1, 0);
            return Ok(false);
        };
        let mut dst = dst.to_path_buf();
        let mut races = 0;
        loop {
            let overwrite = match fs::symlink_metadata(&dst) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => {
                    if self.on_error(&dst, &e.to_string())? {
                        continue;
                    }
                    self.skip_tree(src);
                    return Ok(false);
                }
                Ok(dmd) if is_alias(src, &dst, same_file(&smd, &dmd)) => {
                    // Another spelling of the same name: a plain rename.
                    let (files, bytes) = count_tree(src);
                    return match self.attempt(src, || fs::rename(src, &dst))? {
                        Some(()) => {
                            self.renamed(files, bytes);
                            Ok(true)
                        }
                        None => {
                            self.skip_item(files, bytes);
                            Ok(false)
                        }
                    };
                }
                Ok(dmd) if smd.is_dir() && dmd.is_dir() => return self.move_merge_dir(src, &dst),
                Ok(dmd) if smd.is_dir() != dmd.is_dir() => {
                    if self.on_error(&dst, &type_clash(&dst, smd.is_dir()))? {
                        continue;
                    }
                    self.skip_tree(src);
                    return Ok(false);
                }
                Ok(dmd) => match self.decide(src, &smd, &dst, &dmd)? {
                    Decision::Overwrite => true,
                    Decision::Skip => {
                        self.skip_item(1, if smd.is_file() { smd.len() } else { 0 });
                        return Ok(false);
                    }
                    Decision::Rename(n) => {
                        if valid_name(&n) {
                            dst.set_file_name(n);
                        }
                        continue;
                    }
                },
            };
            let r = if self.force_copy {
                Err(io::Error::from_raw_os_error(libc::EXDEV))
            } else if overwrite {
                fs::rename(src, &dst)
            } else {
                rename_noreplace(src, &dst)
            };
            match r {
                Ok(()) => {
                    let (files, bytes) = if smd.is_dir() {
                        count_tree(&dst)
                    } else {
                        (1, if smd.is_file() { smd.len() } else { 0 })
                    };
                    self.renamed(files, bytes);
                    return Ok(true);
                }
                Err(e) if is_exdev(&e) => {
                    return if smd.is_dir() {
                        self.copy_dir(src, &smd, &dst)
                    } else {
                        self.copy_leaf(src, &smd, dst, Some(overwrite))
                    };
                }
                // Target appeared meanwhile: resolve the conflict again.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && races < 3 => {
                    races += 1;
                    continue;
                }
                Err(e) => {
                    if self.on_error(src, &e.to_string())? {
                        continue;
                    }
                    self.skip_tree(src);
                    return Ok(false);
                }
            }
        }
    }

    fn renamed(&mut self, files: u64, bytes: u64) {
        self.summary.done += files;
        self.progress.done_files += files;
        self.progress.done_bytes += bytes;
        self.emit();
    }

    fn move_merge_dir(&mut self, src: &Path, dst: &Path) -> Result<bool, Stop> {
        let start = self.copied.len();
        let Some(names) = self.attempt(src, || read_dir_sorted(src))? else {
            self.skip_tree(src);
            return Ok(false);
        };
        let mut complete = true;
        for name in names {
            complete &= self.move_item(&src.join(&name), &dst.join(&name))?;
        }
        if complete {
            // Whatever is left in `src` was copied completely.
            self.copied.truncate(start);
            self.copied.push(src.to_path_buf());
        }
        Ok(complete)
    }

    fn delete_copied_sources(&mut self) -> Result<(), Stop> {
        for p in std::mem::take(&mut self.copied) {
            self.check_cancel()?;
            self.set_current(&p);
            self.attempt(&p, || match fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => fs::remove_dir_all(&p),
                Ok(_) => fs::remove_file(&p),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            })?;
        }
        Ok(())
    }

    fn trash_item(&mut self, p: &Path) -> Result<(), Stop> {
        self.check_cancel()?;
        self.set_current(p);
        let (files, bytes) = count_tree(p);
        loop {
            match trash(p) {
                Ok(()) => {
                    self.renamed(files, bytes);
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("trash {}: {e}", p.display());
                    if !self.on_error(p, "Cannot move to Trash")? {
                        self.skip_item(files, bytes);
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Deletes `p` recursively without following symlinks. Ok(true) if gone.
    fn delete_tree(&mut self, p: &Path) -> Result<bool, Stop> {
        self.check_cancel()?;
        self.set_current(p);
        let Some(md) = self.attempt(p, || fs::symlink_metadata(p))? else {
            self.skip_item(1, 0);
            return Ok(false);
        };
        if md.is_dir() {
            let Some(names) = self.attempt(p, || read_dir_sorted(p))? else {
                self.skip_tree(p);
                return Ok(false);
            };
            let mut complete = true;
            for name in names {
                complete &= self.delete_tree(&p.join(name))?;
            }
            // A directory with skipped children stays.
            if !complete {
                return Ok(false);
            }
            if self.attempt(p, || fs::remove_dir(p))?.is_none() {
                self.skip_item(0, 0);
                return Ok(false);
            }
            return Ok(true);
        }
        let size = if md.is_file() { md.len() } else { 0 };
        if self.attempt(p, || fs::remove_file(p))?.is_none() {
            self.skip_item(1, size);
            return Ok(false);
        }
        self.file_done(false, size);
        Ok(true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use filetime::FileTime;
    use std::collections::VecDeque;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestObserver {
        conflict_answers: Mutex<VecDeque<ConflictAnswer>>,
        conflicts: Mutex<Vec<ConflictInfo>>,
        error_answers: Mutex<VecDeque<ErrorAnswer>>,
        errors: Mutex<Vec<(String, String)>>,
        progress: Mutex<Vec<Progress>>,
        /// Cancel once this many progress calls have been seen (0 = never).
        cancel_after: usize,
        progress_count: AtomicUsize,
    }

    impl TestObserver {
        fn with_conflicts(answers: Vec<ConflictAnswer>) -> Self {
            TestObserver {
                conflict_answers: Mutex::new(answers.into()),
                ..Default::default()
            }
        }
        fn with_errors(answers: Vec<ErrorAnswer>) -> Self {
            TestObserver {
                error_answers: Mutex::new(answers.into()),
                ..Default::default()
            }
        }
        fn conflict_count(&self) -> usize {
            self.conflicts.lock().unwrap().len()
        }
    }

    impl OpObserver for TestObserver {
        fn progress(&self, p: &Progress) {
            self.progress_count.fetch_add(1, Ordering::SeqCst);
            self.progress.lock().unwrap().push(p.clone());
        }
        fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
            self.conflicts.lock().unwrap().push(c.clone());
            self.conflict_answers
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
                .unwrap_or_else(|| panic!("unexpected error() call: {path}: {msg}"))
        }
        fn is_cancelled(&self) -> bool {
            self.cancel_after > 0 && self.progress_count.load(Ordering::SeqCst) >= self.cancel_after
        }
    }

    fn write(p: &Path, content: &str) {
        fs::write(p, content).unwrap();
    }
    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }
    fn set_mtime(p: &Path, secs: i64) {
        filetime::set_file_mtime(p, FileTime::from_unix_time(secs, 0)).unwrap();
    }
    fn mtime(p: &Path) -> i64 {
        FileTime::from_last_modification_time(&fs::symlink_metadata(p).unwrap()).unix_seconds()
    }
    fn mode(p: &Path) -> u32 {
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }
    fn chmod(p: &Path, m: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    }
    fn exists(p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok()
    }

    #[test]
    fn copy_dir_recursive_preserves_mtime_and_mode() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir_all(a.join("sub")).unwrap();
        write(&a.join("f1.txt"), "one");
        write(&a.join("sub/f2.sh"), "two");
        chmod(&a.join("f1.txt"), 0o640);
        chmod(&a.join("sub/f2.sh"), 0o755);
        chmod(&a.join("sub"), 0o750);
        set_mtime(&a.join("f1.txt"), 1_000_000_000);
        set_mtime(&a.join("sub/f2.sh"), 1_100_000_000);
        set_mtime(&a.join("sub"), 1_200_000_000);
        let dst = t.path().join("dst");
        fs::create_dir(&dst).unwrap();

        let obs = TestObserver::default();
        let s = copy(std::slice::from_ref(&a), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(
            s,
            OpSummary {
                done: 2,
                skipped: 0,
                cancelled: false
            }
        );

        let c = dst.join("a");
        assert_eq!(read(&c.join("f1.txt")), "one");
        assert_eq!(read(&c.join("sub/f2.sh")), "two");
        assert_eq!(mode(&c.join("f1.txt")), 0o640);
        assert_eq!(mode(&c.join("sub/f2.sh")), 0o755);
        assert_eq!(mode(&c.join("sub")), 0o750);
        assert_eq!(mtime(&c.join("f1.txt")), 1_000_000_000);
        assert_eq!(mtime(&c.join("sub/f2.sh")), 1_100_000_000);
        assert_eq!(mtime(&c.join("sub")), 1_200_000_000);
        // source untouched
        assert_eq!(read(&a.join("f1.txt")), "one");
    }

    #[test]
    fn copy_into_own_subdir_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir(&a).unwrap();
        write(&a.join("f"), "x");
        let obs = TestObserver::default();
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(a.join("b")), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        assert!(!exists(&a.join("b")));
        // also directly into itself
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(a.clone()), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = move_(std::slice::from_ref(&a), &Dest::IntoDir(a.join("b")), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        assert_eq!(fs::read_dir(&a).unwrap().count(), 1);
    }

    #[test]
    fn trash_failure_is_reported_promptly() {
        // A file in a read-only folder cannot be moved to the Trash; the
        // failure must come back through obs.error() at once (no Finder hang).
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("ro");
        fs::create_dir(&dir).unwrap();
        let f = dir.join("f.txt");
        write(&f, "x");
        // Root ignores the permissions (the file would really be trashed): skip.
        if fs::metadata(&f).unwrap().uid() == 0 {
            return;
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip]);
        let t0 = std::time::Instant::now();
        let s = delete(std::slice::from_ref(&f), true, &obs).unwrap();
        let elapsed = t0.elapsed();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
        let errors = obs.errors.lock().unwrap().clone();
        assert_eq!(
            errors,
            vec![(
                f.to_string_lossy().into_owned(),
                "Cannot move to Trash".to_string()
            )]
        );
        assert_eq!(s.skipped, 1);
        assert!(f.exists());
    }

    #[test]
    fn into_itself_detected_for_relative_paths() {
        // cargo runs unit tests with the package root as the current directory
        // and a single-component relative source (empty parent) is the tricky case
        let t = tempfile::tempdir_in(".").unwrap();
        let a = PathBuf::from(t.path().file_name().unwrap());
        write(&a.join("f"), "x");
        let obs = TestObserver::default();
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(a.join("b")), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = move_(std::slice::from_ref(&a), &Dest::IntoDir(a.join("b")), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(a.clone()), &obs);
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(".".into()), &obs);
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        assert!(!exists(&a.join("b")));
    }

    #[test]
    fn into_itself_detected_through_symlinked_dest() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        let a = real.join("A");
        fs::create_dir_all(&a).unwrap();
        let link = t.path().join("link"); // like /tmp -> /private/tmp
        symlink(&real, &link).unwrap();
        let obs = TestObserver::default();
        for dest in [link.join("A/new"), link.join("A/new/../x"), link.join("A")] {
            let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(dest.clone()), &obs);
            assert!(matches!(r, Err(OpError::IntoItself(_))), "{dest:?}: {r:?}");
        }
        // ".." in the missing tail leading out of the source is fine
        let ok = link.join("A/new/../../out");
        let r = copy(std::slice::from_ref(&a), &Dest::IntoDir(ok), &obs);
        assert!(r.is_ok(), "{r:?}");
        assert!(real.join("out/A").is_dir());
        assert!(!exists(&a.join("new")));
    }

    /// True if the temp volume ignores case (default APFS).
    fn case_insensitive(dir: &Path) -> bool {
        write(&dir.join("probe"), "");
        let r = exists(&dir.join("PROBE"));
        fs::remove_file(dir.join("probe")).unwrap();
        r
    }

    #[test]
    fn case_variant_paths_on_case_insensitive_volume() {
        let t = tempfile::tempdir().unwrap();
        if !case_insensitive(t.path()) {
            return;
        }
        let a = t.path().join("Dir");
        fs::create_dir(&a).unwrap();
        write(&a.join("f"), "x");
        let obs = TestObserver::default();
        let r = copy(
            std::slice::from_ref(&a),
            &Dest::IntoDir(t.path().join("dir/sub")),
            &obs,
        );
        assert!(matches!(r, Err(OpError::IntoItself(_))), "{r:?}");
        let r = copy(
            std::slice::from_ref(&a),
            &Dest::AsFile(t.path().join("DIR")),
            &obs,
        );
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        // a move to another spelling is a case-only rename
        let s = move_(
            std::slice::from_ref(&a),
            &Dest::AsFile(t.path().join("dir")),
            &obs,
        )
        .unwrap();
        assert_eq!(s.done, 1);
        let names: Vec<_> = fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["dir"]);
        assert_eq!(read(&t.path().join("dir/f")), "x");
        assert_eq!(obs.conflict_count(), 0);
        // rename() also allows a case-only change
        let p = rename(&t.path().join("dir"), "DIR").unwrap();
        assert_eq!(p, t.path().join("DIR"));
    }

    #[test]
    fn alias_needs_same_parent_and_folded_name() {
        let d = Path::new("/v/d");
        assert!(alias_decision(&d.join("Foo"), &d.join("foo")));
        assert!(alias_decision(&d.join("caf\u{e9}"), &d.join("CAFE\u{301}")));
        // FAT: distinct empty files share an inode number
        assert!(!alias_decision(&d.join("a"), &d.join("b")));
        assert!(!alias_decision(&d.join("a"), Path::new("/v/e/a")));
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a"), "");
        assert!(!is_alias(&t.path().join("a"), &t.path().join("a2"), false));
    }

    #[test]
    fn copy_dir_guard_stops_recursion_into_own_target() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("A");
        fs::create_dir_all(a.join("sub")).unwrap();
        write(&a.join("f"), "x");
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip]);
        // bypasses validation, as a missed into-itself case would
        let mut ctx = Ctx::new(&obs, std::slice::from_ref(&a)).unwrap();
        let r = ctx.copy_item(&a, &a.join("sub/A"));
        assert!(matches!(r, Ok(false)));
        let errors = obs.errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("into itself"));
        assert!(!exists(&a.join("sub/A/sub/A")));
    }

    #[test]
    fn move_to_same_dir_is_noop_error() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f.txt");
        write(&f, "keep");
        let obs = TestObserver::default();
        let r = move_(
            std::slice::from_ref(&f),
            &Dest::IntoDir(t.path().to_path_buf()),
            &obs,
        );
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        let r = copy(
            std::slice::from_ref(&f),
            &Dest::IntoDir(t.path().to_path_buf()),
            &obs,
        );
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        let r = copy(std::slice::from_ref(&f), &Dest::AsFile(f.clone()), &obs);
        assert!(matches!(r, Err(OpError::SameLocation(_))), "{r:?}");
        assert_eq!(read(&f), "keep");
        assert_eq!(obs.conflict_count(), 0);
    }

    fn conflict_setup(t: &Path) -> (PathBuf, PathBuf) {
        let src = t.join("src");
        let dst = t.join("dst");
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dst).unwrap();
        write(&src.join("x"), "new-x");
        write(&src.join("y"), "new-y");
        write(&dst.join("x"), "old-x");
        write(&dst.join("y"), "old-y");
        (src, dst)
    }

    fn srcs(src: &Path) -> Vec<PathBuf> {
        vec![src.join("x"), src.join("y")]
    }

    #[test]
    fn conflict_overwrite_all_asks_once() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::OverwriteAll]);
        let s = copy(&srcs(&src), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(obs.conflict_count(), 1);
        assert_eq!(s.done, 2);
        assert_eq!(read(&dst.join("x")), "new-x");
        assert_eq!(read(&dst.join("y")), "new-y");
        let c = &obs.conflicts.lock().unwrap()[0];
        assert_eq!(c.source, src.join("x").to_string_lossy());
        assert_eq!(c.target, dst.join("x").to_string_lossy());
        assert_eq!((c.source_size, c.target_size), (5, 5));
        // no temp files left behind
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2);
    }

    #[test]
    fn conflict_skip_keeps_target() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs =
            TestObserver::with_conflicts(vec![ConflictAnswer::Skip, ConflictAnswer::Overwrite]);
        let s = copy(&srcs(&src), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(obs.conflict_count(), 2);
        assert_eq!((s.done, s.skipped), (1, 1));
        assert_eq!(read(&dst.join("x")), "old-x");
        assert_eq!(read(&dst.join("y")), "new-y");
    }

    #[test]
    fn conflict_skip_all_keeps_targets_and_sources_on_move() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::SkipAll]);
        let s = move_(&srcs(&src), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(obs.conflict_count(), 1);
        assert_eq!((s.done, s.skipped), (0, 2));
        assert_eq!(read(&dst.join("x")), "old-x");
        assert_eq!(read(&src.join("x")), "new-x");
        assert_eq!(read(&src.join("y")), "new-y");
    }

    #[test]
    fn conflict_overwrite_all_older() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        set_mtime(&src.join("x"), 2_000_000_000);
        set_mtime(&src.join("y"), 2_000_000_000);
        set_mtime(&dst.join("x"), 1_000_000_000); // older -> overwritten
        set_mtime(&dst.join("y"), 2_100_000_000); // newer -> kept
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::OverwriteAllOlder]);
        let s = copy(&srcs(&src), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(obs.conflict_count(), 1);
        assert_eq!((s.done, s.skipped), (1, 1));
        assert_eq!(read(&dst.join("x")), "new-x");
        assert_eq!(read(&dst.join("y")), "old-y");
    }

    #[test]
    fn conflict_rename_uses_new_name() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Rename {
            new_name: "x2".into(),
        }]);
        let s = copy(&[src.join("x")], &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(s.done, 1);
        assert_eq!(read(&dst.join("x")), "old-x");
        assert_eq!(read(&dst.join("x2")), "new-x");
    }

    #[test]
    fn conflict_rename_to_existing_name_asks_again() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs = TestObserver::with_conflicts(vec![
            ConflictAnswer::Rename {
                new_name: "y".into(),
            },
            ConflictAnswer::Cancel,
        ]);
        let s = copy(&[src.join("x")], &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert!(s.cancelled);
        assert_eq!(obs.conflict_count(), 2);
        assert_eq!(read(&dst.join("x")), "old-x");
        assert_eq!(read(&dst.join("y")), "old-y");
    }

    #[test]
    fn conflict_dir_merges_into_existing_dir() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let dst = t.path().join("dst");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(dst.join("a/sub")).unwrap();
        write(&a.join("sub/new"), "n");
        write(&a.join("sub/both"), "src");
        write(&dst.join("a/sub/old"), "o");
        write(&dst.join("a/sub/both"), "dst");
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Skip]);
        let s = copy(std::slice::from_ref(&a), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!((s.done, s.skipped), (1, 1));
        assert_eq!(read(&dst.join("a/sub/new")), "n");
        assert_eq!(read(&dst.join("a/sub/old")), "o");
        assert_eq!(read(&dst.join("a/sub/both")), "dst");
    }

    #[test]
    fn type_clash_is_reported_as_error() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        let dst = t.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(dst.join("x")).unwrap();
        write(&dst.join("x/inside"), "keep");
        write(&src.join("x"), "file");
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip]);
        let s = copy(&[src.join("x")], &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!((s.done, s.skipped), (0, 1));
        assert_eq!(obs.errors.lock().unwrap().len(), 1);
        assert_eq!(obs.conflict_count(), 0);
        assert_eq!(read(&dst.join("x/inside")), "keep");
        // and the move keeps its source
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip]);
        move_(&[src.join("x")], &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(read(&src.join("x")), "file");
        assert_eq!(read(&dst.join("x/inside")), "keep");
    }

    #[test]
    fn symlink_copied_as_link() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir(&a).unwrap();
        write(&a.join("f"), "data");
        symlink("f", a.join("link")).unwrap();
        symlink(&a, a.join("loop")).unwrap();
        let dst = t.path().join("dst");
        let obs = TestObserver::default();
        let s = copy(std::slice::from_ref(&a), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(s.done, 3);
        let c = dst.join("a");
        assert!(fs::symlink_metadata(c.join("link"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(c.join("link")).unwrap(), Path::new("f"));
        assert!(fs::symlink_metadata(c.join("loop"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(c.join("loop")).unwrap(), a);
    }

    #[test]
    fn cancel_mid_file_removes_partial() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("big.bin");
        fs::write(&f, vec![7u8; 5 * 1024 * 1024]).unwrap();
        let dst = t.path().join("dst");
        fs::create_dir(&dst).unwrap();
        // progress #1 is the initial totals, #2 follows the first chunk
        let obs = TestObserver {
            cancel_after: 2,
            ..Default::default()
        };
        let s = copy(std::slice::from_ref(&f), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert!(s.cancelled);
        assert_eq!(s.done, 0);
        // exactly one chunk was copied before the cancel took effect
        let p = obs.progress.lock().unwrap();
        assert_eq!(p.last().unwrap().done_bytes, 1024 * 1024);
        assert!(!exists(&dst.join("big.bin")));
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);
        assert_eq!(fs::metadata(&f).unwrap().len(), 5 * 1024 * 1024);
    }

    #[test]
    fn cancel_during_overwrite_keeps_target() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("big.bin");
        fs::write(&f, vec![7u8; 5 * 1024 * 1024]).unwrap();
        let dst = t.path().join("dst");
        fs::create_dir(&dst).unwrap();
        write(&dst.join("big.bin"), "precious");
        let obs = TestObserver {
            cancel_after: 2,
            conflict_answers: Mutex::new(vec![ConflictAnswer::Overwrite].into()),
            ..Default::default()
        };
        let s = copy(std::slice::from_ref(&f), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert!(s.cancelled);
        assert_eq!(read(&dst.join("big.bin")), "precious");
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 1);
    }

    #[test]
    fn progress_totals_counted_upfront() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir_all(a.join("sub")).unwrap();
        write(&a.join("one"), "1");
        write(&a.join("sub/two"), "22");
        write(&a.join("sub/three"), "333");
        let dst = t.path().join("dst");
        let obs = TestObserver::default();
        copy(std::slice::from_ref(&a), &Dest::IntoDir(dst), &obs).unwrap();
        let p = obs.progress.lock().unwrap();
        assert!(!p.is_empty());
        assert!(p.iter().all(|p| p.total_files == 3 && p.total_bytes == 6));
        let last = p.last().unwrap();
        assert_eq!((last.done_files, last.done_bytes), (3, 6));
    }

    #[test]
    fn error_skip_continues_and_skip_all_stops_asking() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        fs::create_dir(&src).unwrap();
        for n in ["a", "b", "c"] {
            write(&src.join(n), n);
            chmod(&src.join(n), 0o000);
        }
        write(&src.join("ok"), "ok");
        if fs::File::open(src.join("a")).is_ok() {
            return; // running as root; permissions are not enforced
        }
        let dst = t.path().join("dst");
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip, ErrorAnswer::SkipAll]);
        let s = copy(
            std::slice::from_ref(&src),
            &Dest::IntoDir(dst.clone()),
            &obs,
        )
        .unwrap();
        assert_eq!(obs.errors.lock().unwrap().len(), 2);
        assert_eq!((s.done, s.skipped), (1, 3));
        assert_eq!(read(&dst.join("src/ok")), "ok");
        assert!(!exists(&dst.join("src/a")));
    }

    #[test]
    fn error_retry_tries_again() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        write(&f, "data");
        chmod(&f, 0o000);
        if fs::File::open(&f).is_ok() {
            return; // running as root
        }
        let dst = t.path().join("dst");
        struct FixOnError(PathBuf, Mutex<u32>);
        impl OpObserver for FixOnError {
            fn progress(&self, _: &Progress) {}
            fn conflict(&self, _: &ConflictInfo) -> ConflictAnswer {
                panic!("no conflict expected")
            }
            fn error(&self, _: &str, _: &str) -> ErrorAnswer {
                *self.1.lock().unwrap() += 1;
                fs::set_permissions(&self.0, fs::Permissions::from_mode(0o644)).unwrap();
                ErrorAnswer::Retry
            }
            fn is_cancelled(&self) -> bool {
                false
            }
        }
        let obs = FixOnError(f.clone(), Mutex::new(0));
        let s = copy(std::slice::from_ref(&f), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(*obs.1.lock().unwrap(), 1);
        assert_eq!(s.done, 1);
        assert_eq!(read(&dst.join("f")), "data");
    }

    #[test]
    fn sync_falls_back_when_full_sync_is_unsupported() {
        let err = |n| -> io::Result<()> { Err(io::Error::from_raw_os_error(n)) };
        // SMB: F_FULLFSYNC is ENOTSUP, fsync works.
        let mut plain_ran = false;
        assert!(sync_with(
            || err(libc::ENOTSUP),
            || {
                plain_ran = true;
                Ok(())
            }
        )
        .is_ok());
        assert!(plain_ran);
        // Neither supported: the written data stands.
        assert!(sync_with(|| err(libc::ENOTSUP), || err(libc::EINVAL)).is_ok());
        // A real failure is reported, from either step.
        assert_eq!(
            sync_with(|| err(libc::EIO), || Ok(()))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EIO)
        );
        assert_eq!(
            sync_with(|| err(libc::ENOTSUP), || err(libc::EIO))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EIO)
        );
        // Full sync works: plain is not needed.
        assert!(sync_with(|| Ok(()), || panic!("plain fsync not expected")).is_ok());
    }

    #[test]
    fn sync_file_works_on_a_local_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = File::create(tmp.path().join("x")).unwrap();
        sync_file(&f).unwrap();
    }

    #[test]
    fn move_same_device_renames() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir(&a).unwrap();
        write(&a.join("f"), "x");
        let dst = t.path().join("dst");
        fs::create_dir(&dst).unwrap();
        let obs = TestObserver::default();
        let s = move_(std::slice::from_ref(&a), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!(s.done, 1);
        assert!(!exists(&a));
        assert_eq!(read(&dst.join("a/f")), "x");
        // single file under a new name
        let s = move_(
            &[dst.join("a/f")],
            &Dest::AsFile(t.path().join("renamed")),
            &obs,
        )
        .unwrap();
        assert_eq!(s.done, 1);
        assert_eq!(read(&t.path().join("renamed")), "x");
    }

    #[test]
    fn move_dir_merge_keeps_skipped_in_source() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let dst = t.path().join("dst");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(dst.join("a")).unwrap();
        write(&a.join("new"), "n");
        write(&a.join("both"), "src");
        write(&dst.join("a/both"), "dst");
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Skip]);
        let s = move_(std::slice::from_ref(&a), &Dest::IntoDir(dst.clone()), &obs).unwrap();
        assert_eq!((s.done, s.skipped), (1, 1));
        assert_eq!(read(&dst.join("a/new")), "n");
        assert_eq!(read(&dst.join("a/both")), "dst");
        assert_eq!(read(&a.join("both")), "src");
        assert!(!exists(&a.join("new")));
    }

    #[test]
    fn move_cross_device_copies_then_deletes_only_completed() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let dst = t.path().join("dst");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(dst.join("a/sub")).unwrap();
        write(&a.join("top"), "t");
        write(&a.join("sub/new"), "n");
        write(&a.join("sub/both"), "src");
        write(&dst.join("a/sub/both"), "dst");
        symlink("top", a.join("link")).unwrap();
        set_mtime(&a.join("top"), 1_000_000_000);
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Skip]);
        let s = move_impl(
            std::slice::from_ref(&a),
            &Dest::IntoDir(dst.clone()),
            &obs,
            true,
        )
        .unwrap();
        assert_eq!((s.done, s.skipped), (3, 1));
        assert_eq!(read(&dst.join("a/top")), "t");
        assert_eq!(mtime(&dst.join("a/top")), 1_000_000_000);
        assert_eq!(read(&dst.join("a/sub/new")), "n");
        assert_eq!(read(&dst.join("a/sub/both")), "dst");
        assert_eq!(fs::read_link(dst.join("a/link")).unwrap(), Path::new("top"));
        // completed sources removed, skipped one (and its parents) kept
        assert!(!exists(&a.join("top")));
        assert!(!exists(&a.join("link")));
        assert!(!exists(&a.join("sub/new")));
        assert_eq!(read(&a.join("sub/both")), "src");

        // a fully copied tree is removed completely
        let b = t.path().join("b");
        fs::create_dir_all(b.join("deep")).unwrap();
        write(&b.join("deep/f"), "f");
        let obs = TestObserver::default();
        let s = move_impl(
            std::slice::from_ref(&b),
            &Dest::IntoDir(dst.clone()),
            &obs,
            true,
        )
        .unwrap();
        assert_eq!(s.done, 1);
        assert!(!exists(&b));
        assert_eq!(read(&dst.join("b/deep/f")), "f");
    }

    #[test]
    fn move_cross_device_cancel_keeps_all_sources() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir(&a).unwrap();
        write(&a.join("small"), "s");
        fs::write(a.join("zbig"), vec![1u8; 3 * 1024 * 1024]).unwrap();
        let dst = t.path().join("dst");
        // progress: initial, "small" chunk, "small" done, first "zbig" chunk
        let obs = TestObserver {
            cancel_after: 4,
            ..Default::default()
        };
        let s = move_impl(
            std::slice::from_ref(&a),
            &Dest::IntoDir(dst.clone()),
            &obs,
            true,
        )
        .unwrap();
        assert!(s.cancelled);
        assert_eq!(read(&dst.join("a/small")), "s");
        assert!(!exists(&dst.join("a/zbig")));
        assert_eq!(read(&a.join("small")), "s");
        assert_eq!(fs::metadata(a.join("zbig")).unwrap().len(), 3 * 1024 * 1024);
    }

    #[test]
    fn move_cross_device_conflict_cancel_keeps_all_sources() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        fs::remove_file(dst.join("x")).unwrap();
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Cancel]);
        let s = move_impl(&srcs(&src), &Dest::IntoDir(dst.clone()), &obs, true).unwrap();
        assert!(s.cancelled);
        assert_eq!(read(&dst.join("x")), "new-x");
        assert_eq!(read(&src.join("x")), "new-x");
        assert_eq!(read(&src.join("y")), "new-y");
    }

    #[test]
    fn move_cross_device_overwrite_asks_once() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = conflict_setup(t.path());
        let obs = TestObserver::with_conflicts(vec![ConflictAnswer::Overwrite]);
        let s = move_impl(
            std::slice::from_ref(&src.join("x")),
            &Dest::IntoDir(dst.clone()),
            &obs,
            true,
        )
        .unwrap();
        assert_eq!(obs.conflict_count(), 1);
        assert_eq!(s.done, 1);
        assert_eq!(read(&dst.join("x")), "new-x");
        assert!(!exists(&src.join("x")));
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2);
    }

    #[test]
    fn move_cross_device_error_skip_keeps_failed_source() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::create_dir(&a).unwrap();
        write(&a.join("bad"), "bad");
        write(&a.join("good"), "good");
        chmod(&a.join("bad"), 0o000);
        if fs::File::open(a.join("bad")).is_ok() {
            return; // running as root
        }
        let dst = t.path().join("dst");
        let obs = TestObserver::with_errors(vec![ErrorAnswer::Skip]);
        let s = move_impl(
            std::slice::from_ref(&a),
            &Dest::IntoDir(dst.clone()),
            &obs,
            true,
        )
        .unwrap();
        assert_eq!((s.done, s.skipped), (1, 1));
        assert_eq!(read(&dst.join("a/good")), "good");
        assert!(!exists(&a.join("good")));
        chmod(&a.join("bad"), 0o644);
        assert_eq!(read(&a.join("bad")), "bad");
    }

    #[test]
    fn resolve_dest_rules() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path();
        let one = vec![cwd.join("a.txt")];
        let two = vec![cwd.join("a.txt"), cwd.join("b.txt")];
        assert_eq!(
            resolve_dest(&one, "/x/novo.txt", cwd),
            Dest::AsFile(PathBuf::from("/x/novo.txt"))
        );
        assert_eq!(
            resolve_dest(&one, "/x/", cwd),
            Dest::IntoDir(PathBuf::from("/x/"))
        );
        assert_eq!(
            resolve_dest(&two, "/x/novo", cwd),
            Dest::IntoDir(PathBuf::from("/x/novo"))
        );
        fs::create_dir(cwd.join("existing")).unwrap();
        assert_eq!(
            resolve_dest(&one, "existing", cwd),
            Dest::IntoDir(cwd.join("existing"))
        );
        assert_eq!(
            resolve_dest(&one, "rel.txt", cwd),
            Dest::AsFile(cwd.join("rel.txt"))
        );
    }

    #[test]
    fn mkdir_nested() {
        let t = tempfile::tempdir().unwrap();
        let p = mkdir(t.path(), "a/b/c").unwrap();
        assert_eq!(p, t.path().join("a/b/c"));
        assert!(p.is_dir());
        let p = mkdir(t.path(), "single").unwrap();
        assert!(p.is_dir());
        assert_eq!(
            mkdir(t.path(), "single").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            mkdir(t.path(), "").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            mkdir(t.path(), "/abs").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            mkdir(t.path(), "a/../x").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn rename_rejects_existing_and_slash() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        write(&f, "f");
        write(&t.path().join("g"), "g");
        assert_eq!(
            rename(&f, "a/b").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            rename(&f, "").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            rename(&f, "..").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            rename(&f, "g").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(read(&t.path().join("g")), "g");
        let p = rename(&f, "h").unwrap();
        assert_eq!(p, t.path().join("h"));
        assert_eq!(read(&p), "f");
        assert!(!exists(&f));
    }

    #[test]
    fn dir_size_sums_recursive() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("d");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("a"), [0u8; 10]).unwrap();
        fs::write(d.join("sub/b"), [0u8; 20]).unwrap();
        fs::write(t.path().join("big"), [0u8; 1000]).unwrap();
        symlink(t.path().join("big"), d.join("link")).unwrap();
        symlink(t.path(), d.join("dirlink")).unwrap();
        let cancel = AtomicBool::new(false);
        // symlinks count as files of 0 bytes; their targets are not followed
        assert_eq!(dir_size(&d, &cancel).unwrap(), (30, 4));
        cancel.store(true, Ordering::SeqCst);
        assert_eq!(
            dir_size(&d, &cancel).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn delete_permanent_recursive() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("d");
        fs::create_dir_all(d.join("sub/deeper")).unwrap();
        write(&d.join("a"), "a");
        write(&d.join("sub/deeper/b"), "b");
        let outside = t.path().join("outside");
        fs::create_dir(&outside).unwrap();
        write(&outside.join("keep"), "keep");
        symlink(&outside, d.join("sub/link")).unwrap();
        let f = t.path().join("file");
        write(&f, "f");
        let obs = TestObserver::default();
        let s = delete(&[d.clone(), f.clone()], false, &obs).unwrap();
        assert_eq!(
            s,
            OpSummary {
                done: 4,
                skipped: 0,
                cancelled: false
            }
        );
        assert!(!exists(&d));
        assert!(!exists(&f));
        assert_eq!(read(&outside.join("keep")), "keep");
        let last = obs.progress.lock().unwrap().last().cloned().unwrap();
        assert_eq!((last.done_files, last.total_files), (4, 4));
    }

    #[test]
    #[ignore = "touches the user's Trash"]
    fn delete_to_trash() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("fxfm-trash-test.txt");
        write(&f, "x");
        let obs = TestObserver::default();
        let s = delete(std::slice::from_ref(&f), true, &obs).unwrap();
        assert_eq!(s.done, 1);
        assert!(!exists(&f));
    }

    #[test]
    fn serde_shapes() {
        let j = serde_json::to_value(ConflictAnswer::Rename {
            new_name: "x".into(),
        })
        .unwrap();
        assert_eq!(j, serde_json::json!({"kind": "rename", "newName": "x"}));
        let a: ConflictAnswer = serde_json::from_str(r#"{"kind":"overwriteAllOlder"}"#).unwrap();
        assert_eq!(a, ConflictAnswer::OverwriteAllOlder);
        let e: ErrorAnswer = serde_json::from_str(r#"{"kind":"skipAll"}"#).unwrap();
        assert_eq!(e, ErrorAnswer::SkipAll);
        let p = serde_json::to_value(Progress {
            current: "c".into(),
            done_bytes: 1,
            total_bytes: 2,
            done_files: 3,
            total_files: 4,
        })
        .unwrap();
        assert_eq!(p["doneBytes"], 1);
        assert_eq!(p["totalFiles"], 4);
        let err = serde_json::to_value(OpError::IntoItself("/a".into())).unwrap();
        assert_eq!(err["kind"], "intoItself");
        assert!(err["message"].as_str().unwrap().contains("/a"));
    }
}
