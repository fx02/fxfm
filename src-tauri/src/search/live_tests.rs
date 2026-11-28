//! Tests for FSEvents handling: `handle_events` on an index built over a temp tree (fake events,
//! any OS) and the service's live-update bookkeeping. No real event stream is used here.

use super::live::*;
use super::tests::*;
use super::*;
use std::fs;

const FILE: u32 = 0x10000;
const DIR: u32 = 0x20000;

/// An index of a fresh temp tree, as the service would hold it.
fn indexed(prep: impl FnOnce(&Path)) -> (tempfile::TempDir, String, Stored) {
    let dir = tempfile::tempdir().unwrap();
    prep(dir.path());
    let root = dir.path().to_str().unwrap().to_string();
    let mut index = Index::new();
    let rid = index.add_root(&root);
    let opts = ScanOptions {
        exclude: default_excludes(),
        skip_mounts: Vec::new(),
        cancel: Arc::default(),
    };
    scan_into(&mut index, rid, Path::new(&root), &opts);
    let s = Stored {
        index,
        volumes: vec![VolumeState {
            root: root.clone(),
            uuid: None,
            event_id: 0,
            refreshed: 0,
            network: false,
            source: None,
            denied: Vec::new(),
            blocked: Vec::new(),
            complete: true,
        }],
        stamps: HashMap::new(),
    };
    (dir, root, s)
}

fn opts(root: &str) -> EventOpts {
    EventOpts {
        vol_root: root.to_string(),
        exclude: default_excludes(),
        skip_mounts: Vec::new(),
        archives: false,
    }
}

fn ev(path: &str, flags: u32, id: u64) -> Event {
    Event {
        path: path.to_string(),
        flags,
        id,
    }
}

fn has(s: &Stored, path: &str) -> bool {
    s.index.lookup(path).is_some()
}

#[test]
fn handle_events_created_file_is_found() {
    let (dir, root, mut s) = indexed(|d| fs::create_dir(d.join("a")).unwrap());
    let p = format!("{root}/a/new.txt");
    fs::write(&p, b"x").unwrap();
    assert!(!has(&s, &p));
    let f = handle_events(&mut s, &[ev(&p, FILE, 7)], &opts(&root));
    assert!(has(&s, &p));
    assert_eq!(f, vec![Followup::SetEventId(7)]);
    assert_eq!(s.volumes[0].event_id, 7);
    drop(dir);
}

#[test]
fn handle_events_deleted_folder_with_2000_files() {
    let (dir, root, mut s) = indexed(|d| {
        fs::create_dir(d.join("big")).unwrap();
        for i in 0..2000 {
            fs::write(d.join(format!("big/file{i}.dat")), b"").unwrap();
        }
    });
    assert!(has(&s, &format!("{root}/big/file1999.dat")));
    fs::remove_dir_all(dir.path().join("big")).unwrap();
    // FSEvents reports every removed file and the folder.
    let mut events: Vec<Event> = (0..2000)
        .map(|i| ev(&format!("{root}/big/file{i}.dat"), FILE, 10 + i))
        .collect();
    events.push(ev(&format!("{root}/big"), DIR, 3000));
    handle_events(&mut s, &events, &opts(&root));
    assert!(!has(&s, &format!("{root}/big")));
    for i in 0..2000 {
        assert!(!has(&s, &format!("{root}/big/file{i}.dat")));
    }
    assert_eq!(s.index.count_under(s.index.lookup(&root).unwrap()), 1);
}

#[test]
fn handle_events_rename_old_gone_new_present() {
    let (dir, root, mut s) = indexed(|d| fs::write(d.join("old.txt"), b"x").unwrap());
    fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
    let old = format!("{root}/old.txt");
    let new = format!("{root}/new.txt");
    handle_events(
        &mut s,
        &[ev(&old, FILE, 1), ev(&new, FILE, 2)],
        &opts(&root),
    );
    assert!(!has(&s, &old));
    assert!(has(&s, &new));
}

#[test]
fn handle_events_must_scan_sub_dirs_rescans_the_subtree() {
    let (dir, root, mut s) = indexed(|d| fs::create_dir_all(d.join("a/b")).unwrap());
    let a = format!("{root}/a");
    let f = handle_events(
        &mut s,
        &[
            ev(&a, DIR | MUST_SCAN_SUB_DIRS, 5),
            // Below a folder that is rescanned anyway: redundant.
            ev(&format!("{root}/a/b"), DIR | MUST_SCAN_SUB_DIRS, 6),
            ev(&format!("{root}/a/b/x.txt"), FILE, 7),
        ],
        &opts(&root),
    );
    assert_eq!(f, vec![Followup::RescanSubtree(a), Followup::SetEventId(7)]);
    // Stored only by the caller, after the rescan ran.
    assert_eq!(s.volumes[0].event_id, 0);
    drop(dir);
}

