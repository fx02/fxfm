//! Service tests: temp "startup root", temp data dir, fake emitter and fake mount list. Nothing
//! here touches `/`, `$HOME` or the real data folder.

use super::*;
use std::fs;
use std::path::Path;
use std::time::Instant;

pub(super) type Events = Arc<Mutex<Vec<(String, Value)>>>;
pub(super) type Mounts = Arc<Mutex<Vec<Mounted>>>;

pub(super) struct T {
    pub(super) svc: Service,
    pub(super) ev: Events,
    pub(super) mounts: Mounts,
    pub(super) root: tempfile::TempDir,
    pub(super) data: tempfile::TempDir,
}

impl T {
    pub(super) fn root(&self) -> String {
        self.root.path().to_str().unwrap().to_string()
    }
    pub(super) fn p(&self, rel: &str) -> String {
        format!("{}/{rel}", self.root())
    }
}

pub(super) fn tree(root: &Path) {
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b/sub")).unwrap();
    fs::write(root.join("a/hello.txt"), b"hi").unwrap();
    fs::write(root.join("b/sub/hello2.md"), b"there").unwrap();
    fs::write(root.join("b/other.bin"), b"x").unwrap();
}

pub(super) fn settings(index: Option<bool>) -> SearchSettings {
    SearchSettings {
        index,
        ..SearchSettings::default()
    }
}

/// A service on a fresh temp tree; `prep` can write into the data dir before it starts.
pub(super) fn service_with(s: &SearchSettings, prep: impl FnOnce(&Path)) -> T {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    tree(root.path());
    prep(data.path());
    let ev: Events = Arc::default();
    let mounts: Mounts = Arc::default();
    let e = ev.clone();
    let m = mounts.clone();
    let svc = Service::new(
        root.path().to_path_buf(),
        data.path().to_path_buf(),
        Arc::new(move |name: &str, v: Value| e.lock().unwrap().push((name.to_string(), v))),
        Arc::new(move || m.lock().unwrap().clone()),
    );
    svc.start(s);
    T {
        svc,
        ev,
        mounts,
        root,
        data,
    }
}

pub(super) fn service(s: &SearchSettings) -> T {
    service_with(s, |_| {})
}

pub(super) fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn wait_ready(t: &T) {
    wait_for("index ready", || {
        let st = t.svc.status();
        st.state == "ready" && st.volumes.iter().all(|v| v.state != "building")
    });
}

pub(super) fn batches(ev: &Events, id: u64) -> Vec<Value> {
    ev.lock()
        .unwrap()
        .iter()
        .filter(|(n, v)| n == "search://batch" && v["id"] == id)
        .map(|(_, v)| v.clone())
        .collect()
}

/// Runs a search to its `done` batch; returns (all items, final total, batch count).
pub(super) fn run(
    t: &T,
    query: &str,
    under: Option<String>,
    archives: bool,
) -> (Vec<Value>, u64, usize) {
    let id = t
        .svc
        .search_start(SearchReq {
            query: query.into(),
            under,
            archives,
            limit: 1000,
            ..Default::default()
        })
        .unwrap();
    wait_for("search done", || {
        batches(&t.ev, id).iter().any(|b| b["done"] == true)
    });
    let bs = batches(&t.ev, id);
    let items = bs
        .iter()
        .flat_map(|b| b["items"].as_array().unwrap().clone())
        .collect();
    let total = bs.last().unwrap()["total"].as_u64().unwrap();
    (items, total, bs.len())
}

