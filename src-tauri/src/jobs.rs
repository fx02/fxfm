//! Background file operations (copy/move/delete/upload/pack), one thread per job.
//!
//! Events: `job://progress {job, progress}`, `job://conflict {job, info}`,
//! `job://error {job, path, msg}`, `job://done {job, summary | error}`, and
//! `connections://changed {lost}` after `job://done` when a connection died.

use fxfm_core::archive::pack::{pack_zip, PackOptions, Secret, ZipEncryption};
use fxfm_core::ops::{
    ConflictAnswer, ConflictInfo, ErrorAnswer, OpError, OpObserver, OpSummary, Progress,
};
use fxfm_core::vfs::{self, Ctx, Loc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::any::Any;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum JobRequest {
    Copy {
        sources: Vec<String>,
        dest: String,
        cwd: String,
    },
    Move {
        sources: Vec<String>,
        dest: String,
        cwd: String,
    },
    Delete {
        paths: Vec<String>,
        to_trash: bool,
    },
    /// Writes a local file back into an archive or to a server (F4 on such a
    /// file); conflicts are always overwritten.
    Upload {
        local: String,
        target: String,
    },
    /// Packs local `sources` (named relative to `cwd`) into the zip `target`. The password
    /// travels only in this request and is wiped when the request is dropped.
    Pack {
        sources: Vec<String>,
        cwd: String,
        target: String,
        #[serde(default)]
        password: Option<Secret>,
        #[serde(default)]
        encryption: ZipEncryption,
        #[serde(default)]
        overwrite: bool,
    },
}

/// Refusal for packing anything that is not a local file or folder.
pub const PACK_LOCAL_ONLY: &str = "Packing works with local files for now.";

/// The local path of `loc` when it is a local item (an archive file itself counts, its
/// inside does not).
fn local_item(loc: &str) -> Option<PathBuf> {
    match vfs::parse(loc) {
        Loc::Local(p) => Some(p),
        Loc::Archive(a) if a.inner.is_empty() => Some(a.archive),
        _ => None,
    }
}

/// Checks that everything is local and packs. A relative `target` is taken in `cwd`.
fn pack(
    sources: &[String],
    cwd: &str,
    target: &str,
    opts: &PackOptions,
    obs: &dyn OpObserver,
) -> Result<OpSummary, OpError> {
    let local_only = || OpError::Io(PACK_LOCAL_ONLY.into());
    let base = local_item(cwd).ok_or_else(local_only)?;
    if target.contains("://") {
        return Err(local_only());
    }
    let target = base.join(target);
    // The folder must be a real folder, not the inside of an archive.
    let dir = target.parent().ok_or_else(local_only)?;
    if !matches!(vfs::parse(&dir.to_string_lossy()), Loc::Local(_)) {
        return Err(local_only());
    }
    let paths = sources
        .iter()
        .map(|s| local_item(s))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(local_only)?;
    pack_zip(&paths, &base, &target, opts, obs)
}

/// A reply from the UI to a waiting job.
enum Reply {
    Conflict(ConflictAnswer),
    Error(ErrorAnswer),
    Cancel,
}

/// Blocks a job thread until the UI answers a conflict/error question, or the
/// job is cancelled. Tauri-free: the question is announced by the `emit` closure.
pub struct ConflictBridge {
    tx: Sender<Reply>,
    rx: Mutex<Receiver<Reply>>,
    cancelled: AtomicBool,
}

impl ConflictBridge {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        ConflictBridge {
            tx,
            rx: Mutex::new(rx),
            cancelled: AtomicBool::new(false),
        }
    }

    fn ask<T>(&self, emit: impl FnOnce(), pick: impl Fn(Reply) -> Option<T>, cancel: T) -> T {
        let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        // Answers that arrived while nobody was asking are stale.
        while rx.try_recv().is_ok() {}
        // Checked after draining: a cancel from now on leaves `Cancel` in the channel.
        if self.is_cancelled() {
            return cancel;
        }
        emit();
        loop {
            match rx.recv() {
                Ok(Reply::Cancel) | Err(_) => return cancel,
                Ok(r) => {
                    if let Some(a) = pick(r) {
                        return a;
                    }
                }
            }
        }
    }

    pub fn ask_conflict(&self, emit: impl FnOnce()) -> ConflictAnswer {
        let pick = |r| match r {
            Reply::Conflict(a) => Some(a),
            _ => None,
        };
        self.ask(emit, pick, ConflictAnswer::Cancel)
    }

    pub fn ask_error(&self, emit: impl FnOnce()) -> ErrorAnswer {
        let pick = |r| match r {
            Reply::Error(a) => Some(a),
            _ => None,
        };
        self.ask(emit, pick, ErrorAnswer::Cancel)
    }

    pub fn answer_conflict(&self, a: ConflictAnswer) {
        let _ = self.tx.send(Reply::Conflict(a));
    }

    pub fn answer_error(&self, a: ErrorAnswer) {
        let _ = self.tx.send(Reply::Error(a));
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Reply::Cancel);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Lets through at most one progress per interval; the latest suppressed one
/// is kept for `flush`.
struct Throttle {
    last: Option<Instant>,
    pending: Option<Progress>,
}

impl Throttle {
    fn new() -> Self {
        Throttle {
            last: None,
            pending: None,
        }
    }

    /// Returns the progress to emit now, if any.
    fn offer(&mut self, p: &Progress, now: Instant) -> Option<Progress> {
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < PROGRESS_INTERVAL)
        {
            self.pending = Some(p.clone());
            return None;
        }
        self.last = Some(now);
        self.pending = None;
        Some(p.clone())
    }

    /// The final state, if it was not emitted yet.
    fn flush(&mut self) -> Option<Progress> {
        self.pending.take()
    }
}

