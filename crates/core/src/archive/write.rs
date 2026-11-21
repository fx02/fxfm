//! Atomic archive rewrite: add, delete, rename and new folder.
//!
//! Edits are applied in order to a virtual list of entries, then the archive is written once
//! to a hidden temp file next to it (`.<name>.fxfm-<pid>-<n>.tmp`). The temp is synced, given
//! the original's permissions and renamed over the original, and the parent folder is synced.
//! On any error or cancel the temp is removed and the original stays byte-identical.
//!
//! - Zip: unchanged entries are raw-copied without recompression, new entries are Deflate.
//! - Tar and TarGz: a new archive is streamed from the old one; TarGz uses gzip's default level.
//! - Gz: only replacing the content of its one file, or renaming it (only the header changes).
//! - 7z: entries cannot be raw-copied. The archive is extracted to a hidden staging folder
//!   next to it (`.<name>.fxfm-<pid>-<n>.stage/`) and the whole tree is compressed again, so
//!   writing into a 7z is much slower than into the other formats.
//!
//! Rewrites of one archive are serialized within the process, and the rename is refused when
//! the original is no longer the file the edits were planned from (changed by another program
//! meanwhile). Temp files and staging folders left next to the archive by a process that is
//! gone (crash, kill, power loss) are removed at the start of the next rewrite.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
use crate::process::pid_alive;

use super::read::{extract, io_err, sevenz_err, zip_err};
use super::{check_name, ArchiveError, ArchiveIndex, ArchiveKind, IndexCache, NameCheck, S_IFDIR};

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Adds a local file, link or whole folder as `inner`, replacing what is there.
    Add { src: PathBuf, inner: String },
    /// Deletes an entry; a folder goes with its whole subtree.
    Delete { inner: String },
    /// Renames or moves an entry; a folder moves with its whole subtree.
    Rename { from: String, to: String },
    /// Creates an empty folder.
    Mkdir { inner: String },
}

const GZ_ONLY: &str = "Not possible in a .gz file (it holds exactly one file)";
const ENCRYPTED: &str = "Writing into encrypted archives is not supported";
const CHUNK: u64 = 1 << 20;

/// Applies `edits` in order and writes the archive once, atomically. Conflicts are resolved
/// by the caller beforehand: `Add` replaces whatever has the same name.
pub fn rewrite(
    path: &Path,
    kind: ArchiveKind,
    edits: &[Edit],
    cache: &IndexCache,
    obs: &dyn OpObserver,
) -> Result<(), ArchiveError> {
    // Write next to the real file, so a symlink to the archive stays a symlink.
    let real = fs::canonicalize(path).map_err(|e| io_err(&e, path))?;
    let lock = archive_lock(&real);
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    sweep_leftovers(&real);
    // The plan is read from this handle; `commit` checks that the path still names it.
    let file = File::open(&real).map_err(|e| io_err(&e, path))?;
    let orig = Original {
        id: FileId::of(&file.metadata().map_err(|e| io_err(&e, path))?),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
    };
    let index = cache.get(path, kind)?;
    if index.entries.iter().any(|e| e.encrypted) {
        return Err(ArchiveError::Unsupported(ENCRYPTED.into()));
    }
    let res = match kind {
        ArchiveKind::Gz => {
            let op = plan_gz(&index, edits)?;
            commit(&real, &orig, obs, |out| {
                write_gz(&file, &real, &op, out, obs)
            })
        }
        ArchiveKind::Zip => {
            let plan = Plan::new(zip_items(&file, &real)?).apply_all(edits)?;
            let mut prog = Prog::new(obs, &plan.items, 0);
            commit(&real, &orig, obs, |out| {
                write_zip(&file, &real, &plan.items, out, &mut prog)
            })
        }
        ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 | ArchiveKind::TarXz => {
            let (items, hard_links) = tar_items(open_tar(&file, &real, kind)?)?;
            let plan = Plan { items, hard_links }.apply_all(edits)?;
            let mut prog = Prog::new(obs, &plan.items, 0);
            commit(&real, &orig, obs, |out| {
                write_tar(
                    open_tar(&file, &real, kind)?,
                    &plan.items,
                    out,
                    kind,
                    &mut prog,
                )
            })
        }
        ArchiveKind::SevenZ => rewrite_7z(&real, &orig, &index, edits, obs),
    };
    if res.is_ok() {
        cache.invalidate(path);
        cache.invalidate(&real);
    }
    res
}

// ---- virtual entry list ----

#[derive(Clone, Debug)]
enum Src {
    /// Entry number `ord` of the original archive, originally named `name`.
    Orig {
        ord: usize,
        name: String,
    },
    Local(PathBuf),
    NewDir,
}

#[derive(Clone, Debug)]
struct Item {
    /// Normalized inner path; the raw name for `fixed` items.
    path: String,
    /// An original entry whose name is unsafe or empty: copied as it is, never addressed.
    fixed: bool,
    src: Src,
    is_dir: bool,
    is_symlink: bool,
    /// Content bytes written for this item (compressed size for zip raw copies).
    size: u64,
    /// Including the file type bits.
    mode: u32,
    mtime: i64,
}

impl Item {
    fn renamed(&self) -> bool {
        matches!(&self.src, Src::Orig { name, .. } if !self.fixed && *name != self.path)
    }
}

fn norm(inner: &str) -> Result<String, ArchiveError> {
    match check_name(inner) {
        NameCheck::Ok(n) => Ok(n),
        _ => Err(ArchiveError::Io(format!(
            "invalid name in archive: {inner:?}"
        ))),
    }
}

/// True if `p` is `inner` or below it.
fn under(p: &str, inner: &str) -> bool {
    p.starts_with(inner) && (p.len() == inner.len() || p.as_bytes()[inner.len()] == b'/')
}

fn src_err(src: &Path, e: &io::Error) -> ArchiveError {
    ArchiveError::Io(format!("{}: {e}", src.display()))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn new_dir(path: String) -> Item {
    Item {
        path,
        fixed: false,
        src: Src::NewDir,
        is_dir: true,
        is_symlink: false,
        size: 0,
        mode: S_IFDIR | 0o755,
        mtime: now(),
    }
}

struct Plan {
    items: Vec<Item>,
    /// Tar hard links: original entry number -> normalized target.
    hard_links: HashMap<usize, String>,
}

impl Plan {
    fn new(items: Vec<Item>) -> Plan {
        Plan {
            items,
            hard_links: HashMap::new(),
        }
    }

    /// Hard links would lose their data if their target went away or moved, so removing or
    /// renaming `touched` is refused while a surviving link points into it. With `removing`,
    /// links inside `touched` go too and do not count.
    fn check_hard_links(&self, touched: &str, removing: bool) -> Result<(), ArchiveError> {
        for it in &self.items {
            let Src::Orig { ord, .. } = it.src else {
                continue;
            };
            let Some(target) = self.hard_links.get(&ord) else {
                continue;
            };
            if removing && !it.fixed && under(&it.path, touched) {
                continue;
            }
            if under(target, touched) {
                return Err(ArchiveError::Unsupported(format!(
                    "This archive has hard links to {target}; deleting or renaming it is not supported"
                )));
            }
        }
        Ok(())
    }

    fn apply_all(mut self, edits: &[Edit]) -> Result<Plan, ArchiveError> {
        for e in edits {
            self.apply(e)?;
        }
        Ok(self)
    }

    /// True if `p` is an entry or an implicit folder (a prefix of one).
    fn exists(&self, p: &str) -> bool {
        self.items.iter().any(|i| !i.fixed && under(&i.path, p))
    }

    /// A file cannot be the parent of anything.
    fn check_parents(&self, p: &str) -> Result<(), ArchiveError> {
        for (i, _) in p.match_indices('/') {
            let anc = &p[..i];
            if self
                .items
                .iter()
                .any(|it| !it.fixed && !it.is_dir && it.path == anc)
            {
                return Err(ArchiveError::Exists(anc.to_string()));
            }
        }
        Ok(())
    }

    fn remove(&mut self, p: &str) {
        self.items.retain(|i| i.fixed || !under(&i.path, p));
    }

    /// After `p` went away, keep its parent folder when nothing else holds it up.
    fn keep_parent(&mut self, p: &str) {
        let Some(pos) = p.rfind('/') else { return };
        let parent = &p[..pos];
        if !self.exists(parent) {
            self.items.push(new_dir(parent.to_string()));
        }
    }

    fn apply(&mut self, e: &Edit) -> Result<(), ArchiveError> {
        match e {
            Edit::Add { src, inner } => {
                let n = norm(inner)?;
                self.check_parents(&n)?;
                // Walk first, so a failing source leaves the plan unchanged.
                let mut added = Vec::new();
                walk(src, &n, &mut added)?;
                if self.exists(&n) {
                    self.check_hard_links(&n, true)?;
                }
                self.remove(&n);
                self.items.extend(added);
            }
            Edit::Delete { inner } => {
                let n = norm(inner)?;
                if !self.exists(&n) {
                    return Err(ArchiveError::NotFound(n));
                }
                self.check_hard_links(&n, true)?;
                self.remove(&n);
                self.keep_parent(&n);
            }
            Edit::Rename { from, to } => {
                let f = norm(from)?;
                let t = norm(to)?;
                if !self.exists(&f) {
                    return Err(ArchiveError::NotFound(f));
                }
                if self.exists(&t) {
                    return Err(ArchiveError::Exists(t));
                }
                if under(&t, &f) {
                    return Err(ArchiveError::Io(format!("cannot move '{f}' into itself")));
                }
                self.check_parents(&t)?;
                self.check_hard_links(&f, false)?;
                for it in self.items.iter_mut().filter(|i| !i.fixed) {
                    if under(&it.path, &f) {
                        it.path = format!("{t}{}", &it.path[f.len()..]);
                    }
                }
                self.keep_parent(&f);
            }
            Edit::Mkdir { inner } => {
                let n = norm(inner)?;
                if self.exists(&n) {
                    return Err(ArchiveError::Exists(n));
                }
                self.check_parents(&n)?;
                self.items.push(new_dir(n));
            }
        }
        Ok(())
    }
}

/// Lists a local file, link or folder tree as items below `inner`.
fn walk(src: &Path, inner: &str, out: &mut Vec<Item>) -> Result<(), ArchiveError> {
    let md = fs::symlink_metadata(src).map_err(|e| src_err(src, &e))?;
    let ft = md.file_type();
    if !ft.is_dir() && !ft.is_file() && !ft.is_symlink() {
        return Err(ArchiveError::Io(format!(
            "{}: not a regular file",
            src.display()
        )));
    }
    out.push(Item {
        path: inner.to_string(),
        fixed: false,
        src: Src::Local(src.to_path_buf()),
        is_dir: ft.is_dir(),
        is_symlink: ft.is_symlink(),
        size: if ft.is_file() { md.len() } else { 0 },
        mode: md.mode(),
        mtime: md.mtime(),
    });
    if ft.is_dir() {
        let mut names = Vec::new();
        for e in fs::read_dir(src).map_err(|e| src_err(src, &e))? {
            names.push(e.map_err(|e| src_err(src, &e))?.file_name());
        }
        names.sort();
        for n in names {
            let child = src.join(&n);
            let Some(s) = n.to_str() else {
                return Err(ArchiveError::Io(format!(
                    "file name is not valid UTF-8: {}",
                    child.display()
                )));
            };
            walk(&child, &norm(&format!("{inner}/{s}"))?, out)?;
        }
    }
    Ok(())
}

// ---- progress ----

struct Prog<'a> {
    obs: &'a dyn OpObserver,
    total_bytes: u64,
    total_files: u64,
    done_bytes: u64,
    done_files: u64,
    since: u64,
    cur: String,
    cancelled: bool,
}

