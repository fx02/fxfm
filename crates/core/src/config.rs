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

/// One "Define colors by file type" rule: TC masks (`;`- or space-separated) and a CSS color.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TypeRule {
    pub masks: String,
    pub color: String,
}

/// Panel colors (TC Configuration → Color). Saved in camelCase. Loading is lenient: a missing
/// or unusable field gets its default, and files from before the file-type rules migrate
/// (`normal` → `font`, `marked` → `mark`, `archive`/`media` → the matching default rules).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Colors {
    /// Text color of entries no rule colors.
    pub font: String,
    pub background: String,
    /// Background of every second row (same as `background`: no striping).
    pub background2: String,
    /// Text color of marked entries.
    pub mark: String,
    /// Cursor bar (inverted cursor) or cursor frame color.
    pub cursor: String,
    /// Text color on the cursor bar; empty = the entry keeps its own color.
    pub cursor_font: String,
    /// True: the cursor is a filled bar; false: only a frame in `cursor` color.
    pub inverted_cursor: bool,
    /// Hidden entries no file-type rule matches.
    pub hidden: String,
    /// "Define colors by file type" is on.
    pub by_type: bool,
    /// Ordered; the first matching rule wins.
    pub type_rules: Vec<TypeRule>,
}

/// Default file-type rules: text/documents, executables, images, archives, video, audio.
const DEFAULT_TYPE_RULES: [(&str, &str); 6] = [
    (
        "*.log;*.txt;*.ini;*.cfg;*.conf;*.md;*.rtf;*.pdf;*.doc;*.docx;*.xls;*.xlsx;*.ppt;*.pptx;*.odt;*.ods;*.csv;*.json;*.xml;*.yaml;*.yml;*.htm;*.html;*.srt;*.sub",
        "#c8c8c8",
    ),
    (
        "*.app;*.command;*.sh;*.pkg;*.exe;*.bat;*.msi;*.com;*.jar;*.torrent",
        "#5fd35f",
    ),
    (
        "*.jpg;*.jpeg;*.gif;*.tif;*.tiff;*.png;*.heic;*.webp;*.svg;*.psd;*.bmp;*.ico;*.icns;*.raw;*.cr2;*.nef",
        "#ffff4d",
    ),
    (
        "*.zip;*.rar;*.7z;*.gz;*.tgz;*.tar;*.bz2;*.xz;*.zst;*.iso;*.dmg;*.cab;*.arj;*.lha;*.img;*.bin;*.cue",
        "#ff6a4d",
    ),
    (
        "*.avi;*.mpg;*.mpeg;*.mp4;*.m4v;*.mkv;*.mov;*.wmv;*.flv;*.webm;*.ts;*.mts;*.vob;*.3gp",
        "#8a8aff",
    ),
    (
        "*.mp3;*.wav;*.flac;*.ogg;*.oga;*.opus;*.m4a;*.aac;*.aif;*.aiff;*.wma;*.ac3;*.au;*.gsm",
        "#8a8aff",
    ),
];
/// Indexes into `DEFAULT_TYPE_RULES` the old `archive` and `media` colors migrate to.
const ARCHIVE_RULE: usize = 3;
const MEDIA_RULES: [usize; 2] = [4, 5];
/// Defaults of the old fields; a saved old default is not a custom value and is not migrated.
const OLD_MARKED: &str = "#ff4d4d";
const OLD_ARCHIVE: &str = "#ff6a4d";
const OLD_MEDIA: &str = "#8a8aff";

impl Default for Colors {
    fn default() -> Self {
        Self {
            font: "#ffffff".into(),
            background: "#000000".into(),
            background2: "#000000".into(),
            mark: "#ff3b30".into(),
            cursor: "#555555".into(),
            cursor_font: "#ffff4d".into(),
            inverted_cursor: true,
            hidden: "#9a9a9a".into(),
            by_type: true,
            type_rules: DEFAULT_TYPE_RULES
                .iter()
                .map(|(masks, color)| TypeRule {
                    masks: (*masks).into(),
                    color: (*color).into(),
                })
                .collect(),
        }
    }
}

