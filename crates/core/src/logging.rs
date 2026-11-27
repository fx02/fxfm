//! fxfm's log: warnings and errors (and fxfm's own info lines) go to `fxfm.log` in the log
//! folder and to stderr. Past 2 MB the file becomes `fxfm.old.log` and a new one starts, so the
//! log never takes more than about 4 MB. A log line never holds a password: the call sites log
//! paths and error texts only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

const MAX_BYTES: u64 = 2 * 1024 * 1024;

pub struct FileLog {
    path: PathBuf,
    old: PathBuf,
    /// The open file and how many bytes it holds.
    file: Mutex<Option<(File, u64)>>,
    stderr: bool,
}

impl FileLog {
    /// A log writing `dir/fxfm.log` (the folder is created when needed).
    pub fn new(dir: &Path, stderr: bool) -> FileLog {
        FileLog {
            path: dir.join("fxfm.log"),
            old: dir.join("fxfm.old.log"),
            file: Mutex::new(None),
            stderr,
        }
    }

    fn open(&self) -> io::Result<(File, u64)> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let len = f.metadata()?.len();
        Ok((f, len))
    }

    fn write_line(&self, line: &str) {
        let mut g = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_none_or(|(_, len)| *len >= MAX_BYTES) {
            if g.take().is_some_and(|(_, len)| len >= MAX_BYTES)
                || fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES)
            {
                let _ = fs::rename(&self.path, &self.old);
            }
            *g = self.open().ok();
        }
        if let Some((f, len)) = g.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                *len += line.len() as u64;
            }
        }
    }
}

/// `2026-11-27 21:40:05` in local time.
fn now_local() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    // SAFETY: localtime_r fills `tm` from `t`; both are valid for the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return t.to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

impl Log for FileLog {
    /// Warnings and errors from anything; info lines only from fxfm itself.
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= Level::Warn || (m.level() <= Level::Info && m.target().starts_with("fxfm"))
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!(
            "{} {} {}: {}\n",
            now_local(),
            r.level(),
            r.target(),
            r.args()
        );
        if self.stderr {
            eprint!("{line}");
        }
        self.write_line(&line);
    }

    fn flush(&self) {
        if let Some((f, _)) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.flush();
        }
    }
}

/// Installs the log for the whole app (once; a second call does nothing).
pub fn init(dir: &Path) {
    let logger: &'static FileLog = Box::leak(Box::new(FileLog::new(dir, true)));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Info);
        log::info!(target: "fxfm", "fxfm {} started", env!("CARGO_PKG_VERSION"));
    }
}

/// The log folder: `~/Library/Logs/fxfm`.
pub fn default_dir(home: &Path) -> PathBuf {
    home.join("Library/Logs/fxfm")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(log: &FileLog, level: Level, target: &str, msg: &str) {
        log.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{msg}"))
                .build(),
        );
    }

    #[test]
    fn writes_warnings_and_fxfm_info_only() {
        let t = tempfile::tempdir().unwrap();
        let log = FileLog::new(&t.path().join("logs"), false);
        record(
            &log,
            Level::Warn,
            "fxfm_core::ops",
            "kept: it changed after it was copied",
        );
        record(&log, Level::Info, "fxfm_core::search", "index saved");
        record(&log, Level::Info, "tao::platform", "noise");
        record(&log, Level::Debug, "fxfm_core::ops", "detail");
        record(&log, Level::Error, "zip", "bad archive");
        let text = fs::read_to_string(t.path().join("logs/fxfm.log")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with(" WARN fxfm_core::ops: kept: it changed after it was copied"));
        assert!(lines[1].ends_with(" INFO fxfm_core::search: index saved"));
        assert!(lines[2].ends_with(" ERROR zip: bad archive"));
        // `YYYY-MM-DD HH:MM:SS ` in front.
        assert_eq!(lines[0].as_bytes()[4], b'-');
        assert_eq!(lines[0].as_bytes()[19], b' ');
    }

    #[test]
    fn a_full_log_becomes_the_old_one() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("fxfm.log"), vec![b'x'; MAX_BYTES as usize]).unwrap();
        fs::write(t.path().join("fxfm.old.log"), b"older").unwrap();
        let log = FileLog::new(t.path(), false);
        record(&log, Level::Warn, "fxfm", "fresh");
        assert_eq!(
            fs::metadata(t.path().join("fxfm.old.log")).unwrap().len(),
            MAX_BYTES
        );
        let now = fs::read_to_string(t.path().join("fxfm.log")).unwrap();
        assert!(now.ends_with("WARN fxfm: fresh\n"), "{now}");
        // Growing past the limit while running rotates again.
        let big = "y".repeat(MAX_BYTES as usize);
        record(&log, Level::Warn, "fxfm", &big);
        record(&log, Level::Warn, "fxfm", "after");
        let now = fs::read_to_string(t.path().join("fxfm.log")).unwrap();
        assert!(
            now.ends_with("WARN fxfm: after\n") && now.len() < 100,
            "{}",
            now.len()
        );
    }
}