pub(super) fn paths(items: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = items
        .iter()
        .map(|i| i["path"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

#[test]
fn build_scans_root_and_finds_hits() {
    let t = service(&settings(None));
    assert_eq!(t.svc.status().state, "notAsked");
    t.svc.build();
    wait_ready(&t);
    let st = t.svc.status();
    assert_eq!(st.volumes[0].kind, "startup");
    assert_eq!(st.volumes[0].path, t.root());
    assert!(st.volumes[0].mounted && st.volumes[0].enabled);
    assert!(st.volumes[0].entries >= 6, "{:?}", st.volumes[0]);
    assert!(st.volumes[0].refreshed > 0);
    assert!(!st.fda_needed);
    assert!(st.exclude_defaults.iter().any(|e| e == "/System"));
    assert!(t.data.path().join("index/index.bin").is_file());
    assert!(t
        .ev
        .lock()
        .unwrap()
        .iter()
        .any(|(n, v)| n == "search://status" && v["state"] == "ready" && v["fdaNeeded"] == false));
    let (items, total, n) = run(&t, "hello", None, false);
    assert_eq!(n, 1, "an index search sends one batch");
    assert_eq!(total, 2);
    assert_eq!(
        paths(&items),
        vec![t.p("a/hello.txt"), t.p("b/sub/hello2.md")]
    );
    let h = items.iter().find(|i| i["name"] == "hello.txt").unwrap();
    assert_eq!(h["folder"], t.p("a"));
    assert_eq!(h["size"], 2);
    assert_eq!(h["isDir"], false);
    assert_eq!(h["inArchive"], false);
    assert_eq!(h["mounted"], true);
    assert!(h["mtime"].as_i64().unwrap() > 0);
    // `under` inside the index searches the index (one batch) and only below that folder.
    let (items, _, n) = run(&t, "hello", Some(t.p("b")), false);
    assert_eq!(n, 1);
    assert_eq!(paths(&items), vec![t.p("b/sub/hello2.md")]);
}

#[test]
fn corrupt_index_file_is_rebuilt() {
    let t = service_with(&settings(Some(true)), |data| {
        fs::create_dir_all(data.join("index")).unwrap();
        fs::write(data.join("index/index.bin"), b"FXFMIDX\0garbage").unwrap();
    });
    wait_ready(&t);
    let (items, _, _) = run(&t, "hello2", None, false);
    assert_eq!(paths(&items), vec![t.p("b/sub/hello2.md")]);
}

#[test]
fn saved_index_loads_on_start() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    t.svc.shutdown();
    // A second service on the same data dir loads the file; the tree changed, the index did not.
    fs::write(t.root.path().join("a/new-file.txt"), b"n").unwrap();
    let ev: Events = Arc::default();
    let e = ev.clone();
    let svc = Service::new(
        t.root.path().to_path_buf(),
        t.data.path().to_path_buf(),
        Arc::new(move |name: &str, v: Value| e.lock().unwrap().push((name.to_string(), v))),
        Arc::new(Vec::new),
    );
    svc.start(&settings(Some(true)));
    let t2 = T {
        svc,
        ev,
        mounts: Arc::default(),
        root: tempfile::tempdir().unwrap(),
        data: tempfile::tempdir().unwrap(),
    };
    wait_ready(&t2);
    let (items, _, _) = run(&t2, "hello", None, false);
    assert_eq!(items.len(), 2);
    let (items, _, _) = run(&t2, "new-file", None, false);
    assert!(items.is_empty(), "loaded, not rescanned");
}

#[test]
fn newer_search_cancels_the_older_one() {
    let t = service(&settings(None));
    for d in 0..40 {
        let dir = t.root.path().join(format!("d{d}"));
        fs::create_dir(&dir).unwrap();
        for f in 0..100 {
            fs::write(dir.join(format!("file{f}.txt")), b"").unwrap();
        }
    }
    let req = |q: &str| SearchReq {
        query: q.into(),
        under: Some(t.root()),
        archives: false,
        limit: 1000,
        ..Default::default()
    };
    let old = t.svc.search_start(req("file")).unwrap();
    let new = t.svc.search_start(req("file")).unwrap();
    assert!(new > old);
    let mark = t.ev.lock().unwrap().len();
    wait_for("new search done", || {
        batches(&t.ev, new).iter().any(|b| b["done"] == true)
    });
    std::thread::sleep(Duration::from_millis(100));
    let late = t.ev.lock().unwrap()[mark..]
        .iter()
        .filter(|(_, v)| v["id"] == old)
        .count();
    assert_eq!(late, 0, "the cancelled search emitted after cancellation");
    // search_cancel: nothing more for that id once it returns.
    let id = t.svc.search_start(req("file")).unwrap();
    t.svc.search_cancel(id);
    let mark = t.ev.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(300));
    let late = t.ev.lock().unwrap()[mark..]
        .iter()
        .filter(|(_, v)| v["id"] == id)
        .count();
    assert_eq!(late, 0);
}

#[test]
fn containing_text_walks_the_folder_and_needs_one() {
    let t = service(&settings(None));
    let dir = t.root.path().join("notes");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.txt"), b"buy MILK today").unwrap();
    fs::write(dir.join("b.md"), b"milk").unwrap();
    fs::write(dir.join("c.txt"), b"nothing here").unwrap();
    fs::create_dir(dir.join("milk")).unwrap(); // a folder named like the text is no hit
    let req = |query: &str, text: &str, case_sensitive: bool| SearchReq {
        query: query.into(),
        under: Some(dir.to_string_lossy().into_owned()),
        limit: 100,
        text: text.into(),
        case_sensitive,
        ..Default::default()
    };
    let names = |id: u64| {
        wait_for("content search done", || {
            batches(&t.ev, id).iter().any(|b| b["done"] == true)
        });
        let mut n: Vec<String> = batches(&t.ev, id)
            .iter()
            .flat_map(|b| b["items"].as_array().cloned().unwrap_or_default())
            .map(|i| i["name"].as_str().unwrap().to_string())
            .collect();
        n.sort();
        n
    };
    let id = t.svc.search_start(req("", "milk", false)).unwrap();
    assert_eq!(names(id), ["a.txt", "b.md"]);
    let id = t.svc.search_start(req("*.txt", "milk", false)).unwrap();
    assert_eq!(names(id), ["a.txt"]);
    let id = t.svc.search_start(req("", "milk", true)).unwrap();
    assert_eq!(names(id), ["b.md"]);
    // Everywhere (no folder) is refused with a reason.
    let err = t
        .svc
        .search_start(SearchReq {
            under: None,
            ..req("", "milk", false)
        })
        .unwrap_err();
    assert!(err.contains("current folder"), "{err}");
}

#[test]
fn under_without_index_walks() {
    let t = service(&settings(None));
    let (items, total, _) = run(&t, "hello", Some(t.root()), false);
    assert_eq!(total, 2);
    assert_eq!(
        paths(&items),
        vec![t.p("a/hello.txt"), t.p("b/sub/hello2.md")]
    );
    assert!(items.iter().all(|i| i["mounted"] == true));
    let err = t
        .svc
        .search_start(SearchReq {
            query: "hello".into(),
            under: None,
            archives: false,
            limit: 1000,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(err, "no index");
}

#[test]
fn under_outside_the_index_walks() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("hello-out.txt"), b"").unwrap();
    let (items, _, _) = run(
        &t,
        "hello",
        Some(other.path().to_str().unwrap().into()),
        false,
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "hello-out.txt");
}

#[test]
fn forget_drops_an_entry() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    t.svc.forget(&t.p("a/hello.txt"));
    let (items, _, _) = run(&t, "hello", None, false);
    assert_eq!(paths(&items), vec![t.p("b/sub/hello2.md")]);
    // A volume root is never forgotten.
    t.svc.forget(&t.root());
    let (items, _, _) = run(&t, "hello2", None, false);
    assert_eq!(items.len(), 1);
}

