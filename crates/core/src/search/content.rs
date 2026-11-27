//! Content search (Alt+F7 "Containing text"): does a file contain a piece of text?
//!
//! The file is read in blocks; each block overlaps the previous one by more than the text's
//! length, so a match across a block boundary is still found. Bytes are read as UTF-8 (invalid
//! sequences become U+FFFD, so binary files are searched too); UTF-16 files are not matched.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const BLOCK: usize = 256 * 1024;

/// The text to look for, folded once.
#[derive(Debug, Clone)]
pub struct Needle {
    text: String,
    case_sensitive: bool,
    /// Bytes kept from the end of one block for the next (covers any match and a split character).
    overlap: usize,
}

impl Needle {
    /// None for empty text.
    pub fn new(text: &str, case_sensitive: bool) -> Option<Needle> {
        if text.is_empty() {
            return None;
        }
        let text = if case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };
        // Lower-casing can change a character's length (up to 3x in UTF-8): stay generous.
        let overlap = text.len() * 3 + 4;
        Some(Needle {
            text,
            case_sensitive,
            overlap,
        })
    }

    fn found_in(&self, bytes: &[u8]) -> bool {
        let s: Cow<str> = String::from_utf8_lossy(bytes);
        if self.case_sensitive {
            s.contains(self.text.as_str())
        } else {
            s.to_lowercase().contains(self.text.as_str())
        }
    }
}

/// Whether the file at `path` contains `needle`. Stops early (false) when `cancel` is set.
pub fn file_contains(path: &Path, needle: &Needle, cancel: &AtomicBool) -> io::Result<bool> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; BLOCK + needle.overlap];
    let mut kept = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let n = match f.read(&mut buf[kept..]) {
            Ok(0) => return Ok(false),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let end = kept + n;
        if needle.found_in(&buf[..end]) {
            return Ok(true);
        }
        kept = needle.overlap.min(end);
        buf.copy_within(end - kept..end, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn has(content: &[u8], text: &str, case: bool) -> bool {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        fs::write(&p, content).unwrap();
        file_contains(
            &p,
            &Needle::new(text, case).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn finds_text_with_or_without_case() {
        assert!(has(b"Hello World", "world", false));
        assert!(!has(b"Hello World", "world", true));
        assert!(has(b"Hello World", "World", true));
        assert!(!has(b"Hello", "bye", false));
        assert!(has("Čevapi i ŠLJIVE".as_bytes(), "šljive", false));
        assert!(!has("Čevapi i ŠLJIVE".as_bytes(), "šljive", true));
        assert!(Needle::new("", false).is_none());
    }

    #[test]
    fn a_match_across_a_block_boundary_is_found() {
        for shift in [0usize, 1, 2, 3, 7] {
            let mut data = vec![b'x'; BLOCK - shift];
            data.extend_from_slice("Ključna riječ".as_bytes());
            data.extend(vec![b'y'; 1000]);
            assert!(has(&data, "ključna RIJEČ", false), "shift {shift}");
        }
        // Far past the first block, and with nothing there.
        let mut data = vec![b'a'; BLOCK * 3 + 17];
        data.extend_from_slice(b"needle");
        assert!(has(&data, "needle", true));
        assert!(!has(&vec![b'a'; BLOCK * 2], "needle", true));
    }

    #[test]
    fn binary_files_are_searched_and_cancel_stops() {
        let mut data = vec![0u8, 255, 254, 1, 2];
        data.extend_from_slice(b"PK marker");
        data.extend([0xC3u8, 0x28]);
        assert!(has(&data, "marker", false));
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("f");
        fs::write(&p, b"abc").unwrap();
        let n = Needle::new("abc", true).unwrap();
        assert!(!file_contains(&p, &n, &AtomicBool::new(true)).unwrap());
        assert!(file_contains(&t.path().join("missing"), &n, &AtomicBool::new(false)).is_err());
    }
}
