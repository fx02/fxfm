//! Reading archive indexes.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::ops::{ErrorAnswer, OpObserver, Progress};

use super::{
    check_name, normalize_name, ArchiveError, ArchiveIndex, ArchiveKind, IndexEntry, NameCheck,
    S_IFDIR, S_IFLNK, S_IFREG,
};

pub(crate) fn io_err(e: &io::Error, path: &Path) -> ArchiveError {
    match e.kind() {
        io::ErrorKind::NotFound => ArchiveError::NotFound(path.to_string_lossy().into_owned()),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData => {
            ArchiveError::Corrupt(e.to_string())
        }
        _ => ArchiveError::Io(e.to_string()),
    }
}

/// Reads the entry list of `path` without extracting any data.
pub fn read_index(path: &Path, kind: ArchiveKind) -> Result<ArchiveIndex, ArchiveError> {
    let raw = match kind {
        ArchiveKind::Zip => read_zip(path)?,
        k @ (ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 | ArchiveKind::TarXz) => {
            let f = File::open(path).map_err(|e| io_err(&e, path))?;
            read_tar(super::tar_decoder(k, BufReader::new(f)))?
        }
        ArchiveKind::Gz => read_gz(path)?,
        ArchiveKind::SevenZ => read_7z(path)?,
    };
    Ok(ArchiveIndex::build(raw))
}

fn type_bits(mode: u32, is_dir: bool, is_symlink: bool) -> u32 {
    let t = if is_dir {
        S_IFDIR
    } else if is_symlink {
        S_IFLNK
    } else {
        S_IFREG
    };
    (mode & 0o7777) | t
}

#[derive(Default)]
struct Meta {
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    mtime: i64,
    /// Permission bits (with or without type bits); a default is used when `None`.
    mode: Option<u32>,
    encrypted: bool,
}

fn push(raw: &mut Vec<IndexEntry>, name: &str, m: Meta) {
    let Some(path) = normalize_name(name) else {
        return;
    };
    let mode = m.mode.unwrap_or(if m.is_dir { 0o755 } else { 0o644 });
    raw.push(IndexEntry {
        path,
        is_dir: m.is_dir,
        is_symlink: m.is_symlink,
        size: if m.is_dir { 0 } else { m.size },
        mtime: m.mtime,
        mode: type_bits(mode, m.is_dir, m.is_symlink),
        encrypted: m.encrypted,
    });
}

// ---- zip ----

pub(crate) fn zip_err(e: zip::result::ZipError, path: &Path) -> ArchiveError {
    use zip::result::ZipError as Z;
    match e {
        Z::Io(e) => io_err(&e, path),
        Z::UnsupportedArchive(m) => ArchiveError::Unsupported(m.to_string()),
        Z::CompressionMethodNotSupported(m) => {
            ArchiveError::Unsupported(format!("unsupported compression method {m}"))
        }
        Z::InvalidPassword => ArchiveError::WrongPassword,
        other => ArchiveError::Corrupt(other.to_string()),
    }
}

fn read_zip(path: &Path) -> Result<Vec<IndexEntry>, ArchiveError> {
    let f = File::open(path).map_err(|e| io_err(&e, path))?;
    let mut z = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| zip_err(e, path))?;
    let mut raw = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let e = z.by_index_raw(i).map_err(|e| zip_err(e, path))?;
        let name = match e.name() {
            Ok(n) => n.into_owned(),
            Err(_) => String::from_utf8_lossy(e.name_raw()).into_owned(),
        };
        let is_dir = e.is_dir() || name.ends_with('/');
        let mtime = super::ziptime::entry_mtime(&e);
        push(
            &mut raw,
            &name,
            Meta {
                is_dir,
                is_symlink: e.is_symlink(),
                size: e.size(),
                mtime,
                mode: e.unix_mode(),
                encrypted: e.encrypted(),
            },
        );
    }
    Ok(raw)
}

// ---- tar ----

fn read_tar<R: Read>(r: R) -> Result<Vec<IndexEntry>, ArchiveError> {
    let mut ar = tar::Archive::new(r);
    let mut raw = Vec::new();
    // Anything that fails while walking the stream (truncation, bad header, bad gzip) is a
    // damaged archive.
    let bad = |e: io::Error| ArchiveError::Corrupt(e.to_string());
    for e in ar.entries().map_err(bad)? {
        let e = e.map_err(bad)?;
        let h = e.header();
        let ty = h.entry_type();
        if ty.is_pax_global_extensions() || ty.is_pax_local_extensions() || ty.is_gnu_longname() {
            continue;
        }
        let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        push(
            &mut raw,
            &name,
            Meta {
                is_dir: ty.is_dir(),
                is_symlink: ty.is_symlink(),
                size: h.size().unwrap_or(0),
                mtime: h.mtime().map_or(0, |m| m as i64),
                mode: h.mode().ok(),
                encrypted: false,
            },
        );
    }
    Ok(raw)
}

// ---- gz ----