type EmitFn = Box<dyn Fn(&str, Value) + Send + Sync>;

/// `OpObserver` of one job: forwards to the UI through `emit`.
pub struct TauriObserver {
    job: u64,
    bridge: Arc<ConflictBridge>,
    throttle: Mutex<Throttle>,
    emit: EmitFn,
}

impl TauriObserver {
    fn flush(&self) {
        let p = self
            .throttle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush();
        if let Some(p) = p {
            (self.emit)("job://progress", json!({"job": self.job, "progress": p}));
        }
    }
}

impl OpObserver for TauriObserver {
    fn progress(&self, p: &Progress) {
        let now = Instant::now();
        let p = self
            .throttle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .offer(p, now);
        if let Some(p) = p {
            (self.emit)("job://progress", json!({"job": self.job, "progress": p}));
        }
    }

    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.flush();
        self.bridge.ask_conflict(|| {
            (self.emit)("job://conflict", json!({"job": self.job, "info": c}));
        })
    }

    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.flush();
        self.bridge.ask_error(|| {
            (self.emit)(
                "job://error",
                json!({"job": self.job, "path": path, "msg": msg}),
            );
        })
    }

    fn is_cancelled(&self) -> bool {
        self.bridge.is_cancelled()
    }
}

/// Answers every conflict with Overwrite; the rest goes to the real observer.
struct OverwriteObserver<'a>(&'a dyn OpObserver);

impl OpObserver for OverwriteObserver<'_> {
    fn progress(&self, p: &Progress) {
        self.0.progress(p)
    }
    fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
        ConflictAnswer::Overwrite
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.0.error(path, msg)
    }
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