impl<'de> Deserialize<'de> for Colors {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        let text = |name: &str| raw.get(name).and_then(|v| v.as_str()).map(str::to_owned);
        let flag = |name: &str| raw.get(name).and_then(|v| v.as_bool());
        // An old value counts only when it differs from that field's old default.
        let custom = |name: &str, old_default: &str| {
            text(name).filter(|v| !v.eq_ignore_ascii_case(old_default))
        };
        let mut c = Colors::default();
        if let Some(v) = text("font").or_else(|| text("normal")) {
            c.font = v;
        }
        if let Some(v) = text("mark").or_else(|| custom("marked", OLD_MARKED)) {
            c.mark = v;
        }
        for (field, name) in [
            (&mut c.background, "background"),
            (&mut c.background2, "background2"),
            (&mut c.cursor, "cursor"),
            (&mut c.cursor_font, "cursorFont"),
            (&mut c.hidden, "hidden"),
        ] {
            if let Some(v) = text(name) {
                *field = v;
            }
        }
        if let Some(v) = flag("invertedCursor") {
            c.inverted_cursor = v;
        }
        if let Some(v) = flag("byType") {
            c.by_type = v;
        }
        match raw.get("typeRules").and_then(|v| v.as_array()) {
            Some(rules) => {
                c.type_rules = rules
                    .iter()
                    .filter_map(|r| serde_json::from_value(r.clone()).ok())
                    .collect();
            }
            None => {
                if let Some(v) = custom("archive", OLD_ARCHIVE) {
                    c.type_rules[ARCHIVE_RULE].color = v;
                }
                if let Some(v) = custom("media", OLD_MEDIA) {
                    for i in MEDIA_RULES {
                        c.type_rules[i].color = v.clone();
                    }
                }
            }
        }
        Ok(c)
    }
}

/// A physical Mac modifier key that can act as TC's Alt or Ctrl. WebKit does not report Fn to
/// the page (`getModifierState("Fn")` is always false), so the app tracks the Fn key itself
/// with a native event monitor and tells the windows (`keys://fn`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MacKey {
    Option,
    Control,
    Command,
    Fn,
}

/// Which Mac keys act as TC's Alt and Ctrl. Saved as `{"alt": "option", "ctrl": "control"}`;
/// a missing, unknown or doubled key gives the defaults (Option = Alt, Control = Ctrl).
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: MacKey,
    pub ctrl: MacKey,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            alt: MacKey::Option,
            ctrl: MacKey::Control,
        }
    }
}

impl<'de> Deserialize<'de> for Modifiers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        let key = |name: &str| {
            raw.get(name)
                .and_then(|v| serde_json::from_value::<MacKey>(v.clone()).ok())
        };
        let def = Modifiers::default();
        let m = Modifiers {
            alt: key("alt").unwrap_or(def.alt),
            ctrl: key("ctrl").unwrap_or(def.ctrl),
        };
        Ok(if m.alt == m.ctrl { def } else { m })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub editor: EditorConfig,
    pub colors: Colors,
    pub show_command_line: bool,
    /// The drive button bar above each panel (the drive dropdown stays). Off by default.
    pub show_drive_bar: bool,
    /// Each panel's header: drive dropdown, free space and the path line.
    pub show_panel_header: bool,
    /// Each panel's status line (the quick search box still shows while typing).
    pub show_panel_footer: bool,
    /// The F-key bar at the bottom of the window.
    pub show_fkey_bar: bool,
    pub font_size: u32,
    pub fda_hint_shown: bool,
    /// The first-run warning about keys macOS takes for itself was shown.
    pub conflict_hint_shown: bool,
    /// Output device of the F3 audio player, by name; None = the system default.
    pub audio_output: Option<String>,
    /// F3 player volume, 0.0 to 1.0.
    pub audio_volume: f64,
    /// Multi-rename presets by name (the dialog's options, as the UI keeps them).
    pub rename_presets: std::collections::BTreeMap<String, serde_json::Value>,
    /// Which Mac keys act as TC's Alt and Ctrl.
    pub modifiers: Modifiers,
    /// Command ids that existed when `keymap.json` was last saved or migrated; commands not
    /// in it get their free default keys at startup. None: a keymap from before this field.
    pub keymap_commands: Option<Vec<String>>,
    /// Search (Alt+F7) index settings.
    pub search: SearchSettings,
    /// Which keys type into the panel quick search (TC Alt+letters): "command",
    /// "controlCommand", "option", "letters" or "off". The UI treats unknown values as
    /// "command".
    pub quick_search: String,
}