fn read_gz(path: &Path) -> Result<Vec<IndexEntry>, ArchiveError> {
    let mut f = File::open(path).map_err(|e| io_err(&e, path))?;
    let md = f.metadata().map_err(|e| io_err(&e, path))?;
    let corrupt = |m: &str| ArchiveError::Corrupt(m.to_string());
    let mut head = [0u8; 10];
    f.read_exact(&mut head)
        .map_err(|_| corrupt("truncated gzip header"))?;
    if head[0] != 0x1f || head[1] != 0x8b || head[2] != 8 {
        return Err(corrupt("not a gzip file"));
    }
    let flags = head[3];
    let mut mtime = i64::from(u32::from_le_bytes([head[4], head[5], head[6], head[7]]));
    let mut r = BufReader::new(&mut f);
    if flags & 4 != 0 {
        let mut xlen = [0u8; 2];
        r.read_exact(&mut xlen)
            .map_err(|_| corrupt("truncated gzip header"))?;
        io::copy(
            &mut (&mut r).take(u64::from(u16::from_le_bytes(xlen))),
            &mut io::sink(),
        )
        .map_err(|e| io_err(&e, path))?;
    }
    let mut header_name = None;
    if flags & 8 != 0 {
        let mut buf = Vec::new();
        io::BufRead::read_until(&mut r, 0, &mut buf).map_err(|e| io_err(&e, path))?;
        if buf.pop() != Some(0) {
            return Err(corrupt("truncated gzip header"));
        }
        // The gzip name field is Latin-1.
        let s: String = buf.iter().map(|&b| b as char).collect();
        header_name = s.rsplit('/').next().map(str::to_string);
    }
    let header_len = r.stream_position().map_err(|e| io_err(&e, path))?;
    drop(r);
    // Header, at least an empty deflate stream (2 bytes) and the 8 byte trailer.
    if md.len() < header_len + 10 {
        return Err(corrupt("truncated gzip file"));
    }
    f.seek(SeekFrom::End(-4)).map_err(|e| io_err(&e, path))?;
    let mut isize = [0u8; 4];
    f.read_exact(&mut isize).map_err(|e| io_err(&e, path))?;
    let size = u64::from(u32::from_le_bytes(isize));
    if mtime == 0 {
        use std::os::unix::fs::MetadataExt;
        mtime = md.mtime();
    }
    let stem = || {
        let n = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let n = n.unwrap_or_default();
        n[..n.len().saturating_sub(3)].to_string()
    };
    let name = header_name
        .filter(|n| normalize_name(n).is_some())
        .unwrap_or_else(stem);
    let mut raw = Vec::new();
    push(
        &mut raw,
        &name,
        Meta {
            size,
            mtime,
            ..Meta::default()
        },
    );
    Ok(raw)
}

// ---- 7z ----

pub(crate) fn sevenz_err(e: sevenz_rust2::Error, path: &Path) -> ArchiveError {
    use sevenz_rust2::Error as E;
    const ENCRYPTED: &str = "Encrypted 7z archives are not supported";
    match e {
        E::PasswordRequired | E::MaybeBadPassword(_) => ArchiveError::Unsupported(ENCRYPTED.into()),
        E::UnsupportedCompressionMethod(m) if m.to_lowercase().contains("aes") => {
            ArchiveError::Unsupported(ENCRYPTED.into())
        }
        E::UnsupportedCompressionMethod(m) => {
            ArchiveError::Unsupported(format!("unsupported 7z compression method {m}"))
        }
        E::Unsupported(m) => ArchiveError::Unsupported(m.into_owned()),
        E::Io(e, _) | E::FileOpen(e, _) => io_err(&e, path),
        other => ArchiveError::Corrupt(other.to_string()),
    }
}

fn sevenz_mtime(e: &sevenz_rust2::ArchiveEntry) -> i64 {
    if e.has_last_modified_date {
        let ft: u64 = e.last_modified_date().into();
        (ft / 10_000_000) as i64 - 11_644_473_600
    } else {
        0
    }
}

/// 7-Zip stores unix mode in the high 16 bits when bit 0x8000 is set.
fn sevenz_mode(e: &sevenz_rust2::ArchiveEntry) -> Option<u32> {
    let attr = e.windows_attributes();
    (e.has_windows_attributes && attr & 0x8000 != 0).then_some(attr >> 16)
}

fn read_7z(path: &Path) -> Result<Vec<IndexEntry>, ArchiveError> {
    let a = sevenz_rust2::Archive::open(path).map_err(|e| sevenz_err(e, path))?;
    if (0..a.files.len()).any(|i| a.is_file_encrypted(i)) {
        return Err(ArchiveError::Unsupported(
            "Encrypted 7z archives are not supported".into(),
        ));
    }
    let mut raw = Vec::with_capacity(a.files.len());
    for e in &a.files {
        if e.is_anti_item() {
            continue;
        }
        let mtime = sevenz_mtime(e);
        let mode = sevenz_mode(e);
        let is_symlink = mode.is_some_and(|m| m & 0o170000 == S_IFLNK);
        push(
            &mut raw,
            e.name(),
            Meta {
                is_dir: e.is_directory(),
                is_symlink,
                size: e.size(),
                mtime,
                mode,
                encrypted: false,
            },
        );
    }
    Ok(raw)
}

// ---- extraction ----

const CHUNK: u64 = 1 << 20;

/// Which entries `items` select and where they land relative to the destination: an item
/// keeps its own name and what is below it, but not its parents.
struct Selector {
    /// `(item, length of the parent prefix to strip including its '/')`.
    items: Vec<(String, usize)>,
}

impl Selector {
    fn new(items: &[String]) -> Selector {
        let items = items
            .iter()
            .map(|i| i.trim_matches('/').to_string())
            .map(|i| {
                let strip = i.rfind('/').map_or(0, |p| p + 1);
                (i, strip)
            })
            .collect();
        Selector { items }
    }

