//! Content detection and text decoding for the F3 viewer.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Maximum number of bytes read for text display.
pub const TEXT_LIMIT: u64 = 10 * 1024 * 1024;
/// Maximum number of bytes returned for hex display.
pub const HEX_LIMIT: usize = 1024 * 1024;
/// Prefix inspected for NUL bytes to detect binary content.
const SNIFF_LEN: usize = 8 * 1024;

/// What the viewer should show. Images and PDFs are loaded by the UI itself
/// (asset protocol), so no bytes are sent for them. `Hex.bytes` serializes as
/// a JSON number array (at most `HEX_LIMIT` bytes).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ViewContent {
    #[serde(rename_all = "camelCase")]
    Text {
        encoding: String,
        text: String,
        truncated: bool,
        /// Size of the file on disk in bytes.
        total: u64,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        mime: String,
    },
    Pdf,
    #[serde(rename_all = "camelCase")]
    Hex {
        bytes: Vec<u8>,
        offset: u64,
        total: u64,
    },
}

fn image_mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Decide how to present `path` and read what is needed for it.
pub fn read_for_view(path: &Path) -> io::Result<ViewContent> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if let Some(mime) = image_mime(&ext) {
        return Ok(ViewContent::Image {
            mime: mime.to_string(),
        });
    }
    if ext == "pdf" {
        return Ok(ViewContent::Pdf);
    }

    let (mut buf, total, truncated) = read_limited(path)?;

    let utf16_bom = buf.starts_with(&[0xFF, 0xFE]) || buf.starts_with(&[0xFE, 0xFF]);
    if !utf16_bom && buf[..buf.len().min(SNIFF_LEN)].contains(&0) {
        buf.truncate(HEX_LIMIT);
        return Ok(ViewContent::Hex {
            bytes: buf,
            offset: 0,
            total,
        });
    }

    trim_partial(
        &mut buf,
        truncated,
        if utf16_bom { Trim::Utf16 } else { Trim::Utf8 },
    );
    let (text, encoding) = decode_text(&buf);
    Ok(ViewContent::Text {
        encoding: encoding.to_string(),
        text,
        truncated,
        total,
    })
}

/// Read the first `TEXT_LIMIT` bytes of a regular file: (bytes, file size, truncated).
fn read_limited(path: &Path) -> io::Result<(Vec<u8>, u64, bool)> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut buf = Vec::new();
    File::open(path)?
        .take(TEXT_LIMIT + 1)
        .read_to_end(&mut buf)?;
    let truncated = buf.len() as u64 > TEXT_LIMIT;
    if truncated {
        buf.truncate(TEXT_LIMIT as usize);
    }
    Ok((buf, meta.len(), truncated))
}

/// How a cut-off buffer must be trimmed so no character is split.
enum Trim {
    None,
    Utf8,
    Utf16,
}

fn trim_partial(buf: &mut Vec<u8>, truncated: bool, trim: Trim) {
    match trim {
        Trim::Utf16 => buf.truncate(buf.len() & !1),
        Trim::Utf8 if truncated => {
            if let Err(e) = std::str::from_utf8(buf) {
                if e.error_len().is_none() {
                    buf.truncate(e.valid_up_to());
                }
            }
        }
        _ => {}
    }
}

/// Decode bytes to text: UTF-8 (BOM stripped), UTF-16 with BOM, else windows-1250.
pub fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "UTF-8");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (s, _) = encoding_rs::UTF_16LE.decode_without_bom_handling(rest);
        return (s.into_owned(), "UTF-16LE");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (s, _) = encoding_rs::UTF_16BE.decode_without_bom_handling(rest);
        return (s.into_owned(), "UTF-16BE");
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "UTF-8");
    }
    let (s, _) = encoding_rs::WINDOWS_1250.decode_without_bom_handling(bytes);
    (s.into_owned(), "windows-1250")
}

