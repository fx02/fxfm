//! Packing local files into a new zip archive (Files → Pack files, Alt+F5).
//!
//! The sources are listed first (folders recursively, links as links), then the archive is
//! written to a hidden `.<name>.fxfm-<pid>-<n>.part` next to the target and renamed into
//! place. A cancel or a write error removes the `.part`; an existing target is only replaced
//! when the caller says so, and stays untouched otherwise. Unreadable items go through
//! [`OpObserver::error`] (Retry / Skip / Skip all / Cancel) like a copy.
//!
//! The password is never logged or put into an error message; it lives in a [`Zeroizing`]
//! string that is wiped when the options are dropped.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use zeroize::Zeroizing;
use zip::unstable::write::FileOptionsExt;
use zip::write::FullFileOptions;
use zip::ZipWriter;

use super::write::{sibling, TempFile};
use super::ziptime::entry_options;
use crate::ops::{
    location, rename_noreplace, ErrorAnswer, OpError, OpObserver, OpSummary, Progress,
};

/// Refusal for a target that is one of the sources or inside one of them.
pub const INTO_ITSELF: &str = "Cannot pack an archive into itself";

/// How a password-protected zip is encrypted.
#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ZipEncryption {
    /// WinZip AES-256 (AE-2): secure, but Finder's Archive Utility cannot open it.
    #[default]
    Aes256,
    /// The legacy PKWARE cipher: opens everywhere, easy to break.
    ZipCrypto,
}

/// A password as it arrives from the UI: wiped from memory when dropped, never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub Zeroizing<String>);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Secret(Zeroizing::new(s)))
    }
}

pub struct PackOptions {
    /// None or empty: no encryption.
    pub password: Option<Zeroizing<String>>,
    pub encryption: ZipEncryption,
    /// Replace an existing target; otherwise an existing target is `OpError::Exists`.
    pub overwrite: bool,
}

impl fmt::Debug for PackOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackOptions")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("encryption", &self.encryption)
            .field("overwrite", &self.overwrite)
            .finish()
    }
}

/// Packs `sources` (local paths) into the zip `target`. Entry names are the sources' paths
/// relative to `base` (the folder they are listed in). A cancel gives
/// `Ok(summary)` with `cancelled` set and leaves nothing behind.
pub fn pack_zip(
    sources: &[PathBuf],
    base: &Path,
    target: &Path,
    opts: &PackOptions,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    pack_impl(sources, base, target, opts, obs, None)
}

/// `pack_zip`; `write_limit` makes writing fail after that many bytes (test seam for a full
/// disk).
fn pack_impl(
    sources: &[PathBuf],
    base: &Path,
    target: &Path,
    opts: &PackOptions,
    obs: &dyn OpObserver,
    write_limit: Option<u64>,
) -> Result<OpSummary, OpError> {
    let password = match opts.password.as_deref() {
        Some(p) if p.is_empty() => return Err(OpError::Io("The password is empty".into())),
        Some(p) => Some(p.as_str()),
        None => None,
    };
    check_not_into_itself(sources, target)?;
    let names = sources
        .iter()
        .map(|src| entry_name(src, base))
        .collect::<Result<Vec<_>, _>>()?;
    check_target(target, opts.overwrite)?;
    let mut p = Packer {
        obs,
        summary: OpSummary::default(),
        progress: Progress::default(),
        skip_all: false,
    };
    let mut items = Vec::new();
    for (src, name) in sources.iter().zip(names) {
        if p.collect(src, name, &mut items).is_err() {
            p.summary.cancelled = true;
            return Ok(p.summary);
        }
    }
    let files = items.iter().filter(|i| i.kind != Kind::Dir);
    p.progress.total_files = files.clone().count() as u64;
    p.progress.total_bytes = files.map(|i| i.size).sum();
    p.emit();
    let mut part = create_part(target)?;
    let file = OpenOptions::new()
        .write(true)
        .open(&part.path)
        .map_err(|e| fail(target, e))?;
    let file = LimitedFile {
        f: file,
        left: write_limit,
    };
    let out = match p.write_zip(file, &items, password, opts.encryption, target) {
        Ok(f) => f,
        Err(Fatal::Stop) => {
            p.summary.cancelled = true;
            return Ok(p.summary);
        }
        Err(Fatal::Op(e)) => return Err(e),
    };
    out.sync_all().map_err(|e| fail(target, e))?;
    drop(out);
    if obs.is_cancelled() {
        p.summary.cancelled = true;
        return Ok(p.summary);
    }
    let placed = if opts.overwrite {
        fs::rename(&part.path, target)
    } else {
        rename_noreplace(&part.path, target)
    };
    match placed {
        Ok(()) => part.done = true,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(OpError::Exists(target.to_string_lossy().into_owned()))
        }
        Err(e) => return Err(fail(target, e)),
    }
    if let Some(dir) = target.parent() {
        if let Err(e) = File::open(dir).and_then(|d| d.sync_all()) {
            log::warn!("fsync of {} failed: {e}", dir.display());
        }
    }
    Ok(p.summary)
}