    fn rel(&self, name: &str) -> Option<String> {
        self.items.iter().find_map(|(item, strip)| {
            let hit = item.is_empty()
                || name == item
                || (name.starts_with(item.as_str()) && name.as_bytes()[item.len()] == b'/');
            hit.then(|| name[*strip..].to_string())
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Dir,
    File,
    Symlink,
}

fn read_err(e: &io::Error) -> ArchiveError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData => {
            ArchiveError::Corrupt(e.to_string())
        }
        _ => ArchiveError::Io(e.to_string()),
    }
}

/// Writes entries below `dest` and keeps progress, cancellation and error answers.
/// Item error for an entry whose name differs from an already extracted one only in case or
/// Unicode form: on this disk both would be one file.
pub const SAME_FILE_NAME: &str =
    "another item with this name in different case was already written here (this disk ignores case)";

struct Sink<'a> {
    dest: std::path::PathBuf,
    obs: &'a dyn OpObserver,
    total_bytes: u64,
    total_files: u64,
    done_bytes: u64,
    done_files: u64,
    since_report: u64,
    skip_all: bool,
    cur: String,
    /// Directories get their mode and mtime once their content is written.
    dirs: Vec<(std::path::PathBuf, i64, u32)>,
    /// Created symlinks `(path, rel)`, verified once the whole tree exists.
    links: Vec<(std::path::PathBuf, String)>,
    /// Files written so far, by their exact entry name. `dest` starts empty, so an existing
    /// file that is not one of these is another entry under a name that differs only in case
    /// or Unicode form (the disk ignores both): it must not be overwritten.
    written: std::collections::HashSet<String>,
}

impl Sink<'_> {
    fn check_cancel(&self) -> Result<(), ArchiveError> {
        if self.obs.is_cancelled() {
            Err(ArchiveError::Cancelled)
        } else {
            Ok(())
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

    /// Reports a problem with one entry. The entry is skipped unless the user cancels.
    fn item_error(&mut self, path: &str, msg: &str) -> Result<(), ArchiveError> {
        if self.skip_all {
            return Ok(());
        }
        match self.obs.error(path, msg) {
            ErrorAnswer::Cancel => Err(ArchiveError::Cancelled),
            ErrorAnswer::SkipAll => {
                self.skip_all = true;
                Ok(())
            }
            ErrorAnswer::Retry | ErrorAnswer::Skip => Ok(()),
        }
    }

    /// Maps a raw entry name to its destination path, or `None` when the entry is not
    /// selected. Unsafe names are reported and skipped.
    fn select(&mut self, sel: &Selector, raw: &str) -> Result<Option<String>, ArchiveError> {
        match check_name(raw) {
            NameCheck::Ok(n) => Ok(sel.rel(&n)),
            NameCheck::Empty => Ok(None),
            NameCheck::Unsafe => {
                self.item_error(raw, &format!("unsafe path in archive: {raw}"))?;
                Ok(None)
            }
        }
    }

    /// Creates the parent directories of `rel`, refusing to pass through a symlink.
    fn prepare_parent(&self, rel: &str) -> Result<(), String> {
        let mut cur = self.dest.clone();
        let parent = rel.rfind('/').map_or("", |p| &rel[..p]);
        for c in parent.split('/').filter(|c| !c.is_empty()) {
            cur.push(c);
            match std::fs::symlink_metadata(&cur) {
                Ok(md) if md.file_type().is_symlink() => {
                    return Err(format!("unsafe path in archive: {rel}"));
                }
                Ok(md) if md.is_dir() => {}
                Ok(_) => return Err(format!("{} is not a directory", cur.display())),
                Err(_) => std::fs::create_dir(&cur).map_err(|e| e.to_string())?,
            }
        }
        Ok(())
    }

    /// True if a link at `rel` pointing to `target` stays inside `dest` once resolved.
    fn link_is_safe(&self, rel: &str, target: &str) -> bool {
        let bytes = target.as_bytes();
        let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        if target.is_empty() || target.starts_with('/') || drive {
            return false;
        }
        let mut cur = self.dest.clone();
        if let Some(p) = rel.rfind('/') {
            cur.push(&rel[..p]);
        }
        for c in target.split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    if !cur.pop() || !cur.starts_with(&self.dest) {
                        return false;
                    }
                }
                n => {
                    cur.push(n);
                    let is_link =
                        std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink());
                    if is_link {
                        if let Ok(real) = std::fs::canonicalize(&cur) {
                            if !real.starts_with(&self.dest) {
                                return false;
                            }
                            cur = real;
                        }
                    }
                }
            }
        }
        true
    }

    /// Copies `data` into `out`. The inner result is a write failure, the outer one a read
    /// failure (damaged archive) or a cancel.
    fn copy(
        &mut self,
        out: &mut File,
        data: &mut dyn Read,
    ) -> Result<io::Result<()>, ArchiveError> {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match data.read(&mut buf) {
                Ok(0) => return Ok(Ok(())),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(read_err(&e)),
            };
            if let Err(e) = out.write_all(&buf[..n]) {
                return Ok(Err(e));
            }
            self.done_bytes += n as u64;
            self.since_report += n as u64;
            if self.since_report >= CHUNK {
                self.since_report = 0;
                self.report();
                self.check_cancel()?;
            }
        }
    }

    fn entry(
        &mut self,
        rel: &str,
        kind: Kind,
        mode: Option<u32>,
        mtime: i64,
        data: &mut dyn Read,
    ) -> Result<(), ArchiveError> {
        self.check_cancel()?;
        self.cur = rel.to_string();
        if let Err(msg) = self.prepare_parent(rel) {
            return self.item_error(rel, &msg);
        }
        let path = self.dest.join(rel);
        let ft = filetime::FileTime::from_unix_time(mtime, 0);
        // Setuid and setgid bits are never restored.
        let mode = mode.map(|m| m & 0o1777);
        match kind {
            Kind::Dir => {
                match std::fs::symlink_metadata(&path) {
                    Ok(md) if md.is_dir() => {}
                    Ok(_) => return self.item_error(rel, "exists and is not a directory"),
                    Err(_) => {
                        if let Err(e) = std::fs::create_dir(&path) {
                            return self.item_error(rel, &e.to_string());
                        }
                    }
                }
                self.dirs.push((path, mtime, mode.unwrap_or(0o755)));
            }
            Kind::File => {
                // A repeated entry (tar appends) replaces the earlier one; another name that
                // lands on the same file is an error, so a move never deletes its original.
                if !self.written.contains(rel)
                    && std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink())
                {
                    return self.item_error(rel, SAME_FILE_NAME);
                }
                if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                    let _ = std::fs::remove_file(&path);
                }
                let mut out = match File::create(&path) {
                    Ok(f) => f,
                    Err(e) => return self.item_error(rel, &e.to_string()),
                };
                let r = self.copy(&mut out, data);
                drop(out);
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        let _ = std::fs::remove_file(&path);
                        return self.item_error(rel, &e.to_string());
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&path);
                        return Err(e);
                    }
                }
                self.written.insert(rel.to_string());
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(mode.unwrap_or(0o644));
                let _ = std::fs::set_permissions(&path, perm);
                let _ = filetime::set_file_mtime(&path, ft);
                self.done_files += 1;
                self.report();
            }
            Kind::Symlink => {
                let mut raw = Vec::new();
                if let Err(e) = data.take(4096).read_to_end(&mut raw) {
                    return Err(read_err(&e));
                }
                let target = String::from_utf8_lossy(&raw).into_owned();
                if !self.link_is_safe(rel, &target) {
                    return self.item_error(rel, &format!("unsafe link in archive: {rel}"));
                }
                if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_dir()) {
                    let _ = std::fs::remove_file(&path);
                }
                if let Err(e) = std::os::unix::fs::symlink(&target, &path) {
                    return self.item_error(rel, &e.to_string());
                }
                let _ = filetime::set_symlink_file_times(&path, ft, ft);
                self.links.push((path, rel.to_string()));
                self.done_files += 1;
                self.report();
            }
        }
        Ok(())
    }

    /// Links are checked on creation only against what existed then; a later entry can
    /// change what they resolve to. Remove every link that now leaves `dest`.
    fn verify_links(&mut self) -> Result<(), ArchiveError> {
        loop {
            let mut removed = false;
            for (path, rel) in std::mem::take(&mut self.links) {
                let is_link =
                    std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
                let escapes =
                    std::fs::canonicalize(&path).is_ok_and(|real| !real.starts_with(&self.dest));
                if is_link && escapes {
                    let _ = std::fs::remove_file(&path);
                    self.done_files = self.done_files.saturating_sub(1);
                    removed = true;
                    self.item_error(&rel, &format!("unsafe link in archive: {rel}"))?;
                } else {
                    self.links.push((path, rel));
                }
            }
            if !removed {
                return Ok(());
            }
        }
    }

    fn finish(mut self) -> Result<u64, ArchiveError> {
        use std::os::unix::fs::PermissionsExt;
        self.verify_links()?;
        // Children first, so a read-only parent does not get in the way.
        while let Some((path, mtime, mode)) = self.dirs.pop() {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
            let _ = filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(mtime, 0));
        }
        Ok(self.done_files)
    }
}