pub fn run_job(ctx: &Ctx, req: &JobRequest, obs: &dyn OpObserver) -> Result<OpSummary, OpError> {
    match req {
        JobRequest::Copy { sources, dest, cwd } => vfs::copy(ctx, sources, dest, cwd, obs),
        JobRequest::Move { sources, dest, cwd } => vfs::move_(ctx, sources, dest, cwd, obs),
        JobRequest::Delete { paths, to_trash } => vfs::delete(ctx, paths, *to_trash, obs),
        JobRequest::Upload { local, target } => {
            // Never create a folder named like an archive that is gone.
            vfs::check_upload_target(ctx, target).map_err(OpError::Io)?;
            let cwd = Path::new(local)
                .parent()
                .map_or_else(|| "/".to_string(), |p| p.to_string_lossy().into_owned());
            vfs::copy(
                ctx,
                std::slice::from_ref(local),
                target,
                &cwd,
                &OverwriteObserver(obs),
            )
        }
        JobRequest::Pack {
            sources,
            cwd,
            target,
            password,
            encryption,
            overwrite,
        } => {
            let opts = PackOptions {
                password: password.as_ref().map(|p| p.0.clone()),
                encryption: *encryption,
                overwrite: *overwrite,
            };
            pack(sources, cwd, target, &opts, obs)
        }
    }
}

fn panic_message(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Runs an operation; its error, or a panic as
/// `{"kind": "internal", "message": "internal error: <panic msg>"}`, becomes JSON.
fn guarded(f: impl FnOnce() -> Result<OpSummary, OpError>) -> Result<OpSummary, Value> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(summary)) => Ok(summary),
        Ok(Err(e)) => Err(json!(e)),
        Err(p) => Err(json!({
            "kind": "internal",
            "message": format!("internal error: {}", panic_message(&*p)),
        })),
    }
}

type Running = Arc<Mutex<HashMap<u64, Arc<ConflictBridge>>>>;

/// Removes a job from the registry when its thread ends, even by panic.
struct Unregister {
    running: Running,
    job: u64,
}

impl Drop for Unregister {
    fn drop(&mut self) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.job);
    }
}

/// Running jobs by id.
#[derive(Default)]
pub struct Jobs {
    next: AtomicU64,
    running: Running,
}

impl Jobs {
    /// Registers a job and runs `body(job, bridge)` in its own thread; the job
    /// stays registered until the thread ends.
    fn spawn(&self, body: impl FnOnce(u64, Arc<ConflictBridge>) + Send + 'static) -> u64 {
        let job = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let bridge = Arc::new(ConflictBridge::new());
        self.lock().insert(job, bridge.clone());
        let unregister = Unregister {
            running: self.running.clone(),
            job,
        };
        std::thread::spawn(move || {
            let _unregister = unregister;
            body(job, bridge);
        });
        job
    }

