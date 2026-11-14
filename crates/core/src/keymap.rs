//! Command registry and Total-Commander-style keymap.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

pub const KEYMAP_FILE: &str = "keymap.json";

/// Combos macOS reserves for itself (Mission Control, Spotlight, ...).
pub const SYSTEM_RESERVED: &[&str] = &[
    "Ctrl+Up",
    "Ctrl+Down",
    "Ctrl+Left",
    "Ctrl+Right",
    "Ctrl+Space",
    "Ctrl+Alt+Space",
];

const NAMED_KEYS: &[&str] = &[
    "Tab",
    "Enter",
    "Backspace",
    "Delete",
    "Insert",
    "Space",
    "Escape",
    "Up",
    "Down",
    "Left",
    "Right",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "NumAdd",
    "NumSubtract",
    "NumMultiply",
    "=",
    "-",
    "\\",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid key combo: {0:?}")]
pub struct ParseComboError(String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub key: String,
}

fn canonical_key(name: &str) -> Option<String> {
    if let Some(digits) = name.strip_prefix(['F', 'f']) {
        let n = digits.parse::<u8>().ok().filter(|n| (1..=12).contains(n));
        // Plain decimal only: "F05" is not a key name.
        if let Some(n) = n.filter(|n| n.to_string() == digits) {
            return Some(format!("F{n}"));
        }
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase().to_string());
        }
    }
    NAMED_KEYS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(name))
        .map(|k| k.to_string())
}

impl FromStr for KeyCombo {
    type Err = ParseComboError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseComboError(s.to_string());
        let mut parts: Vec<&str> = s.split('+').collect();
        let key = canonical_key(parts.pop().ok_or_else(err)?).ok_or_else(err)?;
        let mut c = KeyCombo {
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            key,
        };
        for m in parts {
            let flag = match m.to_ascii_lowercase().as_str() {
                "ctrl" => &mut c.ctrl,
                "alt" | "option" => &mut c.alt,
                "shift" => &mut c.shift,
                "cmd" => &mut c.cmd,
                _ => return Err(err()),
            };
            *flag = true;
        }
        Ok(c)
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.cmd, "Cmd+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(&self.key)
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandDef {
    pub id: &'static str,
    pub menu: &'static str,
    pub title: &'static str,
    pub defaults: &'static [&'static str],
    pub available: bool,
}

pub static COMMANDS: &[CommandDef] = &[
    CommandDef {
        id: "cm_view",
        menu: "Files",
        title: "View",
        defaults: &["F3"],
        available: true,
    },
    CommandDef {
        id: "cm_edit",
        menu: "Files",
        title: "Edit",
        defaults: &["F4"],
        available: true,
    },
    CommandDef {
        id: "cm_edit_new",
        menu: "Files",
        title: "Edit new file",
        defaults: &["Shift+F4"],
        available: true,
    },
    CommandDef {
        id: "cm_copy",
        menu: "Files",
        title: "Copy",
        defaults: &["F5"],
        available: true,
    },
    CommandDef {
        id: "cm_move",
        menu: "Files",
        title: "Move",
        defaults: &["F6"],
        available: true,
    },
    CommandDef {
        id: "cm_rename",
        menu: "Files",
        title: "Rename",
        defaults: &["F2", "Shift+F6"],
        available: true,
    },
    CommandDef {
        id: "cm_mkdir",
        menu: "Files",
        title: "New folder",
        defaults: &["F7"],
        available: true,
    },
    CommandDef {
        id: "cm_delete",
        menu: "Files",
        title: "Delete",
        defaults: &["F8", "Delete"],
        available: true,
    },
    CommandDef {
        id: "cm_delete_permanent",
        menu: "Files",
        title: "Delete permanently",
        defaults: &["Shift+F8", "Shift+Delete"],
        available: true,
    },
    CommandDef {
        id: "cm_exit",
        menu: "Files",
        title: "Exit",
        defaults: &["Alt+F4"],
        available: true,
    },
    CommandDef {
        id: "cm_mark",
        menu: "Mark",
        title: "Mark/unmark",
        defaults: &["Space", "Insert"],
        available: true,
    },
    CommandDef {
        id: "cm_mark_mask",
        menu: "Mark",
        title: "Select group",
        defaults: &["NumAdd", "Ctrl+="],
        available: true,
    },
    CommandDef {
        id: "cm_unmark_mask",
        menu: "Mark",
        title: "Unselect group",
        defaults: &["NumSubtract", "Ctrl+-"],
        available: true,
    },
    CommandDef {
        id: "cm_invert",
        menu: "Mark",
        title: "Invert selection",
        defaults: &["NumMultiply", "Ctrl+8"],
        available: true,
    },
    CommandDef {
        id: "cm_mark_all",
        menu: "Mark",
        title: "Select all",
        defaults: &["Ctrl+A"],
        available: true,
    },
    CommandDef {
        id: "cm_switch_panel",
        menu: "Commands",
        title: "Switch panel",
        defaults: &["Tab"],
        available: true,
    },
    CommandDef {
        id: "cm_hotlist",
        menu: "Commands",
        title: "Directory hotlist",
        defaults: &["Ctrl+D"],
        available: true,
    },
    CommandDef {
        id: "cm_parent",
        menu: "Commands",
        title: "Parent directory",
        defaults: &["Backspace", "Ctrl+PageUp"],
        available: true,
    },
    CommandDef {
        id: "cm_root",
        menu: "Commands",
        title: "Root directory",
        defaults: &["Ctrl+\\"],
        available: true,
    },
    CommandDef {
        id: "cm_swap_panels",
        menu: "Commands",
        title: "Swap panels",
        defaults: &["Ctrl+U"],
        available: true,
    },
    CommandDef {
        id: "cm_target_equal_source",
        menu: "Commands",
        title: "Target = Source",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_search",
        menu: "Commands",
        title: "Search",
        defaults: &["Alt+F7"],
        available: false,
    },
    CommandDef {
        id: "cm_multi_rename",
        menu: "Files",
        title: "Multi-rename",
        defaults: &["Ctrl+M"],
        available: false,
    },
    CommandDef {
        id: "cm_chmod",
        menu: "Files",
        title: "Change attributes…",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_connect",
        menu: "Net",
        title: "Connect to server",
        defaults: &["Ctrl+F"],
        available: true,
    },
    CommandDef {
        id: "cm_disconnect",
        menu: "Net",
        title: "Disconnect",
        defaults: &["Ctrl+Shift+F"],
        available: true,
    },
    CommandDef {
        id: "cm_drive_left",
        menu: "Show",
        title: "Left drive",
        defaults: &["Alt+F1"],
        available: true,
    },
    CommandDef {
        id: "cm_drive_right",
        menu: "Show",
        title: "Right drive",
        defaults: &["Alt+F2"],
        available: true,
    },
    CommandDef {
        id: "cm_refresh",
        menu: "Show",
        title: "Refresh",
        defaults: &["Ctrl+R"],
        available: true,
    },
    CommandDef {
        id: "cm_toggle_hidden",
        menu: "Show",
        title: "Show hidden files",
        defaults: &["F1", "Ctrl+H"],
        available: true,
    },
    CommandDef {
        id: "cm_cmdline_focus",
        menu: "Commands",
        title: "Command line",
        defaults: &["Ctrl+Enter"],
        available: true,
    },
    CommandDef {
        id: "cm_config_keyboard",
        menu: "Configuration",
        title: "Keyboard…",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_config_options",
        menu: "Configuration",
        title: "Options…",
        defaults: &[],
        available: true,
    },
];