/// Extracts `items` (inner paths; a folder brings its whole subtree) into the existing, empty
/// directory `dest`. Structure is kept relative to the parent of each item. Returns the number
/// of files and links written.
pub fn extract(
    path: &Path,
    kind: ArchiveKind,
    items: &[String],
    dest: &Path,
    password: Option<&str>,
    obs: &dyn OpObserver,
) -> Result<u64, ArchiveError> {
    let sel = Selector::new(items);
    let index = read_index(path, kind)?;
    let (mut total_bytes, mut total_files) = (0, 0);
    for e in index.entries.iter().filter(|e| !e.is_dir) {
        if sel.rel(&e.path).is_some() {
            total_bytes += e.size;
            total_files += 1;
        }
    }
    let dest = std::fs::canonicalize(dest).map_err(|e| io_err(&e, dest))?;
    let mut sink = Sink {
        dest,
        obs,
        total_bytes,
        total_files,
        done_bytes: 0,
        done_files: 0,
        since_report: 0,
        skip_all: false,
        cur: String::new(),
        dirs: Vec::new(),
        links: Vec::new(),
        written: Default::default(),
    };
    match kind {
        ArchiveKind::Zip => extract_zip(path, &sel, password, &mut sink)?,
        k @ (ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 | ArchiveKind::TarXz) => {
            let f = File::open(path).map_err(|e| io_err(&e, path))?;
            extract_tar(super::tar_decoder(k, BufReader::new(f)), &sel, &mut sink)?;
        }
        ArchiveKind::Gz => extract_gz(path, &index, &sel, &mut sink)?,
        ArchiveKind::SevenZ => extract_7z(path, &sel, &mut sink)?,
    }
    sink.finish()
}