fn fail(target: &Path, e: impl fmt::Display) -> OpError {
    OpError::Io(format!("{}: {e}", target.display()))
}

/// The target's folder exists, and the target is not a folder (nor, without `overwrite`,
/// anything else).
fn check_target(target: &Path, overwrite: bool) -> Result<(), OpError> {
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .ok_or_else(|| OpError::Io(format!("{}: not a file path", target.display())))?;
    if !dir.is_dir() {
        return Err(OpError::Io(format!("{}: no such folder", dir.display())));
    }
    match fs::symlink_metadata(target) {
        Ok(md) if md.is_dir() => Err(OpError::Io(format!(
            "{}: a folder of that name exists",
            target.display()
        ))),
        Ok(_) if !overwrite => Err(OpError::Exists(target.to_string_lossy().into_owned())),
        _ => Ok(()),
    }
}

/// A new, empty `.<name>.fxfm-<pid>-<n>.part` next to `target`.
fn create_part(target: &Path) -> Result<TempFile, OpError> {
    let mut tries = 0;
    loop {
        let path = sibling(target, "part");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(TempFile { path, done: false }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
            Err(e) => return Err(fail(target, e)),
        }
    }
}

/// Refuses a target that is a source itself (also through another spelling of the same
/// file) or lies inside a source folder: the new archive would replace what it is made of.
fn check_not_into_itself(sources: &[PathBuf], target: &Path) -> Result<(), OpError> {
    let t = location(target);
    let tmd = fs::symlink_metadata(target).ok();
    for src in sources {
        let s = location(src);
        let smd = fs::symlink_metadata(src).ok();
        let same_file =
            matches!((&smd, &tmd), (Some(a), Some(b)) if a.dev() == b.dev() && a.ino() == b.ino());
        let is_dir = smd.as_ref().is_some_and(|m| m.is_dir());
        if s == t || same_file || (is_dir && t.starts_with(&s)) {
            return Err(OpError::Io(INTO_ITSELF.into()));
        }
    }
    Ok(())
}

/// `src` relative to `base` with `/` separators (its file name when it is not below `base`).
/// Never absolute and never with `..`: such a source is refused.
fn entry_name(src: &Path, base: &Path) -> Result<String, OpError> {
    let invalid = || {
        OpError::Io(format!(
            "{}: cannot be packed (no file name)",
            src.display()
        ))
    };
    let rel = src
        .strip_prefix(base)
        .ok()
        .filter(|r| !r.as_os_str().is_empty())
        .or_else(|| src.file_name().map(Path::new))
        .ok_or_else(invalid)?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(n) => parts.push(n.to_string_lossy()),
            Component::CurDir => {}
            _ => return Err(invalid()),
        }
    }
    if parts.is_empty() {
        return Err(invalid());
    }
    Ok(parts.join("/"))
}

/// The `.part` file, failing writes once `left` bytes are used up (`None`: no limit).
struct LimitedFile {
    f: File,
    left: Option<u64>,
}

impl Write for LimitedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(left) = self.left.as_mut() {
            if *left < buf.len() as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "No space left on device",
                ));
            }
            *left -= buf.len() as u64;
        }
        self.f.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}

impl Seek for LimitedFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.f.seek(pos)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Link,
}