/// Decode `bytes` with an explicit encoding label (e.g. "UTF-8", "windows-1250",
/// "ISO-8859-2"). Unknown labels fall back to UTF-8; invalid bytes become U+FFFD.
pub fn decode_as(bytes: &[u8], label: &str) -> String {
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

/// Re-read `path` (first `TEXT_LIMIT` bytes) as text in the given encoding.
pub fn read_text_as(path: &Path, label: &str) -> io::Result<ViewContent> {
    let (mut buf, total, truncated) = read_limited(path)?;
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let trim = if enc == encoding_rs::UTF_8 {
        Trim::Utf8
    } else if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE {
        Trim::Utf16
    } else {
        Trim::None
    };
    trim_partial(&mut buf, truncated, trim);
    Ok(ViewContent::Text {
        encoding: enc.name().to_string(),
        text: decode_as(&buf, label),
        truncated,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn utf8_with_bom() {
        let (t, e) = decode_text(b"\xEF\xBB\xBFhello \xC5\xA1");
        assert_eq!((t.as_str(), e), ("hello š", "UTF-8"));
        let (t, e) = decode_text("plain č".as_bytes());
        assert_eq!((t.as_str(), e), ("plain č", "UTF-8"));
    }

    #[test]
    fn utf16_boms() {
        let (t, e) = decode_text(&[0xFF, 0xFE, b'h', 0, 0x61, 0x01]);
        assert_eq!((t.as_str(), e), ("hš", "UTF-16LE"));
        let (t, e) = decode_text(&[0xFE, 0xFF, 0, b'h', 0x01, 0x61]);
        assert_eq!((t.as_str(), e), ("hš", "UTF-16BE"));
    }

    #[test]
    fn cp1250_croatian() {
        let (t, e) = decode_text(&[0x9A, 0xE6, 0xE8, 0x9E, 0xF0]);
        assert_eq!((t.as_str(), e), ("šćčžđ", "windows-1250"));
    }

    #[test]
    fn binary_goes_hex() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.bin");
        fs::write(&p, [1u8, 2, 0, 3]).unwrap();
        assert_eq!(
            read_for_view(&p).unwrap(),
            ViewContent::Hex {
                bytes: vec![1, 2, 0, 3],
                offset: 0,
                total: 4
            }
        );
    }

    #[test]
    fn hex_is_capped() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.bin");
        fs::write(&p, vec![0u8; HEX_LIMIT + 5]).unwrap();
        match read_for_view(&p).unwrap() {
            ViewContent::Hex { bytes, total, .. } => {
                assert_eq!(bytes.len(), HEX_LIMIT);
                assert_eq!(total, (HEX_LIMIT + 5) as u64);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn truncates_large_text() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.txt");
        fs::write(&p, vec![b'a'; TEXT_LIMIT as usize + 10]).unwrap();
        match read_for_view(&p).unwrap() {
            ViewContent::Text {
                truncated, text, ..
            } => {
                assert!(truncated);
                assert_eq!(text.len() as u64, TEXT_LIMIT);
            }
            other => panic!("{other:?}"),
        }
        let q = d.path().join("small.txt");
        fs::write(&q, "hi").unwrap();
        assert_eq!(
            read_for_view(&q).unwrap(),
            ViewContent::Text {
                encoding: "UTF-8".into(),
                text: "hi".into(),
                truncated: false,
                total: 2
            }
        );
    }

    #[test]
    fn truncation_does_not_split_utf8_char() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.txt");
        let mut data = vec![b'a'; TEXT_LIMIT as usize - 1];
        data.extend("š".as_bytes()); // straddles the limit
        data.extend(b"bbbb");
        fs::write(&p, data).unwrap();
        match read_for_view(&p).unwrap() {
            ViewContent::Text {
                encoding,
                truncated,
                ..
            } => {
                assert!(truncated);
                assert_eq!(encoding, "UTF-8");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn image_and_pdf_by_extension() {
        let p = Path::new("/nonexistent/x.PNG");
        assert_eq!(
            read_for_view(p).unwrap(),
            ViewContent::Image {
                mime: "image/png".into()
            }
        );
        let p = Path::new("/nonexistent/x.jpg");
        assert_eq!(
            read_for_view(p).unwrap(),
            ViewContent::Image {
                mime: "image/jpeg".into()
            }
        );
        let p = Path::new("/nonexistent/x.svg");
        assert_eq!(
            read_for_view(p).unwrap(),
            ViewContent::Image {
                mime: "image/svg+xml".into()
            }
        );
        let p = Path::new("/nonexistent/doc.Pdf");
        assert_eq!(read_for_view(p).unwrap(), ViewContent::Pdf);
    }

    #[test]
    fn utf16_file_is_text_not_hex() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u16.txt");
        fs::write(&p, [0xFF, 0xFE, b'h', 0, b'i', 0]).unwrap();
        assert_eq!(
            read_for_view(&p).unwrap(),
            ViewContent::Text {
                encoding: "UTF-16LE".into(),
                text: "hi".into(),
                truncated: false,
                total: 6
            }
        );
        let q = d.path().join("big16.txt");
        let mut data = vec![0xFF, 0xFE];
        data.extend([b'a', 0].repeat(TEXT_LIMIT as usize / 2));
        fs::write(&q, data).unwrap();
        match read_for_view(&q).unwrap() {
            ViewContent::Text {
                text, truncated, ..
            } => {
                assert!(truncated);
                assert!(!text.contains('\u{FFFD}'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn non_regular_files_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let fifo = d.path().join("f");
        let st = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(st.success());
        let e = read_for_view(&fifo).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(read_for_view(d.path()).is_err());
    }

    #[test]
    fn serde_shape() {
        let v = serde_json::to_value(ViewContent::Pdf).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "pdf"}));
        let v = serde_json::to_value(ViewContent::Hex {
            bytes: vec![1],
            offset: 0,
            total: 1,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"hex","bytes":[1],"offset":0,"total":1})
        );
    }

    #[test]
    fn decode_as_iso_8859_2() {
        assert_eq!(decode_as(&[0xB9], "ISO-8859-2"), "š");
        assert_eq!(decode_as(&[0x9A], "windows-1250"), "š");
        assert_eq!(decode_as("š".as_bytes(), "UTF-8"), "š");
    }

    #[test]
    fn read_text_as_uses_label() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        fs::write(&p, [0xB9]).unwrap();
        assert_eq!(
            read_text_as(&p, "ISO-8859-2").unwrap(),
            ViewContent::Text {
                encoding: "ISO-8859-2".into(),
                text: "š".into(),
                truncated: false,
                total: 1
            }
        );
    }

    #[test]
    fn redecode_truncated_file_has_no_replacement_char() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("u.txt");
        let mut data = vec![b'a'; TEXT_LIMIT as usize - 1];
        data.extend("š".as_bytes()); // straddles the limit
        data.extend(b"bbbb");
        fs::write(&p, &data).unwrap();
        match read_text_as(&p, "UTF-8").unwrap() {
            ViewContent::Text {
                text,
                truncated,
                total,
                ..
            } => {
                assert!(truncated);
                assert!(!text.contains('\u{FFFD}'));
                assert_eq!(total, data.len() as u64);
            }
            other => panic!("{other:?}"),
        }
        let q = d.path().join("odd16.txt");
        fs::write(&q, [b'h', 0, b'i']).unwrap(); // dangling half code unit
        match read_text_as(&q, "UTF-16LE").unwrap() {
            ViewContent::Text { text, .. } => assert!(!text.contains('\u{FFFD}')),
            other => panic!("{other:?}"),
        }
    }
}
