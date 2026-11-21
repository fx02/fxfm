//! In-memory file name index: one tree per indexed volume, names in a string arena.
//!
//! Layout (about 40 bytes per entry plus the name bytes):
//! - `names`: one `String` arena. Each entry's original name is stored at `off`; when the folded
//!   name ([`fold`]) differs from it, the folded name follows right after it (`F_FOLD_SEP`),
//!   otherwise both views share the same bytes. Most names are lowercase ASCII and cost once.
//! - `entries`: one fixed-size [`Entry`] per id with the tree links (`parent`, `first_child`,
//!   `next_sibling`) and the metadata.
//! - `table`: an open-addressing hash table of ids keyed by (parent, name), used by `insert`
//!   and `lookup`; 4 bytes per slot, kept at most 3/4 full.
//!
//! Growth is bounded so the 3M-entry memory budget holds in the live state, not only after a
//! shrink: small structures double, but past [`BIG_ENTRIES`] entries / [`BIG_NAMES`] arena bytes
//! / [`BIG_TABLE`] slots, the vectors grow by 1/16 and the table is resized to 2/3 load whenever it
//! passes 3/4 (so its size need not be a power of two; 1.5 slots per entry right after a resize).
//! Measured at 3M entries: 222 MB after the build, 228 MB at worst (first insert after a shrink).
//! Decision: while a vector is reallocated, its old and new buffers coexist for a moment, so the
//! process peak during a scan can briefly exceed the steady-state number (the table frees its old
//! buffer before allocating the new one).
//!
//! Removed entries are tombstoned (`F_DEAD`) and their ids are not reused until [`Index::compact`].

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{fold, rank, Query, Scope};

pub type EntryId = u32;

/// Marks "no entry" in links and empty hash table slots.
pub(crate) const NONE: u32 = u32::MAX;

pub(crate) const F_DIR: u8 = 1;
pub(crate) const F_ARCHIVE: u8 = 2;
pub(crate) const F_DEAD: u8 = 4;
pub(crate) const F_ROOT: u8 = 8;
/// The folded name is stored after the original name instead of sharing its bytes.
pub(crate) const F_FOLD_SEP: u8 = 16;

/// Entries scanned between two checks of the cancel flag.
const CANCEL_EVERY: usize = 1 << 16;
/// Below this many entries a search runs on the calling thread.
const PARALLEL_MIN: usize = 1 << 16;
const THREADS: usize = 4;

/// Above these sizes the entries / name arena grow by 1/16 and the hash table by 1/8, not 2x.
const BIG_ENTRIES: usize = 1 << 20;
const BIG_NAMES: usize = 16 << 20;
const BIG_TABLE: usize = 1 << 20;

/// A search hit as [`rank`] takes it: (score, mtime, id).
type Hit = (u8, i64, u32);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub in_archive: bool,
}

/// What [`apply_change`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Removed,
    Upserted(EntryId),
    /// A new folder was added: the caller should scan it.
    NewDir(EntryId),
    /// The path is not (or its parent is not) indexed; nothing changed.
    Unknown,
}

/// One indexed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) mtime: i64,
    pub(crate) size: u64,
    /// Byte offset of the name in the `names` arena.
    pub(crate) off: u32,
    pub(crate) parent: u32,
    pub(crate) first_child: u32,
    pub(crate) next_sibling: u32,
    pub(crate) name_len: u16,
    pub(crate) folded_len: u16,
    pub(crate) flags: u8,
}

/// The name index. Plain owned data (no interior mutability): `Send + Sync`, searched via `&self`.
#[derive(Debug, Default, Clone)]
pub struct Index {
    pub(crate) entries: Vec<Entry>,
    pub(crate) names: String,
    pub(crate) roots: Vec<EntryId>,
    /// Hash table of live ids keyed by (parent, name); `NONE` marks an empty slot. Length is 0 or
    /// [`table_size_for`]. Not part of the persistent state: rebuild with [`Index::rebuild_table`].
    pub(crate) table: Vec<u32>,
    /// Live entries (= occupied table slots).
    pub(crate) live: usize,
}

impl Index {
    pub fn new() -> Index {
        Index::default()
    }

    /// Adds (or finds) the root of an indexed volume, given as an absolute path such as `/` or
    /// `/Volumes/Mdisk`.
    pub fn add_root(&mut self, root: &str) -> EntryId {
        let root = if root.len() > 1 {
            root.trim_end_matches('/')
        } else {
            root
        };
        if let Some(id) = self.find(NONE, root) {
            return id;
        }
        let id = self.push(NONE, root, F_ROOT | F_DIR, 0, 0);
        self.roots.push(id);
        id
    }

    /// Adds `name` under `parent`, or replaces the meta of the existing entry with that name.
    pub fn insert(&mut self, parent: EntryId, name: &str, meta: Meta) -> EntryId {
        debug_assert!(
            self.is_live(parent),
            "insert under a dead or unknown parent"
        );
        let name = clip(name);
        if let Some(id) = self.find(parent, name) {
            self.set_meta(id, meta);
            return id;
        }
        self.push(parent, name, meta_flags(meta), meta.size, meta.mtime)
    }

