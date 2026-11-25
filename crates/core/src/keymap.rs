//! Command registry and Total-Commander-style keymap.

use crate::config::{MacKey, Modifiers, Settings, SETTINGS_FILE};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

pub const KEYMAP_FILE: &str = "keymap.json";

/// Combos macOS reserves for itself (Mission Control, Spotlight, ...), in physical keys:
/// `Ctrl` = Control, `Alt` = Option (see [`KeyCombo::physical`]).
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

impl KeyCombo {
    /// The physical keys of this TC combo under `m`, written with `ctrl` = Control,
    /// `alt` = Option and `cmd` = Command. A modifier mapped to Fn has no flag here (see
    /// [`KeyCombo::uses_fn`]).
    pub fn physical(&self, m: &Modifiers) -> KeyCombo {
        let mut p = KeyCombo {
            ctrl: false,
            alt: false,
            shift: self.shift,
            cmd: false,
            key: self.key.clone(),
        };
        let held = [
            (self.ctrl, m.ctrl),
            (self.alt, m.alt),
            (self.cmd, MacKey::Command),
        ];
        for (on, key) in held {
            if on {
                match key {
                    MacKey::Control => p.ctrl = true,
                    MacKey::Option => p.alt = true,
                    MacKey::Command => p.cmd = true,
                    MacKey::Fn => {}
                }
            }
        }
        p
    }

    /// True when this TC combo needs the Fn key under `m`.
    pub fn uses_fn(&self, m: &Modifiers) -> bool {
        (self.ctrl && m.ctrl == MacKey::Fn) || (self.alt && m.alt == MacKey::Fn)
    }