#[test]
fn forget_during_a_copy_is_replayed_after_the_swap() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let cancel = lock(&t.svc.0.run).cancel.clone();
    let copy = t.svc.0.begin_merge(Stored::default(), None);
    t.svc.forget(&t.p("a/hello.txt"));
    assert!(t.svc.0.finish_swap(copy, &cancel));
    let (items, _, _) = run(&t, "hello", None, false);
    assert_eq!(paths(&items), vec![t.p("b/sub/hello2.md")]);
}

fn write_tar(path: &Path) {
    let f = fs::File::create(path).unwrap();
    let mut b = tar::Builder::new(f);
    let data = b"inner";
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.set_mtime(1_700_000_000);
    h.set_cksum();
    b.append_data(&mut h, "docs/inner-hello.txt", &data[..])
        .unwrap();
    b.finish().unwrap();
}

#[test]
fn archive_entries_name_and_folder() {
    let s = SearchSettings {
        index: None,
        archives: true,
        ..SearchSettings::default()
    };
    let t = service(&s);
    write_tar(&t.root.path().join("pack.tar"));
    let check = |items: &[Value]| {
        let i = items
            .iter()
            .find(|i| i["path"] == t.p("pack.tar/docs/inner-hello.txt"))
            .unwrap_or_else(|| panic!("{items:?}"));
        assert_eq!(i["name"], "pack.tar ▸ docs/inner-hello.txt");
        assert_eq!(i["folder"], t.p("pack.tar/docs"));
        assert_eq!(i["inArchive"], true);
        assert_eq!(i["size"], 5);
    };
    // Walk.
    let (items, _, _) = run(&t, "inner-hello", Some(t.root()), true);
    check(&items);
    // Without the archives flag the walk does not enter archives.
    let (items, _, _) = run(&t, "inner-hello", Some(t.root()), false);
    assert!(items.is_empty());
    // Index.
    t.svc.build();
    wait_ready(&t);
    let (items, _, _) = run(&t, "inner-hello", None, true);
    check(&items);
    let (items, _, _) = run(&t, "inner-hello", None, false);
    assert!(items.is_empty());
}

