//! Minimal FSEvents bindings (macOS only): a file-level event stream delivered on its own
//! dispatch queue, the current event id and the event-store UUID of a device.
//!
//! The stream callback only converts the C arrays into a `Vec<Event>` and hands it to the
//! caller's closure, which must not block (the search service sends it into a channel).

use std::ffi::{c_char, c_void, CStr};
use std::ptr;

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

/// One file system event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub path: String,
    pub flags: u32,
    pub id: u64,
}

// Stream creation flags.
pub const CREATE_NO_DEFER: u32 = 0x02;
pub const CREATE_WATCH_ROOT: u32 = 0x04;
pub const CREATE_FILE_EVENTS: u32 = 0x10;

// Event flags are in `search::live`, which handles them on every OS.

/// `kFSEventStreamEventIdSinceNow`.
#[cfg(test)]
pub const SINCE_NOW: u64 = u64::MAX;
/// Seconds FSEvents waits to coalesce events into one callback.
pub const LATENCY: f64 = 1.0;

type StreamRef = *mut c_void;
type Queue = *mut c_void;
type Callback = Box<dyn Fn(Vec<Event>) + Send + 'static>;

#[repr(C)]
struct Context {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type StreamCallback = extern "C" fn(
    stream: StreamRef,
    info: *mut c_void,
    num_events: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: StreamCallback,
        context: *const Context,
        paths: *const c_void,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> StreamRef;
    fn FSEventStreamSetDispatchQueue(stream: StreamRef, queue: Queue);
    fn FSEventStreamStart(stream: StreamRef) -> u8;
    fn FSEventStreamStop(stream: StreamRef);
    fn FSEventStreamInvalidate(stream: StreamRef);
    fn FSEventStreamRelease(stream: StreamRef);
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(dev: libc::dev_t) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFUUIDCreateString(allocator: *const c_void, uuid: *const c_void) -> CFStringRef;
    fn CFRelease(cf: *const c_void);
}

// libdispatch is part of libSystem.
extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> Queue;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> Queue;
    fn dispatch_set_target_queue(object: Queue, queue: Queue);
    fn dispatch_sync_f(queue: Queue, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_release(object: Queue);
}

/// `QOS_CLASS_BACKGROUND`.
const QOS_BACKGROUND: isize = 0x09;

extern "C" fn on_events(
    _stream: StreamRef,
    info: *mut c_void,
    num_events: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    if info.is_null() || num_events == 0 {
        return;
    }
    // SAFETY: without kFSEventStreamCreateFlagUseCFTypes `paths` is a C array of `num_events`
    // NUL-terminated strings; `flags` and `ids` have `num_events` items. `info` is the boxed
    // callback owned by the `Stream`, which outlives every callback (see `Drop`).
    let events = unsafe {
        let paths = paths as *const *const c_char;
        (0..num_events)
            .map(|i| Event {
                path: CStr::from_ptr(*paths.add(i)).to_string_lossy().into_owned(),
                flags: *flags.add(i),
                id: *ids.add(i),
            })
            .collect()
    };
    // SAFETY: see above.
    let cb = unsafe { &*(info as *const Callback) };
    cb(events);
}

extern "C" fn noop(_: *mut c_void) {}

/// A running event stream; dropping it stops, invalidates and releases it.
pub struct Stream {
    stream: StreamRef,
    queue: Queue,
    info: *mut Callback,
}

// SAFETY: the stream is scheduled on its own serial dispatch queue; FSEventStreamStop,
// Invalidate and Release may be called from any thread, and the callback box is only freed after
// the queue was drained (see `Drop`).
unsafe impl Send for Stream {}

impl Stream {
    /// Starts a file-level stream on `paths` from event id `since` ([`SINCE_NOW`] for live events
    /// only). `on` runs on the stream's dispatch queue with each batch and must not block.
    pub fn start(
        paths: &[String],
        since: u64,
        latency: f64,
        on: impl Fn(Vec<Event>) + Send + 'static,
    ) -> Result<Stream, String> {
        let cf: Vec<CFString> = paths.iter().map(|p| CFString::new(p)).collect();
        let array = CFArray::from_CFTypes(&cf);
        let info: *mut Callback = Box::into_raw(Box::new(Box::new(on)));
        let ctx = Context {
            version: 0,
            info: info as *mut c_void,
            retain: ptr::null(),
            release: ptr::null(),
            copy_description: ptr::null(),
        };
        // SAFETY: all arguments are valid for the call; FSEventStreamCreate copies the context
        // and retains the path array.
        let stream = unsafe {
            FSEventStreamCreate(
                ptr::null(),
                on_events,
                &ctx,
                array.as_concrete_TypeRef() as *const c_void,
                since,
                latency,
                CREATE_FILE_EVENTS | CREATE_NO_DEFER | CREATE_WATCH_ROOT,
            )
        };
        if stream.is_null() {
            // SAFETY: never handed to a stream.
            drop(unsafe { Box::from_raw(info) });
            return Err(format!("FSEventStreamCreate failed for {paths:?}"));
        }
        // SAFETY: plain libdispatch calls; the label is a static C string.
        let queue = unsafe {
            let q = dispatch_queue_create(c"fxfm.fsevents".as_ptr(), ptr::null());
            dispatch_set_target_queue(q, dispatch_get_global_queue(QOS_BACKGROUND, 0));
            FSEventStreamSetDispatchQueue(stream, q);
            q
        };
        let s = Stream {
            stream,
            queue,
            info,
        };
        // SAFETY: the stream is valid and scheduled.
        if unsafe { FSEventStreamStart(s.stream) } == 0 {
            return Err(format!("FSEventStreamStart failed for {paths:?}"));
        }
        Ok(s)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream and queue are valid; after Invalidate no new callback is scheduled,
        // and the empty dispatch_sync waits for one that is running, so `info` can be freed.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            dispatch_sync_f(self.queue, ptr::null_mut(), noop);
            dispatch_release(self.queue);
            drop(Box::from_raw(self.info));
        }
    }
}