fn extract_zip(
    path: &Path,
    sel: &Selector,
    password: Option<&str>,
    sink: &mut Sink,
) -> Result<(), ArchiveError> {
    let f = File::open(path).map_err(|e| io_err(&e, path))?;
    let mut z = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| zip_err(e, path))?;
    struct Planned {
        index: usize,
        rel: String,
        kind: Kind,
        encrypted: bool,
    }
    let mut plan = Vec::new();
    for i in 0..z.len() {
        let e = z.by_index_raw(i).map_err(|e| zip_err(e, path))?;
        let name = match e.name() {
            Ok(n) => n.into_owned(),
            Err(_) => String::from_utf8_lossy(e.name_raw()).into_owned(),
        };
        let kind = if e.is_dir() || name.ends_with('/') {
            Kind::Dir
        } else if e.is_symlink() {
            Kind::Symlink
        } else {
            Kind::File
        };
        let encrypted = e.encrypted() && kind != Kind::Dir;
        drop(e);
        let Some(rel) = sink.select(sel, &name)? else {
            continue;
        };
        plan.push(Planned {
            index: i,
            rel,
            kind,
            encrypted,
        });
    }
    // Settle the password on the first encrypted entry before anything is written.
    if let Some(first) = plan.iter().find(|p| p.encrypted) {
        let pw = password.ok_or(ArchiveError::PasswordRequired)?;
        z.by_index_decrypt(first.index, pw.as_bytes())
            .map(|_| ())
            .map_err(|e| zip_err(e, path))?;
    }
    for p in plan {
        let mut e = if p.encrypted {
            let pw = password.ok_or(ArchiveError::PasswordRequired)?;
            z.by_index_decrypt(p.index, pw.as_bytes())
        } else {
            z.by_index(p.index)
        }
        .map_err(|e| zip_err(e, path))?;
        let mtime = super::ziptime::entry_mtime(&e);
        let mode = e.unix_mode();
        sink.entry(&p.rel, p.kind, mode, mtime, &mut e)?;
    }
    Ok(())
}

fn extract_tar<R: Read>(r: R, ar: &Selector, sink: &mut Sink) -> Result<(), ArchiveError> {
    let mut a = tar::Archive::new(r);
    let bad = |e: io::Error| ArchiveError::Corrupt(e.to_string());
    for e in a.entries().map_err(bad)? {
        let mut e = e.map_err(bad)?;
        let ty = e.header().entry_type();
        if ty.is_pax_global_extensions() || ty.is_pax_local_extensions() || ty.is_gnu_longname() {
            continue;
        }
        let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        let Some(rel) = sink.select(ar, &name)? else {
            continue;
        };
        let mode = e.header().mode().ok();
        let mtime = e.header().mtime().map_or(0, |m| m as i64);
        if ty.is_dir() {
            sink.entry(&rel, Kind::Dir, mode, mtime, &mut io::empty())?;
        } else if ty.is_symlink() {
            let target = e
                .link_name_bytes()
                .map(|b| b.into_owned())
                .unwrap_or_default();
            sink.entry(&rel, Kind::Symlink, mode, mtime, &mut &target[..])?;
        } else if ty.is_file() || ty.is_contiguous() || ty.is_gnu_sparse() {
            sink.entry(&rel, Kind::File, mode, mtime, &mut e)?;
        } else {
            let msg = format!("unsupported entry type in archive: {name}");
            sink.item_error(&rel, &msg)?;
        }
    }
    Ok(())
}

fn extract_gz(
    path: &Path,
    index: &ArchiveIndex,
    sel: &Selector,
    sink: &mut Sink,
) -> Result<(), ArchiveError> {
    let Some(e) = index.entries.first() else {
        return Ok(());
    };
    let Some(rel) = sel.rel(&e.path) else {
        return Ok(());
    };
    let f = File::open(path).map_err(|e| io_err(&e, path))?;
    // A damaged deflate body or trailer surfaces as a read error here.
    let mut d = flate2::read::GzDecoder::new(BufReader::new(f));
    sink.entry(&rel, Kind::File, Some(e.mode), e.mtime, &mut d)
}