#[test]
fn other_volumes_scan_unmount_and_disable() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    fs::write(vol.path().join("on-volume.txt"), b"v").unwrap();
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "Mdisk".into(),
        network: false,
        source: None,
    }];
    t.svc.volumes_changed();
    let st = t.svc.status();
    let v = st.volumes.iter().find(|v| v.path == vp).unwrap();
    assert_eq!(
        (v.kind, v.enabled, v.mounted, v.state),
        ("local", false, true, "off")
    );
    let mut s = settings(Some(true));
    s.volumes = vec![SearchVolume {
        path: vp.clone(),
        enabled: true,
    }];
    t.svc.apply_settings(&s);
    wait_ready(&t);
    let (items, _, _) = run(&t, "on-volume", None, false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["mounted"], true);
    let v = t
        .svc
        .status()
        .volumes
        .into_iter()
        .find(|v| v.path == vp)
        .unwrap();
    assert_eq!((v.state, v.name.as_str()), ("ready", "Mdisk"));
    // Unmounted: entries stay, reported as not mounted.
    t.mounts.lock().unwrap().clear();
    t.svc.volumes_changed();
    let (items, _, _) = run(&t, "on-volume", None, false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["mounted"], false);
    let v = t
        .svc
        .status()
        .volumes
        .into_iter()
        .find(|v| v.path == vp)
        .unwrap();
    assert_eq!((v.mounted, v.state), (false, "stale"));
    // Disabled: entries go.
    s.volumes[0].enabled = false;
    t.svc.apply_settings(&s);
    wait_for("volume removed", || {
        let id = t
            .svc
            .search_start(SearchReq {
                query: "on-volume".into(),
                under: None,
                archives: false,
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        wait_for("done", || {
            batches(&t.ev, id).iter().any(|b| b["done"] == true)
        });
        batches(&t.ev, id)[0]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert!(!t.svc.status().volumes.iter().any(|v| v.path == vp));
}

#[test]
fn an_exclusion_on_another_volume_rescans_that_volume() {
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    fs::create_dir(vol.path().join("skip")).unwrap();
    fs::write(vol.path().join("skip/in-skipped-folder.txt"), b"v").unwrap();
    let mut s = settings(Some(true));
    s.volumes = vec![SearchVolume {
        path: vp.clone(),
        enabled: true,
    }];
    let t = service(&s);
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "Mdisk".into(),
        network: false,
        source: None,
    }];
    t.svc.volumes_changed();
    wait_ready(&t);
    wait_for("volume indexed", || {
        run(&t, "in-skipped-folder", None, false).0.len() == 1
    });
    s.exclude = vec![format!("{vp}/skip")];
    t.svc.apply_settings(&s);
    wait_for("excluded folder dropped", || {
        run(&t, "in-skipped-folder", None, false).0.is_empty()
    });
}

#[test]
fn turning_off_drops_the_index() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    t.svc.apply_settings(&settings(Some(false)));
    wait_for("index dropped", || t.svc.0.read().is_none());
    let st = t.svc.status();
    assert_eq!(st.state, "off");
    assert_eq!(st.entries, 0);
    assert!(!t.data.path().join("index/index.bin").exists());
    assert!(t
        .svc
        .search_start(SearchReq {
            query: "hello".into(),
            under: None,
            archives: false,
            limit: 10,
            ..Default::default()
        })
        .is_err());
}

#[test]
fn storage_move_saves_to_the_new_folder() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let new = tempfile::tempdir().unwrap();
    t.svc.set_data_dir(new.path());
    wait_for("moved", || new.path().join("index/index.bin").is_file());
    wait_for("old removed", || {
        !t.data.path().join("index/index.bin").exists()
    });
}

#[test]
fn rebuild_retries_a_subtree() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    fs::write(t.root.path().join("b/sub/hello3.txt"), b"").unwrap();
    t.svc.rebuild(Some(t.p("b")));
    wait_for("subtree rescanned", || {
        let (items, _, _) = run(&t, "hello3", None, false);
        items.len() == 1
    });
    fs::write(t.root.path().join("a/hello4.txt"), b"").unwrap();
    t.svc.rebuild(None);
    wait_for("full rebuild", || {
        let (items, _, _) = run(&t, "hello4", None, false);
        items.len() == 1
    });
}

#[test]
fn refresh_timer_rule() {
    let h = 3600;
    assert!(due_for_refresh(0, 100 * h, 24));
    assert!(!due_for_refresh(100 * h, 100 * h + 23 * h, 24));
    assert!(due_for_refresh(100 * h, 100 * h + 24 * h, 24));
    assert!(!due_for_refresh(0, 100 * h, 0), "0 = manual only");
}

#[test]
fn compaction_threshold() {
    assert!(!needs_compaction(0, 100));
    assert!(!needs_compaction(25, 100));
    assert!(needs_compaction(40, 100));
}

#[test]
fn skip_mounts_leave_out_the_startup_aliases() {
    let m = |p: &str| Mounted {
        path: p.into(),
        name: p.into(),
        network: false,
        source: None,
    };
    let mounted = vec![
        m("/"),
        m("/System/Volumes/Data"),
        m("/Volumes/A"),
        m("/Volumes/B"),
    ];
    assert_eq!(
        skip_mounts("/", "/", &mounted),
        vec!["/Volumes/A".to_string(), "/Volumes/B".to_string()]
    );
    assert_eq!(
        skip_mounts("/Volumes/A", "/", &mounted),
        vec!["/Volumes/B".to_string()]
    );
}