#[test]
fn handle_events_new_folder_is_rescanned_once() {
    let (dir, root, mut s) = indexed(|_| {});
    fs::create_dir_all(dir.path().join("n/m")).unwrap();
    fs::write(dir.path().join("n/m/f.txt"), b"").unwrap();
    let n = format!("{root}/n");
    let f = handle_events(
        &mut s,
        &[
            ev(&format!("{root}/n/m/f.txt"), FILE, 3),
            ev(&format!("{root}/n/m"), DIR, 2),
            ev(&n, DIR, 1),
        ],
        &opts(&root),
    );
    assert!(has(&s, &n));
    assert_eq!(f, vec![Followup::RescanSubtree(n), Followup::SetEventId(3)]);
}

#[test]
fn handle_events_kernel_dropped_rescans_the_volume() {
    for flag in [
        KERNEL_DROPPED,
        USER_DROPPED,
        EVENT_IDS_WRAPPED,
        ROOT_CHANGED,
    ] {
        let (dir, root, mut s) = indexed(|d| fs::write(d.join("keep.txt"), b"").unwrap());
        fs::remove_file(dir.path().join("keep.txt")).unwrap();
        let f = handle_events(
            &mut s,
            &[
                ev(&format!("{root}/keep.txt"), FILE, 1),
                ev(&root, flag | MUST_SCAN_SUB_DIRS, 2),
            ],
            &opts(&root),
        );
        assert_eq!(
            f,
            vec![Followup::RescanVolume(root.clone())],
            "flag {flag:#x}"
        );
        // The id is not moved past the dropped events: the volume must be rescanned.
        assert_eq!(s.volumes[0].event_id, 0, "flag {flag:#x}");
        // The volume is rescanned as a whole; nothing else is applied.
        assert!(has(&s, &format!("{root}/keep.txt")));
    }
}

#[test]
fn handle_events_root_is_never_removed() {
    let (dir, root, mut s) = indexed(|d| fs::write(d.join("x.txt"), b"").unwrap());
    let gone = format!("{root}-gone");
    // An event for the root itself while it is unreadable must not wipe the volume.
    let mut o = opts(&root);
    o.vol_root = gone.clone();
    s.volumes[0].root = gone.clone();
    s.index = {
        let mut i = Index::new();
        let r = i.add_root(&gone);
        i.insert(r, "x.txt", Meta::default());
        i
    };
    handle_events(&mut s, &[ev(&gone, DIR, 1)], &o);
    assert!(has(&s, &gone));
    assert!(has(&s, &format!("{gone}/x.txt")));
    // MustScanSubDirs on the root means the whole volume.
    let f = handle_events(&mut s, &[ev(&gone, DIR | MUST_SCAN_SUB_DIRS, 2)], &o);
    assert_eq!(f, vec![Followup::RescanVolume(gone)]);
    drop(dir);
}

#[test]
fn handle_events_excluded_paths_change_nothing() {
    // A default exclusion by name (".Trashes" on macOS, "lost+found" on Linux).
    let name = if cfg!(target_os = "macos") {
        ".Trashes"
    } else {
        "lost+found"
    };
    let (dir, root, mut s) = indexed(|d| {
        fs::create_dir_all(d.join("skip/deep")).unwrap();
        fs::create_dir_all(d.join("x").join(name).join("in")).unwrap();
        fs::create_dir_all(d.join("mnt")).unwrap();
    });
    let mut o = opts(&root);
    o.exclude.push(format!("{root}/skip"));
    o.skip_mounts.push(format!("{root}/mnt"));
    let before = s.index.len();
    for p in [
        format!("{root}/skip/deep/a.txt"),
        format!("{root}/x/{name}/in/b.txt"),
        format!("{root}/mnt/c.txt"),
    ] {
        fs::write(&p, b"").unwrap();
        let f = handle_events(&mut s, &[ev(&p, FILE, 1)], &o);
        assert!(!has(&s, &p), "{p}");
        assert_eq!(f, vec![Followup::SetEventId(1)]);
    }
    assert_eq!(s.index.len(), before);
    drop(dir);
}

#[test]
fn handle_events_absolute_excludes_above_the_volume_do_not_apply() {
    // The volumes folder ("/Volumes", "/media") is a default exclusion of the startup disk,
    // not of volumes mounted below it; exclusions by name hold everywhere.
    let (vols, name, tmp, home) = if cfg!(target_os = "macos") {
        (
            "/Volumes",
            ".Spotlight-V100",
            "/private/tmp/x",
            "/Users/x/a.txt",
        )
    } else {
        ("/media", "lost+found", "/proc/1/x", "/home/x/a.txt")
    };
    let o = EventOpts {
        vol_root: format!("{vols}/Disk"),
        exclude: default_excludes(),
        skip_mounts: Vec::new(),
        archives: false,
    };
    assert!(!o.excluded(&format!("{vols}/Disk/a/b.txt")));
    assert!(o.excluded(&format!("{vols}/Disk/a/{name}/x")));
    let o = EventOpts {
        vol_root: "/".into(),
        ..o
    };
    assert!(o.excluded(&format!("{vols}/Disk/a/b.txt")));
    assert!(o.excluded(tmp));
    assert!(!o.excluded(home));
}

