//! Versioned disk format of the name index (`index.bin`).
//!
//! Layout, all integers little-endian:
//! - header: magic `FXFMIDX\0` (8 bytes), [`FORMAT_VERSION`] (u32), payload length (u64);
//! - payload: entry count (u64), name arena size (u64), the entries (33 bytes each: mtime, size,
//!   name offset, parent, first child, next sibling, flags; links already in the compact
//!   numbering), name_len and folded_len (u16 each) per entry, the name arena, the roots, the
//!   volumes and the archive stamps (strings are u32 length + UTF-8);
//! - trailer: CRC-32 (IEEE) of the payload (u32).
//!
//! `save` never mutates or clones the index: it serializes only live entries, renumbering them
//! on the fly with a one-word-per-entry remap table, so the file is always compact (no
//! tombstones) whatever state the in-memory index is in. The hash table is not stored; `load`
//! rebuilds it. `load` streams the file (no whole-file buffer) and builds vectors of exact
//! capacity, so its peak is the loaded index itself. Every count, offset and link read from the
//! file is validated against the remaining payload, so a damaged file yields
//! [`LoadError::Corrupt`], never a panic or a huge allocation.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::archives::ArchiveStamp;
use super::index::{Entry, Index, F_ARCHIVE, F_DEAD, F_DIR, F_FOLD_SEP, F_ROOT, NONE};

/// 2: volumes carry `blocked` (EPERM folders) next to `denied`.
pub const FORMAT_VERSION: u32 = 2;

const MAGIC: &[u8; 8] = b"FXFMIDX\0";
const HEADER_LEN: u64 = 8 + 4 + 8;
const FILE_NAME: &str = "index.bin";
const TMP_NAME: &str = "index.bin.tmp";
/// Serialized size of one [`Entry`].
const ENTRY_LEN: u64 = 33;
const KNOWN_FLAGS: u8 = F_DIR | F_ARCHIVE | F_ROOT | F_FOLD_SEP;

/// One indexed volume and what is known about its freshness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeState {
    pub root: String,
    pub uuid: Option<String>,
    pub event_id: u64,
    /// Unix seconds of the last full or incremental refresh.
    pub refreshed: i64,
    pub network: bool,
    /// The `mntfrom` of a network volume.
    pub source: Option<String>,
    /// Folders that could not be read for lack of permission (EACCES).
    pub denied: Vec<String>,
    /// Folders macOS blocked (EPERM: privacy or system protection).
    pub blocked: Vec<String>,
    pub complete: bool,
}

/// Everything persisted in `index.bin`.
#[derive(Debug, Clone, Default)]
pub struct Stored {
    pub index: Index,
    pub volumes: Vec<VolumeState>,
    pub stamps: HashMap<String, ArchiveStamp>,
}

#[derive(Debug)]
pub enum LoadError {
    /// No index file.
    Missing,
    /// A file with another format version (the caller rebuilds).
    OldVersion(u32),
    Corrupt(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Missing => write!(f, "no index file"),
            LoadError::OldVersion(v) => write!(f, "index format version {v}"),
            LoadError::Corrupt(m) => write!(f, "corrupt index file: {m}"),
        }
    }
}

impl std::error::Error for LoadError {}

fn corrupt<T>(msg: impl Into<String>) -> Result<T, LoadError> {
    Err(LoadError::Corrupt(msg.into()))
}

// ---------------------------------------------------------------------------------------------
// CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320)

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC_TABLE: [u32; 256] = crc_table();

