//! The Fn (globe) key as TC's Alt or Ctrl: WebKit never reports Fn to the page, so a native
//! event monitor watches the Fn key itself and tells every window (`keys://fn`, `{down}`)
//! before the key that comes with it. macOS sets the Fn ("function") flag on every arrow,
//! F-key and Home/End event too, so only the Fn key's own flag changes (key code 63) count.
// macOS only; the other systems use none of this.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde::Serialize;

/// `keys://fn` payload.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnState {
    pub down: bool,
}

/// Virtual key code of the Fn key (`kVK_Function`).
pub const FN_KEY_CODE: u16 = 63;

/// The Fn state after a flags-changed event, or None when the event is not about Fn: the Fn
/// key's own event, or any modifier change that shows Fn released while it was down.
pub fn fn_change(key_code: u16, function_flag: bool, was_down: bool) -> Option<bool> {
    if key_code == FN_KEY_CODE {
        return (function_flag != was_down).then_some(function_flag);
    }
    (was_down && !function_flag).then_some(false)
}

/// Installs the monitor (main thread, once, at startup).
#[cfg(target_os = "macos")]
pub fn start(app: tauri::AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Emitter;

    static DOWN: AtomicBool = AtomicBool::new(false);
    let block = RcBlock::new(move |ev: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for the duration of the call.
        let e = unsafe { ev.as_ref() };
        let flag = e.modifierFlags().contains(NSEventModifierFlags::Function);
        if let Some(down) = fn_change(e.keyCode(), flag, DOWN.load(Ordering::Relaxed)) {
            DOWN.store(down, Ordering::Relaxed);
            let _ = app.emit("keys://fn", FnState { down });
        }
        ev.as_ptr()
    });
    // SAFETY: the handler returns the event it got (a valid pointer).
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::FlagsChanged, &block)
    };
    // The monitor lives as long as the app.
    std::mem::forget(monitor);
}

#[cfg(not(target_os = "macos"))]
pub fn start(_app: tauri::AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_fn_key_itself_presses_fn() {
        assert_eq!(fn_change(FN_KEY_CODE, true, false), Some(true));
        assert_eq!(fn_change(FN_KEY_CODE, false, true), Some(false));
        assert_eq!(fn_change(FN_KEY_CODE, true, true), None);
        // Another modifier (Shift = 56) while an arrow-style function flag is set: no change.
        assert_eq!(fn_change(56, true, false), None);
        assert_eq!(fn_change(56, true, true), None);
        // A modifier change that shows Fn gone (released while another window had focus).
        assert_eq!(fn_change(56, false, true), Some(false));
        assert_eq!(fn_change(56, false, false), None);
    }
}