/// The id of the newest event in the system.
pub fn current_event_id() -> u64 {
    // SAFETY: no arguments.
    unsafe { FSEventsGetCurrentEventId() }
}

/// The UUID of the FSEvents store of a device; it changes when that event history is no longer
/// valid (e.g. the store was reset), so stored event ids must not be reused then.
pub fn volume_uuid(dev: libc::dev_t) -> Option<String> {
    // SAFETY: the returned CFUUID (create rule) is released here; the string is wrapped under
    // the create rule and released by `CFString`'s Drop.
    unsafe {
        let uuid = FSEventsCopyUUIDForDevice(dev);
        if uuid.is_null() {
            return None;
        }
        let s = CFUUIDCreateString(ptr::null(), uuid);
        CFRelease(uuid);
        if s.is_null() {
            return None;
        }
        Some(CFString::wrap_under_create_rule(s).to_string())
    }
}

/// The event-store UUID of the device that holds `path`.
pub fn path_uuid(path: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(path).ok()?.dev();
    volume_uuid(dev as libc::dev_t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    /// The session scratchpad (never `$HOME`); override with `FXFM_PROBE_DIR`.
    fn scratch() -> std::path::PathBuf {
        let base = std::env::var("FXFM_PROBE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        tempfile::Builder::new()
            .prefix("fxfm-fsevents")
            .tempdir_in(base)
            .unwrap()
            .keep()
    }

    #[test]
    #[ignore = "uses the real FSEvents service"]
    fn fsevents_stream_reports_a_created_file() {
        let dir = scratch();
        let (tx, rx) = channel();
        let tx = std::sync::Mutex::new(tx);
        let path = dir.to_string_lossy().into_owned();
        let s = Stream::start(&[path], SINCE_NOW, LATENCY, move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        })
        .unwrap();
        std::fs::write(dir.join("created.txt"), b"x").unwrap();
        let start = Instant::now();
        let mut found = false;
        while start.elapsed() < Duration::from_secs(5) && !found {
            if let Ok(batch) = rx.recv_timeout(Duration::from_millis(200)) {
                found = batch.iter().any(|e| e.path.ends_with("/created.txt"));
            }
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found, "no event for created.txt within 5 s");
    }

    #[test]
    #[ignore = "uses the real FSEvents service"]
    fn fsevents_current_id_and_uuid() {
        assert!(current_event_id() > 0);
        let dir = scratch();
        assert!(path_uuid(&dir.to_string_lossy()).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