#[test]
fn handle_events_paths_outside_the_volume_are_ignored() {
    let (dir, root, mut s) = indexed(|_| {});
    let f = handle_events(&mut s, &[ev("/elsewhere/a.txt", FILE, 4)], &opts(&root));
    assert_eq!(f, vec![Followup::SetEventId(4)]);
    drop(dir);
}

#[test]
fn handle_events_history_done_only_moves_the_event_id() {
    let (dir, root, mut s) = indexed(|_| {});
    let n = s.index.len();
    let f = handle_events(&mut s, &[ev(&root, HISTORY_DONE, 99)], &opts(&root));
    assert_eq!(f, vec![Followup::SetEventId(99)]);
    assert_eq!(s.index.len(), n);
    drop(dir);
}

#[test]
fn handle_events_changed_archive_is_reread() {
    let (dir, root, mut s) = indexed(|d| fs::write(d.join("pack.zip"), b"old").unwrap());
    let zip = format!("{root}/pack.zip");
    let md = fs::symlink_metadata(&zip).unwrap();
    s.stamps.insert(zip.clone(), stamp_of(&md));
    let mut o = opts(&root);
    o.archives = true;
    // Same stamp: nothing to re-read.
    let f = handle_events(&mut s, &[ev(&zip, FILE, 1)], &o);
    assert_eq!(f, vec![Followup::SetEventId(1)]);
    fs::write(&zip, b"new and longer").unwrap();
    let f = handle_events(&mut s, &[ev(&zip, FILE, 2)], &o);
    assert_eq!(
        f,
        vec![
            Followup::RescanSubtree(zip.clone()),
            Followup::SetEventId(2)
        ]
    );
    // Archives off: a plain file update.
    let f = handle_events(&mut s, &[ev(&zip, FILE, 3)], &opts(&root));
    assert_eq!(f, vec![Followup::SetEventId(3)]);
    // A removed archive drops its stamp.
    fs::remove_file(&zip).unwrap();
    handle_events(&mut s, &[ev(&zip, FILE, 4)], &o);
    assert!(!s.stamps.contains_key(&zip));
    drop(dir);
}

#[test]
fn data_volume_paths_map_to_firmlinked_paths() {
    assert_eq!(
        index_path("/System/Volumes/Data/Users/x/a.txt", "/"),
        "/Users/x/a.txt"
    );
    assert_eq!(index_path("/System/Volumes/Data", "/"), "/");
    assert_eq!(index_path("/Users/x/dir/", "/"), "/Users/x/dir");
    // Only for the startup disk.
    assert_eq!(
        index_path("/System/Volumes/Data/Users/x", "/Volumes/D"),
        "/System/Volumes/Data/Users/x"
    );
}

#[test]
fn replay_needs_a_stored_id_and_the_same_uuid() {
    let mut v = VolumeState {
        root: "/".into(),
        uuid: Some("U1".into()),
        event_id: 500,
        refreshed: 0,
        network: false,
        source: None,
        denied: Vec::new(),
        blocked: Vec::new(),
        complete: true,
    };
    assert_eq!(replay_from(&v, Some("U1")), Some(500));
    assert_eq!(replay_from(&v, Some("U2")), None);
    assert_eq!(replay_from(&v, None), None);
    v.event_id = 0;
    assert_eq!(replay_from(&v, Some("U1")), None);
    v.event_id = 500;
    v.complete = false;
    assert_eq!(replay_from(&v, Some("U1")), None);
}

// ---- service ----

#[test]
fn live_events_update_the_index_and_its_event_id() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    // The scan stored the event id taken when it started; a later batch moves it on.
    let since = t.svc.0.read().as_ref().unwrap().volumes[0].event_id;
    let p = t.p("a/live.txt");
    fs::write(&p, b"").unwrap();
    t.svc.0.on_events(&t.root(), vec![ev(&p, FILE, since + 42)]);
    assert_eq!(run(&t, "live.txt", None, false).0.len(), 1);
    let g = t.svc.0.read();
    let v = &g.as_ref().unwrap().volumes[0];
    assert_eq!(v.event_id, since + 42);
    drop(g);
    // A new folder is scanned right away.
    fs::create_dir_all(t.root.path().join("nd/deep")).unwrap();
    fs::write(t.root.path().join("nd/deep/inside.txt"), b"").unwrap();
    t.svc
        .0
        .on_events(&t.root(), vec![ev(&t.p("nd"), DIR, since + 43)]);
    assert_eq!(run(&t, "inside.txt", None, false).0.len(), 1);
    assert!(lock(&t.svc.0.run).dirty);
}