    /// Finds an absolute path; a path into an archive is `<archive path>/<inner path>`.
    pub fn lookup(&self, path: &str) -> Option<EntryId> {
        let mut best: Option<(EntryId, &str, usize)> = None;
        for &r in &self.roots {
            let rn = self.name(r);
            let rest = if rn == "/" {
                path.strip_prefix('/')
            } else {
                path.strip_prefix(rn).and_then(|s| {
                    if s.is_empty() {
                        Some(s)
                    } else {
                        s.strip_prefix('/')
                    }
                })
            };
            if let Some(rest) = rest {
                if best.is_none_or(|(_, _, len)| rn.len() > len) {
                    best = Some((r, rest, rn.len()));
                }
            }
        }
        let (mut cur, rest, _) = best?;
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            cur = self.find(cur, part)?;
        }
        Some(cur)
    }

    /// Removes the entry and its whole subtree. Ids stay tombstoned until [`Index::compact`].
    pub fn remove(&mut self, id: EntryId) {
        if !self.is_live(id) {
            return;
        }
        let parent = self.entries[id as usize].parent;
        let next = self.entries[id as usize].next_sibling;
        if parent == NONE {
            self.roots.retain(|&r| r != id);
        } else if self.entries[parent as usize].first_child == id {
            self.entries[parent as usize].first_child = next;
        } else {
            let mut c = self.entries[parent as usize].first_child;
            while c != NONE {
                let n = self.entries[c as usize].next_sibling;
                if n == id {
                    self.entries[c as usize].next_sibling = next;
                    break;
                }
                c = n;
            }
        }
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            self.table_remove(x);
            self.live -= 1;
            let e = &mut self.entries[x as usize];
            e.flags |= F_DEAD;
            let mut c = e.first_child;
            while c != NONE {
                stack.push(c);
                c = self.entries[c as usize].next_sibling;
            }
        }
    }

    /// The live children of `id`, in no particular order.
    pub fn children(&self, id: EntryId) -> Vec<EntryId> {
        let mut out = Vec::new();
        if !self.is_live(id) {
            return out;
        }
        let mut c = self.entries[id as usize].first_child;
        while c != NONE {
            out.push(c);
            c = self.entries[c as usize].next_sibling;
        }
        out
    }

    /// The absolute path: the original (not folded) names joined with `/`.
    pub fn path(&self, id: EntryId) -> String {
        let mut chain = Vec::new();
        let mut c = id;
        while c != NONE {
            chain.push(c);
            c = self.entries[c as usize].parent;
        }
        let mut out = String::new();
        for &c in chain.iter().rev() {
            if !out.is_empty() && !out.ends_with('/') {
                out.push('/');
            }
            out.push_str(self.name(c));
        }
        out
    }

    pub fn meta(&self, id: EntryId) -> Meta {
        let e = &self.entries[id as usize];
        Meta {
            is_dir: e.flags & F_DIR != 0,
            size: e.size,
            mtime: e.mtime,
            in_archive: e.flags & F_ARCHIVE != 0,
        }
    }

    /// The parent of `id`; None for a volume root.
    pub fn parent(&self, id: EntryId) -> Option<EntryId> {
        let p = self.entries[id as usize].parent;
        (p != NONE).then_some(p)
    }

    /// Tombstoned entries waiting for [`Index::compact`].
    pub fn dead(&self) -> usize {
        self.entries.len() - self.live
    }

    /// Live entries in the subtree of `id`, `id` included; 0 when `id` is dead.
    pub fn count_under(&self, id: EntryId) -> usize {
        if !self.is_live(id) {
            return 0;
        }
        let mut n = 0;
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            n += 1;
            let mut c = self.entries[x as usize].first_child;
            while c != NONE {
                stack.push(c);
                c = self.entries[c as usize].next_sibling;
            }
        }
        n
    }

    /// Copies the volume rooted at `root` in `src` (live entries only) into this index and
    /// returns its new root. Used to build a fresh, tombstone-free index next to the live one.
    pub fn copy_root(&mut self, src: &Index, root: EntryId) -> EntryId {
        let new_root = self.add_root(src.name(root));
        self.copy_children(src, root, new_root);
        new_root
    }

    /// Puts `name` (with `meta`) under the live `parent`, replacing an existing entry of that name
    /// and its subtree, and copies the children of `from_root` in `from` below it. `from` is a
    /// separate scan of that folder (for example rooted at its full path). Cost is proportional
    /// to the old and new subtree sizes.
    pub fn graft(
        &mut self,
        parent: EntryId,
        name: &str,
        meta: Meta,
        from: &Index,
        from_root: EntryId,
    ) -> EntryId {
        if let Some(old) = self.find(parent, clip(name)) {
            self.remove(old);
        }
        let id = self.insert(parent, name, meta);
        self.copy_children(from, from_root, id);
        id
    }

    /// Like [`Index::copy_root`], but leaves out every entry inside an archive.
    pub fn copy_root_without_archives(&mut self, src: &Index, root: EntryId) -> EntryId {
        let new_root = self.add_root(src.name(root));
        self.copy_tree(src, root, new_root, false);
        new_root
    }

    /// Copies the live children of `from` in `src` (recursively) below `to` (for example the
    /// listing of an unchanged archive from the live index into a fresh scan).
    pub fn copy_children(&mut self, src: &Index, from: EntryId, to: EntryId) {
        self.copy_tree(src, from, to, true);
    }

    fn copy_tree(&mut self, src: &Index, from: EntryId, to: EntryId, archives: bool) {
        let mut stack = vec![(from, to)];
        while let Some((from, to)) = stack.pop() {
            let mut c = src.entries[from as usize].first_child;
            while c != NONE {
                let meta = src.meta(c);
                if archives || !meta.in_archive {
                    let id = self.insert(to, src.name(c), meta);
                    stack.push((c, id));
                }
                c = src.entries[c as usize].next_sibling;
            }
        }
    }

    /// The volume root that `id` lives under.
    pub fn root_of(&self, id: EntryId) -> EntryId {
        let mut c = id;
        loop {
            let p = self.entries[c as usize].parent;
            if p == NONE {
                return c;
            }
            c = p;
        }
    }

    /// Returns the best `limit` hits (see [`rank`]) and the total number of hits. Roots are never
    /// hits. Returns nothing when the query is empty, the scope folder is not indexed, or
    /// `cancel` is set during the scan.
    pub fn search(
        &self,
        q: &Query,
        scope: &Scope,
        limit: usize,
        cancel: &AtomicBool,
    ) -> (Vec<EntryId>, usize) {
        if q.is_empty() {
            return (Vec::new(), 0);
        }
        let (archives, under) = match scope {
            Scope::Everywhere { archives } => (*archives, None),
            Scope::Under { path, archives } => match self.lookup(path) {
                Some(id) => (*archives, Some(id)),
                None => return (Vec::new(), 0),
            },
        };
        let n = self.entries.len();
        let parts: Vec<Option<(Vec<Hit>, usize)>> = if n < PARALLEL_MIN {
            vec![self.scan(0..n, q, archives, under, limit, cancel)]
        } else {
            let chunk = n.div_ceil(THREADS);
            std::thread::scope(|s| {
                let handles: Vec<_> = (0..THREADS)
                    .map(|k| {
                        let range = (k * chunk).min(n)..((k + 1) * chunk).min(n);
                        s.spawn(move || self.scan(range, q, archives, under, limit, cancel))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("search worker panicked"))
                    .collect()
            })
        };
        if cancel.load(Ordering::Relaxed) {
            return (Vec::new(), 0);
        }
        let mut total = 0;
        let mut hits = Vec::new();
        for part in parts {
            let Some((h, t)) = part else {
                return (Vec::new(), 0);
            };
            total += t;
            hits.extend(h);
        }
        rank(&mut hits, limit);
        (hits.into_iter().map(|h| h.2).collect(), total)
    }

    /// Live entries, roots included.
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Estimated heap use from the capacities of the internal vectors.
    pub fn heap_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Entry>()
            + self.names.capacity()
            + self.table.capacity() * std::mem::size_of::<u32>()
            + self.roots.capacity() * std::mem::size_of::<EntryId>()
    }

    /// Releases the spare capacity left by vector growth (optional: growth is bounded anyway).
    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
        self.names.shrink_to_fit();
        self.roots.shrink_to_fit();
    }

    /// Drops tombstones and their name bytes. Ids change; old ids must not be used afterwards.
    pub fn compact(&mut self) {
        let mut map = vec![NONE; self.entries.len()];
        let mut bytes = 0;
        let mut count = 0u32;
        for (old, e) in self.entries.iter().enumerate() {
            if e.flags & F_DEAD == 0 {
                map[old] = count;
                count += 1;
                bytes += stored_len(e);
            }
        }
        let remap = |x: u32| if x == NONE { NONE } else { map[x as usize] };
        let mut entries = Vec::with_capacity(count as usize);
        let mut names = String::with_capacity(bytes);
        for e in self.entries.iter().filter(|e| e.flags & F_DEAD == 0) {
            let start = e.off as usize;
            let off = names.len() as u32;
            names.push_str(&self.names[start..start + stored_len(e)]);
            debug_assert!(e.first_child == NONE || map[e.first_child as usize] != NONE);
            debug_assert!(e.next_sibling == NONE || map[e.next_sibling as usize] != NONE);
            entries.push(Entry {
                off,
                parent: remap(e.parent),
                first_child: remap(e.first_child),
                next_sibling: remap(e.next_sibling),
                ..*e
            });
        }
        self.roots = self.roots.iter().map(|&r| map[r as usize]).collect();
        self.entries = entries;
        self.names = names;
        self.rebuild_table();
    }

    /// Rebuilds the (parent, name) hash table from `entries` (e.g. after loading from disk).
    pub(crate) fn rebuild_table(&mut self) {
        let live = self
            .entries
            .iter()
            .filter(|e| e.flags & F_DEAD == 0)
            .count();
        self.live = live;
        self.table = Vec::new();
        self.table = vec![NONE; table_size_for(live)];
        for id in 0..self.entries.len() as u32 {
            if self.entries[id as usize].flags & F_DEAD == 0 {
                place(&mut self.table, &self.entries, &self.names, id);
            }
        }
    }

    /// The original name of `id`.
    pub fn name(&self, id: EntryId) -> &str {
        name_of(&self.entries, &self.names, id)
    }

    /// The folded name of `id` (see [`fold`]).
    pub(crate) fn folded(&self, id: EntryId) -> &str {
        let e = &self.entries[id as usize];
        let mut start = e.off as usize;
        if e.flags & F_FOLD_SEP != 0 {
            start += e.name_len as usize;
        }
        &self.names[start..start + e.folded_len as usize]
    }

    fn is_live(&self, id: EntryId) -> bool {
        self.entries
            .get(id as usize)
            .is_some_and(|e| e.flags & F_DEAD == 0)
    }

    fn set_meta(&mut self, id: EntryId, meta: Meta) {
        let e = &mut self.entries[id as usize];
        e.flags = (e.flags & (F_ROOT | F_FOLD_SEP)) | meta_flags(meta);
        e.size = meta.size;
        e.mtime = meta.mtime;
    }

    fn push(&mut self, parent: u32, name: &str, flags: u8, size: u64, mtime: i64) -> EntryId {
        assert!(self.entries.len() < NONE as usize, "index full");
        let id = self.entries.len() as u32;
        let off = u32::try_from(self.names.len()).expect("name arena over 4 GB");
        let folded = if is_folded_ascii(name) {
            None
        } else {
            let mut f = fold(name);
            f.truncate(clip(&f).len());
            (f != name).then_some(f)
        };
        let need = name.len() + folded.as_ref().map_or(0, String::len);
        let (len, cap) = (self.names.len(), self.names.capacity());
        if len + need > cap {
            let target = cap + headroom(cap, BIG_NAMES);
            self.names.reserve_exact((target - len).max(need));
        }
        if self.entries.len() == self.entries.capacity() {
            let cap = self.entries.capacity();
            self.entries.reserve_exact(headroom(cap, BIG_ENTRIES));
        }
        self.names.push_str(name);
        let mut flags = flags;
        let mut folded_len = name.len();
        if let Some(f) = folded {
            self.names.push_str(&f);
            flags |= F_FOLD_SEP;
            folded_len = f.len();
        }
        let next_sibling = if parent == NONE {
            NONE
        } else {
            std::mem::replace(&mut self.entries[parent as usize].first_child, id)
        };
        self.entries.push(Entry {
            mtime,
            size,
            off,
            parent,
            first_child: NONE,
            next_sibling,
            name_len: name.len() as u16,
            folded_len: folded_len as u16,
            flags,
        });
        self.live += 1;
        if self.live * 4 > self.table.len() * 3 {
            // Free the old table first so the two never coexist.
            self.table = Vec::new();
            self.table = vec![NONE; table_size_for(self.live)];
            for x in 0..self.entries.len() as u32 {
                if self.entries[x as usize].flags & F_DEAD == 0 {
                    place(&mut self.table, &self.entries, &self.names, x);
                }
            }
        } else {
            place(&mut self.table, &self.entries, &self.names, id);
        }
        id
    }

    /// The live entry named `name` under `parent` (`NONE` for roots).
    fn find(&self, parent: u32, name: &str) -> Option<EntryId> {
        if self.table.is_empty() {
            return None;
        }
        let n = self.table.len();
        let mut i = slot(hash(parent, name.as_bytes()), n);
        loop {
            let id = self.table[i];
            if id == NONE {
                return None;
            }
            if self.entries[id as usize].parent == parent && self.name(id) == name {
                return Some(id);
            }
            i = next(i, n);
        }
    }

    /// Deletes `id` from the hash table (backward-shift deletion, no tombstones).
    fn table_remove(&mut self, id: EntryId) {
        let n = self.table.len();
        let mut i = home(&self.entries, &self.names, id, n);
        while self.table[i] != id {
            debug_assert!(self.table[i] != NONE, "id missing from table");
            i = next(i, n);
        }
        let mut j = i;
        loop {
            j = next(j, n);
            let x = self.table[j];
            if x == NONE {
                break;
            }
            let k = home(&self.entries, &self.names, x, n);
            let stays = if i <= j {
                i < k && k <= j
            } else {
                i < k || k <= j
            };
            if !stays {
                self.table[i] = x;
                i = j;
            }
        }
        self.table[i] = NONE;
    }

    /// Scans `range` of ids; `None` when cancelled. Keeps at most a few times `limit` hits.
    fn scan(
        &self,
        range: Range<usize>,
        q: &Query,
        archives: bool,
        under: Option<EntryId>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Option<(Vec<Hit>, usize)> {
        let skip = F_DEAD | F_ROOT | if archives { 0 } else { F_ARCHIVE };
        let cap = limit.saturating_mul(2).max(4096);
        let mut hits = Vec::new();
        let mut total = 0;
        for (k, id) in range.enumerate() {
            if k % CANCEL_EVERY == 0 && cancel.load(Ordering::Relaxed) {
                return None;
            }
            let e = &self.entries[id];
            if e.flags & skip != 0 {
                continue;
            }
            let Some(score) = q.score(self.folded(id as u32)) else {
                continue;
            };
            if under.is_some_and(|u| !self.is_below(id as u32, u)) {
                continue;
            }
            total += 1;
            hits.push((score, e.mtime, id as u32));
            if hits.len() >= cap {
                rank(&mut hits, limit);
            }
        }
        Some((hits, total))
    }

    /// Whether `ancestor` is on the parent chain of `id` (not `id` itself).
    fn is_below(&self, id: EntryId, ancestor: EntryId) -> bool {
        let mut c = self.entries[id as usize].parent;
        while c != NONE {
            if c == ancestor {
                return true;
            }
            c = self.entries[c as usize].parent;
        }
        false
    }
}