/// Running CRC state (not yet finalized): start with `!0`, finish with `!state`.
fn crc_update(mut c: u32, data: &[u8]) -> u32 {
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

// ---------------------------------------------------------------------------------------------
// Save

struct CrcWriter<W: Write> {
    inner: W,
    crc: u32,
    written: u64,
}

impl<W: Write> Write for CrcWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc = crc_update(self.crc, &buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn put_str<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let len = u32::try_from(s.len()).map_err(|_| io::Error::other("string too long"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(s.as_bytes())
}

fn put_opt_str<W: Write>(w: &mut W, s: &Option<String>) -> io::Result<()> {
    match s {
        Some(s) => {
            w.write_all(&[1])?;
            put_str(w, s)
        }
        None => w.write_all(&[0]),
    }
}

fn write_payload<W: Write>(w: &mut W, s: &Stored) -> io::Result<()> {
    let ix = &s.index;
    // Remap table: old id -> compact id (NONE for tombstones).
    let mut map = vec![NONE; ix.entries.len()];
    let (mut count, mut bytes) = (0u32, 0u64);
    for (old, e) in ix.entries.iter().enumerate() {
        if e.flags & F_DEAD == 0 {
            map[old] = count;
            count += 1;
            bytes += stored_len(e) as u64;
        }
    }
    let remap = |x: u32| if x == NONE { NONE } else { map[x as usize] };
    w.write_all(&u64::from(count).to_le_bytes())?;
    w.write_all(&bytes.to_le_bytes())?;
    let mut off = 0u32;
    for e in ix.entries.iter().filter(|e| e.flags & F_DEAD == 0) {
        let mut rec = [0u8; ENTRY_LEN as usize];
        rec[0..8].copy_from_slice(&e.mtime.to_le_bytes());
        rec[8..16].copy_from_slice(&e.size.to_le_bytes());
        rec[16..20].copy_from_slice(&off.to_le_bytes());
        rec[20..24].copy_from_slice(&remap(e.parent).to_le_bytes());
        rec[24..28].copy_from_slice(&remap(e.first_child).to_le_bytes());
        rec[28..32].copy_from_slice(&remap(e.next_sibling).to_le_bytes());
        rec[32] = e.flags;
        w.write_all(&rec)?;
        off += stored_len(e) as u32;
    }
    // name_len and folded_len (u16 each) of every live entry, in entry order.
    for e in ix.entries.iter().filter(|e| e.flags & F_DEAD == 0) {
        w.write_all(&e.name_len.to_le_bytes())?;
        w.write_all(&e.folded_len.to_le_bytes())?;
    }
    for e in ix.entries.iter().filter(|e| e.flags & F_DEAD == 0) {
        let start = e.off as usize;
        w.write_all(&ix.names.as_bytes()[start..start + stored_len(e)])?;
    }
    w.write_all(&(ix.roots.len() as u32).to_le_bytes())?;
    for &r in &ix.roots {
        w.write_all(&remap(r).to_le_bytes())?;
    }
    w.write_all(&(s.volumes.len() as u32).to_le_bytes())?;
    for v in &s.volumes {
        put_str(w, &v.root)?;
        put_opt_str(w, &v.uuid)?;
        w.write_all(&v.event_id.to_le_bytes())?;
        w.write_all(&v.refreshed.to_le_bytes())?;
        w.write_all(&[v.network as u8, v.complete as u8])?;
        put_opt_str(w, &v.source)?;
        for list in [&v.denied, &v.blocked] {
            w.write_all(&(list.len() as u32).to_le_bytes())?;
            for d in list {
                put_str(w, d)?;
            }
        }
    }
    let mut stamps: Vec<_> = s.stamps.iter().collect();
    stamps.sort_by(|a, b| a.0.cmp(b.0));
    w.write_all(&(stamps.len() as u32).to_le_bytes())?;
    for (path, st) in stamps {
        put_str(w, path)?;
        w.write_all(&st.size.to_le_bytes())?;
        w.write_all(&st.mtime.to_le_bytes())?;
    }
    Ok(())
}

fn stored_len(e: &Entry) -> usize {
    e.name_len as usize
        + if e.flags & F_FOLD_SEP != 0 {
            e.folded_len as usize
        } else {
            0
        }
}

/// Writes `dir/index.bin` atomically (temp file, fsync, rename), creating `dir` when missing.
/// Only live entries are written, so the file is compact; `s` is not modified.
pub fn save(dir: &Path, s: &Stored) -> io::Result<()> {
    write_unsynced(dir, s)?.commit()
}

/// The first half of [`save`]: the temp file is written (into the page cache, not yet flushed
/// to disk) and the index is no longer needed. [`PendingSave::commit`] does the slow part
/// (fsync, rename) without it, so a caller holding the index lock can release it first.
pub struct PendingSave {
    dir: std::path::PathBuf,
    file: File,
}

pub fn write_unsynced(dir: &Path, s: &Stored) -> io::Result<PendingSave> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(TMP_NAME);
    match write_file(&tmp, s) {
        Ok(file) => Ok(PendingSave {
            dir: dir.to_path_buf(),
            file,
        }),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

impl PendingSave {
    /// Flushes the temp file and publishes it as `index.bin`.
    pub fn commit(self) -> io::Result<()> {
        let tmp = self.dir.join(TMP_NAME);
        let result = crate::ops::sync_file(&self.file)
            .and_then(|()| fs::rename(&tmp, self.dir.join(FILE_NAME)));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        } else if let Ok(d) = File::open(&self.dir) {
            let _ = d.sync_all();
        }
        result
    }
}

fn write_file(tmp: &Path, s: &Stored) -> io::Result<File> {
    let mut bw = BufWriter::with_capacity(1 << 20, File::create(tmp)?);
    bw.write_all(MAGIC)?;
    bw.write_all(&FORMAT_VERSION.to_le_bytes())?;
    bw.write_all(&0u64.to_le_bytes())?; // payload length, patched below
    let mut cw = CrcWriter {
        inner: bw,
        crc: !0,
        written: 0,
    };
    write_payload(&mut cw, s)?;
    let (len, crc) = (cw.written, !cw.crc);
    let mut bw = cw.inner;
    bw.write_all(&crc.to_le_bytes())?;
    let mut f = bw.into_inner().map_err(|e| e.into_error())?;
    f.seek(SeekFrom::Start(12))?;
    f.write_all(&len.to_le_bytes())?;
    Ok(f)
}

// ---------------------------------------------------------------------------------------------
// Load

/// Reads the payload sequentially, tracking the CRC and how many payload bytes are left, so no
/// count read from the file can ask for more than the file holds.
struct Payload<R: Read> {
    inner: R,
    remaining: u64,
    crc: u32,
}

impl<R: Read> Payload<R> {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), LoadError> {
        if buf.len() as u64 > self.remaining {
            return corrupt("payload ends early");
        }
        self.inner
            .read_exact(buf)
            .map_err(|e| LoadError::Corrupt(format!("read: {e}")))?;
        self.remaining -= buf.len() as u64;
        self.crc = crc_update(self.crc, buf);
        Ok(())
    }
    fn u8(&mut self) -> Result<u8, LoadError> {
        let mut b = [0u8; 1];
        self.fill(&mut b)?;
        Ok(b[0])
    }
    fn u32(&mut self) -> Result<u32, LoadError> {
        let mut b = [0u8; 4];
        self.fill(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn u64(&mut self) -> Result<u64, LoadError> {
        let mut b = [0u8; 8];
        self.fill(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn i64(&mut self) -> Result<i64, LoadError> {
        Ok(self.u64()? as i64)
    }
    /// A count of items that take at least `min` payload bytes each.
    fn count(&mut self, min: u64) -> Result<usize, LoadError> {
        let n = self.u32()? as u64;
        if n.saturating_mul(min) > self.remaining {
            return corrupt("count exceeds payload");
        }
        Ok(n as usize)
    }
    fn bytes(&mut self, len: u64) -> Result<Vec<u8>, LoadError> {
        if len > self.remaining {
            return corrupt("length exceeds payload");
        }
        let mut v = vec![0u8; len as usize];
        self.fill(&mut v)?;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, LoadError> {
        let len = u64::from(self.u32()?);
        String::from_utf8(self.bytes(len)?).or_else(|_| corrupt("string is not UTF-8"))
    }
    fn opt_string(&mut self) -> Result<Option<String>, LoadError> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.string().map(Some),
            _ => corrupt("bad option tag"),
        }
    }
    fn flag(&mut self) -> Result<bool, LoadError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => corrupt("bad bool"),
        }
    }
}

/// Loads `dir/index.bin`. The returned index has exact-capacity vectors and a rebuilt hash table.
pub fn load(dir: &Path) -> Result<Stored, LoadError> {
    let file = match File::open(dir.join(FILE_NAME)) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(e) => return corrupt(format!("open: {e}")),
    };
    let file_len = file
        .metadata()
        .map_err(|e| LoadError::Corrupt(format!("stat: {e}")))?
        .len();
    let mut r = BufReader::with_capacity(1 << 20, file);
    let mut head = [0u8; HEADER_LEN as usize];
    if r.read_exact(&mut head).is_err() || &head[..8] != MAGIC {
        return corrupt("bad header");
    }
    let version = u32::from_le_bytes(head[8..12].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(LoadError::OldVersion(version));
    }
    let payload_len = u64::from_le_bytes(head[12..20].try_into().unwrap());
    if file_len != HEADER_LEN.saturating_add(payload_len).saturating_add(4) {
        return corrupt("file length does not match the header");
    }
    let mut p = Payload {
        inner: r,
        remaining: payload_len,
        crc: !0,
    };
    let stored = read_payload(&mut p)?;
    if p.remaining != 0 {
        return corrupt("unread payload bytes");
    }
    let mut tail = [0u8; 4];
    p.inner
        .read_exact(&mut tail)
        .map_err(|e| LoadError::Corrupt(format!("read: {e}")))?;
    if u32::from_le_bytes(tail) != !p.crc {
        return corrupt("checksum mismatch");
    }
    Ok(stored)
}

fn read_payload<R: Read>(p: &mut Payload<R>) -> Result<Stored, LoadError> {
    let count = p.u64()?;
    let names_len = p.u64()?;
    // Entry records (33 bytes) + lengths (4 bytes) per entry, then the names.
    let per_entry = ENTRY_LEN + 4;
    if count > u64::from(NONE) - 1
        || count.saturating_mul(per_entry) > p.remaining
        || names_len > u64::from(u32::MAX)
        || count * per_entry + names_len > p.remaining
    {
        return corrupt("entry counts exceed payload");
    }
    let n = count as usize;
    let mut entries: Vec<Entry> = Vec::with_capacity(n);
    let mut rec = [0u8; ENTRY_LEN as usize];
    for _ in 0..n {
        p.fill(&mut rec)?;
        entries.push(Entry {
            mtime: i64::from_le_bytes(rec[0..8].try_into().unwrap()),
            size: u64::from_le_bytes(rec[8..16].try_into().unwrap()),
            off: u32::from_le_bytes(rec[16..20].try_into().unwrap()),
            parent: u32::from_le_bytes(rec[20..24].try_into().unwrap()),
            first_child: u32::from_le_bytes(rec[24..28].try_into().unwrap()),
            next_sibling: u32::from_le_bytes(rec[28..32].try_into().unwrap()),
            name_len: 0,
            folded_len: 0,
            flags: rec[32],
        });
    }
    let mut lens = [0u8; 4];
    for e in entries.iter_mut() {
        p.fill(&mut lens)?;
        e.name_len = u16::from_le_bytes([lens[0], lens[1]]);
        e.folded_len = u16::from_le_bytes([lens[2], lens[3]]);
    }
    let names =
        String::from_utf8(p.bytes(names_len)?).or_else(|_| corrupt("name arena is not UTF-8"))?;
    validate_entries(&entries, &names)?;

    let nroots = p.count(4)?;
    let mut roots = Vec::with_capacity(nroots);
    for _ in 0..nroots {
        let r = p.u32()?;
        match entries.get(r as usize) {
            Some(e) if e.flags & F_ROOT != 0 => roots.push(r),
            _ => return corrupt("root is not a root entry"),
        }
    }
    let root_entries = entries.iter().filter(|e| e.flags & F_ROOT != 0).count();
    if root_entries != roots.len() {
        return corrupt("root list does not match root entries");
    }

    let nvol = p.count(4)?;
    let mut volumes = Vec::with_capacity(nvol);
    for _ in 0..nvol {
        let root = p.string()?;
        let uuid = p.opt_string()?;
        let event_id = p.u64()?;
        let refreshed = p.i64()?;
        let network = p.flag()?;
        let complete = p.flag()?;
        let source = p.opt_string()?;
        let mut lists = [Vec::new(), Vec::new()];
        for list in lists.iter_mut() {
            let n = p.count(4)?;
            list.reserve_exact(n);
            for _ in 0..n {
                list.push(p.string()?);
            }
        }
        let [denied, blocked] = lists;
        volumes.push(VolumeState {
            root,
            uuid,
            event_id,
            refreshed,
            network,
            source,
            denied,
            blocked,
            complete,
        });
    }
    let nst = p.count(20)?;
    let mut stamps = HashMap::with_capacity(nst);
    for _ in 0..nst {
        let path = p.string()?;
        let size = p.u64()?;
        let mtime = p.i64()?;
        stamps.insert(path, ArchiveStamp { size, mtime });
    }

    let mut index = Index {
        entries,
        names,
        roots,
        table: Vec::new(),
        live: 0,
    };
    index.rebuild_table();
    Ok(Stored {
        index,
        volumes,
        stamps,
    })
}

/// Checks everything the index code relies on: no tombstones, names inside the arena on char
/// boundaries, and links that keep the tree acyclic (a parent precedes its children, a sibling
/// link points to an earlier entry of the same parent, a first child follows its parent).
fn validate_entries(entries: &[Entry], names: &str) -> Result<(), LoadError> {
    let n = entries.len() as u32;
    let mut cursor = 0usize;
    for (id, e) in entries.iter().enumerate() {
        let id = id as u32;
        if e.flags & !KNOWN_FLAGS != 0 {
            return corrupt("bad entry flags");
        }
        let (name_len, folded_len) = (e.name_len as usize, e.folded_len as usize);
        let stored = stored_len(e);
        if e.flags & F_FOLD_SEP == 0 && folded_len != name_len {
            return corrupt("folded length without separate copy");
        }
        let start = e.off as usize;
        // Names are laid out back to back in entry order.
        if start != cursor || start + stored > names.len() {
            return corrupt("name outside the arena");
        }
        cursor = start + stored;
        if [start, start + name_len, start + stored]
            .iter()
            .any(|&c| !names.is_char_boundary(c))
        {
            return corrupt("name is not on a character boundary");
        }
        if e.flags & F_ROOT != 0 {
            if e.parent != NONE || e.next_sibling != NONE {
                return corrupt("root with links");
            }
        } else if e.parent >= id || e.parent == NONE {
            return corrupt("bad parent link");
        }
        if e.first_child != NONE && (e.first_child <= id || e.first_child >= n) {
            return corrupt("bad first-child link");
        }
        if e.next_sibling != NONE {
            if e.next_sibling >= id {
                return corrupt("bad sibling link");
            }
            if entries[e.next_sibling as usize].parent != e.parent {
                return corrupt("sibling of another parent");
            }
        }
    }
    if cursor != names.len() {
        return corrupt("unused name bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::index::{EntryId, Meta};
    use std::time::Instant;

    fn dir_meta() -> Meta {
        Meta {
            is_dir: true,
            ..Meta::default()
        }
    }

    fn file_meta(size: u64, mtime: i64) -> Meta {
        Meta {
            size,
            mtime,
            ..Meta::default()
        }
    }

    /// Every live entry as (path, meta), sorted: independent of id numbering.
    fn dump(i: &Index) -> Vec<(String, Meta)> {
        let mut out = Vec::new();
        let mut stack: Vec<EntryId> = i.roots.clone();
        while let Some(id) = stack.pop() {
            out.push((i.path(id), i.meta(id)));
            stack.extend(i.children(id));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn sample() -> Stored {
        let mut index = Index::new();
        let r = index.add_root("/");
        let docs = index.insert(r, "Documents", dir_meta());
        index.insert(docs, "Résumé Ünï.TXT", file_meta(10, 100));
        index.insert(docs, "plain.txt", file_meta(20, 200));
        let tmp = index.insert(r, "scratch", dir_meta());
        index.insert(tmp, "gone.bin", file_meta(1, 1));
        let arc = index.insert(
            r,
            "pack.zip",
            Meta {
                size: 99,
                mtime: 5,
                ..Meta::default()
            },
        );
        let _ = arc;
        let a = index.lookup("/pack.zip").unwrap();
        let inner = index.insert(
            a,
            "inner",
            Meta {
                is_dir: true,
                in_archive: true,
                ..Meta::default()
            },
        );
        index.insert(
            inner,
            "deep.txt",
            Meta {
                size: 3,
                mtime: 4,
                in_archive: true,
                ..Meta::default()
            },
        );
        let m = index.add_root("/Volumes/Ext");
        index.insert(m, "movie.mkv", file_meta(1 << 33, -5));
        // Tombstones: the saved file must not carry them.
        index.remove(tmp);
        let volumes = vec![
            VolumeState {
                root: "/".into(),
                uuid: Some("ABCD-1234".into()),
                event_id: 77,
                refreshed: 1_700_000_000,
                network: false,
                source: None,
                denied: vec!["/Users/x/Library".into(), "/Users/x/Mail".into()],
                blocked: vec!["/Users/x/Desktop".into()],
                complete: true,
            },
            VolumeState {
                root: "/Volumes/Ext".into(),
                uuid: None,
                event_id: u64::MAX,
                refreshed: -1,
                network: true,
                source: Some("//host/share".into()),
                denied: vec![],
                blocked: vec![],
                complete: false,
            },
        ];
        let mut stamps = HashMap::new();
        stamps.insert("/pack.zip".to_string(), ArchiveStamp { size: 99, mtime: 5 });
        stamps.insert("/b/é.tgz".to_string(), ArchiveStamp { size: 1, mtime: -2 });
        Stored {
            index,
            volumes,
            stamps,
        }
    }

    fn file_of(dir: &Path) -> std::path::PathBuf {
        dir.join(FILE_NAME)
    }

    #[test]
    fn round_trip() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("nested/data");
        let s = sample();
        let before = (s.index.entries.len(), s.index.names.len(), s.index.live);
        save(&dir, &s).unwrap();
        // save does not touch the index (tombstones stay) and leaves no temp file.
        assert_eq!(
            before,
            (s.index.entries.len(), s.index.names.len(), s.index.live)
        );
        assert!(!dir.join(TMP_NAME).exists());
        let l = load(&dir).unwrap();
        assert_eq!(dump(&l.index), dump(&s.index));
        assert_eq!(l.index.len(), s.index.len());
        // Compact: no tombstones were written.
        assert_eq!(l.index.entries.len(), s.index.len());
        assert_eq!(l.volumes, s.volumes);
        assert_eq!(l.stamps, s.stamps);
        assert!(l.index.lookup("/scratch").is_none());
        assert!(l.index.lookup("/Documents/Résumé Ünï.TXT").is_some());
    }

    #[test]
    fn two_step_save_publishes_only_on_commit() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &Stored::default()).unwrap();
        let s = sample();
        let pending = write_unsynced(t.path(), &s).unwrap();
        // Written, not yet published: the old file is still in place.
        assert!(t.path().join(TMP_NAME).exists());
        assert_eq!(load(t.path()).unwrap().index.len(), 0);
        drop(s); // the index is no longer needed for the second step
        pending.commit().unwrap();
        assert!(!t.path().join(TMP_NAME).exists());
        assert_eq!(load(t.path()).unwrap().index.len(), sample().index.len());
    }

    #[test]
    fn loaded_index_is_usable_and_exact() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &sample()).unwrap();
        let mut l = load(t.path()).unwrap().index;
        assert_eq!(l.entries.capacity(), l.entries.len());
        assert_eq!(l.names.capacity(), l.names.len());
        let docs = l.lookup("/Documents").unwrap();
        let id = l.insert(docs, "new.txt", file_meta(1, 1));
        assert_eq!(l.path(id), "/Documents/new.txt");
        assert_eq!(
            l.insert(docs, "plain.txt", file_meta(7, 7)),
            l.lookup("/Documents/plain.txt").unwrap()
        );
        let q = crate::search::Query::parse("ünï");
        let (ids, total) = l.search(
            &q,
            &crate::search::Scope::Everywhere { archives: false },
            10,
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert_eq!(total, 1);
        assert_eq!(l.path(ids[0]), "/Documents/Résumé Ünï.TXT");
    }

    #[test]
    fn empty_stored_round_trips() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &Stored::default()).unwrap();
        let l = load(t.path()).unwrap();
        assert!(l.index.is_empty());
        assert!(l.volumes.is_empty() && l.stamps.is_empty());
    }

    #[test]
    fn missing() {
        let t = tempfile::tempdir().unwrap();
        assert!(matches!(load(t.path()), Err(LoadError::Missing)));
        assert!(matches!(
            load(&t.path().join("absent")),
            Err(LoadError::Missing)
        ));
    }

    #[test]
    fn old_version() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &sample()).unwrap();
        let mut b = fs::read(file_of(t.path())).unwrap();
        b[8..12].copy_from_slice(&0u32.to_le_bytes());
        fs::write(file_of(t.path()), b).unwrap();
        assert!(matches!(load(t.path()), Err(LoadError::OldVersion(0))));
    }

    #[test]
    fn truncated_is_corrupt() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &sample()).unwrap();
        let b = fs::read(file_of(t.path())).unwrap();
        for cut in [b.len() / 2, b.len() - 1, 19, 5, 0] {
            fs::write(file_of(t.path()), &b[..cut]).unwrap();
            assert!(
                matches!(load(t.path()), Err(LoadError::Corrupt(_))),
                "cut {cut}"
            );
        }
        let mut longer = b.clone();
        longer.push(0);
        fs::write(file_of(t.path()), longer).unwrap();
        assert!(matches!(load(t.path()), Err(LoadError::Corrupt(_))));
    }

    #[test]
    fn flipped_byte_is_corrupt() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), &sample()).unwrap();
        let b = fs::read(file_of(t.path())).unwrap();
        for pos in 0..b.len() {
            let mut c = b.clone();
            c[pos] ^= 0x40;
            fs::write(file_of(t.path()), c).unwrap();
            // A flip in the version bytes reads as another version; every other flip is corrupt.
            match load(t.path()) {
                Err(LoadError::Corrupt(_)) => {}
                Err(LoadError::OldVersion(_)) if (8..12).contains(&pos) => {}
                other => panic!("flip at {pos}: {:?}", other.map(|_| ())),
            }
        }
    }

    /// Rewrites the payload and fixes the length and CRC, to test the structural checks.
    fn forge(dir: &Path, edit: impl FnOnce(&mut Vec<u8>)) -> Result<Stored, LoadError> {
        let mut b = fs::read(file_of(dir)).unwrap();
        let mut payload = b[HEADER_LEN as usize..b.len() - 4].to_vec();
        edit(&mut payload);
        b.truncate(HEADER_LEN as usize);
        b[12..20].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        let crc = !crc_update(!0, &payload);
        b.extend_from_slice(&payload);
        b.extend_from_slice(&crc.to_le_bytes());
        fs::write(file_of(dir), b).unwrap();
        load(dir)
    }

    #[test]
    fn forged_payloads_are_corrupt_not_panics() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        save(d, &sample()).unwrap();
        // Huge entry count with a valid checksum: must not allocate or panic.
        let r = forge(d, |p| {
            p[0..8].copy_from_slice(&(u32::MAX as u64 - 2).to_le_bytes())
        });
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        let r = forge(d, |p| p[0..8].copy_from_slice(&u64::MAX.to_le_bytes()));
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        // Names length beyond the payload.
        let r = forge(d, |p| p[8..16].copy_from_slice(&u64::MAX.to_le_bytes()));
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        // A parent link pointing at itself / far away (entry 1: parent at 16+33+20).
        let r = forge(d, |p| {
            p[16 + 33 + 20..16 + 33 + 24].copy_from_slice(&1u32.to_le_bytes())
        });
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        let r = forge(d, |p| {
            p[16 + 33 + 20..16 + 33 + 24].copy_from_slice(&9999u32.to_le_bytes())
        });
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        // A name offset outside the arena.
        let r = forge(d, |p| {
            p[16 + 33 + 16..16 + 33 + 20].copy_from_slice(&u32::MAX.to_le_bytes())
        });
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        // Trailing garbage inside the payload.
        let r = forge(d, |p| p.push(0));
        assert!(matches!(r, Err(LoadError::Corrupt(_))));
        // Every truncation of the payload (with a fixed-up CRC) is a clean error.
        save(d, &sample()).unwrap();
        let len = fs::read(file_of(d)).unwrap().len() - HEADER_LEN as usize - 4;
        for cut in 0..len {
            let r = forge_cut(d, cut);
            assert!(matches!(r, Err(LoadError::Corrupt(_))), "cut {cut}");
        }
    }

    fn forge_cut(dir: &Path, cut: usize) -> Result<Stored, LoadError> {
        let d2 = tempfile::tempdir().unwrap();
        fs::copy(file_of(dir), file_of(d2.path())).unwrap();
        forge(d2.path(), |p| p.truncate(cut))
    }

    /// Run: `cargo test -p fxfm-core --release search::store::tests::budget_3m_disk -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn budget_3m_disk() {
        let mut index = Index::new();
        let r = index.add_root("/");
        for d in 0..1000u32 {
            let did = index.insert(r, &format!("Project Folder {d:04}"), dir_meta());
            for f in 0..3000u32 {
                let name = if f % 2 == 0 {
                    format!("Report_{d:04}_{f:05}.TXT")
                } else {
                    format!("f{f}_data_{d}.txt")
                };
                index.insert(did, &name, file_meta(f as u64, (d * f) as i64));
            }
        }
        // Churn: tombstones that save must drop.
        let victim = index.lookup("/Project Folder 0500").unwrap();
        index.remove(victim);
        let live = index.len();
        let s = Stored {
            index,
            volumes: vec![VolumeState {
                root: "/".into(),
                uuid: None,
                event_id: 1,
                refreshed: 0,
                network: false,
                source: None,
                denied: vec![],
                blocked: vec![],
                complete: true,
            }],
            stamps: HashMap::new(),
        };
        let t = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        save(t.path(), &s).unwrap();
        let save_t = t0.elapsed();
        let size = fs::metadata(file_of(t.path())).unwrap().len();
        let heap_before = s.index.heap_bytes();
        drop(s);
        let t0 = Instant::now();
        let l = load(t.path()).unwrap();
        let load_t = t0.elapsed();
        let heap = l.index.heap_bytes();
        eprintln!(
            "entries {live}: file {} MB, save {save_t:?}, load {load_t:?}, heap before save {} MB, after load {} MB",
            size >> 20,
            heap_before >> 20,
            heap >> 20
        );
        assert_eq!(l.index.len(), live);
        assert!(size <= 300 << 20, "file {size}");
        assert!(heap <= 250 << 20, "heap {heap}");
    }
}