impl<'a> Prog<'a> {
    /// `done_bytes` starts at `base` (7z counts its staging phase first).
    fn new(obs: &'a dyn OpObserver, items: &[Item], base: u64) -> Prog<'a> {
        let files = items.iter().filter(|i| !i.is_dir);
        Prog {
            obs,
            total_bytes: base + files.clone().map(|i| i.size).sum::<u64>(),
            total_files: files.count() as u64,
            done_bytes: base,
            done_files: 0,
            since: 0,
            cur: String::new(),
            cancelled: false,
        }
    }

    fn report(&self) {
        self.obs.progress(&Progress {
            current: self.cur.clone(),
            done_bytes: self.done_bytes,
            total_bytes: self.total_bytes,
            done_files: self.done_files,
            total_files: self.total_files,
        });
    }

    fn check(&mut self) -> Result<(), ArchiveError> {
        if self.obs.is_cancelled() {
            self.cancelled = true;
            return Err(ArchiveError::Cancelled);
        }
        Ok(())
    }

    fn start(&mut self, it: &Item) -> Result<(), ArchiveError> {
        self.check()?;
        self.cur = it.path.clone();
        Ok(())
    }

    /// Counts `n` written bytes; false once the user cancelled.
    fn add(&mut self, n: u64) -> bool {
        self.done_bytes += n;
        self.since += n;
        if self.since >= CHUNK {
            self.since = 0;
            self.report();
            if self.obs.is_cancelled() {
                self.cancelled = true;
                return false;
            }
        }
        true
    }

    fn item_done(&mut self, it: &Item) -> Result<(), ArchiveError> {
        if !it.is_dir {
            self.done_files += 1;
        }
        self.report();
        self.check()
    }

    /// A failure caused by a cancel inside a copy is a cancel.
    fn fail(&self, e: impl std::fmt::Display) -> ArchiveError {
        if self.cancelled {
            ArchiveError::Cancelled
        } else {
            ArchiveError::Io(e.to_string())
        }
    }
}

/// Reads through `r`, counting bytes into the progress and stopping on cancel.
struct Meter<'p, 'a, R> {
    r: R,
    p: &'p mut Prog<'a>,
    n: u64,
}

impl<'p, 'a, R> Meter<'p, 'a, R> {
    fn new(r: R, p: &'p mut Prog<'a>) -> Self {
        Meter { r, p, n: 0 }
    }
}

impl<R: Read> Read for Meter<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.r.read(buf)?;
        self.n += n as u64;
        if !self.p.add(n as u64) {
            return Err(io::Error::other("operation cancelled"));
        }
        Ok(n)
    }
}

// ---- atomic replace ----

/// One lock per archive (canonical path), held for a whole rewrite: a second rewrite of the
/// same archive plans from the first one's result instead of overwriting it.
static LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = LazyLock::new(Default::default);

fn archive_lock(real: &Path) -> Arc<Mutex<()>> {
    let mut m = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Locks only the map holds are not in use.
    m.retain(|_, l| Arc::strong_count(l) > 1);
    m.entry(real.to_path_buf()).or_default().clone()
}

/// Which file, and which version of it, a path names.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FileId {
    dev: u64,
    ino: u64,
    len: u64,
    mtime_ns: i128,
}

impl FileId {
    fn of(md: &fs::Metadata) -> FileId {
        FileId {
            dev: md.dev(),
            ino: md.ino(),
            len: md.len(),
            mtime_ns: i128::from(md.mtime()) * 1_000_000_000 + i128::from(md.mtime_nsec()),
        }
    }
}

/// The archive as the edits were planned from it.
struct Original {
    id: FileId,
    /// For messages.
    name: String,
}

/// A temp file or staging folder of `sibling` left by a process that is gone.
fn dead_leftover(file_name: &str, prefix: &str) -> Option<&'static str> {
    let rest = file_name.strip_prefix(prefix)?;
    let (pid_seq, ext) = rest.rsplit_once('.')?;
    let ext = ["tmp", "stage"].into_iter().find(|e| *e == ext)?;
    let (pid, seq) = pid_seq.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(pid) || !digits(seq) {
        return None;
    }
    let pid: u32 = pid.parse().ok()?;
    (pid != std::process::id() && !pid_alive(pid)).then_some(ext)
}

