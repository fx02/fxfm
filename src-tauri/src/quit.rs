//! Quitting with file operations running: Cmd+Q, Exit and the red button ask the window first
//! (`app://quit-asked`), which confirms and calls `quit_app`. Without running operations, or
//! on a second request within a few seconds (a hung window can never keep fxfm open), fxfm
//! quits at once. Quitting cancels the operations (`Jobs::cancel_all` on exit).

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::state::AppState;

/// When the window was last asked to confirm.
static ASKED: Mutex<Option<Instant>> = Mutex::new(None);

const AGAIN: Duration = Duration::from_secs(5);

/// Whether a quit request should ask first: operations are running and no request was asked
/// about in the last `AGAIN`. Records the ask.
fn should_ask(running: usize, now: Instant) -> bool {
    let mut last = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    if running == 0 || last.is_some_and(|t| now.duration_since(t) < AGAIN) {
        return false;
    }
    *last = Some(now);
    true
}

pub fn request<R: Runtime>(app: &AppHandle<R>) {
    let running = app.state::<AppState>().jobs.count();
    if should_ask(running, Instant::now())
        && app
            .emit_to(
                "main",
                "app://quit-asked",
                serde_json::json!({ "running": running }),
            )
            .is_ok()
    {
        return;
    }
    app.exit(0);
}

/// The window confirmed: quit (the running operations are cancelled on the way out).
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_only_with_jobs_and_not_twice_in_a_row() {
        let t = Instant::now();
        assert!(!should_ask(0, t));
        assert!(should_ask(2, t));
        assert!(
            !should_ask(2, t + Duration::from_secs(1)),
            "second request quits"
        );
        assert!(should_ask(1, t + AGAIN + Duration::from_secs(1)));
    }
}