/// Applies one file system change (e.g. from FSEvents). `None` stat removes the path and its
/// subtree; `Some` updates or inserts it, but only when its parent is already indexed.
pub fn apply_change(index: &mut Index, path: &str, stat: Option<Meta>) -> Change {
    let Some(meta) = stat else {
        return match index.lookup(path) {
            Some(id) => {
                index.remove(id);
                Change::Removed
            }
            None => Change::Unknown,
        };
    };
    if let Some(id) = index.lookup(path) {
        if index.entries[id as usize].flags & F_ROOT != 0 {
            return Change::Upserted(id);
        }
        let was_dir = index.meta(id).is_dir;
        if was_dir && !meta.is_dir {
            for c in index.children(id) {
                index.remove(c);
            }
        }
        index.set_meta(id, meta);
        return if meta.is_dir && !was_dir {
            Change::NewDir(id)
        } else {
            Change::Upserted(id)
        };
    }
    let Some((parent_path, name)) = path.rsplit_once('/') else {
        return Change::Unknown;
    };
    if name.is_empty() {
        return Change::Unknown;
    }
    let parent_path = if parent_path.is_empty() {
        "/"
    } else {
        parent_path
    };
    let Some(parent) = index.lookup(parent_path) else {
        return Change::Unknown;
    };
    let id = index.insert(parent, name, meta);
    if meta.is_dir {
        Change::NewDir(id)
    } else {
        Change::Upserted(id)
    }
}