#[test]
fn network_refresh_queues_due_volumes() {
    let t = service(&settings(None));
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    fs::write(vol.path().join("net-file.txt"), b"v").unwrap();
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "share".into(),
        network: true,
        source: Some("//u@host/share".into()),
    }];
    let mut s = settings(Some(true));
    s.volumes = vec![SearchVolume {
        path: vp.clone(),
        enabled: true,
    }];
    t.svc.apply_settings(&s);
    wait_ready(&t);
    let v = t
        .svc
        .status()
        .volumes
        .into_iter()
        .find(|v| v.path == vp)
        .unwrap();
    assert_eq!(v.kind, "network");
    let first = v.refreshed;
    fs::write(vol.path().join("net-later.txt"), b"v").unwrap();
    // Not due yet: nothing happens.
    t.svc.tick(first + 3600);
    wait_ready(&t);
    let (items, _, _) = run(&t, "net-later", None, false);
    assert!(items.is_empty());
    // A day later: rescanned.
    t.svc.tick(first + 24 * 3600);
    wait_for("refreshed", || {
        let (items, _, _) = run(&t, "net-later", None, false);
        items.len() == 1
    });
}

fn dead_and_live(t: &T) -> (usize, usize) {
    let g = t.svc.0.read();
    let s = g.as_ref().unwrap();
    (s.index.dead(), s.index.len())
}

#[test]
fn timer_compacts_and_rescans_leave_no_tombstones() {
    let t = service(&settings(None));
    for f in 0..30 {
        fs::write(t.root.path().join(format!("a/tmp{f}.log")), b"").unwrap();
    }
    t.svc.build();
    wait_ready(&t);
    for f in 0..30 {
        t.svc.forget(&t.p(&format!("a/tmp{f}.log")));
    }
    assert_eq!(dead_and_live(&t).0, 30);
    t.svc.tick(now());
    wait_for("compacted", || dead_and_live(&t).0 == 0);
    let (items, _, _) = run(&t, "hello", None, false);
    assert_eq!(items.len(), 2);
    // A full rescan of the volume swaps in a fresh index.
    t.svc.forget(&t.p("b/other.bin"));
    t.svc.rebuild(None);
    wait_for("rescanned", || {
        let (items, _, _) = run(&t, "other.bin", None, false);
        items.len() == 1
    });
    wait_ready(&t);
    assert_eq!(dead_and_live(&t).0, 0);
}

#[test]
fn turning_off_during_a_merge_discards_the_copy() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let cancel = lock(&t.svc.0.run).cancel.clone();
    let copy = t.svc.0.begin_merge(Stored::default(), None);
    t.svc.apply_settings(&settings(Some(false)));
    wait_for("index dropped", || t.svc.0.read().is_none());
    assert!(!t.svc.0.save_copy(&copy, &cancel));
    assert!(!t.svc.0.finish_swap(copy, &cancel));
    assert!(t.svc.0.read().is_none());
    assert!(!t.data.path().join("index/index.bin").exists());
    assert_eq!(t.svc.status().entries, 0);
}

#[test]
fn volume_gone_after_its_scan_keeps_the_old_entries() {
    let t = service(&settings(None));
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    fs::write(vol.path().join("kept.txt"), b"v").unwrap();
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "share".into(),
        network: true,
        source: None,
    }];
    let mut s = settings(Some(true));
    s.volumes = vec![SearchVolume {
        path: vp.clone(),
        enabled: true,
    }];
    t.svc.apply_settings(&s);
    wait_ready(&t);
    let before = t
        .svc
        .status()
        .volumes
        .into_iter()
        .find(|v| v.path == vp)
        .unwrap();
    // The share drops while it is being rescanned: the cached mount list still has it, the
    // mount table no longer does, and the scan sees an emptied folder.
    fs::remove_file(vol.path().join("kept.txt")).unwrap();
    t.mounts.lock().unwrap().clear();
    let statuses = || {
        t.ev.lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == "search://status")
            .count()
    };
    let mark = statuses();
    t.svc.refresh(&vp);
    wait_for("refresh job done", || statuses() >= mark + 2);
    let (items, _, _) = run(&t, "kept", None, false);
    assert_eq!(items.len(), 1, "old entries kept");
    let g = t.svc.0.read();
    let v = g
        .as_ref()
        .unwrap()
        .volumes
        .iter()
        .find(|v| v.root == vp)
        .unwrap();
    assert_eq!(v.refreshed, before.refreshed);
}