/// Removes `.<name>.fxfm-<pid>-<n>.tmp` files and `.stage` folders next to `real` whose
/// process is gone. Anything else, including leftovers of live processes, is left alone.
fn sweep_leftovers(real: &Path) {
    let (Some(dir), Some(name)) = (real.parent(), real.file_name()) else {
        return;
    };
    let prefix = format!(".{}.fxfm-", name.to_string_lossy());
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let file_name = e.file_name();
        let Some(ext) = file_name.to_str().and_then(|n| dead_leftover(n, &prefix)) else {
            continue;
        };
        let p = e.path();
        let Ok(md) = fs::symlink_metadata(&p) else {
            continue;
        };
        let r = match ext {
            "tmp" if md.is_file() => fs::remove_file(&p),
            "stage" if md.is_dir() => {
                open_up(&p);
                fs::remove_dir_all(&p)
            }
            _ => continue,
        };
        match r {
            Ok(()) => log::info!("removed leftover {}", p.display()),
            Err(e) => log::warn!("removing leftover {}: {e}", p.display()),
        }
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// `.<name>.fxfm-<pid>-<n>.<ext>` next to `real`.
pub(super) fn sibling(real: &Path, ext: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    real.with_file_name(format!(".{name}.fxfm-{}-{n}.{ext}", std::process::id()))
}

/// Removes the temp file unless it was renamed into place.
pub(super) struct TempFile {
    pub(super) path: PathBuf,
    pub(super) done: bool,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.done {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Writes the new archive with `write` into a temp file and replaces `real` with it.
fn commit(
    real: &Path,
    orig: &Original,
    obs: &dyn OpObserver,
    write: impl FnOnce(File) -> Result<File, ArchiveError>,
) -> Result<(), ArchiveError> {
    let io = |e: io::Error| ArchiveError::Io(e.to_string());
    let perms = fs::metadata(real).map_err(io)?.permissions();
    let mut tmp = TempFile {
        path: sibling(real, "tmp"),
        done: false,
    };
    let f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp.path)
        .map_err(io)?;
    let f = write(f)?;
    crate::ops::sync_file(&f).map_err(io)?;
    drop(f);
    if obs.is_cancelled() {
        return Err(ArchiveError::Cancelled);
    }
    fs::set_permissions(&tmp.path, perms).map_err(io)?;
    // Another program may have replaced or changed the archive meanwhile; its version wins.
    if fs::metadata(real).ok().map(|m| FileId::of(&m)) != Some(orig.id) {
        return Err(ArchiveError::Io(format!(
            "{} was changed by someone else while fxfm was writing it; nothing was changed",
            orig.name
        )));
    }
    fs::rename(&tmp.path, real).map_err(io)?;
    tmp.done = true;
    if let Some(parent) = real.parent() {
        if let Err(e) = File::open(parent).and_then(|d| d.sync_all()) {
            log::warn!("fsync of {} failed: {e}", parent.display());
        }
    }
    Ok(())
}

fn into_file(w: BufWriter<File>) -> Result<File, ArchiveError> {
    w.into_inner()
        .map_err(|e| ArchiveError::Io(e.error().to_string()))
}

// ---- zip ----

/// A new reader on `file` (the open original), positioned at its start.
fn reopen(file: &File, real: &Path) -> Result<File, ArchiveError> {
    let mut f = file.try_clone().map_err(|e| io_err(&e, real))?;
    f.rewind().map_err(|e| io_err(&e, real))?;
    Ok(f)
}

fn open_zip(file: &File, real: &Path) -> Result<zip::ZipArchive<BufReader<File>>, ArchiveError> {
    zip::ZipArchive::new(BufReader::new(reopen(file, real)?)).map_err(|e| zip_err(e, real))
}

fn zip_items(file: &File, real: &Path) -> Result<Vec<Item>, ArchiveError> {
    let mut z = open_zip(file, real)?;
    let mut items = Vec::with_capacity(z.len());
    for ord in 0..z.len() {
        let e = z.by_index_raw(ord).map_err(|e| zip_err(e, real))?;
        let raw = match e.name() {
            Ok(n) => n.into_owned(),
            Err(_) => String::from_utf8_lossy(e.name_raw()).into_owned(),
        };
        let is_dir = e.is_dir() || raw.ends_with('/');
        let (path, fixed) = match check_name(&raw) {
            NameCheck::Ok(n) => (n, false),
            _ => (raw, true),
        };
        items.push(Item {
            src: Src::Orig {
                ord,
                name: path.clone(),
            },
            path,
            fixed,
            is_dir,
            is_symlink: e.is_symlink(),
            size: if is_dir { 0 } else { e.compressed_size() },
            mode: e.unix_mode().unwrap_or(0),
            mtime: 0,
        });
    }
    Ok(items)
}

fn write_zip(
    file: &File,
    real: &Path,
    items: &[Item],
    out: File,
    prog: &mut Prog,
) -> Result<File, ArchiveError> {
    let mut z = open_zip(file, real)?;
    let mut w = zip::ZipWriter::new(BufWriter::new(out));
    if !z.comment().is_empty() {
        w.set_raw_comment(z.comment().into())
            .map_err(|e| prog.fail(e))?;
    }
    for it in items {
        prog.start(it)?;
        // Local DOS time plus an extended timestamp, as Info-ZIP and Finder expect.
        let opts = super::ziptime::entry_options(it.mode, it.mtime);
        match &it.src {
            Src::Orig { ord, .. } => {
                let f = z.by_index_raw(*ord).map_err(|e| zip_err(e, real))?;
                let r = if it.renamed() {
                    let name = if it.is_dir {
                        format!("{}/", it.path)
                    } else {
                        it.path.clone()
                    };
                    w.raw_copy_file_rename(f, name)
                } else {
                    w.raw_copy_file(f)
                };
                r.map_err(|e| prog.fail(e))?;
                prog.add(it.size);
            }
            Src::NewDir => w
                .add_directory(it.path.as_str(), opts)
                .map_err(|e| prog.fail(e))?,
            Src::Local(_) if it.is_dir => w
                .add_directory(it.path.as_str(), opts)
                .map_err(|e| prog.fail(e))?,
            Src::Local(src) if it.is_symlink => {
                let target = fs::read_link(src).map_err(|e| src_err(src, &e))?;
                w.add_symlink(it.path.as_str(), target.to_string_lossy(), opts)
                    .map_err(|e| prog.fail(e))?;
            }
            Src::Local(src) => {
                let f = File::open(src).map_err(|e| src_err(src, &e))?;
                let len = f.metadata().map_err(|e| src_err(src, &e))?.len();
                let opts = opts.large_file(len >= u64::from(u32::MAX));
                w.start_file(it.path.as_str(), opts)
                    .map_err(|e| prog.fail(e))?;
                io::copy(&mut Meter::new(f, prog), &mut w).map_err(|e| prog.fail(e))?;
            }
        }
        prog.item_done(it)?;
    }
    let bw = w.finish().map_err(|e| prog.fail(e))?;
    into_file(bw)
}

// ---- tar ----

fn open_tar(file: &File, real: &Path, kind: ArchiveKind) -> Result<Box<dyn Read>, ArchiveError> {
    let f = BufReader::new(reopen(file, real)?);
    Ok(super::tar_decoder(kind, f))
}

fn tar_skip(ty: tar::EntryType) -> bool {
    ty.is_pax_global_extensions() || ty.is_pax_local_extensions() || ty.is_gnu_longname()
}

fn corrupt(e: io::Error) -> ArchiveError {
    ArchiveError::Corrupt(e.to_string())
}

type TarPlan = (Vec<Item>, HashMap<usize, String>);

fn tar_items(r: impl Read) -> Result<TarPlan, ArchiveError> {
    let mut a = tar::Archive::new(r);
    let mut items = Vec::new();
    let mut hard_links = HashMap::new();
    let mut ord = 0;
    for e in a.entries().map_err(corrupt)? {
        let e = e.map_err(corrupt)?;
        let h = e.header();
        let ty = h.entry_type();
        if tar_skip(ty) {
            continue;
        }
        let raw = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        let (path, fixed) = match check_name(&raw) {
            NameCheck::Ok(n) => (n, false),
            _ => (raw, true),
        };
        if ty.is_hard_link() {
            let target = e.link_name_bytes().unwrap_or_default();
            if let NameCheck::Ok(t) = check_name(&String::from_utf8_lossy(&target)) {
                hard_links.insert(ord, t);
            }
        }
        items.push(Item {
            src: Src::Orig {
                ord,
                name: path.clone(),
            },
            path,
            fixed,
            is_dir: ty.is_dir(),
            is_symlink: ty.is_symlink(),
            size: if ty.is_dir() { 0 } else { e.size() },
            mode: h.mode().unwrap_or(0),
            mtime: h.mtime().map_or(0, |m| m as i64),
        });
        ord += 1;
    }
    Ok((items, hard_links))
}

fn write_tar(
    src: impl Read,
    items: &[Item],
    out: File,
    kind: ArchiveKind,
    prog: &mut Prog,
) -> Result<File, ArchiveError> {
    let sink = super::TarSink::new(kind, BufWriter::new(out)).map_err(|e| prog.fail(e))?;
    let sink = tar_entries(src, items, tar::Builder::new(sink), prog)?;
    into_file(sink.finish().map_err(|e| prog.fail(e))?)
}

fn tar_entries<W: Write>(
    src: impl Read,
    items: &[Item],
    mut b: tar::Builder<W>,
    prog: &mut Prog,
) -> Result<W, ArchiveError> {
    let by_ord: HashMap<usize, &Item> = items
        .iter()
        .filter_map(|it| match it.src {
            Src::Orig { ord, .. } => Some((ord, it)),
            _ => None,
        })
        .collect();
    // Original entries keep their order, so they are streamed first.
    let mut a = tar::Archive::new(src);
    let mut ord = 0;
    for e in a.entries().map_err(corrupt)? {
        let mut e = e.map_err(corrupt)?;
        let ty = e.header().entry_type();
        if tar_skip(ty) {
            continue;
        }
        let this = ord;
        ord += 1;
        let Some(it) = by_ord.get(&this) else {
            continue;
        };
        prog.start(it)?;
        copy_tar_entry(&mut b, &mut e, it, prog)?;
        prog.item_done(it)?;
    }
    for it in items.iter().filter(|i| !matches!(i.src, Src::Orig { .. })) {
        prog.start(it)?;
        let mut h = tar::Header::new_gnu();
        h.set_mode(it.mode & 0o7777);
        h.set_mtime(it.mtime.max(0) as u64);
        h.set_size(0);
        let r = match &it.src {
            Src::Local(src) if it.is_symlink => {
                let target = fs::read_link(src).map_err(|e| src_err(src, &e))?;
                h.set_entry_type(tar::EntryType::Symlink);
                b.append_link(&mut h, &it.path, target)
            }
            Src::Local(src) if !it.is_dir => {
                let f = File::open(src).map_err(|e| src_err(src, &e))?;
                let len = f.metadata().map_err(|e| src_err(src, &e))?.len();
                h.set_entry_type(tar::EntryType::Regular);
                h.set_size(len);
                let mut m = Meter::new(f.take(len), prog);
                let r = b.append_data(&mut h, &it.path, &mut m);
                if r.is_ok() && m.n != len {
                    return Err(ArchiveError::Io(format!(
                        "{}: file changed while adding",
                        src.display()
                    )));
                }
                r
            }
            _ => {
                h.set_entry_type(tar::EntryType::Directory);
                b.append_data(&mut h, &it.path, io::empty())
            }
        };
        r.map_err(|e| prog.fail(e))?;
        prog.item_done(it)?;
    }
    b.into_inner().map_err(|e| prog.fail(e))
}

/// Streams one original entry, keeping its header and pax records; only the name changes
/// when the entry is renamed.
fn copy_tar_entry<W: Write, R: Read>(
    b: &mut tar::Builder<W>,
    e: &mut tar::Entry<R>,
    it: &Item,
    prog: &mut Prog,
) -> Result<(), ArchiveError> {
    let mut h = e.header().clone();
    let ty = h.entry_type();
    let full = e.path_bytes().into_owned();
    let target = e.link_name_bytes().map(|t| t.into_owned());
    // Names are written through the header (and GNU long names), so pax path records go.
    let pax: Vec<(String, Vec<u8>)> = match e.pax_extensions() {
        Ok(Some(px)) => px
            .filter_map(Result::ok)
            .filter_map(|x| Some((x.key().ok()?.to_string(), x.value_bytes().to_vec())))
            .filter(|(k, _)| k != "path" && k != "linkpath")
            .collect(),
        _ => Vec::new(),
    };
    if !pax.is_empty() {
        b.append_pax_extensions(pax.iter().map(|(k, v)| (k.as_str(), v.as_slice())))
            .map_err(|e| prog.fail(e))?;
    }
    let mut changed = false;
    if ty.is_gnu_sparse() {
        // The entry reads back expanded, so it is stored as a regular file.
        let real_size = h.as_gnu().and_then(|g| g.real_size().ok());
        h.set_entry_type(tar::EntryType::Regular);
        h.set_size(real_size.unwrap_or_else(|| e.size()));
        changed = true;
    }
    let verbatim = !it.renamed()
        && h.path_bytes() == full.as_slice()
        && target.as_deref() == h.link_name_bytes().as_deref();
    let name: PathBuf = if it.renamed() {
        PathBuf::from(&it.path)
    } else {
        PathBuf::from(OsStr::from_bytes(&full))
    };
    let r = if verbatim {
        if changed {
            h.set_cksum();
        }
        b.append(&h, Meter::new(e, prog))
    } else if ty.is_symlink() || ty.is_hard_link() {
        let t = target.unwrap_or_default();
        b.append_link(&mut h, &name, OsStr::from_bytes(&t))
    } else {
        b.append_data(&mut h, &name, Meter::new(e, prog))
    };
    r.map_err(|e| prog.fail(e))
}

// ---- gz ----

enum GzOp {
    /// New content from a local file, under the existing name.
    Replace { src: PathBuf, name: String },
    /// Only the name in the header changes.
    Rename { to: String },
}

fn plan_gz(index: &ArchiveIndex, edits: &[Edit]) -> Result<GzOp, ArchiveError> {
    let unsupported = || ArchiveError::Unsupported(GZ_ONLY.into());
    let Some(entry) = index.entries.first() else {
        return Err(ArchiveError::Corrupt("empty gzip file".into()));
    };
    match edits {
        [Edit::Add { src, inner }] => {
            if norm(inner)? != entry.path {
                return Err(unsupported());
            }
            let md = fs::metadata(src).map_err(|e| src_err(src, &e))?;
            if !md.is_file() {
                return Err(unsupported());
            }
            Ok(GzOp::Replace {
                src: src.clone(),
                name: entry.path.clone(),
            })
        }
        [Edit::Rename { from, to }] => {
            let f = norm(from)?;
            if f != entry.path {
                return Err(ArchiveError::NotFound(f));
            }
            let t = norm(to)?;
            if t.contains('/') {
                return Err(unsupported());
            }
            if t == entry.path {
                return Err(ArchiveError::Exists(t));
            }
            if latin1(&t).is_none() {
                return Err(ArchiveError::Unsupported(
                    "This name cannot be stored in a .gz file".into(),
                ));
            }
            Ok(GzOp::Rename { to: t })
        }
        _ => Err(unsupported()),
    }
}

/// The gzip name field is Latin-1 and NUL-terminated.
fn latin1(s: &str) -> Option<Vec<u8>> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).ok().filter(|&b| b != 0))
        .collect()
}