/// A volume the user added to the search index.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchVolume {
    pub path: String,
    pub enabled: bool,
}

impl Default for SearchVolume {
    /// A listed volume is meant to be indexed.
    fn default() -> Self {
        Self {
            path: String::new(),
            enabled: true,
        }
    }
}

/// Search index settings. The startup disk is always indexed when `index == Some(true)`
/// and is not listed in `volumes`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchSettings {
    /// None = never asked (the first Alt+F7 shows the prompt); Some(true) = build and keep
    /// current; Some(false) = off.
    pub index: Option<bool>,
    /// Other local and network volumes the user enabled.
    pub volumes: Vec<SearchVolume>,
    /// User exclusions (absolute paths), on top of the default excludes.
    pub exclude: Vec<String>,
    pub archives: bool,
    pub gz_limit_mb: u64,
    pub network_archives: bool,
    /// 0 = manual only.
    pub network_refresh_hours: u32,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            index: None,
            volumes: Vec::new(),
            exclude: Vec::new(),
            archives: false,
            gz_limit_mb: 500,
            network_archives: false,
            network_refresh_hours: 24,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor: EditorConfig::System,
            colors: Colors::default(),
            show_command_line: true,
            show_drive_bar: false,
            show_panel_header: true,
            show_panel_footer: true,
            show_fkey_bar: true,
            font_size: 17,
            fda_hint_shown: false,
            conflict_hint_shown: false,
            audio_output: None,
            audio_volume: 1.0,
            rename_presets: Default::default(),
            modifiers: Modifiers::default(),
            keymap_commands: None,
            search: SearchSettings::default(),
            quick_search: "command".into(),
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
    /// Hidden files shown (toggled with the hidden-files command); None: a session from
    /// before this field, which means shown.
    pub show_hidden: Option<bool>,
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
    fn search_defaults() {
        let s = SearchSettings::default();
        assert_eq!(s.index, None);
        assert!(!s.archives);
        assert_eq!(s.gz_limit_mb, 500);
        assert!(!s.network_archives);
        assert_eq!(s.network_refresh_hours, 24);
    }

    #[test]
    fn old_settings_without_search_load() {
        let s: Settings = serde_json::from_str(r#"{"fontSize": 14}"#).unwrap();
        assert_eq!(s.search, SearchSettings::default());
        let p: Settings = serde_json::from_str(r#"{"search": {"index": true}}"#).unwrap();
        assert_eq!(p.search.index, Some(true));
        assert_eq!(p.search.gz_limit_mb, 500);
    }

    #[test]
    fn quick_search_key_defaults_to_command_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"fontSize": 14}"#).unwrap();
        assert_eq!(old.quick_search, "command");
        let s: Settings = serde_json::from_str(r#"{"quickSearch": "off"}"#).unwrap();
        assert_eq!(s.quick_search, "off");
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["quickSearch"], "off");
    }

    #[test]
    fn search_volume_without_enabled_loads() {
        let p: Settings = serde_json::from_str(
            r#"{"search": {"index": true, "volumes": [{"path": "/Volumes/M"}]}}"#,
        )
        .unwrap();
        assert_eq!(p.search.index, Some(true));
        assert_eq!(
            p.search.volumes,
            vec![SearchVolume {
                path: "/Volumes/M".into(),
                enabled: true
            }]
        );
    }

    #[test]
    fn settings_defaults() {
        let s = Settings::default();
        assert_eq!(s.font_size, 17);
        assert_eq!(s.colors.font, "#ffffff");
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
    fn session_keeps_hidden_files_choice() {
        let old: SessionState = serde_json::from_str(r#"{"active": "left"}"#).unwrap();
        assert_eq!(old.show_hidden, None);
        let s: SessionState = serde_json::from_str(r#"{"showHidden": false}"#).unwrap();
        assert_eq!(s.show_hidden, Some(false));
        assert_eq!(serde_json::to_value(&s).unwrap()["showHidden"], false);
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
            show_hidden: Some(false),
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
        assert!(!s.show_drive_bar);
        assert!(s.show_panel_header && s.show_panel_footer && s.show_fkey_bar);
    }

    #[test]
    fn old_settings_without_modifiers_load_with_defaults() {
        let old = r#"{"editor":{"kind":"system"},"fontSize":15}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.modifiers, Modifiers::default());
        assert_eq!(
            (s.modifiers.alt, s.modifiers.ctrl),
            (MacKey::Option, MacKey::Control)
        );
        assert_eq!(s.font_size, 15);
    }

    #[test]
    fn modifiers_json_shape_and_fallbacks() {
        let s = Settings {
            modifiers: Modifiers {
                alt: MacKey::Option,
                ctrl: MacKey::Command,
            },
            ..Settings::default()
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json["modifiers"],
            serde_json::json!({"alt": "option", "ctrl": "command"})
        );
        let back: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(back, s);
        let parse = |m: &str| -> Modifiers {
            serde_json::from_str::<Settings>(&format!(r#"{{"modifiers":{m},"fontSize":14}}"#))
                .unwrap()
                .modifiers
        };
        // Fn is a key; unknown and doubled keys fall back, the rest survives.
        assert_eq!(
            parse(r#"{"alt":"fn","ctrl":"command"}"#),
            Modifiers {
                alt: MacKey::Fn,
                ctrl: MacKey::Command
            }
        );
        assert_eq!(
            parse(r#"{"alt":"hyper","ctrl":"command"}"#).alt,
            MacKey::Option
        );
        assert_eq!(
            parse(r#"{"alt":"command","ctrl":"command"}"#),
            Modifiers::default()
        );
        assert_eq!(parse(r#"{"ctrl":"option"}"#), Modifiers::default());
        assert_eq!(parse("7"), Modifiers::default());
    }
    #[test]
    fn default_colors_have_six_type_rules_in_order() {
        let c = Colors::default();
        let colors: Vec<&str> = c.type_rules.iter().map(|r| r.color.as_str()).collect();
        assert_eq!(
            colors,
            ["#c8c8c8", "#5fd35f", "#ffff4d", "#ff6a4d", "#8a8aff", "#8a8aff"]
        );
        assert!(c.type_rules[0].masks.starts_with("*.log;*.txt;"));
        assert!(c.type_rules[1].masks.starts_with("*.app;"));
        assert!(c.type_rules[2].masks.starts_with("*.jpg;"));
        assert!(c.type_rules[3].masks.starts_with("*.zip;"));
        assert!(c.type_rules[4].masks.starts_with("*.avi;"));
        assert!(c.type_rules[5].masks.starts_with("*.mp3;"));
        assert_eq!(
            (
                c.font.as_str(),
                c.background.as_str(),
                c.background2.as_str()
            ),
            ("#ffffff", "#000000", "#000000")
        );
        assert_eq!(
            (c.mark.as_str(), c.cursor.as_str(), c.cursor_font.as_str()),
            ("#ff3b30", "#555555", "#ffff4d")
        );
        assert_eq!(c.hidden, "#9a9a9a");
        assert!(c.inverted_cursor && c.by_type);
    }

    /// The UI tests read the defaults from this fixture; it must match the Rust defaults.
    #[test]
    fn default_colors_match_the_ui_fixture() {
        let text = include_str!("../../../tests/fixtures/default_colors.json");
        let fixture: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_value(Colors::default()).unwrap(), fixture);
    }

    #[test]
    fn old_colors_migrate_custom_values() {
        let old = r##"{"fontSize":15,"colors":{"normal":"#eeeeee","hidden":"#777777",
            "archive":"#ff0000","media":"#00ff00","marked":"#ff00ff",
            "background":"#101010","cursor":"#333333"}}"##;
        let c = serde_json::from_str::<Settings>(old).unwrap().colors;
        assert_eq!(c.font, "#eeeeee");
        assert_eq!(c.mark, "#ff00ff");
        assert_eq!(c.hidden, "#777777");
        assert_eq!(c.background, "#101010");
        assert_eq!(c.cursor, "#333333");
        let d = Colors::default();
        assert_eq!(c.background2, d.background2);
        assert_eq!(c.cursor_font, d.cursor_font);
        assert!(c.inverted_cursor && c.by_type);
        let colors: Vec<&str> = c.type_rules.iter().map(|r| r.color.as_str()).collect();
        assert_eq!(
            colors,
            ["#c8c8c8", "#5fd35f", "#ffff4d", "#ff0000", "#00ff00", "#00ff00"]
        );
        for (a, b) in c.type_rules.iter().zip(&d.type_rules) {
            assert_eq!(a.masks, b.masks);
        }
    }

    #[test]
    fn old_colors_with_old_defaults_get_the_new_defaults() {
        let old = r##"{"colors":{"normal":"#ffffff","hidden":"#9a9a9a","archive":"#FF6A4D",
            "media":"#8a8aff","marked":"#ff4d4d","background":"#000000","cursor":"#555555"}}"##;
        let c = serde_json::from_str::<Settings>(old).unwrap().colors;
        assert_eq!(c, Colors::default());
    }

    #[test]
    fn new_colors_roundtrip_and_ignore_old_fields() {
        let c = Colors {
            font: "#010101".into(),
            background: "#020202".into(),
            background2: "#030303".into(),
            mark: "#040404".into(),
            cursor: "#050505".into(),
            cursor_font: String::new(),
            inverted_cursor: false,
            hidden: "#060606".into(),
            by_type: false,
            type_rules: vec![TypeRule {
                masks: "www*".into(),
                color: "#070707".into(),
            }],
        };
        let s = Settings {
            colors: c.clone(),
            ..Settings::default()
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["colors"]["cursorFont"], "");
        assert_eq!(json["colors"]["invertedCursor"], false);
        assert_eq!(json["colors"]["typeRules"][0]["masks"], "www*");
        let back: Settings = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, s);
        // An explicit rule list wins over stray old fields.
        let mut j = json;
        j["colors"]["archive"] = "#ff0000".into();
        let back: Settings = serde_json::from_value(j).unwrap();
        assert_eq!(back.colors, c);
    }

    #[test]
    fn bad_color_fields_fall_back_without_losing_the_settings() {
        let text = r##"{"fontSize":14,"colors":{"font":7,"mark":"#111111","invertedCursor":"no",
            "typeRules":[{"masks":"*.a","color":"#222222"},{"masks":3},"x"]}}"##;
        let s: Settings = serde_json::from_str(text).unwrap();
        assert_eq!(s.font_size, 14);
        let d = Colors::default();
        assert_eq!(s.colors.font, d.font);
        assert_eq!(s.colors.mark, "#111111");
        assert!(s.colors.inverted_cursor);
        assert_eq!(
            s.colors.type_rules,
            vec![TypeRule {
                masks: "*.a".into(),
                color: "#222222".into()
            }]
        );
        let s: Settings = serde_json::from_str(r#"{"colors":"red"}"#).unwrap();
        assert_eq!(s.colors, d);
    }
}