fn meta_flags(meta: Meta) -> u8 {
    (if meta.is_dir { F_DIR } else { 0 }) | (if meta.in_archive { F_ARCHIVE } else { 0 })
}

/// Names longer than `u16::MAX` bytes are cut at a char boundary (no real file system allows them).
fn clip(s: &str) -> &str {
    if s.len() <= u16::MAX as usize {
        return s;
    }
    let mut end = u16::MAX as usize;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// ASCII without uppercase letters is its own fold, so no second copy is needed.
fn is_folded_ascii(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase())
}

/// Arena bytes used by an entry (name plus a separate folded copy, if any).
fn stored_len(e: &Entry) -> usize {
    e.name_len as usize
        + if e.flags & F_FOLD_SEP != 0 {
            e.folded_len as usize
        } else {
            0
        }
}

fn name_of<'a>(entries: &[Entry], names: &'a str, id: EntryId) -> &'a str {
    let e = &entries[id as usize];
    &names[e.off as usize..e.off as usize + e.name_len as usize]
}

/// Extra capacity to add when a structure of capacity `cap` is full: double while it is small,
/// then grow by 1/16 so the spare capacity stays bounded.
fn headroom(cap: usize, big: usize) -> usize {
    if cap < big {
        cap.max(64)
    } else {
        cap / 16
    }
}

