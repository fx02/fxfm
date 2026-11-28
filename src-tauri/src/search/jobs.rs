//! Jobs that change the index: load, volume scans, subtree rescans (graft), volume removal.

use super::live::{volume_uuid, EventOpts};
use super::*;

impl Inner {
    pub(super) fn load(&self) {
        // The mount list may be older than this job (the index was turned on later): the
        // enabled volumes mounted now are queued below.
        self.refresh_mounts();
        let cancel = lock(&self.run).cancel.clone();
        if self.read().is_none() {
            let dir = lock(&self.run).dir.clone();
            let loaded = {
                let _bg = background();
                load(&dir)
            };
            match loaded {
                Ok(s) => {
                    let counts = volume_counts(&s);
                    let complete = complete_roots(&s);
                    let mut g = self.write();
                    // Not when the index was turned off during the load.
                    if self.still_current(&cancel) && g.is_none() {
                        let mut run = lock(&self.run);
                        run.counts = counts;
                        run.complete = complete;
                        *g = Some(s);
                    }
                }
                Err(LoadError::Missing) => {}
                Err(e) => eprintln!("fxfm: search index: {e}; rebuilding"),
            }
        }
        self.queue_missing();
        self.start_streams();
    }

    pub(super) fn scan_volume(&self, root: &str) {
        let (settings, cancel, mounted) = {
            let run = lock(&self.run);
            if !run.pending.contains(root) {
                return; // a duplicate job; the scan already ran
            }
            (
                run.settings.clone(),
                run.cancel.clone(),
                run.mounted.clone(),
            )
        };
        let startup = root == self.root;
        let vol = mounted.iter().find(|m| m.path == root);
        if !startup && vol.is_none() {
            // Not mounted: its entries stay as they are.
            self.done_pending(root, &cancel);
            return;
        }
        let network = vol.is_some_and(|m| m.network) && !startup;
        // Changes made while the scan runs (it holds no lock) are recorded and replayed after
        // the swap; the result is current as of `since`.
        let since = if network {
            0
        } else {
            self.begin_recording(root)
        };
        self.scan_and_swap(root, &settings, &cancel, &mounted, vol, network, since);
        self.done_pending(root, &cancel);
        self.end_recording(root);
    }

    /// The body of [`Inner::scan_volume`] after the mount check.
    #[allow(clippy::too_many_arguments)]
    fn scan_and_swap(
        &self,
        root: &str,
        settings: &SearchSettings,
        cancel: &Arc<AtomicBool>,
        mounted: &[Mounted],
        vol: Option<&Mounted>,
        network: bool,
        since: u64,
    ) {
        let startup = root == self.root;
        // The volume is scanned into a fresh index (no lock held); the other volumes are copied
        // in afterwards, so the result has no tombstones and needs no compaction.
        let mut fresh = Index::new();
        let rid = fresh.add_root(root);
        let opts = ScanOptions {
            exclude: excludes(settings),
            skip_mounts: skip_mounts(root, &self.root, mounted),
            cancel: cancel.clone(),
        };
        let report = {
            let _bg = background();
            scan_into(&mut fresh, rid, Path::new(root), &opts)
        };
        let mut stamps = HashMap::new();
        if !report.cancelled && settings.archives {
            self.reuse_archives(&mut fresh, rid, &mut stamps, cancel);
            let _bg = background();
            index_archives_under(
                &mut fresh,
                rid,
                &archive_options(settings),
                network,
                &mut stamps,
                cancel,
            );
        }
        #[cfg(test)]
        if let Some(hook) = lock(&self.scan_hook).take() {
            hook();
        }
        if cancel.load(Ordering::Relaxed) || (!startup && !self.volume_present(root)) {
            // Cancelled, or the volume went away during the scan (a dropped share reads as
            // empty or partial): keep the old entries.
            return;
        }
        let n = fresh.count_under(rid).saturating_sub(1) as u64;
        let mut s = self.begin_merge(
            Stored {
                index: fresh,
                volumes: Vec::new(),
                stamps,
            },
            Some(root),
        );
        s.volumes.push(VolumeState {
            root: root.to_string(),
            uuid: if network { None } else { volume_uuid(root) },
            event_id: since,
            refreshed: now(),
            network,
            source: if network {
                vol.and_then(|m| m.source.clone())
            } else {
                None
            },
            denied: report.denied,
            blocked: report.blocked,
            complete: true,
        });
        if self.save_copy(&s, cancel) && self.finish_swap(s, cancel) {
            lock(&self.run).counts.insert(root.to_string(), n);
        } else {
            self.abort_copy();
        }
    }

