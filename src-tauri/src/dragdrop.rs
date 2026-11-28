//! Drag & drop: the native drag-out of panel entries (an AppKit dragging session on macOS, a
//! GTK drag with `text/uri-list` on Linux) and the modifier state the UI reads when a drop
//! arrives (the drop event itself carries none).
//!
//! Linux has no file promises: archive and server entries are written to a temporary folder
//! when the receiver asks for the data, and their copies are dragged (copy only).
//!
//! Local files go on the pasteboard as file URLs, so the receiver may copy or move them
//! (Finder's own rules: move on the same disk, Option copies, Cmd moves). Archive and server
//! entries are file promises (`NSFilePromiseProvider`): nothing is extracted or downloaded
//! until the receiver asks, and then the item (a folder with everything in it) is written
//! straight to the location the receiver names, on fxfm's own operation queue. Promised
//! items are copy only.
// The drag image badge and NSDragOperation helpers are macOS only.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::state::AppState;
use fxfm_core::archive::split_location;
use fxfm_core::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
use fxfm_core::remote::{parse_remote, RemoteError};
use fxfm_core::vfs;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What a drag carries. All entries come from one panel folder, so they are of one kind.
#[derive(Clone, Debug, PartialEq)]
pub enum DragItems {
    /// Local files and folders (an archive file itself is a local file).
    Local(Vec<PathBuf>),
    /// Entries inside an archive or on a server.
    Promised(Vec<Promise>),
}

/// An archive or server entry, written only when the receiver asks for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Promise {
    /// fxfm location (`<archive>/<inner>` or `sftp://server/path`).
    pub location: String,
    /// File name the receiver gets.
    pub name: String,
    pub is_dir: bool,
}

/// Kind of a location in a drag.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Local,
    Archive,
    Server,
}

fn kind_of(p: &str) -> Kind {
    if parse_remote(p).is_some() {
        Kind::Server
    } else if split_location(p).is_some_and(|a| !a.inner.is_empty()) {
        Kind::Archive
    } else {
        Kind::Local
    }
}

/// True when a raw `/` segment is `.` or `..` (`Path::components` silently drops inner ".").
fn dotted(p: &str) -> bool {
    p.split('/').any(|c| c == "." || c == "..")
}

/// Checks the entries of a drag-out (`dirs[i]`: `paths[i]` is a folder):
/// - local: absolute paths that exist (symlinks included);
/// - inside an archive: absolute (an archive is recognized only when its file exists);
/// - on a server: an item below the root.
///
/// No `.`/`..` segments anywhere, and all entries of one kind.
pub fn validate_paths(paths: &[String], dirs: &[bool]) -> Result<DragItems, String> {
    if paths.is_empty() {
        return Err("Nothing to drag".into());
    }
    if dirs.len() != paths.len() {
        return Err("Folder flags do not match the dragged items".into());
    }
    let kind = kind_of(&paths[0]);
    if paths.iter().any(|p| kind_of(p) != kind) {
        return Err("All items must come from the same folder".into());
    }
    if kind == Kind::Local {
        return paths
            .iter()
            .map(|p| {
                if !p.starts_with('/') || dotted(p) {
                    return Err(format!("Not a local path: {p}"));
                }
                let path = PathBuf::from(p);
                match path.symlink_metadata() {
                    Ok(_) => Ok(path),
                    Err(_) => Err(format!("Not found: {p}")),
                }
            })
            .collect::<Result<_, _>>()
            .map(DragItems::Local);
    }
    paths
        .iter()
        .zip(dirs)
        .map(|(p, &is_dir)| {
            let bad = || format!("Not a valid location: {p}");
            let inner = match kind {
                Kind::Server => {
                    let r = parse_remote(p).ok_or_else(bad)?;
                    if r.path == "/" || dotted(&r.path) {
                        return Err(bad());
                    }
                    r.path
                }
                _ => {
                    let a = split_location(p).ok_or_else(bad)?;
                    if !p.starts_with('/') || dotted(p) {
                        return Err(bad());
                    }
                    a.inner
                }
            };
            let name = inner.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if name.is_empty() {
                return Err(bad());
            }
            Ok(Promise {
                location: p.clone(),
                name: name.to_string(),
                is_dir,
            })
        })
        .collect::<Result<_, _>>()
        .map(DragItems::Promised)
}

/// Text of the count badge on the drag image: none for one item.
pub fn badge_text(count: usize) -> Option<String> {
    match count {
        0 | 1 => None,
        2..=999 => Some(count.to_string()),
        _ => Some("999+".into()),
    }
}

/// Badge frame `(x, y, width, height)` in a square canvas of `canvas` points (origin bottom
/// left) for a label `text_width` wide: a pill in the top right corner, at least round.
pub fn badge_rect(canvas: f64, text_width: f64) -> (f64, f64, f64, f64) {
    const HEIGHT: f64 = 22.0;
    const PAD: f64 = 7.0;
    let width = (text_width + 2.0 * PAD).clamp(HEIGHT, canvas);
    (canvas - width, canvas - HEIGHT, width, HEIGHT)
}