fn write_gz(
    file: &File,
    real: &Path,
    op: &GzOp,
    out: File,
    obs: &dyn OpObserver,
) -> Result<File, ArchiveError> {
    match op {
        GzOp::Replace { src, name } => {
            let f = File::open(src).map_err(|e| src_err(src, &e))?;
            let md = f.metadata().map_err(|e| src_err(src, &e))?;
            let item = Item {
                path: name.clone(),
                fixed: false,
                src: Src::Local(src.clone()),
                is_dir: false,
                is_symlink: false,
                size: md.len(),
                mode: md.mode(),
                mtime: md.mtime(),
            };
            let mut prog = Prog::new(obs, std::slice::from_ref(&item), 0);
            prog.start(&item)?;
            let mut b = flate2::GzBuilder::new().mtime(u32::try_from(md.mtime()).unwrap_or(0));
            if let Some(n) = latin1(name) {
                b = b.filename(n);
            }
            let mut enc = b.write(BufWriter::new(out), flate2::Compression::default());
            io::copy(&mut Meter::new(f, &mut prog), &mut enc).map_err(|e| prog.fail(e))?;
            let bw = enc.finish().map_err(|e| prog.fail(e))?;
            prog.item_done(&item)?;
            into_file(bw)
        }
        GzOp::Rename { to } => {
            let mut r = BufReader::new(reopen(file, real)?);
            let len = file.metadata().map_err(|e| io_err(&e, real))?.len();
            let bad = |_| ArchiveError::Corrupt("truncated gzip header".into());
            let mut head = [0u8; 10];
            r.read_exact(&mut head).map_err(bad)?;
            let flags = head[3];
            let mut extra = Vec::new();
            if flags & 4 != 0 {
                let mut xlen = [0u8; 2];
                r.read_exact(&mut xlen).map_err(bad)?;
                extra.extend_from_slice(&xlen);
                let mut data = vec![0u8; usize::from(u16::from_le_bytes(xlen))];
                r.read_exact(&mut data).map_err(bad)?;
                extra.extend(data);
            }
            let mut skip = Vec::new();
            if flags & 8 != 0 {
                io::BufRead::read_until(&mut r, 0, &mut skip).map_err(bad)?;
            }
            let mut comment = Vec::new();
            if flags & 16 != 0 {
                io::BufRead::read_until(&mut r, 0, &mut comment).map_err(bad)?;
            }
            if flags & 2 != 0 {
                let mut crc = [0u8; 2];
                r.read_exact(&mut crc).map_err(bad)?;
            }
            // Keep FTEXT, FEXTRA and FCOMMENT; set FNAME; drop the header CRC.
            head[3] = (flags & (1 | 4 | 16)) | 8;
            let mut name = latin1(to).unwrap_or_default();
            name.push(0);
            let item = Item {
                path: to.clone(),
                fixed: false,
                src: Src::NewDir,
                is_dir: false,
                is_symlink: false,
                size: len,
                mode: 0,
                mtime: 0,
            };
            let mut prog = Prog::new(obs, std::slice::from_ref(&item), 0);
            prog.start(&item)?;
            let mut w = BufWriter::new(out);
            for part in [&head[..], &extra, &name, &comment] {
                w.write_all(part).map_err(|e| prog.fail(e))?;
            }
            io::copy(&mut Meter::new(r, &mut prog), &mut w).map_err(|e| prog.fail(e))?;
            prog.item_done(&item)?;
            into_file(w)
        }
    }
}

// ---- 7z ----

/// Removes the staging tree, first making its folders and files accessible again (extraction
/// applies archived modes such as 0o500 or 0o000).
struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        open_up(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Gives the owner rwx on folders and read on files below `p`, without following links.
fn open_up(p: &Path) {
    let Ok(md) = fs::symlink_metadata(p) else {
        return;
    };
    let mode = md.permissions().mode();
    if md.is_dir() {
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                open_up(&e.path());
            }
        }
    } else if md.is_file() && mode & 0o400 == 0 {
        let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode | 0o400));
    }
}

/// Forwards to the user's observer during staging, with the total of both phases.
/// Any staging error aborts the rewrite: a skipped entry would be missing from the new
/// archive, so the user is not asked.
struct StagingObs<'a> {
    obs: &'a dyn OpObserver,
    total_bytes: u64,
    total_files: u64,
    failure: std::sync::Mutex<Option<String>>,
}

impl OpObserver for StagingObs<'_> {
    fn progress(&self, p: &Progress) {
        self.obs.progress(&Progress {
            current: p.current.clone(),
            done_bytes: p.done_bytes,
            total_bytes: self.total_bytes,
            done_files: 0,
            total_files: self.total_files,
        });
    }
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.obs.conflict(c)
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        let mut f = self.failure.lock().unwrap();
        if f.is_none() {
            *f = Some(format!("{path}: {msg}"));
        }
        ErrorAnswer::Cancel
    }
    fn is_cancelled(&self) -> bool {
        self.obs.is_cancelled()
    }
}