    /// Whether a volume is still mounted and readable (checked after its scan).
    fn volume_present(&self, root: &str) -> bool {
        let now = (self.mounts)();
        let present = now.iter().any(|m| m.path == root) && std::fs::read_dir(root).is_ok();
        lock(&self.run).mounted = now;
        present
    }

    /// Rescans one folder or file of an indexed volume (a "Retry" of a denied folder; Task 8
    /// calls it for FSEvents rescans). The path is scanned into its own small index with no lock
    /// held; then the write lock is taken only to drop the old subtree and graft the new one, so
    /// it is held for time proportional to the subtree, never for a copy of the whole index. A
    /// merge running meanwhile gets the graft replayed through the journal. A volume root gets a
    /// full scan instead.
    pub(crate) fn rescan_path(&self, path: &str) {
        let path = trim(path);
        let (settings, cancel, mounted) = {
            let run = lock(&self.run);
            (
                run.settings.clone(),
                run.cancel.clone(),
                run.mounted.clone(),
            )
        };
        if settings.index != Some(true) {
            return;
        }
        let Some(vol) = self.read().as_ref().and_then(|s| {
            s.volumes
                .iter()
                .filter(|v| is_under(&path, &v.root))
                .max_by_key(|v| v.root.len())
                .cloned()
        }) else {
            return;
        };
        if path == vol.root {
            self.enqueue_scan(&path);
            return;
        }
        let name = last_name(&path);
        let exclude = excludes(&settings);
        let filter = EventOpts {
            vol_root: vol.root.clone(),
            exclude: exclude.clone(),
            skip_mounts: skip_mounts(&vol.root, &self.root, &mounted),
            archives: settings.archives,
        };
        if filter.excluded(&path) {
            return;
        }
        let md = {
            let _bg = background();
            std::fs::symlink_metadata(&path).ok()
        };
        let mut sub = Index::new();
        let sub_root = sub.add_root(&path);
        let mut stamps = HashMap::new();
        let mut denied = Vec::new();
        let mut blocked = Vec::new();
        let archive_opts = archive_options(&settings);
        if let Some(md) = &md {
            if md.is_dir() && !is_dataless(md) {
                let opts = ScanOptions {
                    exclude,
                    skip_mounts: skip_mounts(&vol.root, &self.root, &mounted),
                    cancel: cancel.clone(),
                };
                let report = {
                    let _bg = background();
                    scan_into(&mut sub, sub_root, Path::new(&path), &opts)
                };
                denied = report.denied;
                blocked = report.blocked;
                if settings.archives && !report.cancelled {
                    self.reuse_archives(&mut sub, sub_root, &mut stamps, &cancel);
                    let _bg = background();
                    index_archives_under(
                        &mut sub,
                        sub_root,
                        &archive_opts,
                        vol.network,
                        &mut stamps,
                        &cancel,
                    );
                }
            } else if md.is_file() && settings.archives && ArchiveKind::from_name(&name).is_some() {
                let _bg = background();
                index_archive(
                    &mut sub,
                    sub_root,
                    Path::new(&path),
                    &archive_opts,
                    vol.network,
                    md,
                );
                stamps.insert(path.clone(), stamp_of(md));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let g = Arc::new(Graft {
            meta: md.as_ref().map(meta_of),
            path,
            vol_root: vol.root.clone(),
            sub,
            sub_root,
            stamps,
            denied,
            blocked,
        });
        let mut w = self.write();
        if !self.still_current(&cancel) {
            return;
        }
        let Some(s) = w.as_mut() else {
            return;
        };
        let delta = apply_graft(s, &g);
        let compact = needs_compaction(s.index.dead(), s.index.len());
        if let Some(j) = lock(&self.journal).as_mut() {
            j.push(Mutation::Graft(g));
        }
        {
            let mut run = lock(&self.run);
            run.dirty = true;
            if let Some(c) = run.counts.get_mut(&vol.root) {
                *c = (*c as i64 + delta).max(0) as u64;
            }
        }
        drop(w);
        if compact {
            self.queue_compact();
        }
    }

    /// Spec: an archive is re-read only when its size or mtime changes. Every archive below
    /// `root` in the separate scan `sub` whose file still has the size and mtime stamped in the
    /// live index gets its listing copied from the live index (with that stamp, so
    /// `index_archives_under` then skips it) instead of being read again. Files are stat'ed with
    /// no lock held; the read lock is taken briefly per archive for the copy.
    pub(super) fn reuse_archives(
        &self,
        sub: &mut Index,
        root: EntryId,
        stamps: &mut HashMap<String, ArchiveStamp>,
        cancel: &AtomicBool,
    ) {
        let mut found: Vec<(EntryId, String)> = Vec::new();
        let mut stack = sub.children(root);
        while let Some(id) = stack.pop() {
            let m = sub.meta(id);
            if m.in_archive {
                continue;
            }
            if m.is_dir {
                stack.extend(sub.children(id));
            } else if ArchiveKind::from_name(sub.name(id)).is_some() {
                found.push((id, sub.path(id)));
            }
        }
        for (id, path) in found {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let md = {
                let _bg = background();
                std::fs::metadata(&path)
            };
            let Ok(md) = md else {
                continue;
            };
            let now = stamp_of(&md);
            let g = self.read();
            let Some(live) = g.as_ref() else {
                return;
            };
            if live.stamps.get(&path) != Some(&now) {
                continue;
            }
            let Some(lid) = live.index.lookup(&path) else {
                continue;
            };
            let lm = live.index.meta(lid);
            if lm.is_dir || lm.in_archive {
                continue;
            }
            sub.copy_children(&live.index, lid, id);
            stamps.insert(path, now);
        }
    }

    /// Archive indexing was turned off: drops every entry inside an archive and all archive
    /// stamps with a merge (no disk access, no rescan).
    pub(super) fn drop_archives(&self) {
        if self.read().is_none() {
            return;
        }
        let cancel = lock(&self.run).cancel.clone();
        let mut s = self.begin_merge_with(Stored::default(), None, false);
        s.stamps.clear();
        let counts = volume_counts(&s);
        if self.save_copy(&s, &cancel) && self.finish_swap(s, &cancel) {
            lock(&self.run).counts = counts;
        } else {
            self.abort_copy();
        }
    }

    /// An archive option changed: reads again every indexed archive in `set` (each one is a
    /// small subtree rescan of the archive file, which never consults its stamp).
    pub(super) fn reread_archives(&self, set: ArchiveSet) {
        let paths: Vec<String> = {
            let g = self.read();
            let Some(s) = g.as_ref() else {
                return;
            };
            let network: Vec<&str> = s
                .volumes
                .iter()
                .filter(|v| v.network)
                .map(|v| v.root.as_str())
                .collect();
            s.stamps
                .keys()
                .filter(|p| match set {
                    ArchiveSet::Gz => ArchiveKind::from_name(&last_name(p))
                        .is_some_and(ArchiveKind::is_stream_compressed),
                    ArchiveSet::Network => network.iter().any(|r| is_under(p, r)),
                })
                .cloned()
                .collect()
        };
        let cancel = lock(&self.run).cancel.clone();
        for p in paths {
            if !self.still_current(&cancel) {
                return;
            }
            self.rescan_path(&p);
        }
    }

    pub(super) fn remove_volume(&self, root: &str) {
        if root == self.root || self.read().is_none() {
            return;
        }
        self.unwatch(Some(root));
        let cancel = lock(&self.run).cancel.clone();
        let s = self.begin_merge(Stored::default(), Some(root));
        if self.save_copy(&s, &cancel) && self.finish_swap(s, &cancel) {
            lock(&self.run).counts.remove(root);
        } else {
            self.abort_copy();
        }
    }
}

/// Entries per volume root (without the root) of a loaded index.
fn volume_counts(s: &Stored) -> HashMap<String, u64> {
    s.volumes
        .iter()
        .filter_map(|v| {
            let id = s.index.lookup(&v.root)?;
            Some((
                v.root.clone(),
                s.index.count_under(id).saturating_sub(1) as u64,
            ))
        })
        .collect()
}