#[test]
fn events_during_a_full_scan_are_replayed_after_the_swap() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    // A change made after the scan read the folder, but before the swap.
    let inner = t.svc.0.clone();
    let p = t.p("a/late.txt");
    let root = t.root();
    let late = t.svc.0.read().as_ref().unwrap().volumes[0].event_id + 1_000_000;
    *lock(&t.svc.0.scan_hook) = Some(Box::new(move || {
        fs::write(&p, b"").unwrap();
        inner.on_events(&root, vec![ev(&p, FILE, late)]);
    }));
    t.svc.rebuild(Some(t.root()));
    wait_for("hook ran", || lock(&t.svc.0.scan_hook).is_none());
    wait_ready(&t);
    wait_for("late file", || {
        run(&t, "late.txt", None, false).0.len() == 1
    });
    let g = t.svc.0.read();
    assert_eq!(g.as_ref().unwrap().volumes[0].event_id, late);
}

#[test]
fn graft_bursts_queue_one_compaction() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    // Without a worker the job stays queued, so the flag stays set.
    lock(&t.svc.0.jobs).take();
    let before = t.svc.0.compact_sends.load(Ordering::Relaxed);
    for _ in 0..5 {
        t.svc.0.queue_compact();
    }
    assert_eq!(t.svc.0.compact_sends.load(Ordering::Relaxed), before + 1);
    // Running the job clears the flag; the next burst queues again.
    t.svc.0.run_job(Job::Compact);
    t.svc.0.queue_compact();
    t.svc.0.queue_compact();
    assert_eq!(t.svc.0.compact_sends.load(Ordering::Relaxed), before + 2);
}

#[test]
fn rescan_path_keeps_volumes_below_an_absolute_exclusion() {
    // A volume mounted below an excluded folder (like /Volumes/X) is still rescanned.
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let mut s = lock(&t.svc.0.run).settings.clone();
    s.exclude.push(parent_of(&t.root()));
    lock(&t.svc.0.run).settings = s;
    fs::write(t.root.path().join("a/grafted.txt"), b"").unwrap();
    t.svc.0.rescan_path(&t.p("a"));
    assert_eq!(run(&t, "grafted", None, false).0.len(), 1);
}

#[test]
fn handle_events_drop_makes_the_volume_unreplayable_after_save_and_load() {
    let (dir, root, mut s) = indexed(|d| fs::write(d.join("a.txt"), b"").unwrap());
    s.volumes[0].uuid = Some("U1".into());
    s.volumes[0].event_id = 100;
    assert_eq!(replay_from(&s.volumes[0], Some("U1")), Some(100));
    handle_events(&mut s, &[ev(&root, KERNEL_DROPPED, 200)], &opts(&root));
    // A later ordinary batch must not make it replayable again before the rescan.
    fs::write(dir.path().join("b.txt"), b"").unwrap();
    handle_events(
        &mut s,
        &[ev(&format!("{root}/b.txt"), FILE, 300)],
        &opts(&root),
    );
    // Saved (timer or quit) before the queued rescan ran, then loaded at the next start.
    let data = tempfile::tempdir().unwrap();
    save(data.path(), &s).unwrap();
    let loaded = load(data.path()).unwrap();
    let v = loaded.volumes.iter().find(|v| v.root == root).unwrap();
    assert_eq!(replay_from(v, Some("U1")), None);
    drop(dir);
}

#[test]
fn handle_events_subtree_rescans_hold_back_the_event_id() {
    let (dir, root, mut s) = indexed(|_| {});
    s.volumes[0].event_id = 100;
    fs::create_dir(dir.path().join("nd")).unwrap();
    let f = handle_events(&mut s, &[ev(&format!("{root}/nd"), DIR, 150)], &opts(&root));
    assert_eq!(
        f,
        vec![
            Followup::RescanSubtree(format!("{root}/nd")),
            Followup::SetEventId(150)
        ]
    );
    assert_eq!(s.volumes[0].event_id, 100);
}

#[test]
fn live_event_id_is_stored_after_the_subtree_rescan() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let since = t.svc.0.read().as_ref().unwrap().volumes[0].event_id;
    fs::create_dir_all(t.root.path().join("sub2/x")).unwrap();
    fs::write(t.root.path().join("sub2/x/in2.txt"), b"").unwrap();
    t.svc
        .0
        .on_events(&t.root(), vec![ev(&t.p("sub2"), DIR, since + 9)]);
    assert_eq!(run(&t, "in2.txt", None, false).0.len(), 1);
    let id = t.svc.0.read().as_ref().unwrap().volumes[0].event_id;
    assert_eq!(id, since + 9);
}