struct Item {
    src: PathBuf,
    name: String,
    kind: Kind,
    size: u64,
    mode: u32,
    mtime: i64,
}

/// The user cancelled.
struct Stop;

enum Fatal {
    Stop,
    /// Writing the archive failed: the whole pack fails.
    Op(OpError),
}

impl From<Stop> for Fatal {
    fn from(_: Stop) -> Self {
        Fatal::Stop
    }
}

/// How reading one source into the archive ended.
enum CopyError {
    Read(io::Error),
    Write(io::Error),
    Stop,
}

const BUF: usize = 256 * 1024;

struct Packer<'a> {
    obs: &'a dyn OpObserver,
    summary: OpSummary,
    progress: Progress,
    skip_all: bool,
}

impl Packer<'_> {
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

    /// Ok(true) = retry, Ok(false) = skip.
    fn on_error(&mut self, path: &Path, msg: &str) -> Result<bool, Stop> {
        if self.skip_all {
            return Ok(false);
        }
        match self.obs.error(&path.to_string_lossy(), msg) {
            ErrorAnswer::Retry => Ok(true),
            ErrorAnswer::Skip => Ok(false),
            ErrorAnswer::SkipAll => {
                self.skip_all = true;
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

    fn skip(&mut self, size: u64) {
        self.summary.skipped += 1;
        self.progress.done_files += 1;
        self.progress.done_bytes += size;
        self.emit();
    }

    fn done(&mut self) {
        self.summary.done += 1;
        self.progress.done_files += 1;
        self.emit();
    }

    /// Lists `src` (a folder with everything below it) as items named from `name`.
    fn collect(&mut self, src: &Path, name: String, out: &mut Vec<Item>) -> Result<(), Stop> {
        self.check_cancel()?;
        let md = self.attempt(src, || {
            let md = fs::symlink_metadata(src)?;
            let ft = md.file_type();
            if ft.is_dir() || ft.is_file() || ft.is_symlink() {
                Ok(md)
            } else {
                Err(io::Error::other("not a file, folder or link"))
            }
        })?;
        let Some(md) = md else {
            self.summary.skipped += 1;
            return Ok(());
        };
        let ft = md.file_type();
        let kind = if ft.is_dir() {
            Kind::Dir
        } else if ft.is_symlink() {
            Kind::Link
        } else {
            Kind::File
        };
        out.push(Item {
            src: src.to_path_buf(),
            name: name.clone(),
            kind,
            size: if kind == Kind::File { md.len() } else { 0 },
            mode: md.mode(),
            mtime: md.mtime(),
        });
        if kind != Kind::Dir {
            return Ok(());
        }
        let Some(children) = self.attempt(src, || read_dir_sorted(src))? else {
            self.summary.skipped += 1;
            return Ok(());
        };
        for c in children {
            let child_name = format!("{name}/{}", c.to_string_lossy());
            self.collect(&src.join(&c), child_name, out)?;
        }
        Ok(())
    }

    fn write_zip(
        &mut self,
        file: LimitedFile,
        items: &[Item],
        password: Option<&str>,
        encryption: ZipEncryption,
        target: &Path,
    ) -> Result<File, Fatal> {
        let bad = |e: zip::result::ZipError| Fatal::Op(fail(target, e));
        let mut w = ZipWriter::new(BufWriter::new(file));
        for it in items {
            self.check_cancel()?;
            self.progress.current = it.src.to_string_lossy().into_owned();
            // Local DOS time plus an extended timestamp, as Info-ZIP and Finder expect.
            let opts = entry_options(it.mode, it.mtime);
            let opts = encrypt(opts, password, encryption).map_err(bad)?;
            match it.kind {
                Kind::Dir => w.add_directory(it.name.as_str(), opts).map_err(bad)?,
                Kind::Link => match self.attempt(&it.src, || fs::read_link(&it.src))? {
                    Some(link) => {
                        w.add_symlink(it.name.as_str(), link.to_string_lossy(), opts)
                            .map_err(bad)?;
                        self.done();
                    }
                    None => self.skip(0),
                },
                Kind::File => self.add_file(&mut w, it, opts, target)?,
            }
        }
        let bw = w.finish().map_err(bad)?;
        bw.into_inner()
            .map(|l| l.f)
            .map_err(|e| Fatal::Op(fail(target, e.error())))
    }

    /// Adds one file; a read error asks (Retry starts the entry again, Skip drops it).
    fn add_file(
        &mut self,
        w: &mut ZipWriter<BufWriter<LimitedFile>>,
        it: &Item,
        opts: FullFileOptions<'_, '_>,
        target: &Path,
    ) -> Result<(), Fatal> {
        let bad = |e: zip::result::ZipError| Fatal::Op(fail(target, e));
        loop {
            let Some(f) = self.attempt(&it.src, || File::open(&it.src))? else {
                self.skip(it.size);
                return Ok(());
            };
            let len = f.metadata().map_or(it.size, |m| m.len());
            w.start_file(
                it.name.as_str(),
                opts.clone().large_file(len >= u64::from(u32::MAX)),
            )
            .map_err(bad)?;
            let before = self.progress.done_bytes;
            match self.copy(f, w) {
                Ok(()) => {
                    self.done();
                    return Ok(());
                }
                Err(CopyError::Stop) => return Err(Fatal::Stop),
                Err(CopyError::Write(e)) => return Err(Fatal::Op(fail(target, e))),
                Err(CopyError::Read(e)) => {
                    w.abort_file().map_err(bad)?;
                    self.progress.done_bytes = before;
                    if !self.on_error(&it.src, &e.to_string())? {
                        self.skip(it.size);
                        return Ok(());
                    }
                }
            }
        }
    }

    fn copy(&mut self, mut f: File, w: &mut impl Write) -> Result<(), CopyError> {
        let mut buf = vec![0u8; BUF];
        loop {
            if self.obs.is_cancelled() {
                return Err(CopyError::Stop);
            }
            let n = match f.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CopyError::Read(e)),
            };
            w.write_all(&buf[..n]).map_err(CopyError::Write)?;
            self.progress.done_bytes += n as u64;
            self.emit();
        }
    }
}