    /// The label macOS menus use for the physical keys: `⌃⌥⇧⌘` then the key (`⌥F5`).
    pub fn label(&self, m: &Modifiers) -> String {
        let p = self.physical(m);
        let mut out = String::new();
        if self.uses_fn(m) {
            out.push_str("fn ");
        }
        for (on, sym) in [(p.ctrl, '⌃'), (p.alt, '⌥'), (p.shift, '⇧'), (p.cmd, '⌘')] {
            if on {
                out.push(sym);
            }
        }
        out.push_str(match p.key.as_str() {
            "Up" => "↑",
            "Down" => "↓",
            "Left" => "←",
            "Right" => "→",
            k => k,
        });
        out
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
        id: "cm_packfiles",
        menu: "Files",
        title: "Pack files…",
        defaults: &["Alt+F5"],
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
        id: "cm_copy_names_to_clip",
        menu: "Mark",
        title: "Copy names to clipboard",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_copy_full_names_to_clip",
        menu: "Mark",
        title: "Copy names with path to clipboard",
        defaults: &[],
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
        available: true,
    },
    CommandDef {
        id: "cm_feed_listbox",
        menu: "Commands",
        title: "Feed search results to listbox",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_multi_rename",
        menu: "Files",
        title: "Multi-rename…",
        defaults: &["Ctrl+M"],
        available: true,
    },
    CommandDef {
        id: "cm_context_menu",
        menu: "Files",
        title: "Context menu",
        defaults: &["Shift+F10"],
        available: true,
    },
    CommandDef {
        id: "cm_cut_to_clipboard",
        menu: "Files",
        title: "Cut to clipboard",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_copy_to_clipboard",
        menu: "Files",
        title: "Copy to clipboard",
        defaults: &[],
        available: true,
    },
    CommandDef {
        id: "cm_paste_from_clipboard",
        menu: "Files",
        title: "Paste from clipboard",
        defaults: &[],
        available: true,
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

    /// Binds the default keys of commands that are new since the keymap was saved, unless
    /// another command has the key. `known`: the command ids of that time; None (a keymap
    /// from before they were recorded) counts the commands without any key as new. Returns
    /// whether anything was added.
    pub fn add_new_defaults(&mut self, known: Option<&[String]>) -> bool {
        let mut changed = false;
        for c in COMMANDS {
            let new = match known {
                Some(ids) => !ids.iter().any(|id| id == c.id),
                None => self.combos_for(c.id).is_empty(),
            };
            if !new {
                continue;
            }
            for d in c.defaults {
                let combo: KeyCombo = d.parse().expect("default combo parses");
                if self.resolve(&combo).is_none() {
                    self.bind(&combo, c.id);
                    changed = true;
                }
            }
        }
        changed
    }

    /// `(combo, command)` pairs whose physical keys under `m` are a combo macOS reserves
    /// (none of the listed combos involves Fn).
    pub fn system_conflicts(&self, m: &Modifiers) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .filter(|(k, _)| {
                k.parse::<KeyCombo>().is_ok_and(|c| {
                    !c.uses_fn(m) && SYSTEM_RESERVED.contains(&c.physical(m).to_string().as_str())
                })
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Every command id, as recorded in `Settings::keymap_commands`.
pub fn command_ids() -> Vec<String> {
    COMMANDS.iter().map(|c| c.id.to_string()).collect()
}

/// Loads `keymap.json` from `dir`. When it exists, commands added since it was saved get
/// their free default keys (see [`Keymap::add_new_defaults`]); then the keymap (if changed)
/// and `settings.keymap_commands` are saved, so each new command is added only once.
pub fn load_migrating(dir: &Path, settings: &mut Settings) -> Keymap {
    let mut keymap = Keymap::load_or_default(dir);
    if !dir.join(KEYMAP_FILE).exists() {
        return keymap;
    }
    if keymap.add_new_defaults(settings.keymap_commands.as_deref()) {
        if let Err(e) = crate::storage::save_json(dir, KEYMAP_FILE, &keymap) {
            log::warn!("keymap: saving new default keys: {e}");
        }
    }
    let ids = command_ids();
    if settings.keymap_commands.as_ref() != Some(&ids) {
        settings.keymap_commands = Some(ids);
        if let Err(e) = crate::storage::save_json(dir, SETTINGS_FILE, &*settings) {
            log::warn!("keymap: saving the command list: {e}");
        }
    }
    keymap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cm_search_available() {
        let c = COMMANDS.iter().find(|c| c.id == "cm_search").unwrap();
        assert!(c.available);
        assert_eq!(c.defaults, &["Alt+F7"]);
        let f = COMMANDS.iter().find(|c| c.id == "cm_feed_listbox").unwrap();
        assert!(f.available && f.defaults.is_empty());
    }

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

    fn mods(alt: MacKey, ctrl: MacKey) -> Modifiers {
        Modifiers { alt, ctrl }
    }

    #[test]
    fn system_conflicts_lists_reserved() {
        let def = Modifiers::default();
        let mut k = Keymap::defaults();
        assert!(k.system_conflicts(&def).is_empty());
        k.bind(&c("Ctrl+Up"), "cm_parent");
        assert_eq!(
            k.system_conflicts(&def),
            vec![("Ctrl+Up".to_string(), "cm_parent".to_string())]
        );
        // Command as Ctrl: TC Ctrl+Up is Cmd+Up, which macOS leaves alone ...
        let cmd = mods(MacKey::Option, MacKey::Command);
        assert!(k.system_conflicts(&cmd).is_empty());
        // ... and Control as Alt makes TC Alt+Up the reserved Control+Up.
        k.bind(&c("Alt+Up"), "cm_root");
        let swapped = mods(MacKey::Control, MacKey::Option);
        assert_eq!(
            k.system_conflicts(&swapped),
            vec![("Alt+Up".to_string(), "cm_root".to_string())]
        );
    }

    #[test]
    fn physical_keys_and_labels_follow_the_mapping() {
        let def = Modifiers::default();
        assert_eq!(c("Alt+F5").label(&def), "⌥F5");
        assert_eq!(c("Ctrl+Shift+F").label(&def), "⌃⇧F");
        assert_eq!(c("Ctrl+Up").label(&def), "⌃↑");
        assert_eq!(c("F3").label(&def), "F3");
        assert_eq!(c("Cmd+Q").label(&def), "⌘Q");
        let cmd_alt = mods(MacKey::Command, MacKey::Control);
        assert_eq!(c("Alt+F5").label(&cmd_alt), "⌘F5");
        assert_eq!(c("Alt+F5").physical(&cmd_alt).to_string(), "Cmd+F5");
        let swapped = mods(MacKey::Control, MacKey::Option);
        assert_eq!(c("Ctrl+Alt+Space").label(&swapped), "⌃⌥Space");
        assert_eq!(c("Ctrl+D").label(&swapped), "⌥D");
        assert_eq!(c("Ctrl+D").physical(&swapped).to_string(), "Alt+D");
        let fn_alt = mods(MacKey::Fn, MacKey::Control);
        assert_eq!(c("Alt+F5").label(&fn_alt), "fn F5");
        assert_eq!(c("Ctrl+Alt+Shift+X").label(&fn_alt), "fn ⌃⇧X");
        assert!(c("Alt+F5").uses_fn(&fn_alt));
        assert!(!c("Ctrl+F5").uses_fn(&fn_alt));
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
        let readme =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
                .expect("README.md at the repository root");
        let start = "<!-- keys:start -->\n";
        let current = readme
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once("<!-- keys:end -->"))
            .map(|(t, _)| t)
            .expect("README key table markers");
        assert_eq!(
            current, table,
            "README key table is stale; replace it with:\n{table}"
        );
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
    fn pack_files_is_alt_f5() {
        let def = COMMANDS.iter().find(|c| c.id == "cm_packfiles").unwrap();
        assert_eq!((def.menu, def.title), ("Files", "Pack files…"));
        assert_eq!(def.defaults, ["Alt+F5"]);
        assert!(def.available);
        assert_eq!(
            Keymap::defaults().resolve(&c("Option+F5")),
            Some("cm_packfiles")
        );
    }

    /// A keymap.json from before Pack files: the defaults without Alt+F5.
    fn old_keymap(dir: &Path) {
        let mut k = Keymap::defaults();
        k.unbind(&c("Alt+F5"));
        crate::storage::save_json(dir, KEYMAP_FILE, &k).unwrap();
    }

    #[test]
    fn old_keymap_gets_new_default_keys_once() {
        let dir = tempfile::tempdir().unwrap();
        old_keymap(dir.path());
        let mut s = Settings::default();
        let k = load_migrating(dir.path(), &mut s);
        assert_eq!(k.resolve(&c("Alt+F5")), Some("cm_packfiles"));
        assert_eq!(s.keymap_commands, Some(command_ids()));
        // Both files were written; keymap.json is still a plain map.
        let saved: Settings = crate::storage::load_json(dir.path(), SETTINGS_FILE);
        assert_eq!(saved.keymap_commands, Some(command_ids()));
        let raw = std::fs::read_to_string(dir.path().join(KEYMAP_FILE)).unwrap();
        let map: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(map.get("Alt+F5").map(String::as_str), Some("cm_packfiles"));

        // The user removes Alt+F5 afterwards: it stays removed.
        let mut k = k;
        k.unbind(&c("Alt+F5"));
        crate::storage::save_json(dir.path(), KEYMAP_FILE, &k).unwrap();
        let mut s: Settings = crate::storage::load_json(dir.path(), SETTINGS_FILE);
        let k = load_migrating(dir.path(), &mut s);
        assert_eq!(k.resolve(&c("Alt+F5")), None);
        assert!(k.combos_for("cm_packfiles").is_empty());
    }

    #[test]
    fn new_defaults_never_take_a_used_key() {
        let mut k = Keymap::defaults();
        k.unbind(&c("Alt+F5"));
        k.bind(&c("Alt+F5"), "cm_view");
        let mut known = command_ids();
        known.retain(|id| id != "cm_packfiles");
        assert!(!k.add_new_defaults(Some(&known)));
        assert_eq!(k.resolve(&c("Alt+F5")), Some("cm_view"));
        assert!(k.combos_for("cm_packfiles").is_empty());
        // The same with no recorded list.
        assert!(!k.add_new_defaults(None));
        assert_eq!(k.resolve(&c("Alt+F5")), Some("cm_view"));
    }

    #[test]
    fn new_defaults_with_a_list_add_only_unlisted_commands() {
        let mut k = Keymap::defaults();
        k.unbind(&c("Alt+F5"));
        k.unbind(&c("F3")); // cm_view: known, deliberately unbound
        let mut known = command_ids();
        known.retain(|id| id != "cm_packfiles");
        assert!(k.add_new_defaults(Some(&known)));
        assert_eq!(k.resolve(&c("Alt+F5")), Some("cm_packfiles"));
        assert_eq!(k.resolve(&c("F3")), None);
    }

    #[test]
    fn no_keymap_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        assert_eq!(load_migrating(dir.path(), &mut s), Keymap::defaults());
        assert_eq!(s.keymap_commands, None);
        assert!(!dir.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn serializes_as_plain_map() {
        let mut k = Keymap::default();
        k.bind(&c("F5"), "cm_copy");
        assert_eq!(serde_json::to_string(&k).unwrap(), r#"{"F5":"cm_copy"}"#);
    }
}
