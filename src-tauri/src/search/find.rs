//! Running a search: index or direct walk, result items and batch events.

use super::*;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub mtime: i64,
    pub size: u64,
    pub is_dir: bool,
    pub in_archive: bool,
    pub mounted: bool,
}

/// An item for the UI; `archive` is the path of the archive holding an in-archive entry.
fn make_item(
    path: String,
    meta: fxfm_core::search::Meta,
    archive: Option<&str>,
    mounted: bool,
) -> Item {
    let folder = parent_of(&path);
    let name = match archive {
        Some(a) if meta.in_archive && path.len() > a.len() + 1 => {
            format!("{} ▸ {}", last_name(a), &path[a.len() + 1..])
        }
        _ => last_name(&path),
    };
    Item {
        name,
        folder,
        mtime: meta.mtime,
        size: meta.size,
        is_dir: meta.is_dir,
        in_archive: meta.in_archive,
        mounted,
        path,
    }
}

fn index_item(ix: &Index, id: EntryId, root: &str, mounted: &HashSet<String>) -> Item {
    let meta = ix.meta(id);
    let vol = ix.path(ix.root_of(id));
    let archive = meta.in_archive.then(|| {
        let mut p = ix.parent(id);
        while let Some(x) = p.filter(|&x| ix.meta(x).in_archive) {
            p = ix.parent(x);
        }
        p.map(|x| ix.path(x)).unwrap_or_default()
    });
    let is_mounted = vol == root || mounted.contains(&vol);
    make_item(ix.path(id), meta, archive.as_deref(), is_mounted)
}

/// The archive file a walk hit inside an archive lives in: the first prefix that is a file.
fn walk_archive(path: &str) -> Option<String> {
    path.match_indices('/')
        .skip(1)
        .map(|(i, _)| &path[..i])
        .find(|p| Path::new(p).is_file())
        .map(str::to_string)
}

fn walk_item(h: WalkHit) -> Item {
    let archive = if h.meta.in_archive {
        walk_archive(&h.path)
    } else {
        None
    };
    make_item(h.path, h.meta, archive.as_deref(), true)
}

impl Inner {
    /// Whether a search (everywhere, or below `under`) can be answered from the index.
    pub(super) fn index_covers(
        &self,
        under: Option<&str>,
        settings: &SearchSettings,
        mounted: &[Mounted],
    ) -> bool {
        if settings.index != Some(true) {
            return false;
        }
        let g = self.read();
        let Some(s) = g.as_ref() else {
            return false;
        };
        let complete = |root: &str| s.volumes.iter().any(|v| v.root == root && v.complete);
        let Some(under) = under else {
            return complete(&self.root);
        };
        // The innermost volume holding `under`: an indexed root or any mounted volume.
        let best = s
            .volumes
            .iter()
            .map(|v| v.root.as_str())
            .chain(mounted.iter().map(|m| m.path.as_str()))
            .filter(|r| !STARTUP_ALIASES.contains(r) || *r == self.root)
            .chain(std::iter::once(self.root.as_str()))
            .filter(|r| is_under(under, r))
            .max_by_key(|r| r.len());
        let Some(best) = best else {
            return false;
        };
        complete(best)
            && !excludes(settings)
                .iter()
                .any(|x| x.starts_with('/') && is_under(under, x))
            && s.index.lookup(under).is_some()
    }

    /// Emits a batch if search `id` is still current; holding `current` makes a cancel final.
    pub(super) fn emit_batch(
        &self,
        id: u64,
        cancel: &AtomicBool,
        items: Vec<Item>,
        total: u64,
        done: bool,
    ) {
        let cur = lock(&self.current);
        if cancel.load(Ordering::Relaxed) || cur.as_ref().map(|c| c.0) != Some(id) {
            return;
        }
        (self.emit)(
            "search://batch",
            json!({"id": id, "items": items, "total": total, "done": done}),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn index_search(
        &self,
        id: u64,
        cancel: &AtomicBool,
        q: &Query,
        under: Option<String>,
        archives: bool,
        limit: usize,
        mounted: &[Mounted],
    ) {
        let mounted: HashSet<String> = mounted.iter().map(|m| m.path.clone()).collect();
        let (items, total) = {
            let g = self.read();
            let Some(s) = g.as_ref() else {
                drop(g);
                self.emit_batch(id, cancel, Vec::new(), 0, true);
                return;
            };
            let scope = match under {
                Some(path) => Scope::Under { path, archives },
                None => Scope::Everywhere { archives },
            };
            let (ids, total) = s.index.search(q, &scope, limit, cancel);
            let items: Vec<Item> = ids
                .into_iter()
                .map(|i| index_item(&s.index, i, &self.root, &mounted))
                .collect();
            (items, total as u64)
        };
        self.emit_batch(id, cancel, items, total, true);
    }

    pub(super) fn walk(
        &self,
        id: u64,
        cancel: &AtomicBool,
        q: &Query,
        under: &str,
        archives: Option<ArchiveOptions>,
        limit: usize,
    ) {
        let mut total = 0u64;
        let mut sent = 0usize;
        walk_search(
            Path::new(under),
            q,
            archives.as_ref(),
            cancel,
            &mut |hits: Vec<WalkHit>| {
                total += hits.len() as u64;
                // Hits past the display cap are counted, not sent.
                let items: Vec<Item> = hits
                    .into_iter()
                    .take(limit.saturating_sub(sent))
                    .map(walk_item)
                    .collect();
                sent += items.len();
                self.emit_batch(id, cancel, items, total, false);
            },
        );
        self.emit_batch(id, cancel, Vec::new(), total, true);
    }
}