fn rewrite_7z(
    real: &Path,
    orig: &Original,
    index: &ArchiveIndex,
    edits: &[Edit],
    obs: &dyn OpObserver,
) -> Result<(), ArchiveError> {
    // The index leaves out unsafe names, so a rewrite would silently drop those entries.
    let a = sevenz_rust2::Archive::open(real).map_err(|e| sevenz_err(e, real))?;
    let unsafe_name = a
        .files
        .iter()
        .any(|e| !e.is_anti_item() && matches!(check_name(e.name()), NameCheck::Unsafe));
    if unsafe_name {
        return Err(ArchiveError::Unsupported(
            "This archive contains entries with unsafe names; fxfm will not rewrite it".into(),
        ));
    }
    let items = index
        .entries
        .iter()
        .enumerate()
        .map(|(ord, e)| Item {
            path: e.path.clone(),
            fixed: false,
            src: Src::Orig {
                ord,
                name: e.path.clone(),
            },
            is_dir: e.is_dir,
            is_symlink: e.is_symlink,
            size: e.size,
            mode: e.mode,
            mtime: e.mtime,
        })
        .collect();
    let plan = Plan::new(items).apply_all(edits)?;
    let staged: u64 = index
        .entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.size)
        .sum();
    let mut prog = Prog::new(obs, &plan.items, staged);

    let stage = Stage(sibling(real, "stage"));
    fs::create_dir(&stage.0).map_err(|e| ArchiveError::Io(e.to_string()))?;
    let fwd = StagingObs {
        obs,
        total_bytes: prog.total_bytes,
        total_files: prog.total_files,
        failure: std::sync::Mutex::new(None),
    };
    let staged = extract(
        real,
        ArchiveKind::SevenZ,
        &[String::new()],
        &stage.0,
        None,
        &fwd,
    );
    if let Some(msg) = fwd.failure.lock().unwrap().take() {
        return Err(ArchiveError::Io(format!(
            "cannot stage archive for writing: {msg}"
        )));
    }
    staged?;
    open_up(&stage.0);
    commit(real, orig, obs, |out| {
        write_7z(&stage.0, &plan.items, out, &mut prog)
    })
}

fn nt_time(secs: i64) -> sevenz_rust2::NtTime {
    let ticks = (secs + 11_644_473_600).max(0) as u64 * 10_000_000;
    sevenz_rust2::NtTime::from(ticks)
}

fn write_7z(
    stage: &Path,
    items: &[Item],
    out: File,
    prog: &mut Prog,
) -> Result<File, ArchiveError> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = ArchiveWriter::new(BufWriter::new(out)).map_err(|e| sevenz_err(e, stage))?;
    for it in items {
        prog.start(it)?;
        let source = match &it.src {
            Src::Orig { name, .. } => Some(stage.join(name)),
            Src::Local(p) => Some(p.clone()),
            Src::NewDir => None,
        };
        let mut entry = if it.is_dir {
            ArchiveEntry::new_directory(&it.path)
        } else {
            ArchiveEntry::new_file(&it.path)
        };
        entry.has_last_modified_date = true;
        entry.last_modified_date = nt_time(it.mtime);
        entry.has_windows_attributes = true;
        let dos = if it.is_dir { 0x10 } else { 0x20 };
        entry.windows_attributes = dos | 0x8000 | ((it.mode & 0xffff) << 16);
        let r = match source {
            _ if it.is_dir => w.push_archive_entry::<&[u8]>(entry, None).map(|_| ()),
            Some(src) if it.is_symlink => {
                // Staging never skips an entry, so anything missing is an error.
                let target = fs::read_link(&src).map_err(|e| src_err(&src, &e))?;
                let t = target.as_os_str().as_bytes();
                w.push_archive_entry(entry, Some(t)).map(|_| ())
            }
            Some(src) => {
                let f = File::open(&src).map_err(|e| src_err(&src, &e))?;
                w.push_archive_entry(entry, Some(Meter::new(f, prog)))
                    .map(|_| ())
            }
            None => w.push_archive_entry::<&[u8]>(entry, None).map(|_| ()),
        };
        r.map_err(|e| prog.fail(e))?;
        prog.item_done(it)?;
    }
    let bw = w.finish().map_err(|e| prog.fail(e))?;
    into_file(bw)
}