fn encrypt<'k>(
    opts: FullFileOptions<'static, 'static>,
    password: Option<&'k str>,
    encryption: ZipEncryption,
) -> zip::result::ZipResult<FullFileOptions<'k, 'static>> {
    match (password, encryption) {
        (None, _) => Ok(opts),
        (Some(pw), ZipEncryption::Aes256) => Ok(opts.with_aes_encryption(zip::AesMode::Aes256, pw)),
        (Some(pw), ZipEncryption::ZipCrypto) => opts.with_deprecated_encryption(pw.as_bytes()),
    }
}

fn read_dir_sorted(dir: &Path) -> io::Result<Vec<OsString>> {
    let mut names = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, Progress};
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Mutex;

    const MTIME: i64 = 1_700_000_000;

    /// Answers errors with `answer`; cancels when `cancel_on_progress` and a progress arrives;
    /// runs `on_progress` on every progress.
    struct Obs {
        answer: ErrorAnswer,
        errors: AtomicU32,
        cancel_on_progress: bool,
        cancelled: AtomicBool,
        on_progress: Mutex<Option<Box<dyn FnMut() + Send>>>,
    }

    fn obs(answer: ErrorAnswer) -> Obs {
        Obs {
            answer,
            errors: AtomicU32::new(0),
            cancel_on_progress: false,
            cancelled: AtomicBool::new(false),
            on_progress: Mutex::new(None),
        }
    }

    impl OpObserver for Obs {
        fn progress(&self, _p: &Progress) {
            if let Some(f) = self.on_progress.lock().unwrap().as_mut() {
                f();
            }
            if self.cancel_on_progress {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            panic!("packing never asks about conflicts");
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            self.errors.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }

    fn plain() -> PackOptions {
        PackOptions {
            password: None,
            encryption: ZipEncryption::Aes256,
            overwrite: false,
        }
    }

    fn with_password(pw: &str, encryption: ZipEncryption) -> PackOptions {
        PackOptions {
            password: Some(Zeroizing::new(pw.to_string())),
            encryption,
            overwrite: false,
        }
    }

    fn set_mtime(p: &Path) {
        filetime::set_symlink_file_times(
            p,
            filetime::FileTime::from_unix_time(MTIME, 0),
            filetime::FileTime::from_unix_time(MTIME, 0),
        )
        .unwrap();
    }

    const UNICODE: &str = "čćž ünï.txt";

    /// base/{a.txt (0750), sub/b.txt, sub/empty/, .hidden, UNICODE, link -> a.txt}.
    fn tree() -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("base");
        std::fs::create_dir_all(base.join("sub/empty")).unwrap();
        std::fs::write(base.join("a.txt"), b"alpha").unwrap();
        std::fs::set_permissions(base.join("a.txt"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        std::fs::write(base.join("sub/b.txt"), b"beta").unwrap();
        std::fs::write(base.join(".hidden"), b"secret").unwrap();
        std::fs::write(base.join(UNICODE), b"unicode").unwrap();
        std::os::unix::fs::symlink("a.txt", base.join("link")).unwrap();
        for p in ["a.txt", "sub/b.txt", ".hidden", UNICODE, "link"] {
            set_mtime(&base.join(p));
        }
        let sources = ["a.txt", "sub", ".hidden", UNICODE, "link"]
            .iter()
            .map(|n| base.join(n))
            .collect();
        (t, base, sources)
    }

    fn open(p: &Path) -> zip::ZipArchive<std::fs::File> {
        zip::ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap()
    }

    fn names(p: &Path) -> Vec<String> {
        let mut z = open(p);
        let mut v: Vec<String> = (0..z.len())
            .map(|i| z.by_index_raw(i).unwrap().name().unwrap().into_owned())
            .collect();
        v.sort();
        v
    }

    fn read(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Vec<u8> {
        let mut f = z.by_name(name).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        v
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".part"))
            .collect()
    }

    #[test]
    fn roundtrip_keeps_tree_links_modes_and_times() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        let s = pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap();
        assert_eq!(
            s,
            OpSummary {
                done: 5,
                skipped: 0,
                cancelled: false
            }
        );
        assert_eq!(
            names(&target),
            [
                ".hidden",
                "a.txt",
                "link",
                "sub/",
                "sub/b.txt",
                "sub/empty/",
                UNICODE
            ]
        );
        let mut z = open(&target);
        assert_eq!(read(&mut z, "a.txt"), b"alpha");
        assert_eq!(read(&mut z, "sub/b.txt"), b"beta");
        assert_eq!(read(&mut z, ".hidden"), b"secret");
        assert_eq!(read(&mut z, UNICODE), b"unicode");
        {
            let a = z.by_name("a.txt").unwrap();
            assert_eq!(a.unix_mode().unwrap() & 0o7777, 0o750);
            assert_eq!(a.compression(), zip::CompressionMethod::Deflated);
            let m = a.last_modified().unwrap();
            // The DOS field holds local time, as Finder and Info-ZIP read it.
            let want = super::super::ziptime::dos_local(MTIME);
            assert_eq!(
                (
                    m.year(),
                    m.month(),
                    m.day(),
                    m.hour(),
                    m.minute(),
                    m.second()
                ),
                (
                    want.year(),
                    want.month(),
                    want.day(),
                    want.hour(),
                    want.minute(),
                    want.second()
                )
            );
            assert!(!a.encrypted());
        }
        {
            let l = z.by_name("link").unwrap();
            assert!(l.is_symlink());
        }
        assert_eq!(read(&mut z, "link"), b"a.txt");
        assert!(z.by_name("sub/empty/").unwrap().is_dir());
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn times_carry_an_extended_timestamp_and_read_back_in_fxfm() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap();
        let mut z = open(&target);
        let a = z.by_name("a.txt").unwrap();
        let ext = a.extra_data_fields().find_map(|f| match f {
            zip::ExtraField::ExtendedTimestamp(ts) => ts.mod_time(),
            _ => None,
        });
        assert_eq!(ext, Some(MTIME as u32), "0x5455 with the UTC mtime");
        drop(a);
        let idx = super::super::read::read_index(&target, super::super::ArchiveKind::Zip).unwrap();
        for name in ["a.txt", "sub/b.txt", "link"] {
            let e = idx.entries.iter().find(|e| e.path == name).unwrap();
            assert_eq!(e.mtime, MTIME, "{name}");
        }
    }

    #[test]
    fn packing_an_archive_onto_itself_is_refused() {
        let (t, base, _) = tree();
        let archive = base.join("a.zip");
        pack_zip(
            &[base.join("a.txt")],
            &base,
            &archive,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap();
        let before = std::fs::read(&archive).unwrap();
        let opts = PackOptions {
            overwrite: true,
            ..plain()
        };
        let into_itself = OpError::Io(INTO_ITSELF.into());
        let e = pack_zip(
            &[archive.clone()],
            &base,
            &archive,
            &opts,
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap_err();
        assert_eq!(e, into_itself);
        assert_eq!(std::fs::read(&archive).unwrap(), before);
        // Into a folder that is packed, at any depth.
        let inner = base.join("sub/empty/x.zip");
        let e = pack_zip(
            &[base.join("sub")],
            &base,
            &inner,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap_err();
        assert_eq!(e, into_itself);
        assert!(!inner.exists());
        assert!(leftovers(&base).is_empty() && leftovers(t.path()).is_empty());
    }

    #[test]
    fn a_write_error_removes_the_part() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        let e = pack_impl(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
            Some(40),
        )
        .unwrap_err();
        assert!(
            matches!(&e, OpError::Io(m) if m.contains("No space left")),
            "{e:?}"
        );
        assert!(!target.exists());
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn entry_names_are_relative() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        // One source given from outside `base`: it goes in under its own name.
        let mut all = sources.clone();
        let outside = t.path().join("outside.txt");
        std::fs::write(&outside, b"o").unwrap();
        all.push(outside);
        pack_zip(&all, &base, &target, &plain(), &obs(ErrorAnswer::Cancel)).unwrap();
        let names = names(&target);
        assert!(names.contains(&"outside.txt".to_string()));
        for n in &names {
            assert!(!n.starts_with('/'), "{n}");
            assert!(!n.split('/').any(|c| c == ".."), "{n}");
        }
        assert_eq!(entry_name(&base.join("x/y"), &base).unwrap(), "x/y");
        assert!(entry_name(Path::new("/"), &base).is_err());
        assert!(entry_name(&base.join(".."), &base).is_err());
    }

    /// The UTF-8 flag (general purpose bit 11) is set in the local header of a non-ASCII name.
    #[test]
    fn non_ascii_names_carry_the_utf8_flag() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap();
        let bytes = std::fs::read(&target).unwrap();
        let name = UNICODE.as_bytes();
        let at = bytes
            .windows(name.len())
            .position(|w| w == name)
            .expect("name in the archive");
        let header = &bytes[at - 30..at];
        assert_eq!(&header[..4], b"PK\x03\x04");
        let flags = u16::from_le_bytes([header[6], header[7]]);
        assert_ne!(flags & (1 << 11), 0);
    }

    fn check_encrypted(encryption: ZipEncryption) {
        let (t, base, sources) = tree();
        let target = t.path().join("secret.zip");
        let opts = with_password("correct horse", encryption);
        pack_zip(&sources, &base, &target, &opts, &obs(ErrorAnswer::Cancel)).unwrap();
        let mut z = open(&target);
        {
            let mut f = z.by_name_decrypt("a.txt", b"correct horse").unwrap();
            assert!(f.encrypted());
            let mut v = Vec::new();
            f.read_to_end(&mut v).unwrap();
            assert_eq!(v, b"alpha");
        }
        // A wrong password is refused when opening (or, for ZipCrypto's 1-byte check, at
        // the latest by the CRC when reading).
        let wrong = match z.by_name_decrypt("sub/b.txt", b"wrong") {
            Err(_) => true,
            Ok(mut f) => {
                let mut v = Vec::new();
                f.read_to_end(&mut v).is_err() || v != b"beta"
            }
        };
        assert!(wrong, "{encryption:?}: wrong password opened the file");
        assert!(z.by_name("a.txt").is_err(), "no password must not read");
    }

    #[test]
    fn aes256_needs_the_right_password() {
        check_encrypted(ZipEncryption::Aes256);
    }

    #[test]
    fn zipcrypto_needs_the_right_password() {
        check_encrypted(ZipEncryption::ZipCrypto);
    }

    #[test]
    fn empty_password_is_refused() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        let opts = with_password("", ZipEncryption::Aes256);
        let e = pack_zip(&sources, &base, &target, &opts, &obs(ErrorAnswer::Cancel)).unwrap_err();
        assert!(matches!(e, OpError::Io(_)), "{e:?}");
        assert!(!target.exists());
    }

    #[test]
    fn cancel_leaves_no_part_and_no_target() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        let mut o = obs(ErrorAnswer::Cancel);
        o.cancel_on_progress = true;
        let s = pack_zip(&sources, &base, &target, &plain(), &o).unwrap();
        assert!(s.cancelled);
        assert!(!target.exists());
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn cancel_leaves_an_existing_target_untouched() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        std::fs::write(&target, b"old archive").unwrap();
        let mut o = obs(ErrorAnswer::Cancel);
        o.cancel_on_progress = true;
        let opts = PackOptions {
            overwrite: true,
            ..plain()
        };
        let s = pack_zip(&sources, &base, &target, &opts, &o).unwrap();
        assert!(s.cancelled);
        assert_eq!(std::fs::read(&target).unwrap(), b"old archive");
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn existing_target_needs_overwrite() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        std::fs::write(&target, b"old archive").unwrap();
        let e = pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap_err();
        assert_eq!(e, OpError::Exists(target.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read(&target).unwrap(), b"old archive");
        let opts = PackOptions {
            overwrite: true,
            ..plain()
        };
        pack_zip(&sources, &base, &target, &opts, &obs(ErrorAnswer::Cancel)).unwrap();
        assert_eq!(names(&target).len(), 7);
    }

    #[test]
    fn writes_through_a_hidden_part_file() {
        let (t, base, sources) = tree();
        let target = t.path().join("out.zip");
        let seen = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let o = obs(ErrorAnswer::Cancel);
        let (dir, s) = (t.path().to_path_buf(), seen.clone());
        *o.on_progress.lock().unwrap() = Some(Box::new(move || {
            s.lock().unwrap().extend(leftovers(&dir));
        }));
        pack_zip(&sources, &base, &target, &plain(), &o).unwrap();
        let seen = seen.lock().unwrap();
        let prefix = format!(".out.zip.fxfm-{}-", std::process::id());
        assert!(!seen.is_empty(), "no .part seen while writing");
        assert!(seen.iter().all(|n| n.starts_with(&prefix)), "{seen:?}");
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn unreadable_file_can_be_skipped() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads everything
        }
        let (t, base, sources) = tree();
        let locked = base.join("sub/b.txt");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let target = t.path().join("out.zip");
        let o = obs(ErrorAnswer::Skip);
        let s = pack_zip(&sources, &base, &target, &plain(), &o).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(o.errors.load(Ordering::SeqCst), 1);
        assert_eq!((s.done, s.skipped, s.cancelled), (4, 1, false));
        assert!(!names(&target).contains(&"sub/b.txt".to_string()));
        let mut z = open(&target);
        assert_eq!(read(&mut z, "a.txt"), b"alpha");
    }

    #[test]
    fn unreadable_file_cancel_removes_the_part() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (t, base, sources) = tree();
        let locked = base.join("sub/b.txt");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let target = t.path().join("out.zip");
        let s = pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(s.cancelled);
        assert!(!target.exists());
        assert!(leftovers(t.path()).is_empty());
    }

    #[test]
    fn target_folder_must_exist() {
        let (t, base, sources) = tree();
        let target = t.path().join("missing/out.zip");
        let e = pack_zip(
            &sources,
            &base,
            &target,
            &plain(),
            &obs(ErrorAnswer::Cancel),
        )
        .unwrap_err();
        assert!(matches!(e, OpError::Io(_)), "{e:?}");
    }

    #[test]
    fn options_debug_hides_the_password() {
        let o = with_password("hunter2", ZipEncryption::ZipCrypto);
        let s = format!("{o:?}");
        assert!(!s.contains("hunter2"), "{s}");
        let secret: Secret = serde_json::from_str(r#""hunter2""#).unwrap();
        assert_eq!(secret.0.as_str(), "hunter2");
        assert_eq!(format!("{secret:?}"), "<redacted>");
    }
}