#[test]
fn subtree_rescan_grafts_without_a_clone() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    fs::remove_file(t.root.path().join("b/sub/hello2.md")).unwrap();
    fs::write(t.root.path().join("b/sub/fresh.txt"), b"").unwrap();
    t.svc.0.rescan_path(&t.p("b/sub"));
    let (items, _, _) = run(&t, "hello", None, false);
    assert_eq!(paths(&items), vec![t.p("a/hello.txt")]);
    let (items, _, _) = run(&t, "fresh", None, false);
    assert_eq!(items.len(), 1);
    // A path that is gone is dropped; a new file is added.
    fs::remove_dir_all(t.root.path().join("b/sub")).unwrap();
    t.svc.0.rescan_path(&t.p("b/sub"));
    assert!(run(&t, "fresh", None, false).0.is_empty());
    fs::write(t.root.path().join("b/single.txt"), b"").unwrap();
    t.svc.0.rescan_path(&t.p("b/single.txt"));
    assert_eq!(run(&t, "single", None, false).0.len(), 1);
    // A graft during a merge is replayed on the merged index.
    let cancel = lock(&t.svc.0.run).cancel.clone();
    let copy = t.svc.0.begin_merge(Stored::default(), None);
    fs::write(t.root.path().join("a/during.txt"), b"").unwrap();
    t.svc.0.rescan_path(&t.p("a"));
    assert!(t.svc.0.finish_swap(copy, &cancel));
    assert_eq!(run(&t, "during", None, false).0.len(), 1);
}

#[test]
fn search_start_does_not_wait_for_the_index_lock() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    let inner = t.svc.0.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _g = inner.write();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(600));
    });
    rx.recv().unwrap();
    let start = Instant::now();
    let id = t
        .svc
        .search_start(SearchReq {
            query: "hello".into(),
            under: None,
            archives: false,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert!(start.elapsed() < Duration::from_millis(300));
    holder.join().unwrap();
    wait_for("done", || {
        batches(&t.ev, id).iter().any(|b| b["done"] == true)
    });
    assert_eq!(batches(&t.ev, id)[0]["items"].as_array().unwrap().len(), 2);
}

#[test]
fn start_does_not_wait_for_the_mount_list() {
    // A hung network mount makes the mount list block; `start` (Tauri setup, main thread) must
    // return anyway and the worker reads the list as its first job.
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    tree(root.path());
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let blocked = Arc::new(AtomicBool::new(true));
    let b = blocked.clone();
    let ev: Events = Arc::default();
    let e = ev.clone();
    let svc = Service::new(
        root.path().to_path_buf(),
        data.path().to_path_buf(),
        Arc::new(move |name: &str, v: Value| e.lock().unwrap().push((name.to_string(), v))),
        Arc::new(move || {
            if b.load(Ordering::Relaxed) {
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
                b.store(false, Ordering::Relaxed);
            }
            Vec::new()
        }),
    );
    let start = Instant::now();
    svc.start(&settings(Some(true)));
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "start waited for the mount list"
    );
    release_tx.send(()).unwrap();
    let t = T {
        svc,
        ev,
        mounts: Arc::default(),
        root,
        data,
    };
    wait_ready(&t);
    assert_eq!(run(&t, "hello2", None, false).0.len(), 1);
}

#[test]
fn plain_permission_denied_folders_do_not_ask_for_full_disk_access() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let t = service(&settings(None));
    let locked = t.root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    t.svc.build();
    wait_ready(&t);
    let st = t.svc.status();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(st.unreadable, vec![t.p("locked")]);
    assert!(st.denied.is_empty());
    assert!(!st.fda_needed);
}

#[test]
fn only_blocked_privacy_folders_need_full_disk_access() {
    let vol = |root: &str, denied: &[&str], blocked: &[&str]| VolumeState {
        root: root.into(),
        uuid: None,
        event_id: 0,
        refreshed: 0,
        network: false,
        source: None,
        denied: denied.iter().map(|s| s.to_string()).collect(),
        blocked: blocked.iter().map(|s| s.to_string()).collect(),
        complete: true,
    };
    let vols = vec![
        vol(
            "/",
            &["/usr/sbin/authserver", "/private/etc/cups/certs"],
            &["/Library/Caches/com.apple.aned", "/Users/me/Desktop"],
        ),
        vol("/Volumes/USB", &[], &["/Volumes/USB/Photos"]),
    ];
    let (fda, unreadable) = status::split_denied(&vols, "/Users/me");
    assert_eq!(fda, vec!["/Users/me/Desktop", "/Volumes/USB/Photos"]);
    assert_eq!(
        unreadable,
        vec![
            "/usr/sbin/authserver",
            "/private/etc/cups/certs",
            "/Library/Caches/com.apple.aned"
        ]
    );
    // Only system-protected folders: no hint.
    let (fda, _) = status::split_denied(
        &vols[..1]
            .iter()
            .map(|v| VolumeState {
                blocked: vec!["/Library/Caches/com.apple.aned".into()],
                ..v.clone()
            })
            .collect::<Vec<_>>(),
        "/Users/me",
    );
    assert!(fda.is_empty());
}