    pub fn start(&self, app: AppHandle, ctx: Arc<Ctx>, req: JobRequest) -> u64 {
        self.spawn(move |job, bridge| {
            let before = ctx.sessions.names();
            let lost_app = app.clone();
            let obs = TauriObserver {
                job,
                bridge,
                throttle: Mutex::new(Throttle::new()),
                emit: Box::new(move |name, payload| {
                    if let Err(e) = app.emit(name, payload) {
                        eprintln!("fxfm: emit {name}: {e}");
                    }
                }),
            };
            let result = guarded(|| run_job(&ctx, &req, &obs));
            obs.flush();
            let payload = match result {
                Ok(summary) => json!({"job": job, "summary": summary}),
                Err(error) => json!({"job": job, "error": error}),
            };
            (obs.emit)("job://done", payload);
            // A connection that died during the job was dropped by the core.
            let lost = crate::remote::lost_sessions(&before, &ctx);
            if !lost.is_empty() {
                crate::remote::emit_changed(&lost_app, lost);
            }
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Arc<ConflictBridge>>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The bridge of a running job.
    pub fn get(&self, job: u64) -> Option<Arc<ConflictBridge>> {
        self.lock().get(&job).cloned()
    }

    /// Cancels every running job and waits up to `timeout` for their threads
    /// to finish (cancelled operations clean up their partial output).
    /// Returns true if all finished in time.
    pub fn cancel_all(&self, timeout: Duration) -> bool {
        for b in self.lock().values() {
            b.cancel();
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.lock().is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxfm_core::ops::{ConflictAnswer, ErrorAnswer};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn job_bridge_delivers_answer() {
        let bridge = Arc::new(ConflictBridge::new());
        let b = bridge.clone();
        let t = std::thread::spawn(move || b.ask_conflict(|| {}));
        std::thread::sleep(Duration::from_millis(50));
        bridge.answer_conflict(ConflictAnswer::Skip);
        assert_eq!(t.join().unwrap(), ConflictAnswer::Skip);

        let b = bridge.clone();
        let t = std::thread::spawn(move || b.ask_error(|| {}));
        std::thread::sleep(Duration::from_millis(50));
        bridge.answer_error(ErrorAnswer::Retry);
        assert_eq!(t.join().unwrap(), ErrorAnswer::Retry);

        // cancel unblocks a waiting ask with Cancel
        let b = bridge.clone();
        let t = std::thread::spawn(move || b.ask_conflict(|| {}));
        std::thread::sleep(Duration::from_millis(50));
        bridge.cancel();
        assert_eq!(t.join().unwrap(), ConflictAnswer::Cancel);
        assert!(bridge.is_cancelled());
        // once cancelled, asks return Cancel without waiting
        assert_eq!(bridge.ask_error(|| {}), ErrorAnswer::Cancel);
    }

    #[test]
    fn throttle_limits_rate_and_keeps_final_state() {
        let p = |n| Progress {
            done_files: n,
            ..Progress::default()
        };
        let t0 = Instant::now();
        let mut th = Throttle::new();
        assert_eq!(th.offer(&p(1), t0), Some(p(1)));
        assert_eq!(th.offer(&p(2), t0 + Duration::from_millis(50)), None);
        assert_eq!(th.offer(&p(3), t0 + Duration::from_millis(90)), None);
        assert_eq!(th.flush(), Some(p(3)));
        assert_eq!(th.flush(), None);
        assert_eq!(th.offer(&p(4), t0 + Duration::from_millis(100)), Some(p(4)));
        assert_eq!(th.flush(), None);
    }

    #[test]
    fn job_request_shape() {
        let r: JobRequest =
            serde_json::from_str(r#"{"kind":"delete","paths":["/a"],"toTrash":true}"#).unwrap();
        assert_eq!(
            r,
            JobRequest::Delete {
                paths: vec!["/a".into()],
                to_trash: true
            }
        );
    }

    #[test]
    fn cancel_all_cancels_and_waits_for_jobs() {
        let jobs = Jobs::default();
        let a = jobs.spawn(|_, b| {
            while !b.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let c = jobs.spawn(|_, b| {
            assert_eq!(b.ask_conflict(|| {}), ConflictAnswer::Cancel);
        });
        std::thread::sleep(Duration::from_millis(30));
        assert!(jobs.get(a).is_some() && jobs.get(c).is_some());
        assert!(jobs.cancel_all(Duration::from_secs(3)));
        assert!(jobs.get(a).is_none() && jobs.get(c).is_none());
    }

    #[test]
    fn cancel_all_gives_up_after_timeout() {
        let jobs = Jobs::default();
        jobs.spawn(|_, _| std::thread::sleep(Duration::from_millis(500)));
        let t0 = Instant::now();
        assert!(!jobs.cancel_all(Duration::from_millis(50)));
        assert!(t0.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn panic_in_job_unregisters_and_reports_internal_error() {
        let r = guarded(|| panic!("boom"));
        assert_eq!(
            r,
            Err(json!({"kind": "internal", "message": "internal error: boom"}))
        );
        let e = guarded(|| Err(OpError::Cancelled)).unwrap_err();
        assert_eq!(e["kind"], "cancelled");

        let jobs = Jobs::default();
        let id = jobs.spawn(|_, _| panic!("job panic"));
        let t0 = Instant::now();
        while jobs.get(id).is_some() && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(jobs.get(id).is_none());
    }

    #[test]
    fn progress_is_flushed_before_conflict_and_error() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let l = log.clone();
        let bridge = Arc::new(ConflictBridge::new());
        let b = bridge.clone();
        let obs = TauriObserver {
            job: 1,
            bridge,
            throttle: Mutex::new(Throttle::new()),
            // The "UI" answers each question as soon as it is asked.
            emit: Box::new(move |name, _| {
                l.lock().unwrap().push(name.to_string());
                match name {
                    "job://conflict" => b.answer_conflict(ConflictAnswer::Skip),
                    "job://error" => b.answer_error(ErrorAnswer::Skip),
                    _ => {}
                }
            }),
        };
        let p = Progress::default();
        obs.progress(&p);
        obs.progress(&p); // throttled
        let info = ConflictInfo {
            source: "/a".into(),
            target: "/b".into(),
            source_size: 0,
            source_mtime: 0,
            target_size: 0,
            target_mtime: 0,
        };
        assert_eq!(obs.conflict(&info), ConflictAnswer::Skip);
        obs.progress(&p); // throttled
        assert_eq!(obs.error("/a", "x"), ErrorAnswer::Skip);
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "job://progress",
                "job://progress",
                "job://conflict",
                "job://progress",
                "job://error"
            ]
        );
    }

    #[test]
    fn job_request_upload_serde() {
        let r: JobRequest =
            serde_json::from_str(r#"{"kind":"upload","local":"/t/x","target":"/t/a.zip/x"}"#)
                .unwrap();
        assert_eq!(
            r,
            JobRequest::Upload {
                local: "/t/x".into(),
                target: "/t/a.zip/x".into()
            }
        );
    }

    #[test]
    fn job_request_pack_serde_hides_the_password() {
        let r: JobRequest = serde_json::from_str(
            r#"{"kind":"pack","sources":["/t/a"],"cwd":"/t","target":"/t/a.zip","password":"hunter2","encryption":"zipCrypto","overwrite":true}"#,
        )
        .unwrap();
        let JobRequest::Pack {
            password,
            encryption,
            overwrite,
            ..
        } = &r
        else {
            panic!("{r:?}");
        };
        assert_eq!(password.as_ref().unwrap().0.as_str(), "hunter2");
        assert_eq!((*encryption, *overwrite), (ZipEncryption::ZipCrypto, true));
        assert!(!format!("{r:?}").contains("hunter2"));
        // Without a password, encryption and overwrite: no encryption, never overwrite.
        let r: JobRequest = serde_json::from_str(
            r#"{"kind":"pack","sources":["/t/a"],"cwd":"/t","target":"/t/a.zip"}"#,
        )
        .unwrap();
        assert!(matches!(
            r,
            JobRequest::Pack {
                password: None,
                encryption: ZipEncryption::Aes256,
                overwrite: false,
                ..
            }
        ));
    }

    fn pack_req(sources: Vec<String>, cwd: String, target: String) -> JobRequest {
        JobRequest::Pack {
            sources,
            cwd,
            target,
            password: None,
            encryption: ZipEncryption::Aes256,
            overwrite: false,
        }
    }

    #[test]
    fn pack_job_packs_local_items_including_an_archive_file() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let tar_path = t.path().join("a.tar");
        make_tar(&tar_path, &[("x.txt", b"hello")]);
        std::fs::write(t.path().join("b.txt"), b"bee").unwrap();
        // A relative target is taken in cwd.
        let req = pack_req(
            vec![s(&tar_path), s(&t.path().join("b.txt"))],
            s(t.path()),
            "out.zip".into(),
        );
        let r = run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap();
        assert_eq!(r.done, 2);
        // Read back through the zip reader behind fxfm's archive listing.
        let idx = fxfm_core::archive::read::read_index(
            &t.path().join("out.zip"),
            fxfm_core::archive::ArchiveKind::Zip,
        )
        .unwrap();
        let mut names: Vec<_> = idx.entries.iter().map(|e| e.path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.tar", "b.txt"]);
        // Packing again into the same name is refused until the UI says overwrite.
        let e = run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap_err();
        assert_eq!(e, OpError::Exists(s(&t.path().join("out.zip"))));
    }

    #[test]
    fn pack_job_refuses_archives_and_servers() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let tar_path = t.path().join("a.tar");
        make_tar(&tar_path, &[("x.txt", b"hello")]);
        let local = s(&t.path().join("b.txt"));
        std::fs::write(&local, b"bee").unwrap();
        let refused = OpError::Io(PACK_LOCAL_ONLY.into());
        for req in [
            // a source inside an archive
            pack_req(
                vec![format!("{}/x.txt", s(&tar_path))],
                s(&tar_path),
                s(&t.path().join("o.zip")),
            ),
            // a source on a server
            pack_req(
                vec!["sftp://web/x".into()],
                "sftp://web/".into(),
                s(&t.path().join("o.zip")),
            ),
            // a target inside an archive
            pack_req(
                vec![local.clone()],
                s(t.path()),
                format!("{}/o.zip", s(&tar_path)),
            ),
            // a target on a server
            pack_req(vec![local.clone()], s(t.path()), "sftp://web/o.zip".into()),
        ] {
            let e = run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap_err();
            assert_eq!(e, refused, "{req:?}");
        }
        assert!(!t.path().join("o.zip").exists());
    }

    /// Answers conflicts with `answer`; counts them.
    struct Probe {
        answer: ConflictAnswer,
        conflicts: std::sync::atomic::AtomicU32,
    }

    impl OpObserver for Probe {
        fn progress(&self, _p: &Progress) {}
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            self.conflicts.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            ErrorAnswer::Skip
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    fn probe(answer: ConflictAnswer) -> Probe {
        Probe {
            answer,
            conflicts: Default::default(),
        }
    }

    fn make_tar(path: &Path, files: &[(&str, &[u8])]) {
        let mut b = tar::Builder::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.finish().unwrap();
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn run_job_routes_archive_copy() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let tar_path = t.path().join("a.tar");
        make_tar(&tar_path, &[("d/x.txt", b"hello")]);
        let out = t.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let req = JobRequest::Copy {
            sources: vec![format!("{}/d/x.txt", s(&tar_path))],
            dest: s(&out),
            cwd: s(t.path()),
        };
        let r = run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap();
        assert_eq!(r.done, 1);
        assert_eq!(std::fs::read(out.join("x.txt")).unwrap(), b"hello");
    }

    #[test]
    fn upload_overwrites_without_asking() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let tar_path = t.path().join("a.tar");
        make_tar(&tar_path, &[("x.txt", b"old")]);
        let local = t.path().join("x.txt");
        std::fs::write(&local, b"new").unwrap();
        let req = JobRequest::Upload {
            local: s(&local),
            target: format!("{}/x.txt", s(&tar_path)),
        };
        // The real observer would answer Cancel; it must never be asked.
        let obs = probe(ConflictAnswer::Cancel);
        run_job(&ctx, &req, &obs).unwrap();
        assert_eq!(obs.conflicts.load(Ordering::SeqCst), 0);
        let back = t.path().join("back");
        std::fs::create_dir(&back).unwrap();
        let req = JobRequest::Copy {
            sources: vec![format!("{}/x.txt", s(&tar_path))],
            dest: s(&back),
            cwd: s(t.path()),
        };
        run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap();
        assert_eq!(std::fs::read(back.join("x.txt")).unwrap(), b"new");
    }

    #[test]
    fn upload_into_missing_archive_creates_nothing() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let local = t.path().join("x.txt");
        std::fs::write(&local, b"new").unwrap();
        let gone = t.path().join("a.tar");
        let req = JobRequest::Upload {
            local: s(&local),
            target: format!("{}/x.txt", s(&gone)),
        };
        let e = run_job(&ctx, &req, &probe(ConflictAnswer::Cancel)).unwrap_err();
        assert_eq!(
            e,
            OpError::Io(format!(
                "The archive {} no longer exists; your edited copy is kept",
                s(&gone)
            ))
        );
        assert!(!gone.exists());
        assert_eq!(std::fs::read(&local).unwrap(), b"new");
    }
}