fn extract_7z(path: &Path, sel: &Selector, sink: &mut Sink) -> Result<(), ArchiveError> {
    let mut r = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| sevenz_err(e, path))?;
    let mut fatal: Option<ArchiveError> = None;
    let stop = || sevenz_rust2::Error::Io(io::Error::other("extraction stopped"), "stopped".into());
    let res = r.for_each_entries(|e, data| {
        if e.is_anti_item() {
            return Ok(true);
        }
        let rel = match sink.select(sel, e.name()) {
            Ok(Some(rel)) => rel,
            Ok(None) => return Ok(true),
            Err(err) => {
                fatal = Some(err);
                return Err(stop());
            }
        };
        let mode = sevenz_mode(e);
        let kind = if e.is_directory() {
            Kind::Dir
        } else if mode.is_some_and(|m| m & 0o170000 == S_IFLNK) {
            Kind::Symlink
        } else {
            Kind::File
        };
        match sink.entry(&rel, kind, mode, sevenz_mtime(e), data) {
            Ok(()) => Ok(true),
            Err(err) => {
                fatal = Some(err);
                Err(stop())
            }
        }
    });
    if let Some(e) = fatal {
        return Err(e);
    }
    res.map_err(|e| sevenz_err(e, path))
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::super::*;
    use super::*;

    fn names(v: &[crate::listing::Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    fn list(path: &Path, kind: ArchiveKind, inner: &str) -> Vec<crate::listing::Entry> {
        list_inner(&read_index(path, kind).unwrap(), inner).unwrap()
    }

    #[test]
    fn index_skips_unsafe_names() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar(
            &p,
            &[
                ("../evil", Some(b"x")),
                ("/abs", Some(b"x")),
                ("C:/win", Some(b"x")),
                ("a/../b", Some(b"x")),
                ("ok.txt", Some(b"x")),
            ],
        );
        assert_eq!(names(&list(&p, ArchiveKind::Tar, "")), ["ok.txt"]);
    }

    #[test]
    fn zip_cp437_names_decode() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, &[("a#b.txt", Some(b"x"))]);
        let mut bytes = std::fs::read(&p).unwrap();
        for b in bytes.iter_mut().filter(|b| **b == b'#') {
            *b = 0x87;
        }
        std::fs::write(&p, bytes).unwrap();
        let v = list(&p, ArchiveKind::Zip, "");
        assert_eq!(names(&v), ["a\u{e7}b.txt"]);
    }

    #[test]
    fn zip_entry_metadata() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip(&p, &[("d/", None), ("d/f.txt", Some(b"hello"))]);
        let idx = read_index(&p, ArchiveKind::Zip).unwrap();
        let d = &idx.entries[0];
        assert!(d.is_dir && !d.encrypted);
        assert_eq!(d.mode, 0o40755);
        let f = &idx.entries[1];
        assert_eq!((f.path.as_str(), f.size, f.mode), ("d/f.txt", 5, 0o100644));
        assert!(f.mtime > 1_700_000_000 - 86_400 * 365 * 5);
    }

    #[test]
    fn zip_encrypted_flag() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip_aes(&p, &[("secret.txt", Some(b"hello"))], "pw");
        let idx = read_index(&p, ArchiveKind::Zip).unwrap();
        assert_eq!(idx.entries.len(), 1);
        assert!(idx.entries[0].encrypted);
        assert_eq!(names(&list(&p, ArchiveKind::Zip, "")), ["secret.txt"]);
    }

    #[test]
    fn tar_and_tgz_list_with_metadata() {
        let t = tempfile::tempdir().unwrap();
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz] {
            let p = t.path().join("a.bin");
            build(kind, &p, &[("d", None), ("d/f.txt", Some(b"hello"))]);
            let idx = read_index(&p, kind).unwrap();
            assert_eq!(idx.entries.len(), 2);
            assert!(idx.entries[0].is_dir);
            assert_eq!(idx.entries[1].size, 5);
            assert_eq!(idx.entries[1].mtime, TEST_MTIME as i64);
            assert_eq!(idx.entries[1].mode, 0o100644);
        }
    }

    #[test]
    fn gz_single_entry() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("weird.gz");
        make_gz(&p, Some("notes.txt"), b"hello world");
        let v = list(&p, ArchiveKind::Gz, "");
        assert_eq!(names(&v), ["notes.txt"]);
        assert_eq!(v[0].size, 11);
        assert_eq!(v[0].mtime, TEST_MTIME as i64);

        let p = t.path().join("report.csv.gz");
        make_gz(&p, None, b"a,b\n");
        let v = list(&p, ArchiveKind::Gz, "");
        assert_eq!(names(&v), ["report.csv"]);
        assert_eq!(v[0].size, 4);
    }

    #[test]
    fn seven_z_lists() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.7z");
        make_7z(&p, &[("d/x.txt", Some(b"hello"))]);
        let root = list(&p, ArchiveKind::SevenZ, "");
        assert_eq!(names(&root), ["d"]);
        let d = list(&p, ArchiveKind::SevenZ, "d");
        assert_eq!((names(&d), d[0].size), (vec!["x.txt"], 5));
    }

    #[test]
    fn corrupt_archives_are_reported() {
        let t = tempfile::tempdir().unwrap();
        for kind in [
            ArchiveKind::Zip,
            ArchiveKind::Tar,
            ArchiveKind::TarGz,
            ArchiveKind::Gz,
            ArchiveKind::SevenZ,
        ] {
            let p = t.path().join("a.bin");
            build(kind, &p, &[("some/file.txt", Some(&[b'x'; 5000][..]))]);
            let bytes = std::fs::read(&p).unwrap();
            // A gzip index only reads the header and trailer, so it can only notice damage there.
            let keep = if kind == ArchiveKind::Gz { 15 } else { 100 };
            std::fs::write(&p, &bytes[..keep.min(bytes.len() / 2)]).unwrap();
            let r = read_index(&p, kind);
            assert!(
                matches!(r, Err(ArchiveError::Corrupt(_))),
                "{kind:?}: {r:?}"
            );
        }
    }

    // ---- extraction ----

    use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, Progress};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Obs {
        errors: Mutex<Vec<(String, String)>>,
        cancel_after: usize,
        progress: AtomicUsize,
    }

    impl OpObserver for Obs {
        fn progress(&self, _p: &Progress) {
            self.progress.fetch_add(1, Ordering::SeqCst);
        }
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            panic!("no conflicts expected");
        }
        fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
            self.errors
                .lock()
                .unwrap()
                .push((path.to_string(), msg.to_string()));
            ErrorAnswer::Skip
        }
        fn is_cancelled(&self) -> bool {
            self.cancel_after > 0 && self.progress.load(Ordering::SeqCst) >= self.cancel_after
        }
    }

    fn items(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn mtime_of(p: &Path) -> i64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().mtime()
    }

    #[test]
    fn extract_subtree_relative_to_parent() {
        let t = tempfile::tempdir().unwrap();
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz, ArchiveKind::Zip] {
            let p = t.path().join("a.bin");
            build(
                kind,
                &p,
                &[
                    ("src", None),
                    ("src/lib", None),
                    ("src/lib/a.rs", Some(b"aaa")),
                    ("src/lib/m", None),
                    ("src/lib/m/b.rs", Some(b"bb")),
                    ("src/other.rs", Some(b"no")),
                ],
            );
            let dest = tempfile::tempdir().unwrap();
            let obs = Obs::default();
            let n = extract(&p, kind, &items(&["src/lib"]), dest.path(), None, &obs).unwrap();
            assert_eq!(n, 2, "{kind:?}");
            let d = dest.path();
            assert_eq!(std::fs::read(d.join("lib/a.rs")).unwrap(), b"aaa");
            assert_eq!(std::fs::read(d.join("lib/m/b.rs")).unwrap(), b"bb");
            assert!(!d.join("other.rs").exists() && !d.join("src").exists());
            if kind != ArchiveKind::Zip {
                assert_eq!(mtime_of(&d.join("lib/a.rs")), TEST_MTIME as i64);
                assert_eq!(mtime_of(&d.join("lib/m")), TEST_MTIME as i64);
            }
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(d.join("lib/a.rs"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o7777, 0o644);
        }
    }

    #[test]
    fn extract_refuses_path_traversal() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        make_tar_links(
            &p,
            &[("../evil", Some(b"x")), ("ok.txt", Some(b"fine"))],
            &[("l", "../../etc"), ("good", "ok.txt")],
        );
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("stage");
        std::fs::create_dir(&dest).unwrap();
        let obs = Obs::default();
        let n = extract(&p, ArchiveKind::Tar, &items(&[""]), &dest, None, &obs).unwrap();
        assert!(!outer.path().join("evil").exists());
        assert!(std::fs::symlink_metadata(dest.join("l")).is_err());
        assert_eq!(std::fs::read(dest.join("ok.txt")).unwrap(), b"fine");
        assert_eq!(
            std::fs::read_link(dest.join("good")).unwrap(),
            Path::new("ok.txt")
        );
        assert_eq!(n, 2);
        let errs = obs.errors.lock().unwrap();
        let msgs: Vec<&str> = errs.iter().map(|e| e.1.as_str()).collect();
        assert_eq!(
            msgs,
            [
                "unsafe path in archive: ../evil",
                "unsafe link in archive: l"
            ]
        );
    }

    /// On a disk that ignores case, `A.txt` and `a.txt` are one file: the second is an item
    /// error (so a move keeps its original), the first keeps its content. A name repeated in a
    /// tar still replaces the earlier entry, as tar does.
    #[test]
    fn extract_refuses_names_that_differ_only_in_case() {
        let probe = tempfile::tempdir().unwrap();
        std::fs::write(probe.path().join("X"), b"").unwrap();
        let ignores_case = probe.path().join("x").exists();
        for kind in [ArchiveKind::Tar, ArchiveKind::Zip] {
            let t = tempfile::tempdir().unwrap();
            let p = t.path().join("a.bin");
            build(
                kind,
                &p,
                &[("d/A.txt", Some(b"upper")), ("d/a.txt", Some(b"lower"))],
            );
            let dest = t.path().join("stage");
            std::fs::create_dir(&dest).unwrap();
            let obs = Obs::default();
            let n = extract(&p, kind, &items(&[""]), &dest, None, &obs).unwrap();
            if ignores_case {
                assert_eq!(n, 1, "{kind:?}");
                assert_eq!(
                    std::fs::read(dest.join("d/A.txt")).unwrap(),
                    b"upper",
                    "{kind:?}"
                );
                let errs = obs.errors.lock().unwrap();
                assert_eq!(errs.len(), 1, "{kind:?}");
                assert_eq!(errs[0].1, SAME_FILE_NAME);
            } else {
                assert_eq!(n, 2, "{kind:?}");
            }
        }
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("dup.tar");
        build(
            ArchiveKind::Tar,
            &p,
            &[("x.txt", Some(b"old")), ("x.txt", Some(b"new"))],
        );
        let dest = t.path().join("stage");
        std::fs::create_dir(&dest).unwrap();
        let obs = Obs::default();
        extract(&p, ArchiveKind::Tar, &items(&[""]), &dest, None, &obs).unwrap();
        assert_eq!(std::fs::read(dest.join("x.txt")).unwrap(), b"new");
        assert!(obs.errors.lock().unwrap().is_empty());
    }

    #[test]
    fn extract_reports_unsafe_names() {
        let t = tempfile::tempdir().unwrap();
        let bad = ["/etc/x", "a/../../b", "C:\\x", "../evil"];
        for kind in [ArchiveKind::Tar, ArchiveKind::TarGz, ArchiveKind::Zip] {
            let p = t.path().join("a.bin");
            let mut its: Vec<(&str, Option<&[u8]>)> =
                bad.iter().map(|n| (*n, Some(&b"x"[..]))).collect();
            its.push(("ok.txt", Some(b"fine")));
            build(kind, &p, &its);
            let outer = tempfile::tempdir().unwrap();
            let dest = outer.path().join("stage");
            std::fs::create_dir(&dest).unwrap();
            let obs = Obs::default();
            let n = extract(&p, kind, &items(&[""]), &dest, None, &obs).unwrap();
            assert_eq!(n, 1, "{kind:?}");
            let errs = obs.errors.lock().unwrap();
            let msgs: Vec<String> = errs.iter().map(|e| e.1.clone()).collect();
            let want: Vec<String> = bad
                .iter()
                .map(|n| format!("unsafe path in archive: {n}"))
                .collect();
            assert_eq!(msgs, want, "{kind:?}");
            let names: Vec<_> = std::fs::read_dir(&dest).unwrap().collect();
            assert_eq!(names.len(), 1, "{kind:?}");
            assert_eq!(std::fs::read_dir(outer.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn extract_removes_links_that_end_up_outside() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        // `b -> c/..` looks fine while `c` does not exist; `c -> .` then makes it leave dest.
        make_tar_links(&p, &[("ok.txt", Some(b"x"))], &[("b", "c/.."), ("c", ".")]);
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("stage");
        std::fs::create_dir(&dest).unwrap();
        let obs = Obs::default();
        let n = extract(&p, ArchiveKind::Tar, &items(&[""]), &dest, None, &obs).unwrap();
        let real_dest = std::fs::canonicalize(&dest).unwrap();
        for e in std::fs::read_dir(&dest).unwrap() {
            let path = e.unwrap().path();
            if let Ok(real) = std::fs::canonicalize(&path) {
                assert!(real.starts_with(&real_dest), "{path:?} -> {real:?}");
            }
        }
        assert!(std::fs::symlink_metadata(dest.join("b")).is_err());
        assert!(std::fs::symlink_metadata(dest.join("c")).is_ok());
        assert_eq!(n, 2);
        let errs = obs.errors.lock().unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].1, "unsafe link in archive: b");
    }

    #[test]
    fn extract_refuses_write_through_symlink() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.tar");
        // `s -> sub` is a safe link, then the regular entry `s/f` would be written through it.
        make_tar_links(&p, &[("sub", None)], &[("s", "sub")]);
        let mut raw = std::fs::read(&p).unwrap();
        raw.truncate(raw.len() - 1024); // drop the end-of-archive blocks
        let extra = t.path().join("b.tar");
        make_tar(&extra, &[("s/f", Some(b"x"))]);
        raw.extend(std::fs::read(&extra).unwrap());
        std::fs::write(&p, raw).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let obs = Obs::default();
        extract(&p, ArchiveKind::Tar, &items(&[""]), dest.path(), None, &obs).unwrap();
        assert!(!dest.path().join("sub/f").exists());
        let md = std::fs::symlink_metadata(dest.path().join("s")).unwrap();
        assert!(md.file_type().is_symlink());
        let errs = obs.errors.lock().unwrap();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].1, "unsafe path in archive: s/f");
    }

    #[test]
    fn extract_all_formats() {
        let t = tempfile::tempdir().unwrap();
        let content: &[(&str, Option<&[u8]>)] = &[
            ("a.txt", Some(b"alpha")),
            ("d", None),
            ("d/b.txt", Some(b"beta beta")),
        ];
        for kind in [
            ArchiveKind::Zip,
            ArchiveKind::Tar,
            ArchiveKind::TarGz,
            ArchiveKind::SevenZ,
        ] {
            let p = t.path().join("a.bin");
            build(kind, &p, content);
            let dest = tempfile::tempdir().unwrap();
            let n = extract(&p, kind, &items(&[""]), dest.path(), None, &Obs::default()).unwrap();
            assert_eq!(n, 2, "{kind:?}");
            assert_eq!(std::fs::read(dest.path().join("a.txt")).unwrap(), b"alpha");
            assert_eq!(
                std::fs::read(dest.path().join("d/b.txt")).unwrap(),
                b"beta beta"
            );
        }
        let p = t.path().join("one.gz");
        make_gz(&p, Some("n.txt"), b"gz data");
        let dest = tempfile::tempdir().unwrap();
        let n = extract(
            &p,
            ArchiveKind::Gz,
            &items(&["n.txt"]),
            dest.path(),
            None,
            &Obs::default(),
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            std::fs::read(dest.path().join("n.txt")).unwrap(),
            b"gz data"
        );
        assert_eq!(mtime_of(&dest.path().join("n.txt")), TEST_MTIME as i64);
    }

    #[test]
    fn extract_password() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.zip");
        make_zip_aes(&p, &[("d", None), ("d/s.txt", Some(b"secret"))], "pw");
        let dest = tempfile::tempdir().unwrap();
        let obs = Obs::default();
        let r = extract(&p, ArchiveKind::Zip, &items(&[""]), dest.path(), None, &obs);
        assert_eq!(r, Err(ArchiveError::PasswordRequired));
        let r = extract(
            &p,
            ArchiveKind::Zip,
            &items(&[""]),
            dest.path(),
            Some("no"),
            &obs,
        );
        assert_eq!(r, Err(ArchiveError::WrongPassword));
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        let r = extract(
            &p,
            ArchiveKind::Zip,
            &items(&[""]),
            dest.path(),
            Some("pw"),
            &obs,
        );
        assert_eq!(r, Ok(1));
        assert_eq!(
            std::fs::read(dest.path().join("d/s.txt")).unwrap(),
            b"secret"
        );
    }

    #[test]
    fn extract_cancel_stops() {
        let t = tempfile::tempdir().unwrap();
        let big = vec![7u8; 5 << 20];
        for kind in [ArchiveKind::Zip, ArchiveKind::TarGz, ArchiveKind::SevenZ] {
            let p = t.path().join("a.bin");
            build(kind, &p, &[("big.bin", Some(&big[..]))]);
            let dest = tempfile::tempdir().unwrap();
            let obs = Obs {
                cancel_after: 1,
                ..Default::default()
            };
            let r = extract(&p, kind, &items(&[""]), dest.path(), None, &obs);
            assert_eq!(r, Err(ArchiveError::Cancelled), "{kind:?}");
            assert!(obs.progress.load(Ordering::SeqCst) <= 2);
        }
    }

    #[test]
    fn extract_truncated_gz_is_corrupt() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.gz");
        let data: Vec<u8> = (0..200_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        make_gz(&p, Some("x.bin"), &data);
        let bytes = std::fs::read(&p).unwrap();
        // Keep the real trailer so the index still looks fine, but cut the deflate body.
        let mut cut = bytes[..bytes.len() / 2].to_vec();
        cut.extend_from_slice(&bytes[bytes.len() - 8..]);
        std::fs::write(&p, cut).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let r = extract(
            &p,
            ArchiveKind::Gz,
            &items(&[""]),
            dest.path(),
            None,
            &Obs::default(),
        );
        assert!(matches!(r, Err(ArchiveError::Corrupt(_))), "{r:?}");
    }
}