// `NSDragOperation` bits (checked against AppKit's values in the tests).
pub const OP_COPY: usize = 1;
pub const OP_LINK: usize = 2;
pub const OP_GENERIC: usize = 4;
pub const OP_MOVE: usize = 16;
pub const OP_DELETE: usize = 32;

/// Operations the receiver may perform: local files copy or move (Generic lets Finder pick
/// its default, a move on the same disk), promised items only copy.
pub fn source_mask(items: &DragItems) -> usize {
    match items {
        DragItems::Local(_) => OP_COPY | OP_MOVE | OP_GENERIC,
        DragItems::Promised(_) => OP_COPY,
    }
}

/// `drag://ended` operation for the `NSDragOperation` the receiver performed: "none", "copy",
/// "link" or "move" (Generic and Delete included: the dragged items may be gone).
pub fn operation_name(op: usize) -> &'static str {
    if op & (OP_MOVE | OP_GENERIC | OP_DELETE) != 0 {
        "move"
    } else if op & OP_COPY != 0 {
        "copy"
    } else if op & OP_LINK != 0 {
        "link"
    } else {
        "none"
    }
}

/// Uniform type of a promise (what the receiver sees before the item exists).
pub fn promise_type(p: &Promise) -> &'static str {
    if p.is_dir {
        "public.folder"
    } else {
        "public.data"
    }
}

/// Records the first item error; the item is skipped. Nothing exists at the destination
/// when the write starts, so a conflict can only be a race: it is skipped too.
#[derive(Default)]
struct FirstError(Mutex<Option<String>>);