/// Combo (canonical string) to command id.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(transparent)]
pub struct Keymap {
    bindings: BTreeMap<String, String>,
}

fn is_command(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

impl Keymap {
    pub fn defaults() -> Keymap {
        let mut bindings = BTreeMap::new();
        for c in COMMANDS {
            for d in c.defaults {
                let combo: KeyCombo = d.parse().expect("default combo parses");
                bindings.insert(combo.to_string(), c.id.to_string());
            }
        }
        Keymap { bindings }
    }

    pub fn resolve(&self, c: &KeyCombo) -> Option<&str> {
        self.bindings.get(&c.to_string()).map(String::as_str)
    }

    /// Binds `c` to `cmd`; returns the command that lost this combo, if any
    /// (a different one than `cmd`).
    pub fn bind(&mut self, c: &KeyCombo, cmd: &str) -> Option<String> {
        self.bindings
            .insert(c.to_string(), cmd.to_string())
            .filter(|old| old != cmd)
    }

    pub fn unbind(&mut self, c: &KeyCombo) {
        self.bindings.remove(&c.to_string());
    }

    pub fn combos_for(&self, cmd: &str) -> Vec<KeyCombo> {
        self.bindings
            .iter()
            .filter(|(_, id)| *id == cmd)
            .filter_map(|(k, _)| k.parse().ok())
            .collect()
    }

    /// Missing or unusable file gives the defaults. Entries with an unknown
    /// command id or an unparsable combo are dropped; combos are canonicalized.
    pub fn load_or_default(dir: &Path) -> Keymap {
        let Some(raw) =
            crate::storage::load_json::<Option<BTreeMap<String, String>>>(dir, KEYMAP_FILE)
        else {
            return Keymap::defaults();
        };
        let mut bindings = BTreeMap::new();
        for (combo, cmd) in raw {
            match combo.parse::<KeyCombo>() {
                Ok(c) if is_command(&cmd) => {
                    bindings.insert(c.to_string(), cmd);
                }
                _ => log::warn!("keymap: dropping {combo:?} -> {cmd:?}"),
            }
        }
        Keymap { bindings }
    }

    /// `(combo, command)` pairs bound to a combo macOS reserves.
    pub fn system_conflicts(&self) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .filter(|(k, _)| SYSTEM_RESERVED.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    #[test]
    fn combo_parse_display_roundtrip() {
        assert_eq!(c("ctrl+shift+f6").to_string(), "Ctrl+Shift+F6");
        assert_eq!(c("Option+F1").to_string(), "Alt+F1");
        assert_eq!(c("ctrl+=").to_string(), "Ctrl+=");
        assert_eq!(c("Ctrl+\\").to_string(), "Ctrl+\\");
        assert_eq!(
            c("cmd+alt+shift+ctrl+pageup").to_string(),
            "Ctrl+Alt+Shift+Cmd+PageUp"
        );
        for bad in ["", "Ctrl+", "Foo+F1", "F13", "F0", "F05", "Ctrl+Ctrl", "ab"] {
            assert!(bad.parse::<KeyCombo>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn defaults_cover_table() {
        let k = Keymap::defaults();
        for (combo, cmd) in [
            ("F5", "cm_copy"),
            ("F2", "cm_rename"),
            ("Shift+F6", "cm_rename"),
            ("Space", "cm_mark"),
            ("Ctrl+=", "cm_mark_mask"),
            ("Alt+F7", "cm_search"),
            ("Tab", "cm_switch_panel"),
            ("Ctrl+\\", "cm_root"),
            ("F1", "cm_toggle_hidden"),
            ("Ctrl+H", "cm_toggle_hidden"),
        ] {
            assert_eq!(k.resolve(&c(combo)), Some(cmd), "{combo}");
        }
        assert!(k.combos_for("cm_target_equal_source").is_empty());
        assert_eq!(k.combos_for("cm_rename").len(), 2);
    }

    #[test]
    fn defaults_have_no_duplicates() {
        let total: usize = COMMANDS.iter().map(|c| c.defaults.len()).sum();
        assert_eq!(Keymap::defaults().bindings.len(), total);
        let mut ids: Vec<_> = COMMANDS.iter().map(|c| c.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), COMMANDS.len());
    }

    #[test]
    fn bind_reports_displaced() {
        let mut k = Keymap::defaults();
        assert_eq!(k.bind(&c("F5"), "cm_view"), Some("cm_copy".to_string()));
        assert_eq!(k.resolve(&c("F5")), Some("cm_view"));
        assert_eq!(k.bind(&c("F5"), "cm_view"), None);
        assert_eq!(k.bind(&c("Ctrl+J"), "cm_view"), None);
        k.unbind(&c("F5"));
        assert_eq!(k.resolve(&c("F5")), None);
    }

    #[test]
    fn load_drops_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Keymap::load_or_default(dir.path()), Keymap::defaults());
        std::fs::write(
            dir.path().join(KEYMAP_FILE),
            r#"{"f5":"cm_view","Ctrl+J":"cm_nope","Bogus+F1":"cm_copy","F9":"cm_edit"}"#,
        )
        .unwrap();
        let k = Keymap::load_or_default(dir.path());
        assert_eq!(k.resolve(&c("F5")), Some("cm_view"));
        assert_eq!(k.resolve(&c("F9")), Some("cm_edit"));
        assert_eq!(k.resolve(&c("Ctrl+J")), None);
        assert_eq!(k.bindings.len(), 2);
    }

    #[test]
    fn system_conflicts_lists_reserved() {
        let mut k = Keymap::defaults();
        assert!(k.system_conflicts().is_empty());
        k.bind(&c("Ctrl+Up"), "cm_parent");
        assert_eq!(
            k.system_conflicts(),
            vec![("Ctrl+Up".to_string(), "cm_parent".to_string())]
        );
    }

    /// The README key table is generated from COMMANDS; this keeps it current.
    #[test]
    fn readme_key_table_matches_commands() {
        let mut table = String::from("| Menu | Command | Keys |\n|---|---|---|\n");
        for c in COMMANDS {
            let title = if c.available {
                c.title.to_string()
            } else {
                format!("{} (later version)", c.title)
            };
            let keys = if c.defaults.is_empty() {
                "—".to_string()
            } else {
                c.defaults
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            table.push_str(&format!("| {} | {title} | {keys} |\n", c.menu));
        }
        let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
            .expect("README.md at the repository root");
        let start = "<!-- keys:start -->\n";
        let current = readme
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once("<!-- keys:end -->"))
            .map(|(t, _)| t)
            .expect("README key table markers");
        assert_eq!(current, table, "README key table is stale; replace it with:\n{table}");
    }

    #[test]
    fn connect_commands_available() {
        let def = |id: &str| COMMANDS.iter().find(|c| c.id == id).expect(id);
        assert!(def("cm_connect").available);
        assert!(def("cm_disconnect").available);
        let chmod = def("cm_chmod");
        assert!(chmod.available);
        assert_eq!((chmod.menu, chmod.title), ("Files", "Change attributes…"));
        assert!(chmod.defaults.is_empty());
        let k = Keymap::defaults();
        assert_eq!(k.resolve(&c("Ctrl+F")), Some("cm_connect"));
        assert_eq!(k.resolve(&c("Ctrl+Shift+F")), Some("cm_disconnect"));
    }

    #[test]
    fn serializes_as_plain_map() {
        let mut k = Keymap::default();
        k.bind(&c("F5"), "cm_copy");
        assert_eq!(serde_json::to_string(&k).unwrap(), r#"{"F5":"cm_copy"}"#);
    }
}