/// Settings with the index and archive indexing on.
fn archive_settings() -> SearchSettings {
    SearchSettings {
        index: Some(true),
        archives: true,
        ..SearchSettings::default()
    }
}

/// Makes a file unreadable until dropped, so a re-read of it fails.
struct Unreadable(std::path::PathBuf);

impl Unreadable {
    fn new(p: std::path::PathBuf) -> Unreadable {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
        Unreadable(p)
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o644));
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

fn stamp(t: &T, path: &str) -> Option<ArchiveStamp> {
    t.svc
        .0
        .read()
        .as_ref()
        .and_then(|s| s.stamps.get(path).copied())
}

#[test]
fn full_rescan_reuses_unchanged_archives() {
    if is_root() {
        return;
    }
    let t = service_with(&settings(None), |_| {});
    write_tar(&t.root.path().join("b/pack.tar"));
    t.svc.apply_settings(&archive_settings());
    wait_ready(&t);
    let inner = t.p("b/pack.tar/docs/inner-hello.txt");
    assert_eq!(
        paths(&run(&t, "inner-hello", None, true).0),
        vec![inner.clone()]
    );
    let before = stamp(&t, &t.p("b/pack.tar")).expect("stamped");
    // Unchanged size and mtime: the next full scan must not read it again (it cannot).
    let _u = Unreadable::new(t.root.path().join("b/pack.tar"));
    fs::write(t.root.path().join("a/after.txt"), b"").unwrap();
    t.svc.rebuild(None);
    wait_ready(&t);
    assert_eq!(run(&t, "after", None, false).0.len(), 1, "rescanned");
    assert_eq!(paths(&run(&t, "inner-hello", None, true).0), vec![inner]);
    assert_eq!(stamp(&t, &t.p("b/pack.tar")), Some(before));
}

#[test]
fn subtree_rescan_reuses_unchanged_archives() {
    if is_root() {
        return;
    }
    let t = service_with(&settings(None), |_| {});
    write_tar(&t.root.path().join("b/pack.tar"));
    t.svc.apply_settings(&archive_settings());
    wait_ready(&t);
    let _u = Unreadable::new(t.root.path().join("b/pack.tar"));
    t.svc.0.rescan_path(&t.p("b"));
    assert_eq!(run(&t, "inner-hello", None, true).0.len(), 1);
}

fn in_archive_entries(t: &T) -> usize {
    let g = t.svc.0.read();
    let s = g.as_ref().unwrap();
    let root = s.index.lookup(&t.root()).unwrap();
    let mut n = 0;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        n += usize::from(s.index.meta(id).in_archive);
        stack.extend(s.index.children(id));
    }
    n
}

#[test]
fn turning_archives_off_drops_their_entries_without_a_rescan() {
    let t = service_with(&settings(None), |_| {});
    write_tar(&t.root.path().join("b/pack.tar"));
    t.svc.apply_settings(&archive_settings());
    wait_ready(&t);
    assert!(in_archive_entries(&t) > 0);
    let refreshed = t.svc.status().volumes[0].refreshed;
    // A file that a rescan would find, but dropping archive entries must not rescan.
    fs::write(t.root.path().join("a/unseen.txt"), b"").unwrap();
    t.svc.apply_settings(&settings(Some(true)));
    wait_for("archive entries dropped", || in_archive_entries(&t) == 0);
    wait_ready(&t);
    assert!(stamp(&t, &t.p("b/pack.tar")).is_none());
    assert_eq!(t.svc.status().volumes[0].refreshed, refreshed);
    assert!(run(&t, "unseen", None, false).0.is_empty());
    assert_eq!(
        run(&t, "pack", None, false).0.len(),
        1,
        "the archive file stays"
    );
}

