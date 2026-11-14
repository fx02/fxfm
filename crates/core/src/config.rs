//! User settings and session state, plus restoring panel locations on startup.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SETTINGS_FILE: &str = "settings.json";
pub const SESSION_FILE: &str = "session.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditorConfig {
    /// VS Code if `/Applications/Visual Studio Code.app` exists, else TextEdit (resolved elsewhere).
    System,
    App {
        path: String,
    },
    Terminal {
        command: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Colors {
    pub normal: String,
    pub hidden: String,
    pub archive: String,
    pub media: String,
    pub marked: String,
    pub background: String,
    pub cursor: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            normal: "#ffffff".into(),
            hidden: "#9a9a9a".into(),
            archive: "#ff6a4d".into(),
            media: "#8a8aff".into(),
            marked: "#ff4d4d".into(),
            background: "#000000".into(),
            cursor: "#555555".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub editor: EditorConfig,
    pub colors: Colors,
    pub show_command_line: bool,
    pub font_size: u32,
    pub fda_hint_shown: bool,
    /// The first-run warning about keys macOS takes for itself was shown.
    pub conflict_hint_shown: bool,
    /// Output device of the F3 audio player, by name; None = the system default.
    pub audio_output: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor: EditorConfig::System,
            colors: Colors::default(),
            show_command_line: true,
            font_size: 17,
            fda_hint_shown: false,
            conflict_hint_shown: false,
            audio_output: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    #[default]
    Left,
    Right,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub location: String,
    pub cursor_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionState {
    pub left: Option<PanelState>,
    pub right: Option<PanelState>,
    pub active: Side,
    pub window: Option<WindowGeom>,
}

/// Resolve a saved panel location to a directory that exists now.
/// Scheme locations (`sftp://...`) and paths through a non-archive file give `home`; a path
/// inside an archive gives the folder that contains the archive. Otherwise climb to the nearest existing directory, falling back to `home`.
pub fn restore_path(location: &str, home: &Path) -> PathBuf {
    if location.contains("://") {
        return home.to_path_buf();
    }
    for p in Path::new(location).ancestors() {
        if p.is_dir() {
            return p.to_path_buf();
        }
        if p.exists() {
            let is_archive = p.is_file()
                && p.file_name().is_some_and(|n| {
                    crate::archive::ArchiveKind::from_name(&n.to_string_lossy()).is_some()
                });
            if is_archive {
                if let Some(parent) = p.parent() {
                    return parent.to_path_buf();
                }
            }
            return home.to_path_buf();
        }
    }
    home.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_defaults() {
        let s = Settings::default();
        assert_eq!(s.font_size, 17);
        assert_eq!(s.colors.archive, "#ff6a4d");
        assert_eq!(s.editor, EditorConfig::System);
        assert!(s.show_command_line);
        assert!(!s.fda_hint_shown);
        assert!(!s.conflict_hint_shown);
    }

    #[test]
    fn restore_path_falls_back_to_nearest_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        let loc = tmp.path().join("a/b/c");
        let home = PathBuf::from("/home/x");
        assert_eq!(
            restore_path(loc.to_str().unwrap(), &home),
            tmp.path().join("a")
        );
    }

    #[test]
    fn restore_path_existing_dir_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let home = PathBuf::from("/home/x");
        assert_eq!(
            restore_path(tmp.path().to_str().unwrap(), &home),
            tmp.path()
        );
    }

    #[test]
    fn restore_path_scheme_goes_home() {
        let home = PathBuf::from("/home/x");
        assert_eq!(restore_path("sftp://web-1/var", &home), home);
    }

    #[test]
    fn restore_path_through_file_goes_home() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"x").unwrap();
        let loc = tmp.path().join("b.txt/x");
        let home = PathBuf::from("/home/x");
        assert_eq!(restore_path(loc.to_str().unwrap(), &home), home);
    }

    #[test]
    fn restore_path_archive_goes_to_containing_folder() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.zip"), b"x").unwrap();
        let home = PathBuf::from("/home/x");
        let loc = tmp.path().join("a.zip/src");
        assert_eq!(restore_path(loc.to_str().unwrap(), &home), tmp.path());
        let root = tmp.path().join("a.zip");
        assert_eq!(restore_path(root.to_str().unwrap(), &home), tmp.path());
    }

    #[test]
    fn restore_path_nothing_exists_climbs_to_existing_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = PathBuf::from("/home/x");
        let loc = tmp.path().join("none/x");
        assert_eq!(restore_path(loc.to_str().unwrap(), &home), tmp.path());
        let missing = format!("/nonexistent-{}/x", std::process::id());
        assert_eq!(restore_path(&missing, &home), PathBuf::from("/"));
    }

    #[test]
    fn session_roundtrip_json() {
        let s = SessionState {
            left: Some(PanelState {
                location: "/tmp".into(),
                cursor_name: Some("a.txt".into()),
            }),
            right: None,
            active: Side::Right,
            window: Some(WindowGeom {
                x: 1,
                y: 2,
                w: 800,
                h: 600,
            }),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"cursorName\""));
        assert!(json.contains("\"active\":\"right\""));
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn settings_json_shape_and_storage_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            editor: EditorConfig::Terminal {
                command: "vim".into(),
            },
            ..Settings::default()
        };
        crate::storage::save_json(dir.path(), SETTINGS_FILE, &s).unwrap();
        let json = serde_json::to_string(&s.editor).unwrap();
        assert_eq!(json, r#"{"kind":"terminal","command":"vim"}"#);
        let back: Settings = crate::storage::load_json(dir.path(), SETTINGS_FILE);
        assert_eq!(back, s);
    }

    #[test]
    fn audio_output_roundtrip_with_and_without_a_device() {
        assert_eq!(Settings::default().audio_output, None);
        for out in [None, Some("External Headphones".to_string())] {
            let s = Settings {
                audio_output: out.clone(),
                ..Settings::default()
            };
            let json = serde_json::to_value(&s).unwrap();
            assert_eq!(
                json["audioOutput"],
                out.as_deref().map_or(serde_json::Value::Null, Into::into)
            );
            let back: Settings = serde_json::from_value(json).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn old_settings_without_audio_output_load() {
        let old = r#"{"editor":{"kind":"system"},"showCommandLine":false,"fontSize":15}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.audio_output, None);
        assert_eq!(s.font_size, 15);
        assert!(!s.show_command_line);
    }
}