/// Slot count for `live` entries. Small tables are powers of two (doubling, load 3/8..3/4);
/// from [`BIG_TABLE`] slots on, the table is sized to 2/3 load, so it is resized every 1/8 of
/// growth (when it passes 3/4) and never holds more than 1.5 slots per entry.
fn table_size_for(live: usize) -> usize {
    let pow2 = ((live + 1) * 4 / 3 + 1).next_power_of_two().max(16);
    if pow2 <= BIG_TABLE {
        pow2
    } else {
        (live + 1) * 3 / 2
    }
}

/// Maps a hash onto `0..n` (multiply-shift, works for any `n`).
fn slot(h: u64, n: usize) -> usize {
    ((h as u128 * n as u128) >> 64) as usize
}

/// The next probe position (linear probing, wrapping).
fn next(i: usize, n: usize) -> usize {
    if i + 1 == n {
        0
    } else {
        i + 1
    }
}

fn home(entries: &[Entry], names: &str, id: EntryId, n: usize) -> usize {
    slot(
        hash(
            entries[id as usize].parent,
            name_of(entries, names, id).as_bytes(),
        ),
        n,
    )
}

fn place(table: &mut [u32], entries: &[Entry], names: &str, id: EntryId) {
    let n = table.len();
    let mut i = home(entries, names, id, n);
    while table[i] != NONE {
        i = next(i, n);
    }
    table[i] = id;
}