impl OpObserver for FirstError {
    fn progress(&self, _p: &Progress) {}
    fn conflict(&self, c: &ConflictInfo) -> ConflictAnswer {
        self.note(format!("{} already exists", c.target));
        ConflictAnswer::Skip
    }
    fn error(&self, path: &str, msg: &str) -> ErrorAnswer {
        self.note(format!("{path}: {msg}"));
        ErrorAnswer::Skip
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl FirstError {
    fn note(&self, msg: String) {
        self.0.lock().unwrap().get_or_insert(msg);
    }
}

/// Writes the archive or server entry `location` (a folder with everything in it) to `dest`,
/// the path the receiver of a promise named. An existing item there is never replaced: the
/// copy is kept beside it as `name 2.ext` (`name 3.ext`…), as Finder keeps both. Runs the same
/// copy as F5 (archive extraction, or a download on a forked connection).
pub fn write_promise(ctx: &vfs::Ctx, location: &str, dest: &Path) -> Result<(), String> {
    if !dest.is_absolute() {
        return Err(format!("Not a local path: {}", dest.display()));
    }
    let free = free_name(dest);
    let dest = free.as_path();
    let to = dest
        .to_str()
        .ok_or_else(|| format!("Not a valid path: {}", dest.display()))?;
    let obs = FirstError::default();
    // One source and a target that does not exist: the item is written as `dest` itself.
    let summary =
        vfs::copy(ctx, &[location.to_string()], to, "/", &obs).map_err(|e| e.to_string())?;
    if let Some(msg) = obs.0.lock().unwrap().take() {
        return Err(msg);
    }
    if summary.cancelled || summary.skipped > 0 || dest.symlink_metadata().is_err() {
        return Err(format!("{location} could not be copied"));
    }
    Ok(())
}

/// `dest` when nothing exists there, else the first free `name N.ext` beside it (N from 2).
fn free_name(dest: &Path) -> std::path::PathBuf {
    if dest.symlink_metadata().is_err() {
        return dest.to_path_buf();
    }
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A leading dot is part of the name (`.bashrc` has no extension).
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    (2..)
        .map(|n| dest.with_file_name(format!("{stem} {n}{ext}")))
        .find(|p| p.symlink_metadata().is_err())
        .expect("a free name")
}

/// `drag://ended` payload: `result` is "dropped", "cancelled" or "error" (with `message`);
/// `operation` (see `operation_name`) is what the receiver did with a dropped drag.
/// A promised item that could not be written ends with another "error" after the drop.
#[derive(Serialize, Clone, Debug)]
pub struct DragEnded {
    pub result: &'static str,
    pub operation: Option<&'static str>,
    pub message: Option<String>,
}

impl DragEnded {
    pub fn error(message: String) -> DragEnded {
        DragEnded {
            result: "error",
            operation: None,
            message: Some(message),
        }
    }
}

/// Modifier keys held right now (read when a drop arrives).
#[derive(Serialize, Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
}

/// Starts a native drag of the panel entries `paths` (`dirs[i]`: a folder) out of the window.
/// Returns once the drag is queued on the main thread; `drag://ended` reports how it ended.
#[tauri::command]
pub async fn start_drag(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
    dirs: Vec<bool>,
) -> Result<(), String> {
    // The paths are checked off the main thread: a stat on a share that went away blocks.
    let items = tauri::async_runtime::spawn_blocking(move || validate_paths(&paths, &dirs))
        .await
        .map_err(|e| e.to_string())??;
    if let DragItems::Promised(ps) = &items {
        // Refused now rather than when the receiver asks (the connection is not touched).
        for r in ps.iter().filter_map(|p| parse_remote(&p.location)) {
            if state.vfs.sessions.get(&r.server).is_none() {
                return Err(format!("{}: {}", r.server, RemoteError::Disconnected));
            }
        }
    }
    native::start(window, items, state.vfs.clone())
}

/// The modifier keys held at this moment (the drop event carries none).
#[tauri::command]
pub fn drag_modifiers() -> Modifiers {
    native::modifiers()
}

#[cfg(target_os = "macos")]
mod native {
    use super::{
        badge_rect, badge_text, operation_name, promise_type, source_mask, write_promise,
        DragEnded, DragItems, Modifiers, Promise,
    };
    use block2::{DynBlock, RcBlock};
    use fxfm_core::vfs;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{
        define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSDragOperation, NSDraggingContext, NSDraggingItem,
        NSDraggingSession, NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType,
        NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSImage, NSStringDrawing, NSView, NSWorkspace,
    };
    use objc2_foundation::{
        NSArray, NSDictionary, NSError, NSLocalizedDescriptionKey, NSMutableArray,
        NSOperationQueue, NSPoint, NSRect, NSSize, NSString, NSURL,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tauri::{Emitter, WebviewWindow};

    /// Icon size in points; the drag image is a little larger to fit the count badge.
    const ICON_SIZE: f64 = 64.0;
    const CANVAS: f64 = 76.0;
    /// Promise delegates of this many recent drags are kept (see `KEEP`).
    const KEEP_DRAGS: usize = 8;

    thread_local! {
        /// A promise provider holds its delegate weakly, and the receiver asks for the files
        /// only after the drop (Finder: when the drag session already ended), so the
        /// delegates of the last few drags are kept alive here (main thread only).
        static KEEP: RefCell<VecDeque<Vec<Retained<FilePromise>>>> = const { RefCell::new(VecDeque::new()) };
        /// Where promised items are written (never the main thread); created on first use.
        static QUEUE: RefCell<Option<Retained<NSOperationQueue>>> = const { RefCell::new(None) };
        /// Source of the latest session (it must live until the session ends).
        static SOURCE: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
    }

    struct PromiseIvars {
        location: String,
        name: String,
        ctx: Arc<vfs::Ctx>,
        window: WebviewWindow,
        queue: Retained<NSOperationQueue>,
    }

    define_class!(
        /// Delegate of one `NSFilePromiseProvider`: names the item and writes it on request.
        #[unsafe(super(NSObject))]
        #[name = "FxfmFilePromise"]
        #[ivars = PromiseIvars]
        struct FilePromise;

        unsafe impl NSObjectProtocol for FilePromise {}

        unsafe impl NSFilePromiseProviderDelegate for FilePromise {
            #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
            fn file_name(
                &self,
                _provider: &NSFilePromiseProvider,
                _file_type: &NSString,
            ) -> Retained<NSString> {
                NSString::from_str(&self.ivars().name)
            }

            /// Runs on `queue`: writes the item, then reports to the receiver (and the UI).
            #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
            fn write(
                &self,
                _provider: &NSFilePromiseProvider,
                url: &NSURL,
                done: &DynBlock<dyn Fn(*mut NSError)>,
            ) {
                let iv = self.ivars();
                let res = match url.path() {
                    Some(dest) if url.isFileURL() => {
                        write_promise(&iv.ctx, &iv.location, Path::new(&dest.to_string()))
                    }
                    _ => Err("The destination is not a file location".to_string()),
                };
                match res {
                    Ok(()) => done.call((std::ptr::null_mut(),)),
                    Err(msg) => {
                        let msg = format!("{}: {msg}", iv.name);
                        let err = ns_error(&msg);
                        done.call((Retained::as_ptr(&err).cast_mut(),));
                        let _ = iv.window.emit("drag://ended", DragEnded::error(msg));
                    }
                }
            }

            #[unsafe(method_id(operationQueueForFilePromiseProvider:))]
            fn operation_queue(
                &self,
                _provider: &NSFilePromiseProvider,
            ) -> Retained<NSOperationQueue> {
                self.ivars().queue.clone()
            }
        }
    );

    impl FilePromise {
        fn new(
            p: &Promise,
            ctx: Arc<vfs::Ctx>,
            window: WebviewWindow,
            queue: Retained<NSOperationQueue>,
        ) -> Retained<Self> {
            let this = Self::alloc().set_ivars(PromiseIvars {
                location: p.location.clone(),
                name: p.name.clone(),
                ctx,
                window,
                queue,
            });
            // SAFETY: `NSObject`'s designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    struct SourceIvars {
        window: WebviewWindow,
        mask: usize,
    }

    define_class!(
        /// Source of a drag session: the operations offered and how the drag ended.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "FxfmDragSource"]
        #[ivars = SourceIvars]
        struct DragSource;

        unsafe impl NSObjectProtocol for DragSource {}

        unsafe impl NSDraggingSource for DragSource {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn operation_mask(
                &self,
                _session: &NSDraggingSession,
                _context: NSDraggingContext,
            ) -> NSDragOperation {
                NSDragOperation(self.ivars().mask)
            }

            #[unsafe(method(draggingSession:endedAtPoint:operation:))]
            fn ended(&self, _session: &NSDraggingSession, _point: NSPoint, op: NSDragOperation) {
                let result = if op == NSDragOperation::None {
                    "cancelled"
                } else {
                    "dropped"
                };
                let ended = DragEnded {
                    result,
                    operation: Some(operation_name(op.0)),
                    message: None,
                };
                let _ = self.ivars().window.emit("drag://ended", ended);
            }
        }
    );

    impl DragSource {
        fn new(mtm: MainThreadMarker, window: WebviewWindow, mask: usize) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(SourceIvars { window, mask });
            // SAFETY: `NSObject`'s designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// An `NSError` whose description is `msg` (shown by the receiver, e.g. Finder).
    fn ns_error(msg: &str) -> Retained<NSError> {
        let text = NSString::from_str(msg);
        // SAFETY: the key is an `NSErrorUserInfoKey` and its value an `NSString`.
        unsafe {
            let info: Retained<NSDictionary<NSString, AnyObject>> =
                NSDictionary::from_slices(&[NSLocalizedDescriptionKey], &[text.as_ref()]);
            NSError::errorWithDomain_code_userInfo(&NSString::from_str("fxfm"), 1, Some(&info))
        }
    }

    fn queue() -> Retained<NSOperationQueue> {
        QUEUE.with(|q| {
            q.borrow_mut()
                .get_or_insert_with(|| {
                    let q = NSOperationQueue::new();
                    q.setName(Some(&NSString::from_str("fxfm file promises")));
                    // One item at a time: each server item downloads on its own connection.
                    q.setMaxConcurrentOperationCount(1);
                    q
                })
                .clone()
        })
    }

    pub fn start(
        window: WebviewWindow,
        items: DragItems,
        ctx: Arc<vfs::Ctx>,
    ) -> Result<(), String> {
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                if let Err(e) = begin(&w, items, ctx) {
                    let _ = w.emit("drag://ended", DragEnded::error(e));
                }
            })
            .map_err(|e| e.to_string())
    }

    /// Starts the dragging session (main thread).
    fn begin(w: &WebviewWindow, items: DragItems, ctx: Arc<vfs::Ctx>) -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or("Not on the main thread")?;
        // The button went up before the drag got here: a session started now would stick
        // to the pointer until the next click.
        if NSEvent::pressedMouseButtons() & 1 == 0 {
            let ended = DragEnded {
                result: "cancelled",
                operation: Some("none"),
                message: None,
            };
            let _ = w.emit("drag://ended", ended);
            return Ok(());
        }
        let view = w.ns_view().map_err(|e| e.to_string())?;
        // SAFETY: Tauri hands out the window's live content view; we are on the main thread.
        let view: &NSView = unsafe { &*view.cast::<NSView>() };
        let window = view.window().ok_or("The window has no content")?;
        let content = window.contentView().ok_or("The window has no content")?;
        let pos = window.mouseLocationOutsideOfEventStream();

        let count = match &items {
            DragItems::Local(f) => f.len(),
            DragItems::Promised(p) => p.len(),
        };
        let image = with_badge(drag_icon(&items), count);
        let size = image.size();
        let frame = NSRect::new(
            NSPoint::new(pos.x - size.width / 2.0, pos.y - size.height / 2.0),
            size,
        );
        let mask = source_mask(&items);
        let drag_items: Retained<NSMutableArray<NSDraggingItem>> = NSMutableArray::new();
        let add = |item: Retained<NSDraggingItem>| {
            // One image for the whole group: the first item carries it.
            let contents: Option<&AnyObject> = drag_items.is_empty().then(|| image.as_ref());
            // SAFETY: the contents is an `NSImage` (or none).
            unsafe { item.setDraggingFrame_contents(frame, contents) };
            drag_items.addObject(&item);
        };
        match items {
            DragItems::Local(files) => {
                for f in files {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&f.to_string_lossy()));
                    let writer = ProtocolObject::from_ref(&*url);
                    add(NSDraggingItem::initWithPasteboardWriter(
                        NSDraggingItem::alloc(),
                        writer,
                    ));
                }
            }
            DragItems::Promised(promises) => {
                let queue = queue();
                let mut keep = Vec::with_capacity(promises.len());
                for p in &promises {
                    let delegate = FilePromise::new(p, ctx.clone(), w.clone(), queue.clone());
                    let provider = NSFilePromiseProvider::initWithFileType_delegate(
                        NSFilePromiseProvider::alloc(),
                        &NSString::from_str(promise_type(p)),
                        ProtocolObject::from_ref(&*delegate),
                    );
                    let writer = ProtocolObject::from_ref(&*provider);
                    add(NSDraggingItem::initWithPasteboardWriter(
                        NSDraggingItem::alloc(),
                        writer,
                    ));
                    keep.push(delegate);
                }
                KEEP.with(|k| {
                    let mut k = k.borrow_mut();
                    k.push_back(keep);
                    while k.len() > KEEP_DRAGS {
                        k.pop_front();
                    }
                });
            }
        }