/// A tar.gz of about 1.5 MB that does not compress.
fn write_big_tgz(path: &Path) {
    let gz =
        flate2::write::GzEncoder::new(fs::File::create(path).unwrap(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let data: Vec<u8> = (0..1_500_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let mut h = tar::Header::new_gnu();
    h.set_size(data.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "big-inner.bin", &data[..]).unwrap();
    b.into_inner().unwrap().finish().unwrap();
}

#[test]
fn changing_the_gz_limit_rereads_gz_archives() {
    let t = service_with(&settings(None), |_| {});
    write_big_tgz(&t.root.path().join("a/big.tgz"));
    assert!(fs::metadata(t.root.path().join("a/big.tgz")).unwrap().len() > 1 << 20);
    let mut s = archive_settings();
    s.gz_limit_mb = 1;
    t.svc.apply_settings(&s);
    wait_ready(&t);
    assert!(
        run(&t, "big-inner", None, true).0.is_empty(),
        "over the limit"
    );
    s.gz_limit_mb = 2;
    t.svc.apply_settings(&s);
    wait_for("re-read under the new limit", || {
        run(&t, "big-inner", None, true).0.len() == 1
    });
    s.gz_limit_mb = 1;
    t.svc.apply_settings(&s);
    wait_for("dropped under the lower limit", || {
        run(&t, "big-inner", None, true).0.is_empty()
    });
}

#[test]
fn changing_network_archives_rereads_network_archives() {
    let t = service(&settings(None));
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    write_tar(&vol.path().join("net.tar"));
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "share".into(),
        network: true,
        source: None,
    }];
    t.svc.volumes_changed();
    let mut s = archive_settings();
    s.volumes = vec![SearchVolume {
        path: vp.clone(),
        enabled: true,
    }];
    t.svc.apply_settings(&s);
    wait_ready(&t);
    assert!(
        run(&t, "inner-hello", None, true).0.is_empty(),
        "network archives off"
    );
    s.network_archives = true;
    t.svc.apply_settings(&s);
    wait_for("network archive read", || {
        run(&t, "inner-hello", None, true).0.len() == 1
    });
    s.network_archives = false;
    t.svc.apply_settings(&s);
    wait_for("network archive dropped", || {
        run(&t, "inner-hello", None, true).0.is_empty()
    });
}

#[test]
fn unsaved_changes_are_saved_hourly_not_every_ten_minutes() {
    let t = service(&settings(None));
    t.svc.build();
    wait_ready(&t);
    t.svc.forget(&t.p("a/hello.txt"));
    let last = lock(&t.svc.0.run).last_save;
    assert!(lock(&t.svc.0.run).dirty);
    t.svc.tick(last + 600);
    std::thread::sleep(Duration::from_millis(200));
    assert!(lock(&t.svc.0.run).dirty, "saved after ten minutes");
    t.svc.tick(last + 3600);
    wait_for("hourly save", || !lock(&t.svc.0.run).dirty);
}

#[test]
fn unwatched_local_volume_is_refreshed_daily_even_without_network_refresh() {
    let mut s = settings(Some(true));
    s.network_refresh_hours = 0;
    let t = service(&s);
    wait_ready(&t);
    let first = t.svc.status().volumes[0].refreshed;
    // Its event stream could not be created: no live updates.
    lock(&t.svc.0.live).unwatched.insert(t.root());
    fs::write(t.root.path().join("a/later.txt"), b"").unwrap();
    t.svc.tick(first + 3600);
    wait_ready(&t);
    assert!(run(&t, "later", None, false).0.is_empty());
    t.svc.tick(first + 24 * 3600);
    wait_for("daily refresh", || {
        run(&t, "later", None, false).0.len() == 1
    });
}

#[test]
fn refresh_ignores_volumes_that_are_not_enabled() {
    let t = service(&settings(Some(true)));
    wait_ready(&t);
    let vol = tempfile::tempdir().unwrap();
    let vp = vol.path().to_str().unwrap().to_string();
    fs::write(vol.path().join("not-enabled.txt"), b"").unwrap();
    *t.mounts.lock().unwrap() = vec![Mounted {
        path: vp.clone(),
        name: "share".into(),
        network: true,
        source: None,
    }];
    t.svc.volumes_changed();
    t.svc.refresh(&vp);
    assert!(!lock(&t.svc.0.run).pending.contains(&vp));
    t.svc.rebuild(Some(t.root()));
    wait_ready(&t);
    assert!(run(&t, "not-enabled", None, false).0.is_empty());
}

#[test]
fn storage_move_during_a_first_build_drops_the_old_file() {
    let t = service(&settings(None));
    let old = t.data.path().join("old-index");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join(INDEX_FILE), b"stale").unwrap();
    // The index is on but not built yet: nothing to save, the old file is useless.
    lock(&t.svc.0.run).settings.index = Some(true);
    t.svc.0.run_job(Job::Move(old.clone()));
    assert!(!old.join(INDEX_FILE).exists());
    assert!(!old.exists());
}

#[test]
fn pre_1970_mtimes_agree_between_scans_and_rescans() {
    let t = service(&settings(None));
    let f = t.root.path().join("a/hello.txt");
    let old = std::time::UNIX_EPOCH - Duration::from_secs(1000);
    fs::File::options()
        .write(true)
        .open(&f)
        .unwrap()
        .set_modified(old)
        .unwrap();
    t.svc.build();
    wait_ready(&t);
    let mtime = |t: &T| {
        let g = t.svc.0.read();
        let s = g.as_ref().unwrap();
        s.index
            .meta(s.index.lookup(&t.p("a/hello.txt")).unwrap())
            .mtime
    };
    assert_eq!(mtime(&t), -1000);
    t.svc.0.rescan_path(&t.p("a/hello.txt"));
    assert_eq!(mtime(&t), -1000);
}