/// FxHash-style word hash of (parent, name) with a murmur3 finalizer.
fn hash(parent: u32, name: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h = (parent as u64 ^ ((name.len() as u64) << 32)).wrapping_mul(K);
    for chunk in name.chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(K);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn dir() -> Meta {
        Meta {
            is_dir: true,
            ..Meta::default()
        }
    }
    fn file() -> Meta {
        Meta {
            size: 10,
            mtime: 1,
            ..Meta::default()
        }
    }
    fn arc() -> Meta {
        Meta {
            in_archive: true,
            ..file()
        }
    }
    fn all() -> Scope {
        Scope::Everywhere { archives: false }
    }
    fn find(i: &Index, q: &str, scope: &Scope) -> Vec<String> {
        let (ids, _) = i.search(&Query::parse(q), scope, 1000, &AtomicBool::new(false));
        ids.into_iter().map(|id| i.path(id)).collect()
    }

    #[test]
    fn copy_root_copies_a_volume() {
        let mut src = Index::new();
        let r = src.add_root("/v");
        let a = src.insert(r, "Äa", dir());
        src.insert(a, "b.txt", file());
        let z = src.insert(a, "x.zip", file());
        src.insert(z, "inner.txt", arc());
        src.insert(r, "gone", file());
        src.remove(src.lookup("/v/gone").unwrap());
        let other = src.add_root("/w");
        src.insert(other, "w.txt", file());
        let mut dst = Index::new();
        let d = dst.add_root("/w");
        dst.insert(d, "mine.txt", file());
        let nr = dst.copy_root(&src, r);
        assert_eq!(dst.path(nr), "/v");
        assert_eq!(dst.len(), 2 + 5);
        assert_eq!(dst.dead(), 0);
        let inner = dst.lookup("/v/Äa/x.zip/inner.txt").unwrap();
        assert!(dst.meta(inner).in_archive);
        assert_eq!(dst.meta(dst.lookup("/v/Äa/b.txt").unwrap()), file());
        assert!(dst.lookup("/v/gone").is_none());
        assert_eq!(find(&dst, "äa", &all()), vec!["/v/Äa".to_string()]);
    }

    #[test]
    fn graft_puts_a_scanned_subtree_under_a_live_parent() {
        let mut live = Index::new();
        let r = live.add_root("/v");
        let a = live.insert(r, "a", dir());
        live.insert(a, "old.txt", file());
        // A separate scan of /v/a/new, rooted at its full path.
        let mut sub = Index::new();
        let sr = sub.add_root("/v/a/new");
        let d = sub.insert(sr, "deep", dir());
        sub.insert(d, "x.txt", file());
        let meta = Meta {
            is_dir: true,
            mtime: 7,
            ..Meta::default()
        };
        let g = live.graft(a, "new", meta, &sub, sr);
        assert_eq!(live.path(g), "/v/a/new");
        assert_eq!(live.meta(g), meta);
        assert!(live.lookup("/v/a/new/deep/x.txt").is_some());
        assert!(live.lookup("/v/a/old.txt").is_some());
        assert_eq!(live.len(), 6);
        // Grafting over an existing name replaces its subtree.
        let mut sub2 = Index::new();
        let s2 = sub2.add_root("/v/a/new");
        sub2.insert(s2, "y.txt", file());
        live.graft(a, "new", meta, &sub2, s2);
        assert!(live.lookup("/v/a/new/deep").is_none());
        assert!(live.lookup("/v/a/new/y.txt").is_some());
        assert_eq!(live.len(), 5);
        assert_eq!(find(&live, "x.txt", &all()), Vec::<String>::new());
    }

    #[test]
    fn parent_dead_and_subtree_counts() {
        let mut i = Index::new();
        let r = i.add_root("/v");
        let a = i.insert(r, "a", dir());
        let b = i.insert(a, "b.txt", file());
        let c = i.insert(r, "c.txt", file());
        assert_eq!(i.parent(r), None);
        assert_eq!(i.parent(b), Some(a));
        assert_eq!(i.dead(), 0);
        assert_eq!(i.count_under(r), 4);
        assert_eq!(i.count_under(a), 2);
        i.remove(a);
        assert_eq!(i.dead(), 2);
        assert_eq!(i.count_under(r), 2);
        assert_eq!(i.count_under(a), 0);
        assert_eq!(i.count_under(c), 1);
        i.compact();
        assert_eq!(i.dead(), 0);
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn insert_lookup_path() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let u = i.insert(r, "Users", dir());
        let f = i.insert(u, "a.txt", file());
        assert_eq!(i.lookup("/Users/a.txt"), Some(f));
        assert_eq!(i.path(f), "/Users/a.txt");
        assert_eq!(i.lookup("/"), Some(r));
        assert_eq!(i.lookup("/Users"), Some(u));
        assert_eq!(i.lookup("/Users/b.txt"), None);
        assert_eq!(i.root_of(f), r);
        assert_eq!(i.meta(f), file());
        assert_eq!(i.children(u), vec![f]);
    }

    #[test]
    fn add_root_is_idempotent_and_volumes_are_separate() {
        let mut i = Index::new();
        let r = i.add_root("/");
        assert_eq!(i.add_root("/"), r);
        let v = i.add_root("/Volumes/Mdisk");
        assert_ne!(v, r);
        let f = i.insert(v, "Ünïcode.TXT", file());
        assert_eq!(i.path(f), "/Volumes/Mdisk/Ünïcode.TXT");
        assert_eq!(i.lookup("/Volumes/Mdisk/Ünïcode.TXT"), Some(f));
        assert_eq!(i.root_of(f), v);
        assert_eq!(find(&i, "ünï", &all()), vec!["/Volumes/Mdisk/Ünïcode.TXT"]);
        // Roots are never hits.
        assert!(find(&i, "mdisk", &all()).is_empty());
    }

    #[test]
    fn insert_existing_replaces_meta() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let a = i.insert(r, "a", file());
        let m = Meta {
            size: 99,
            mtime: 7,
            ..file()
        };
        assert_eq!(i.insert(r, "a", m), a);
        assert_eq!(i.meta(a), m);
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn remove_drops_subtree() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let d = i.insert(r, "d", dir());
        i.insert(d, "x", file());
        i.insert(d, "y", file());
        i.remove(d);
        for p in ["/d", "/d/x", "/d/y"] {
            assert_eq!(i.lookup(p), None, "{p}");
        }
        assert!(find(&i, "x", &all()).is_empty());
        assert_eq!(i.len(), 1);
        assert!(i.children(r).is_empty());
    }

    #[test]
    fn rename_via_apply_change() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let d = i.insert(r, "d", dir());
        i.insert(d, "old.txt", file());
        assert_eq!(apply_change(&mut i, "/d/old.txt", None), Change::Removed);
        assert!(matches!(
            apply_change(&mut i, "/d/new.txt", Some(file())),
            Change::Upserted(_)
        ));
        assert!(find(&i, "old", &all()).is_empty());
        assert_eq!(find(&i, "new", &all()), vec!["/d/new.txt"]);
    }

    #[test]
    fn apply_change_new_dir_reports_scan() {
        let mut i = Index::new();
        let r = i.add_root("/");
        i.insert(r, "d", dir());
        let c = apply_change(&mut i, "/d/sub", Some(dir()));
        assert!(matches!(c, Change::NewDir(_)));
        if let Change::NewDir(id) = c {
            assert_eq!(i.lookup("/d/sub"), Some(id));
        }
    }

    #[test]
    fn apply_change_unknown_parent() {
        let mut i = Index::new();
        i.add_root("/");
        assert_eq!(
            apply_change(&mut i, "/nope/x", Some(file())),
            Change::Unknown
        );
        assert_eq!(i.lookup("/nope"), None);
        assert_eq!(
            apply_change(&mut i, "relative/x", Some(file())),
            Change::Unknown
        );
    }

    #[test]
    fn scope_under() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let a = i.insert(r, "a", dir());
        let b = i.insert(r, "b", dir());
        i.insert(a, "x.txt", file());
        i.insert(b, "x.txt", file());
        let s = Scope::Under {
            path: "/a".into(),
            archives: false,
        };
        assert_eq!(find(&i, "x", &s), vec!["/a/x.txt"]);
        let s = Scope::Under {
            path: "/missing".into(),
            archives: false,
        };
        assert!(find(&i, "x", &s).is_empty());
    }

    #[test]
    fn archives_hidden_unless_asked() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let z = i.insert(r, "a.zip", file());
        i.insert(z, "inner.txt", arc());
        assert!(find(&i, "inner", &Scope::Everywhere { archives: false }).is_empty());
        assert_eq!(
            find(&i, "inner", &Scope::Everywhere { archives: true }),
            vec!["/a.zip/inner.txt"]
        );
        assert!(i.lookup("/a.zip/inner.txt").is_some());
    }

    #[test]
    fn search_returns_total_and_cap() {
        let mut i = Index::new();
        let r = i.add_root("/");
        for n in 0..1500 {
            i.insert(r, &format!("f{n}.txt"), file());
        }
        let (ids, total) = i.search(&Query::parse("txt"), &all(), 1000, &AtomicBool::new(false));
        assert_eq!((ids.len(), total), (1000, 1500));
    }

    #[test]
    fn search_ranks_by_class_then_mtime() {
        let mut i = Index::new();
        let r = i.add_root("/");
        i.insert(r, "homework.txt", Meta { mtime: 9, ..file() });
        i.insert(r, "scope of work.txt", Meta { mtime: 9, ..file() });
        i.insert(r, "work old.txt", Meta { mtime: 1, ..file() });
        i.insert(r, "work new.txt", Meta { mtime: 5, ..file() });
        assert_eq!(
            find(&i, "work", &all()),
            vec![
                "/work new.txt",
                "/work old.txt",
                "/scope of work.txt",
                "/homework.txt"
            ]
        );
    }

    #[test]
    fn cancelled_search_returns_nothing() {
        let mut i = Index::new();
        let r = i.add_root("/");
        i.insert(r, "a.txt", file());
        let (ids, total) = i.search(&Query::parse("a"), &all(), 10, &AtomicBool::new(true));
        assert!(ids.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn compact_keeps_paths() {
        let mut i = Index::new();
        let r = i.add_root("/");
        let v = i.add_root("/Volumes/X");
        let mut keep = Vec::new();
        for d in 0..20 {
            let did = i.insert(if d % 2 == 0 { r } else { v }, &format!("Dir{d}"), dir());
            for f in 0..30 {
                i.insert(did, &format!("F{f}.Txt"), file());
            }
            if d % 3 == 0 {
                i.remove(did);
            } else {
                keep.push(i.path(did));
            }
        }
        i.remove(i.lookup("/Dir2/F5.Txt").unwrap());
        let before = i.len();
        i.compact();
        assert_eq!(i.len(), before);
        assert_eq!(i.lookup("/Dir2/F5.Txt"), None);
        let mut seen = 0;
        let (ids, total) = i.search(
            &Query::parse("f"),
            &Scope::Everywhere { archives: true },
            usize::MAX,
            &AtomicBool::new(false),
        );
        assert_eq!(ids.len(), total);
        for id in ids {
            assert_eq!(i.lookup(&i.path(id)), Some(id));
            seen += 1;
        }
        assert_eq!(
            seen,
            before - 2 - keep.len(),
            "files only: no roots, no dirs"
        );
        for p in keep {
            let id = i.lookup(&p).unwrap();
            assert_eq!(i.path(id), p);
            assert_eq!(i.children(id).len(), if p == "/Dir2" { 29 } else { 30 });
        }
        // Still usable after compaction.
        let d = i.lookup("/Dir2").unwrap();
        let n = i.insert(d, "late.txt", file());
        assert_eq!(i.lookup("/Dir2/late.txt"), Some(n));
    }

    #[test]
    fn table_of_any_size_survives_churn() {
        let mut i = Index::new();
        let r = i.add_root("/");
        for n in 0..200 {
            i.insert(r, &format!("n{n}"), file());
        }
        // Force an odd-sized, nearly full table, as big indexes get from the 2/3-load resizes.
        i.table = vec![NONE; 277];
        for id in 0..i.entries.len() as u32 {
            place(&mut i.table, &i.entries, &i.names, id);
        }
        for n in (0..200).step_by(3) {
            i.remove(i.lookup(&format!("/n{n}")).unwrap());
        }
        for n in 0..200 {
            let found = i.lookup(&format!("/n{n}"));
            assert_eq!(found.is_some(), n % 3 != 0, "n{n}");
        }
        for n in 200..300 {
            i.insert(r, &format!("n{n}"), file());
        }
        for n in 0..300 {
            let id = i.lookup(&format!("/n{n}"));
            assert_eq!(id.is_some(), n >= 200 || n % 3 != 0, "n{n}");
            if let Some(id) = id {
                assert_eq!(i.path(id), format!("/n{n}"));
            }
        }
    }

    #[test]
    fn growth_is_bounded_when_big() {
        assert_eq!(headroom(0, BIG_ENTRIES), 64);
        assert_eq!(headroom(1000, BIG_ENTRIES), 1000);
        assert_eq!(headroom(8 << 20, BIG_ENTRIES), 1 << 19);
        assert!(table_size_for(100).is_power_of_two());
        let n = table_size_for(3_000_000);
        assert!(
            (3_000_000 * 4 / 3..=3_000_000 * 3 / 2 + 2).contains(&n),
            "{n}"
        );
    }

    #[test]
    fn index_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Index>();
    }

    /// Run: `cargo test -p fxfm-core --release search::index::tests::budget_3m -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn budget_3m() {
        let t = Instant::now();
        let mut i = Index::new();
        let r = i.add_root("/");
        for d in 0..1000u32 {
            let did = i.insert(r, &format!("Project Folder {d:04}"), dir());
            for f in 0..3000u32 {
                let name = if f % 2 == 0 {
                    format!("Report_{d:04}_{f:05}.TXT")
                } else {
                    format!("f{f}_data_{d}.txt")
                };
                i.insert(
                    did,
                    &name,
                    Meta {
                        size: f as u64,
                        mtime: (d * f) as i64,
                        ..Meta::default()
                    },
                );
            }
        }
        let build = t.elapsed();
        assert_eq!(i.len(), 3_001_001);
        let built = i.heap_bytes();
        // The live index keeps growing after the scan (FSEvents inserts): the budget must hold
        // in that grown state, without a shrink.
        let late = i.insert(r, "late arrivals", dir());
        for n in 0..1000u32 {
            i.insert(late, &format!("Late_File_{n}.TXT"), file());
        }
        let grown = i.heap_bytes();
        // The same after a shrink: the first insert must not double anything.
        i.shrink_to_fit();
        let shrunk = i.heap_bytes();
        i.insert(late, "Ünïcode after shrink.txt", file());
        let after_shrink = i.heap_bytes();
        eprintln!(
            "entries {} build {build:?}; heap MB: after build {}, +1001 inserts {}, shrunk {}, \
             +1 insert after shrink {} (entries {} MB, names {} MB, table {} MB)",
            i.len(),
            built >> 20,
            grown >> 20,
            shrunk >> 20,
            after_shrink >> 20,
            (i.entries.capacity() * std::mem::size_of::<Entry>()) >> 20,
            i.names.capacity() >> 20,
            (i.table.capacity() * 4) >> 20,
        );
        for (what, bytes) in [
            ("after build", built),
            ("after inserts", grown),
            ("after insert past shrink", after_shrink),
        ] {
            assert!(bytes <= 250 << 20, "{what}: heap {bytes}");
        }
        for q in ["f12 txt", "f12 *.txt", "report 0042", "*.txt", "nomatch"] {
            let query = Query::parse(q);
            let t = Instant::now();
            let (ids, total) = i.search(
                &query,
                &Scope::Everywhere { archives: false },
                1000,
                &AtomicBool::new(false),
            );
            let took = t.elapsed();
            eprintln!("query {q:?}: {total} hits ({} kept) in {took:?}", ids.len());
            assert!(took.as_millis() < 500, "{q}: {took:?}");
        }
        let scope = Scope::Under {
            path: "/Project Folder 0007".into(),
            archives: false,
        };
        let t = Instant::now();
        let (_, total) = i.search(&Query::parse("txt"), &scope, 1000, &AtomicBool::new(false));
        eprintln!("scoped query: {total} hits in {:?}", t.elapsed());
        assert_eq!(total, 3000);
        assert!(t.elapsed().as_millis() < 500);
    }
}