        let timestamp = NSApplication::sharedApplication(mtm)
            .currentEvent()
            .map_or(0.0, |e| e.timestamp());
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDragged,
            pos,
            NSEventModifierFlags::empty(),
            timestamp,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        )
        .ok_or("Cannot start the drag")?;
        let source = DragSource::new(mtm, w.clone(), mask);
        content.beginDraggingSessionWithItems_event_source(
            &drag_items,
            &event,
            ProtocolObject::from_ref(&*source),
        );
        SOURCE.with(|s| *s.borrow_mut() = Some(source));
        Ok(())
    }

    /// Finder's icon of the file (one) or of the selection (several); for archive and server
    /// entries the icon of the first one's type.
    fn drag_icon(items: &DragItems) -> Retained<NSImage> {
        let ws = NSWorkspace::sharedWorkspace();
        let img = match items {
            DragItems::Local(files) => {
                let names: Vec<_> = files
                    .iter()
                    .map(|p| NSString::from_str(&p.to_string_lossy()))
                    .collect();
                match names.as_slice() {
                    [one] => Some(ws.iconForFile(one)),
                    _ => ws.iconForFiles(&NSArray::from_retained_slice(&names)),
                }
            }
            DragItems::Promised(ps) => ps.first().map(|p| {
                let ext = PathBuf::from(&p.name)
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned());
                let ty = match ext {
                    Some(e) if !p.is_dir => e,
                    _ => promise_type(p).to_string(),
                };
                // The UTType-based replacement needs another framework binding for the same icon.
                #[allow(deprecated)]
                ws.iconForFileType(&NSString::from_str(&ty))
            }),
        };
        let img = img.unwrap_or_else(|| {
            #[allow(deprecated)]
            ws.iconForFileType(&NSString::from_str("public.data"))
        });
        img.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
        img
    }

    /// The drag image: the icon, with a red badge showing `count` when there are several.
    fn with_badge(icon: Retained<NSImage>, count: usize) -> Retained<NSImage> {
        let Some(label) = badge_text(count) else {
            return icon;
        };
        let label = NSString::from_str(&label);
        let draw = RcBlock::new(move |_rect: NSRect| -> Bool {
            icon.drawInRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(ICON_SIZE, ICON_SIZE),
            ));
            let font = NSFont::boldSystemFontOfSize(13.0);
            let white = NSColor::whiteColor();
            // SAFETY: the attribute values have the types AppKit expects for these keys.
            unsafe {
                let attrs: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                    &[NSFontAttributeName, NSForegroundColorAttributeName],
                    &[font.as_ref(), white.as_ref()],
                );
                let text = label.sizeWithAttributes(Some(&attrs));
                let (x, y, bw, bh) = badge_rect(CANVAS, text.width);
                let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(bw, bh));
                NSColor::systemRedColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, bh / 2.0, bh / 2.0)
                    .fill();
                let at = NSPoint::new(x + (bw - text.width) / 2.0, y + (bh - text.height) / 2.0);
                label.drawAtPoint_withAttributes(at, Some(&attrs));
            }
            Bool::YES
        });
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(CANVAS, CANVAS), false, &draw)
    }

    pub fn modifiers() -> Modifiers {
        let flags = NSEvent::modifierFlags_class();
        Modifiers {
            shift: flags.contains(NSEventModifierFlags::Shift),
        }
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::{write_promise, DragEnded, DragItems, Modifiers};
    use fxfm_core::vfs;
    use gtk::prelude::*;
    use gtk::{gdk, glib};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use tauri::Emitter;

    pub fn start(
        window: tauri::WebviewWindow,
        items: DragItems,
        ctx: Arc<vfs::Ctx>,
    ) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let w = window.clone();
        window
            .run_on_main_thread(move || drop(tx.send(begin(w, items, ctx))))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    /// The `file://` URIs of the dragged items; archive and server entries are written into a
    /// temporary folder first (once per drag, when the receiver asks).
    fn uris(items: &DragItems, ctx: &vfs::Ctx) -> Result<Vec<String>, String> {
        let local = |p: &std::path::Path| gtk::gio::File::for_path(p).uri().to_string();
        match items {
            DragItems::Local(paths) => Ok(paths.iter().map(|p| local(p)).collect()),
            DragItems::Promised(ps) => {
                let dir = ctx.new_temp_dir("drag").map_err(|e| e.to_string())?;
                ps.iter()
                    .map(|p| {
                        let dest = dir.join(&p.name);
                        write_promise(ctx, &p.location, &dest)?;
                        Ok(local(&dest))
                    })
                    .collect()
            }
        }
    }

    fn begin(
        window: tauri::WebviewWindow,
        items: DragItems,
        ctx: Arc<vfs::Ctx>,
    ) -> Result<(), String> {
        let gw = window.gtk_window().map_err(|e| e.to_string())?;
        let targets = gtk::TargetList::new(&[gtk::TargetEntry::new(
            "text/uri-list",
            gtk::TargetFlags::OTHER_APP,
            0,
        )]);
        let actions = match items {
            DragItems::Local(_) => gdk::DragAction::COPY | gdk::DragAction::MOVE,
            DragItems::Promised(_) => gdk::DragAction::COPY,
        };
        let cache: Rc<RefCell<Option<Vec<String>>>> = Rc::default();
        let ids: Rc<RefCell<Vec<glib::SignalHandlerId>>> = Rc::default();
        let w = window.clone();
        let get = gw.connect_drag_data_get(move |_, _, sel, _, _| {
            let mut c = cache.borrow_mut();
            if c.is_none() {
                match uris(&items, &ctx) {
                    Ok(u) => *c = Some(u),
                    Err(msg) => {
                        let _ = w.emit("drag://ended", DragEnded::error(msg));
                        *c = Some(Vec::new());
                    }
                }
            }
            let list: Vec<&str> = c
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect();
            sel.set_uris(&list);
        });
        let w = window.clone();
        let ids_end = ids.clone();
        let end = gw.connect_drag_end(move |gw, dctx| {
            let op = match dctx.selected_action() {
                a if a.contains(gdk::DragAction::MOVE) => "move",
                a if a.contains(gdk::DragAction::COPY) => "copy",
                a if a.contains(gdk::DragAction::LINK) => "link",
                _ => "none",
            };
            let ended = DragEnded {
                result: if op == "none" { "cancelled" } else { "dropped" },
                operation: Some(op),
                message: None,
            };
            let _ = w.emit("drag://ended", ended);
            for id in ids_end.borrow_mut().drain(..) {
                gw.disconnect(id);
            }
        });
        ids.borrow_mut().extend([get, end]);
        // The button is still held (the drag started in the page): GTK takes the pointer.
        gw.drag_begin_with_coordinates(&targets, actions, 1, None, -1, -1)
            .map(|_| ())
            .ok_or_else(|| "The drag could not start".to_string())
    }

    pub fn modifiers() -> Modifiers {
        let shift = gdk::Display::default()
            .and_then(|d| gdk::Keymap::for_display(&d))
            .is_some_and(|k| k.modifier_state() & gdk::ModifierType::SHIFT_MASK.bits() != 0);
        Modifiers { shift }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod native {
    use super::{DragItems, Modifiers};
    use fxfm_core::vfs;
    use std::sync::Arc;

    pub fn start(
        _window: tauri::WebviewWindow,
        _items: DragItems,
        _ctx: Arc<vfs::Ctx>,
    ) -> Result<(), String> {
        Err("Dragging files out is not supported here".into())
    }

    pub fn modifiers() -> Modifiers {
        Modifiers::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    fn files(n: usize) -> Vec<bool> {
        vec![false; n]
    }

    fn promised(items: DragItems) -> Vec<Promise> {
        match items {
            DragItems::Promised(p) => p,
            other => panic!("not promised: {other:?}"),
        }
    }

    #[test]
    fn accepts_existing_local_files_and_folders() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir(t.path().join("dir")).unwrap();
        let paths = vec![s(&t.path().join("a.txt")), s(&t.path().join("dir"))];
        let got = validate_paths(&paths, &[false, true]).unwrap();
        assert_eq!(
            got,
            DragItems::Local(vec![t.path().join("a.txt"), t.path().join("dir")])
        );
    }

    #[test]
    fn an_archive_file_itself_is_local() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        let p = s(&t.path().join("x.zip"));
        assert_eq!(
            validate_paths(&[p], &files(1)).unwrap(),
            DragItems::Local(vec![t.path().join("x.zip")])
        );
    }

    #[test]
    fn a_dangling_symlink_is_still_draggable() {
        let t = tempfile::tempdir().unwrap();
        let link = t.path().join("gone");
        std::os::unix::fs::symlink(t.path().join("nowhere"), &link).unwrap();
        assert!(validate_paths(&[s(&link)], &files(1)).is_ok());
    }

    #[test]
    fn refuses_nothing() {
        assert_eq!(validate_paths(&[], &[]).unwrap_err(), "Nothing to drag");
    }

    #[test]
    fn refuses_missing_folder_flags() {
        let t = tempfile::tempdir().unwrap();
        let p = s(t.path());
        assert!(validate_paths(&[p], &[]).is_err());
    }

    #[test]
    fn refuses_missing_paths() {
        let t = tempfile::tempdir().unwrap();
        let p = s(&t.path().join("missing.txt"));
        assert_eq!(
            validate_paths(std::slice::from_ref(&p), &files(1)).unwrap_err(),
            format!("Not found: {p}")
        );
    }

    #[test]
    fn archive_entries_are_promised() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        let file = s(&t.path().join("x.zip/d/inner.txt"));
        let dir = s(&t.path().join("x.zip/d/sub"));
        let got = promised(validate_paths(&[file.clone(), dir.clone()], &[false, true]).unwrap());
        assert_eq!(
            got,
            vec![
                Promise {
                    location: file,
                    name: "inner.txt".into(),
                    is_dir: false
                },
                Promise {
                    location: dir,
                    name: "sub".into(),
                    is_dir: true
                },
            ]
        );
    }

    #[test]
    fn refuses_entries_of_a_missing_archive() {
        // Without the archive file it is a local path that does not exist.
        let t = tempfile::tempdir().unwrap();
        let p = s(&t.path().join("x.zip/inner.txt"));
        let e = validate_paths(std::slice::from_ref(&p), &files(1)).unwrap_err();
        assert_eq!(e, format!("Not found: {p}"));
    }

    #[test]
    fn server_entries_are_promised() {
        for (loc, name) in [
            ("sftp://user@host/home/a.txt", "a.txt"),
            ("ftp://host/a", "a"),
            ("scp://h/x/y/", "y"),
            ("ftps://h/y", "y"),
        ] {
            let got = promised(validate_paths(&[loc.to_string()], &files(1)).unwrap());
            assert_eq!(got[0].name, name, "{loc}");
            assert_eq!(got[0].location, loc);
        }
    }

    #[test]
    fn refuses_a_server_root_and_dotted_server_paths() {
        for loc in ["sftp://h/", "sftp://h", "ftp://h/a/../b", "scp://h/./a"] {
            let e = validate_paths(&[loc.to_string()], &files(1)).unwrap_err();
            assert_eq!(e, format!("Not a valid location: {loc}"));
        }
    }

    #[test]
    fn refuses_mixed_kinds() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        let local = s(&t.path().join("x.zip"));
        let inner = s(&t.path().join("x.zip/inner.txt"));
        let server = "sftp://h/a".to_string();
        for paths in [
            [local.clone(), inner.clone()],
            [inner, server.clone()],
            [server, local],
        ] {
            assert_eq!(
                validate_paths(&paths, &files(2)).unwrap_err(),
                "All items must come from the same folder"
            );
        }
    }

    #[test]
    fn refuses_relative_and_dotted_paths() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d")).unwrap();
        for p in [
            "a.txt".to_string(),
            "".to_string(),
            format!("{}/d/..", s(t.path())),
            format!("{}/./d", s(t.path())),
        ] {
            let e = validate_paths(std::slice::from_ref(&p), &files(1)).unwrap_err();
            assert_eq!(e, format!("Not a local path: {p}"));
        }
        std::fs::write(t.path().join("x.zip"), b"").unwrap();
        for p in [
            format!("{}/x.zip/d/../a", s(t.path())),
            format!("{}/./x.zip/a", s(t.path())),
        ] {
            let e = validate_paths(std::slice::from_ref(&p), &files(1)).unwrap_err();
            assert_eq!(e, format!("Not a valid location: {p}"));
        }
    }

    #[test]
    fn badge_only_for_several_items() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1), None);
        assert_eq!(badge_text(2).as_deref(), Some("2"));
        assert_eq!(badge_text(999).as_deref(), Some("999"));
        assert_eq!(badge_text(1000).as_deref(), Some("999+"));
    }

    #[test]
    fn badge_sits_in_the_top_right_corner() {
        // Narrow text: a circle.
        assert_eq!(badge_rect(76.0, 4.0), (54.0, 54.0, 22.0, 22.0));
        // Wider text: a pill that grows to the left.
        let (x, y, w, h) = badge_rect(76.0, 30.0);
        assert_eq!((x + w, y + h, w), (76.0, 76.0, 44.0));
        // Never wider than the canvas.
        assert_eq!(badge_rect(76.0, 500.0).2, 76.0);
    }

    #[test]
    fn local_files_may_move_promised_items_only_copy() {
        let local = DragItems::Local(vec![PathBuf::from("/a")]);
        assert_eq!(source_mask(&local), OP_COPY | OP_MOVE | OP_GENERIC);
        let p = Promise {
            location: "sftp://h/a".into(),
            name: "a".into(),
            is_dir: false,
        };
        assert_eq!(source_mask(&DragItems::Promised(vec![p])), OP_COPY);
    }

    #[test]
    fn operation_names() {
        assert_eq!(operation_name(0), "none");
        assert_eq!(operation_name(OP_COPY), "copy");
        assert_eq!(operation_name(OP_LINK), "link");
        assert_eq!(operation_name(OP_MOVE), "move");
        assert_eq!(operation_name(OP_GENERIC), "move");
        assert_eq!(operation_name(OP_DELETE), "move");
        assert_eq!(operation_name(OP_COPY | OP_MOVE), "move");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn operation_bits_match_appkit() {
        use objc2_app_kit::NSDragOperation as Op;
        assert_eq!(Op::Copy.0, OP_COPY);
        assert_eq!(Op::Link.0, OP_LINK);
        assert_eq!(Op::Generic.0, OP_GENERIC);
        assert_eq!(Op::Move.0, OP_MOVE);
        assert_eq!(Op::Delete.0, OP_DELETE);
    }

    #[test]
    fn promise_types() {
        let mut p = Promise {
            location: "sftp://h/a".into(),
            name: "a".into(),
            is_dir: true,
        };
        assert_eq!(promise_type(&p), "public.folder");
        p.is_dir = false;
        assert_eq!(promise_type(&p), "public.data");
    }

    // ---- writing promises (archives; servers go through the same `vfs::copy`)

    fn tar_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut b = tar::Builder::new(std::fs::File::create(path).unwrap());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(1_700_000_000);
            h.set_cksum();
            b.append_data(&mut h, name, *data).unwrap();
        }
        b.finish().unwrap();
    }

    fn archive_fixture() -> (tempfile::TempDir, vfs::Ctx, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let tar = t.path().join("a.tar");
        tar_with(
            &tar,
            &[("x.txt", b"xx"), ("d/b.txt", b"bbb"), ("d/e/c.txt", b"c")],
        );
        std::fs::create_dir(t.path().join("out")).unwrap();
        let ctx = vfs::Ctx::new(t.path().join("tmp-root"));
        (t, ctx, tar)
    }

    #[test]
    fn writes_an_archive_file_to_the_named_destination() {
        let (t, ctx, tar) = archive_fixture();
        let dest = t.path().join("out/renamed.txt");
        write_promise(&ctx, &format!("{}/x.txt", s(&tar)), &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"xx");
    }

    #[test]
    fn writes_an_archive_folder_tree() {
        let (t, ctx, tar) = archive_fixture();
        let dest = t.path().join("out/d");
        write_promise(&ctx, &format!("{}/d", s(&tar)), &dest).unwrap();
        assert_eq!(std::fs::read(dest.join("b.txt")).unwrap(), b"bbb");
        assert_eq!(std::fs::read(dest.join("e/c.txt")).unwrap(), b"c");
    }

    #[test]
    fn keeps_both_when_the_name_exists() {
        let (t, ctx, tar) = archive_fixture();
        let dest = t.path().join("out/x.txt");
        std::fs::write(&dest, b"mine").unwrap();
        std::fs::write(t.path().join("out/x 2.txt"), b"also mine").unwrap();
        write_promise(&ctx, &format!("{}/x.txt", s(&tar)), &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"mine");
        assert_eq!(
            std::fs::read(t.path().join("out/x 2.txt")).unwrap(),
            b"also mine"
        );
        assert_eq!(std::fs::read(t.path().join("out/x 3.txt")).unwrap(), b"xx");
        let dir = t.path().join("out/d");
        std::fs::create_dir(&dir).unwrap();
        write_promise(&ctx, &format!("{}/d", s(&tar)), &dir).unwrap();
        assert!(t.path().join("out/d 2/b.txt").exists());
        assert_eq!(
            free_name(&t.path().join("out/.rc")),
            t.path().join("out/.rc")
        );
    }

    #[test]
    fn a_missing_entry_is_an_error() {
        let (t, ctx, tar) = archive_fixture();
        let dest = t.path().join("out/nope.txt");
        assert!(write_promise(&ctx, &format!("{}/nope.txt", s(&tar)), &dest).is_err());
        assert!(dest.symlink_metadata().is_err());
    }

    #[test]
    fn a_server_without_connection_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let ctx = vfs::Ctx::new(t.path().join("tmp-root"));
        let dest = t.path().join("a.txt");
        assert!(write_promise(&ctx, "sftp://nowhere/a.txt", &dest).is_err());
        assert!(dest.symlink_metadata().is_err());
    }
}