#[cfg(test)]
mod tests {
    use super::super::read::{extract, read_index};
    use super::super::testutil::Item;
    use super::super::testutil::*;
    use super::*;
    use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, Progress};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Obs {
        cancel_after: usize,
        progress: AtomicUsize,
        /// Answer to `error()`; panics when `None`.
        answer: Option<ErrorAnswer>,
        /// Cancel on the first progress that is past the start but short of the total.
        cancel_mid: bool,
        mid_seen: AtomicBool,
        max_done: AtomicU64,
        total: AtomicU64,
    }

    impl OpObserver for Obs {
        fn progress(&self, p: &Progress) {
            self.progress.fetch_add(1, Ordering::SeqCst);
            self.max_done.fetch_max(p.done_bytes, Ordering::SeqCst);
            self.total.store(p.total_bytes, Ordering::SeqCst);
            if p.done_bytes > 0 && p.done_bytes < p.total_bytes {
                self.mid_seen.store(true, Ordering::SeqCst);
            }
        }
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            panic!("no conflicts expected");
        }
        fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
            match &self.answer {
                Some(a) => a.clone(),
                None => panic!("unexpected error {path}: {msg}"),
            }
        }
        fn is_cancelled(&self) -> bool {
            (self.cancel_after > 0 && self.progress.load(Ordering::SeqCst) >= self.cancel_after)
                || (self.cancel_mid && self.mid_seen.load(Ordering::SeqCst))
        }
    }

    const KINDS: [ArchiveKind; 6] = [
        ArchiveKind::Zip,
        ArchiveKind::Tar,
        ArchiveKind::TarGz,
        ArchiveKind::TarBz2,
        ArchiveKind::TarXz,
        ArchiveKind::SevenZ,
    ];

    const BASE: &[Item] = &[
        ("a.txt", Some(b"alpha")),
        ("d", None),
        ("d/b.txt", Some(b"beta beta")),
        ("src", None),
        ("src/lib.rs", Some(b"fn main() {}")),
    ];

    fn name(kind: ArchiveKind) -> &'static str {
        match kind {
            ArchiveKind::Zip => "a.zip",
            ArchiveKind::Tar => "a.tar",
            ArchiveKind::TarGz => "a.tar.gz",
            ArchiveKind::TarBz2 => "a.tar.bz2",
            ArchiveKind::TarXz => "a.tar.xz",
            ArchiveKind::Gz => "a.txt.gz",
            ArchiveKind::SevenZ => "a.7z",
        }
    }

    /// Entries as `path -> Some(content)` for files and `None` for directories, read back
    /// through `read_index` and `extract`.
    fn contents(p: &Path, kind: ArchiveKind) -> BTreeMap<String, Option<Vec<u8>>> {
        let idx = read_index(p, kind).unwrap();
        let dest = tempfile::tempdir().unwrap();
        extract(
            p,
            kind,
            &[String::new()],
            dest.path(),
            None,
            &Obs::default(),
        )
        .unwrap();
        idx.entries
            .iter()
            .map(|e| {
                let data = (!e.is_dir).then(|| std::fs::read(dest.path().join(&e.path)).unwrap());
                if e.is_dir {
                    assert!(dest.path().join(&e.path).is_dir(), "{kind:?} {}", e.path);
                }
                (e.path.clone(), data)
            })
            .collect()
    }

    fn expect(items: &[(&str, Option<&[u8]>)]) -> BTreeMap<String, Option<Vec<u8>>> {
        items
            .iter()
            .map(|(n, d)| (n.to_string(), d.map(|d| d.to_vec())))
            .collect()
    }

    fn base() -> BTreeMap<String, Option<Vec<u8>>> {
        expect(BASE)
    }

    /// Hidden temp or staging names left next to the archive.
    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.') && (n.ends_with(".tmp") || n.contains(".fxfm-")))
            .collect()
    }

    fn run(p: &Path, kind: ArchiveKind, edits: &[Edit]) -> Result<(), ArchiveError> {
        rewrite(p, kind, edits, &IndexCache::new(), &Obs::default())
    }

    fn add(src: &Path, inner: &str) -> Edit {
        Edit::Add {
            src: src.to_path_buf(),
            inner: inner.into(),
        }
    }

    fn rename(from: &str, to: &str) -> Edit {
        Edit::Rename {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn add_file_and_dir() {
        let local = tempfile::tempdir().unwrap();
        std::fs::write(local.path().join("x.txt"), b"new x").unwrap();
        std::fs::create_dir(local.path().join("m")).unwrap();
        std::fs::write(local.path().join("m/y.txt"), b"why").unwrap();
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let edits = [
                add(&local.path().join("x.txt"), "x.txt"),
                add(&local.path().join("m"), "src/m"),
            ];
            run(&p, kind, &edits).unwrap();
            let mut want = base();
            want.extend(expect(&[
                ("x.txt", Some(b"new x")),
                ("src/m", None),
                ("src/m/y.txt", Some(b"why")),
            ]));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn add_replaces_existing() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("a.txt");
        std::fs::write(&src, b"replaced").unwrap();
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let before = read_index(&p, kind).unwrap().entries.len();
            run(&p, kind, &[add(&src, "a.txt")]).unwrap();
            assert_eq!(read_index(&p, kind).unwrap().entries.len(), before);
            let mut want = base();
            want.insert("a.txt".into(), Some(b"replaced".to_vec()));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
        }
    }

    #[test]
    fn delete_dir_subtree() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            run(&p, kind, &[Edit::Delete { inner: "d".into() }]).unwrap();
            let mut want = base();
            want.remove("d");
            want.remove("d/b.txt");
            assert_eq!(contents(&p, kind), want, "{kind:?}");
            let r = run(&p, kind, &[Edit::Delete { inner: "d".into() }]);
            assert!(matches!(r, Err(ArchiveError::NotFound(_))), "{kind:?}");
        }
    }

    #[test]
    fn rename_dir_subtree() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            run(&p, kind, &[rename("d", "e")]).unwrap();
            let mut want = base();
            want.remove("d");
            want.remove("d/b.txt");
            want.extend(expect(&[("e", None), ("e/b.txt", Some(b"beta beta"))]));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
            // A file moves to another folder too.
            run(&p, kind, &[rename("a.txt", "e/a2.txt")]).unwrap();
            want.remove("a.txt");
            want.insert("e/a2.txt".into(), Some(b"alpha".to_vec()));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
        }
    }

    #[test]
    fn rename_to_existing_is_exists() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let orig = std::fs::read(&p).unwrap();
            let r = run(&p, kind, &[rename("a.txt", "d")]);
            assert_eq!(r, Err(ArchiveError::Exists("d".into())), "{kind:?}");
            let r = run(&p, kind, &[rename("nope", "x")]);
            assert_eq!(r, Err(ArchiveError::NotFound("nope".into())), "{kind:?}");
            assert_eq!(std::fs::read(&p).unwrap(), orig);
            assert!(leftovers(t.path()).is_empty());
        }
    }

    #[test]
    fn mkdir_creates_empty_dir() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            run(
                &p,
                kind,
                &[Edit::Mkdir {
                    inner: "d/new".into(),
                }],
            )
            .unwrap();
            let mut want = base();
            want.insert("d/new".into(), None);
            assert_eq!(contents(&p, kind), want, "{kind:?}");
            let idx = read_index(&p, kind).unwrap();
            assert!(super::super::list_inner(&idx, "d/new").unwrap().is_empty());
        }
    }

    #[test]
    fn mkdir_existing_is_exists() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let orig = std::fs::read(&p).unwrap();
            for existing in ["d", "a.txt"] {
                let r = run(
                    &p,
                    kind,
                    &[Edit::Mkdir {
                        inner: existing.into(),
                    }],
                );
                assert_eq!(r, Err(ArchiveError::Exists(existing.into())), "{kind:?}");
            }
            assert_eq!(std::fs::read(&p).unwrap(), orig);
        }
    }

    #[test]
    fn edits_apply_in_order() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("n.txt");
        std::fs::write(&src, b"n").unwrap();
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let edits = [
                Edit::Mkdir { inner: "k".into() },
                add(&src, "k/n.txt"),
                rename("k", "q"),
                Edit::Delete {
                    inner: "src".into(),
                },
            ];
            run(&p, kind, &edits).unwrap();
            let mut want = base();
            want.remove("src");
            want.remove("src/lib.rs");
            want.extend(expect(&[("q", None), ("q/n.txt", Some(b"n"))]));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
        }
    }

    #[test]
    fn delete_last_child_keeps_folder() {
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            // `d` exists only implicitly, as the parent of `d/b.txt`.
            build(kind, &p, &[("a.txt", Some(b"a")), ("d/b.txt", Some(b"b"))]);
            run(
                &p,
                kind,
                &[Edit::Delete {
                    inner: "d/b.txt".into(),
                }],
            )
            .unwrap();
            assert_eq!(
                contents(&p, kind),
                expect(&[("a.txt", Some(b"a")), ("d", None)]),
                "{kind:?}"
            );
        }
    }

    const GZ_MSG: &str = "Not possible in a .gz file (it holds exactly one file)";

    #[test]
    fn gz_rules() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.txt.gz");
        make_gz(&p, Some("notes.txt"), b"old content");
        let src = t.path().join("new.bin");
        std::fs::write(&src, b"new content").unwrap();
        let kind = ArchiveKind::Gz;

        run(&p, kind, &[add(&src, "notes.txt")]).unwrap();
        assert_eq!(
            contents(&p, kind),
            expect(&[("notes.txt", Some(b"new content"))])
        );

        run(&p, kind, &[rename("notes.txt", "renamed.txt")]).unwrap();
        assert_eq!(
            contents(&p, kind),
            expect(&[("renamed.txt", Some(b"new content"))])
        );

        let orig = std::fs::read(&p).unwrap();
        let unsupported = Err(ArchiveError::Unsupported(GZ_MSG.into()));
        for edits in [
            vec![Edit::Mkdir { inner: "x".into() }],
            vec![Edit::Delete {
                inner: "renamed.txt".into(),
            }],
            vec![add(&src, "other.txt")],
            vec![rename("renamed.txt", "a"), rename("a", "b")],
        ] {
            assert_eq!(run(&p, kind, &edits), unsupported, "{edits:?}");
        }
        assert_eq!(std::fs::read(&p).unwrap(), orig);
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn encrypted_zip_write_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip_aes(&p, &[("d", None), ("d/s.txt", Some(b"secret"))], "pw");
        let orig = std::fs::read(&p).unwrap();
        let r = run(&p, ArchiveKind::Zip, &[Edit::Mkdir { inner: "x".into() }]);
        assert_eq!(
            r,
            Err(ArchiveError::Unsupported(
                "Writing into encrypted archives is not supported".into()
            ))
        );
        assert_eq!(std::fs::read(&p).unwrap(), orig);
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn rewrite_cancel_leaves_original_identical() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("big.bin");
        let big: Vec<u8> = (0..(3u32 << 20))
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        std::fs::write(&src, &big).unwrap();
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            let inner = if kind == ArchiveKind::Gz {
                make_gz(&p, Some("a.txt"), &big[..1 << 20]);
                "a.txt"
            } else {
                build(kind, &p, BASE);
                "big.bin"
            };
            let orig = std::fs::read(&p).unwrap();
            let obs = Obs {
                cancel_after: 1,
                ..Default::default()
            };
            let r = rewrite(&p, kind, &[add(&src, inner)], &IndexCache::new(), &obs);
            assert_eq!(r, Err(ArchiveError::Cancelled), "{kind:?}");
            assert_eq!(std::fs::read(&p).unwrap(), orig, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn rewrite_source_error_leaves_original_identical() {
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let orig = std::fs::read(&p).unwrap();
            let r = run(&p, kind, &[add(&t.path().join("missing"), "a.txt")]);
            assert!(matches!(r, Err(ArchiveError::Io(_))), "{kind:?}: {r:?}");
            assert_eq!(std::fs::read(&p).unwrap(), orig, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn rewrite_source_vanishing_while_writing_leaves_original_identical() {
        // The source is checked while planning; an unreadable file only fails when it is read.
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("locked.txt");
        std::fs::write(&src, b"x").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&src).is_ok() {
            return; // running as root
        }
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let orig = std::fs::read(&p).unwrap();
            let r = run(&p, kind, &[add(&src, "a.txt")]);
            assert!(matches!(r, Err(ArchiveError::Io(_))), "{kind:?}: {r:?}");
            assert_eq!(std::fs::read(&p).unwrap(), orig, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn rewrite_keeps_permissions() {
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
            run(&p, kind, &[rename("a.txt", "z.txt")]).unwrap();
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o7777, 0o640, "{kind:?}");
        }
    }

    #[test]
    fn rewrite_invalidates_cache() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar(&p, BASE);
        let cache = IndexCache::new();
        let before = cache.get(&p, ArchiveKind::Tar).unwrap();
        rewrite(
            &p,
            ArchiveKind::Tar,
            &[Edit::Mkdir { inner: "n".into() }],
            &cache,
            &Obs::default(),
        )
        .unwrap();
        let after = cache.get(&p, ArchiveKind::Tar).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&before, &after));
        assert!(after.entries.iter().any(|e| e.path == "n" && e.is_dir));
    }

    #[test]
    fn rewrite_through_symlink_keeps_link() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real.zip");
        make_zip(&real, BASE);
        let link = t.path().join("link.zip");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        run(
            &link,
            ArchiveKind::Zip,
            &[Edit::Mkdir { inner: "n".into() }],
        )
        .unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let idx = read_index(&real, ArchiveKind::Zip).unwrap();
        assert!(idx.entries.iter().any(|e| e.path == "n"));
    }

    #[test]
    fn zip_raw_copy_keeps_compressed_bytes() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        let text = b"compress me ".repeat(500);
        make_zip(
            &p,
            &[("keep.txt", Some(&text[..])), ("other.txt", Some(b"o"))],
        );
        let raw = |p: &Path, name: &str| {
            let mut z = zip::ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap();
            let f = z.by_name(name).unwrap();
            (
                f.compressed_size(),
                f.crc32(),
                f.unix_mode(),
                f.last_modified(),
            )
        };
        let before = raw(&p, "keep.txt");
        assert!(before.0 < text.len() as u64);
        run(
            &p,
            ArchiveKind::Zip,
            &[
                Edit::Delete {
                    inner: "other.txt".into(),
                },
                Edit::Mkdir { inner: "n".into() },
            ],
        )
        .unwrap();
        assert_eq!(raw(&p, "keep.txt"), before);
        // A renamed entry is still a raw copy.
        run(&p, ArchiveKind::Zip, &[rename("keep.txt", "n/kept.txt")]).unwrap();
        assert_eq!(raw(&p, "n/kept.txt"), before);
    }

    #[test]
    fn zip_new_entries_are_deflated_with_mode_and_mtime() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("x.txt");
        std::fs::write(&src, b"deflate me ".repeat(100)).unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o600)).unwrap();
        filetime::set_file_mtime(&src, filetime::FileTime::from_unix_time(1_600_000_000, 0))
            .unwrap();
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, BASE);
        run(&p, ArchiveKind::Zip, &[add(&src, "x.txt")]).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&p).unwrap()).unwrap();
        let f = z.by_name("x.txt").unwrap();
        assert_eq!(f.compression(), zip::CompressionMethod::Deflated);
        assert_eq!(f.unix_mode().unwrap() & 0o7777, 0o600);
        drop(f);
        let idx = read_index(&p, ArchiveKind::Zip).unwrap();
        let e = idx.entries.iter().find(|e| e.path == "x.txt").unwrap();
        assert_eq!(e.mtime, 1_600_000_000);
    }

    #[test]
    fn tar_keeps_unchanged_entry_metadata() {
        let t = tempfile::tempdir().unwrap();
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz] {
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            run(
                &p,
                kind,
                &[rename("d", "e"), Edit::Mkdir { inner: "n".into() }],
            )
            .unwrap();
            let idx = read_index(&p, kind).unwrap();
            for path in ["a.txt", "e", "e/b.txt"] {
                let e = idx.entries.iter().find(|e| e.path == path).unwrap();
                assert_eq!(e.mtime, TEST_MTIME as i64, "{kind:?} {path}");
            }
        }
    }

    #[test]
    fn tar_long_names_survive() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("f");
        std::fs::write(&src, b"long").unwrap();
        let long = format!("{}/{}", "x".repeat(120), "y".repeat(120));
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz] {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            run(&p, kind, &[add(&src, &long)]).unwrap();
            run(&p, kind, &[rename("a.txt", &format!("{long}2"))]).unwrap();
            run(&p, kind, &[Edit::Mkdir { inner: "n".into() }]).unwrap();
            let c = contents(&p, kind);
            assert_eq!(c[&long], Some(b"long".to_vec()), "{kind:?}");
            assert_eq!(c[&format!("{long}2")], Some(b"alpha".to_vec()), "{kind:?}");
        }
    }

    /// A 7z whose folder `ro` has mode 0o500, as staging would extract it.
    fn make_7z_readonly_dir(p: &Path) {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
        let mut w = ArchiveWriter::create(p).unwrap();
        let mut d = ArchiveEntry::new_directory("ro");
        d.has_windows_attributes = true;
        d.windows_attributes = 0x10 | 0x8000 | ((super::super::S_IFDIR | 0o500) << 16);
        w.push_archive_entry::<&[u8]>(d, None).unwrap();
        let mut f = ArchiveEntry::new_file("ro/f.txt");
        f.has_windows_attributes = true;
        f.windows_attributes = 0x20 | 0x8000 | ((super::super::S_IFREG | 0o400) << 16);
        w.push_archive_entry(f, Some(&b"inside"[..])).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn sevenz_staging_is_removed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.7z");
        make_7z_readonly_dir(&p);
        run(
            &p,
            ArchiveKind::SevenZ,
            &[Edit::Mkdir { inner: "n".into() }],
        )
        .unwrap();
        assert!(leftovers(t.path()).is_empty());
        let idx = read_index(&p, ArchiveKind::SevenZ).unwrap();
        let ro = idx.entries.iter().find(|e| e.path == "ro").unwrap();
        assert_eq!(ro.mode & 0o7777, 0o500);
        let f = idx.entries.iter().find(|e| e.path == "ro/f.txt").unwrap();
        assert_eq!(f.mode & 0o7777, 0o400);

        // Error after staging: the staged tree is removed and the original kept.
        let orig = std::fs::read(&p).unwrap();
        let r = run(
            &p,
            ArchiveKind::SevenZ,
            &[add(&t.path().join("missing"), "ro/g.txt")],
        );
        assert!(matches!(r, Err(ArchiveError::Io(_))), "{r:?}");
        assert_eq!(std::fs::read(&p).unwrap(), orig);
        assert!(leftovers(t.path()).is_empty());

        // Cancel during staging.
        let big = vec![3u8; 3 << 20];
        let p2 = t.path().join("b.7z");
        make_7z(&p2, &[("big.bin", Some(&big[..])), ("x", Some(b"x"))]);
        let orig = std::fs::read(&p2).unwrap();
        let obs = Obs {
            cancel_after: 1,
            ..Default::default()
        };
        let r = rewrite(
            &p2,
            ArchiveKind::SevenZ,
            &[Edit::Mkdir { inner: "n".into() }],
            &IndexCache::new(),
            &obs,
        );
        assert_eq!(r, Err(ArchiveError::Cancelled));
        assert_eq!(std::fs::read(&p2).unwrap(), orig);
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn invalid_names_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, BASE);
        for bad in ["", "../x", "/abs", "a/../../b"] {
            let r = run(&p, ArchiveKind::Zip, &[Edit::Mkdir { inner: bad.into() }]);
            assert!(matches!(r, Err(ArchiveError::Io(_))), "{bad:?}: {r:?}");
        }
        let r = run(&p, ArchiveKind::Zip, &[rename("d", "d/inside")]);
        assert!(r.is_err());
    }

    #[test]
    fn rewrite_cancel_mid_copy_leaves_original_identical() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("big.bin");
        let big: Vec<u8> = (0..(3u32 << 20))
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        std::fs::write(&src, &big).unwrap();
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            let inner = if kind == ArchiveKind::Gz {
                make_gz(&p, Some("a.txt"), b"small");
                "a.txt"
            } else {
                build(kind, &p, &[("d", None)]);
                "big.bin"
            };
            let orig = std::fs::read(&p).unwrap();
            let obs = Obs {
                cancel_mid: true,
                ..Default::default()
            };
            let r = rewrite(&p, kind, &[add(&src, inner)], &IndexCache::new(), &obs);
            assert_eq!(r, Err(ArchiveError::Cancelled), "{kind:?}");
            // The cancel came while the big file was streaming.
            assert!(obs.mid_seen.load(Ordering::SeqCst), "{kind:?}");
            let max = obs.max_done.load(Ordering::SeqCst);
            assert!(
                max > 0 && max < obs.total.load(Ordering::SeqCst),
                "{kind:?}"
            );
            assert_eq!(std::fs::read(&p).unwrap(), orig, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    /// A raw 7z entry: name, unix mode with type bits, data (`None` for a folder).
    type Raw7z<'a> = (&'a str, u32, Option<&'a [u8]>);

    /// A 7z with raw entries.
    fn make_7z_raw(p: &Path, entries: &[Raw7z]) {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
        let mut w = ArchiveWriter::create(p).unwrap();
        for (name, mode, data) in entries {
            let dir = data.is_none();
            let mut e = if dir {
                ArchiveEntry::new_directory(name)
            } else {
                ArchiveEntry::new_file(name)
            };
            e.has_windows_attributes = true;
            e.windows_attributes = if dir { 0x10 } else { 0x20 } | 0x8000 | (mode << 16);
            w.push_archive_entry(e, *data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn sevenz_staging_error_aborts_whatever_the_answer() {
        use super::super::{S_IFLNK, S_IFREG};
        let cases: [&[Raw7z]; 2] = [
            // `f/g` cannot be staged because `f` is a file.
            &[
                ("f", S_IFREG | 0o644, Some(b"x")),
                ("f/g", S_IFREG | 0o644, Some(b"y")),
            ],
            // A link that leaves the staging folder is refused while staging.
            &[
                ("ok.txt", S_IFREG | 0o644, Some(b"ok")),
                ("l", S_IFLNK | 0o777, Some(b"../../outside")),
            ],
        ];
        for entries in cases {
            for answer in [ErrorAnswer::Skip, ErrorAnswer::Retry, ErrorAnswer::SkipAll] {
                let t = tempfile::tempdir().unwrap();
                let p = t.path().join("a.7z");
                make_7z_raw(&p, entries);
                let orig = std::fs::read(&p).unwrap();
                let obs = Obs {
                    answer: Some(answer.clone()),
                    ..Default::default()
                };
                let r = rewrite(
                    &p,
                    ArchiveKind::SevenZ,
                    &[Edit::Mkdir { inner: "n".into() }],
                    &IndexCache::new(),
                    &obs,
                );
                assert!(matches!(r, Err(ArchiveError::Io(_))), "{answer:?}: {r:?}");
                assert_eq!(std::fs::read(&p).unwrap(), orig);
                assert!(leftovers(t.path()).is_empty());
            }
        }
    }

    #[test]
    fn sevenz_unsafe_names_refused() {
        use super::super::S_IFREG;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.7z");
        make_7z_raw(
            &p,
            &[
                ("../evil", S_IFREG | 0o644, Some(b"x")),
                ("ok.txt", S_IFREG | 0o644, Some(b"ok")),
            ],
        );
        let orig = std::fs::read(&p).unwrap();
        let r = run(
            &p,
            ArchiveKind::SevenZ,
            &[Edit::Mkdir { inner: "n".into() }],
        );
        assert_eq!(
            r,
            Err(ArchiveError::Unsupported(
                "This archive contains entries with unsafe names; fxfm will not rewrite it".into()
            ))
        );
        assert_eq!(std::fs::read(&p).unwrap(), orig);
        assert!(leftovers(t.path()).is_empty());
    }

    /// A tar with `d/f` and the hard link `h -> d/f`.
    fn make_tar_hardlink(p: &Path, gz: bool) {
        use std::io::Write;
        let w: Box<dyn std::io::Write> = if gz {
            Box::new(flate2::write::GzEncoder::new(
                std::fs::File::create(p).unwrap(),
                flate2::Compression::fast(),
            ))
        } else {
            Box::new(std::fs::File::create(p).unwrap())
        };
        let mut b = tar::Builder::new(w);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Regular);
        h.set_mode(0o644);
        h.set_size(4);
        b.append_data(&mut h, "d/f", &b"data"[..]).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Link);
        h.set_mode(0o644);
        h.set_size(0);
        b.append_link(&mut h, "h", "d/f").unwrap();
        b.into_inner().unwrap().flush().unwrap();
    }

    #[test]
    fn tar_hard_link_targets_refused() {
        let msg = "This archive has hard links to d/f; deleting or renaming it is not supported";
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz] {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            make_tar_hardlink(&p, kind == ArchiveKind::TarGz);
            let orig = std::fs::read(&p).unwrap();
            for edits in [
                vec![Edit::Delete {
                    inner: "d/f".into(),
                }],
                vec![Edit::Delete { inner: "d".into() }],
                vec![rename("d/f", "g")],
                vec![rename("d", "e")],
            ] {
                let r = run(&p, kind, &edits);
                assert_eq!(
                    r,
                    Err(ArchiveError::Unsupported(msg.into())),
                    "{kind:?} {edits:?}"
                );
                assert_eq!(std::fs::read(&p).unwrap(), orig, "{kind:?}");
                assert!(leftovers(t.path()).is_empty());
            }
            // The link itself can go.
            run(&p, kind, &[Edit::Delete { inner: "h".into() }]).unwrap();
            assert_eq!(
                contents(&p, kind),
                expect(&[("d", None), ("d/f", Some(b"data"))])
            );
        }
    }

    /// Runs `f` on the first progress report; errors cancel.
    struct Hook<F: Fn() + Send + Sync> {
        f: F,
        fired: AtomicBool,
    }

    impl<F: Fn() + Send + Sync> Hook<F> {
        fn new(f: F) -> Self {
            Hook {
                f,
                fired: AtomicBool::new(false),
            }
        }
    }

    impl<F: Fn() + Send + Sync> OpObserver for Hook<F> {
        fn progress(&self, _p: &Progress) {
            if !self.fired.swap(true, Ordering::SeqCst) {
                (self.f)();
            }
        }
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            panic!("no conflicts expected");
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            ErrorAnswer::Cancel
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn concurrent_rewrites_of_one_archive_keep_both_edits() {
        let local = tempfile::tempdir().unwrap();
        let x = local.path().join("x.txt");
        let y = local.path().join("y.txt");
        std::fs::write(&x, b"from A").unwrap();
        std::fs::write(&y, b"from B").unwrap();
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let cache = IndexCache::new();
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            std::thread::scope(|s| {
                let (p, cache, y) = (&p, &cache, &y);
                let b = s.spawn(move || {
                    rx.recv().unwrap();
                    rewrite(p, kind, &[add(y, "y.txt")], cache, &Obs::default())
                });
                // A starts writing, lets B start, and takes its time.
                let hook = Hook::new(|| {
                    tx.send(()).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(300));
                });
                let a = rewrite(p, kind, &[add(&x, "x.txt")], cache, &hook);
                assert_eq!(a, Ok(()), "{kind:?}");
                assert_eq!(b.join().unwrap(), Ok(()), "{kind:?}");
            });
            let mut want = base();
            want.insert("x.txt".into(), Some(b"from A".to_vec()));
            want.insert("y.txt".into(), Some(b"from B".to_vec()));
            assert_eq!(contents(&p, kind), want, "{kind:?}");
            assert!(leftovers(t.path()).is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn original_changed_while_writing_is_not_replaced() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("a.txt");
        std::fs::write(&src, b"mine").unwrap();
        for kind in KINDS.into_iter().chain([ArchiveKind::Gz]) {
            for in_place in [false, true] {
                let t = tempfile::tempdir().unwrap();
                let p = t.path().join(name(kind));
                build(kind, &p, BASE);
                // Someone else's version: written in place, or renamed over the original.
                let theirs = t.path().join(format!("theirs-{}", name(kind)));
                build(kind, &theirs, &[("a.txt", Some(b"theirs"))]);
                let mut want = std::fs::read(&theirs).unwrap();
                if in_place {
                    want = std::fs::read(&p).unwrap();
                    want.extend_from_slice(b"junk");
                }
                let hook = Hook::new(|| {
                    if in_place {
                        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
                        std::io::Write::write_all(&mut f, b"junk").unwrap();
                    } else {
                        std::fs::rename(&theirs, &p).unwrap();
                    }
                });
                let r = rewrite(&p, kind, &[add(&src, "a.txt")], &IndexCache::new(), &hook);
                assert_eq!(
                    r,
                    Err(ArchiveError::Io(format!(
                        "{} was changed by someone else while fxfm was writing it; nothing was changed",
                        name(kind)
                    ))),
                    "{kind:?} in place: {in_place}"
                );
                assert_eq!(std::fs::read(&p).unwrap(), want, "{kind:?} {in_place}");
                assert!(leftovers(t.path()).is_empty(), "{kind:?} {in_place}");
            }
        }
    }

    /// A pid that no process has.
    fn dead_pid() -> u32 {
        (1_000_000_000..)
            .find(|&p| !crate::process::pid_alive(p))
            .unwrap()
    }

    #[test]
    fn crash_leftovers_of_dead_processes_are_swept() {
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("x.txt");
        std::fs::write(&src, b"x").unwrap();
        let dead = dead_pid();
        let own = std::process::id();
        for kind in KINDS {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join(name(kind));
            build(kind, &p, BASE);
            let n = name(kind);
            let dead_tmp = t.path().join(format!(".{n}.fxfm-{dead}-3.tmp"));
            let dead_stage = t.path().join(format!(".{n}.fxfm-{dead}-4.stage"));
            let live_tmp = t.path().join(format!(".{n}.fxfm-{own}-999999.tmp"));
            let other_tmp = t.path().join(format!(".other.zip.fxfm-{dead}-1.tmp"));
            let odd = t.path().join(format!(".{n}.fxfm-{dead}-x.tmp"));
            std::fs::write(&dead_tmp, b"partial").unwrap();
            std::fs::create_dir_all(dead_stage.join("d")).unwrap();
            std::fs::write(dead_stage.join("d/f"), b"f").unwrap();
            std::fs::set_permissions(dead_stage.join("d"), std::fs::Permissions::from_mode(0o500))
                .unwrap();
            // A link inside a stage folder is removed, never followed.
            let outside = local.path().join("keep");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("k"), b"k").unwrap();
            std::os::unix::fs::symlink(&outside, dead_stage.join("link")).unwrap();
            for f in [&live_tmp, &other_tmp, &odd] {
                std::fs::write(f, b"keep").unwrap();
            }
            run(&p, kind, &[add(&src, "x.txt")]).unwrap();
            assert!(!dead_tmp.exists(), "{kind:?}");
            assert!(!dead_stage.exists(), "{kind:?}");
            assert!(outside.join("k").exists(), "{kind:?}");
            for f in [&live_tmp, &other_tmp, &odd] {
                assert_eq!(std::fs::read(f).unwrap(), b"keep", "{kind:?} {f:?}");
            }
        }
    }

    /// A zip as written by Windows tools: a legacy header name plus an Info-ZIP Unicode Path
    /// extra field (0x7075) holding the real name.
    fn make_zip_upath(p: &Path) {
        use zip::write::FullFileOptions;
        let legacy = "_.txt";
        let mut crc = flate2::Crc::new();
        crc.update(legacy.as_bytes());
        let mut data = vec![1u8];
        data.extend_from_slice(&crc.sum().to_le_bytes());
        data.extend_from_slice("ä.txt".as_bytes());
        let mut opts = FullFileOptions::default().unix_permissions(0o644);
        opts.add_extra_field(0x7075, &data, false).unwrap();
        let mut w = zip::ZipWriter::new(File::create(p).unwrap());
        w.start_file(legacy, opts).unwrap();
        w.write_all(b"umlaut").unwrap();
        w.start_file("b.txt", FullFileOptions::default()).unwrap();
        w.write_all(b"bee").unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn zip_rename_of_unicode_path_entry_shows_new_name() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip_upath(&p);
        assert_eq!(
            contents(&p, ArchiveKind::Zip),
            expect(&[("b.txt", Some(b"bee")), ("ä.txt", Some(b"umlaut"))])
        );
        run(&p, ArchiveKind::Zip, &[rename("ä.txt", "c.txt")]).unwrap();
        assert_eq!(
            contents(&p, ArchiveKind::Zip),
            expect(&[("b.txt", Some(b"bee")), ("c.txt", Some(b"umlaut"))])
        );
        // An unchanged entry keeps its Unicode name.
        run(&p, ArchiveKind::Zip, &[rename("b.txt", "ö.txt")]).unwrap();
        run(&p, ArchiveKind::Zip, &[rename("c.txt", "ü.txt")]).unwrap();
        assert_eq!(
            contents(&p, ArchiveKind::Zip),
            expect(&[("ö.txt", Some(b"bee")), ("ü.txt", Some(b"umlaut"))])
        );
        // Info-ZIP unzip, when present, agrees.
        if let Ok(out) = std::process::Command::new("unzip")
            .arg("-Z1")
            .arg(&p)
            .output()
        {
            if out.status.success() {
                let listed = String::from_utf8_lossy(&out.stdout);
                let mut names: Vec<&str> = listed.lines().collect();
                names.sort_unstable();
                assert_eq!(names, ["ö.txt", "ü.txt"], "{listed}");
            }
        }
    }
}
