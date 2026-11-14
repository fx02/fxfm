//! Directory hotlist (Ctrl+D): a tree of `cd` shortcuts, persisted as `hotlist.json`.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const HOTLIST_FILE: &str = "hotlist.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HotItem {
    pub title: String,
    pub command: String,
    pub target: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HotNode {
    Item(HotItem),
    Submenu {
        title: String,
        children: Vec<HotNode>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Hotlist {
    pub nodes: Vec<HotNode>,
    pub hide_char: Option<char>,
}

/// True if `s` starts with a URL-like scheme (`sftp://...`).
fn has_scheme(s: &str) -> bool {
    match s.find("://") {
        Some(i) if i > 0 => {
            let scheme = &s[..i];
            scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        _ => false,
    }
}

/// Parses `cd X` (case-insensitive, trimmed). Expands a leading `~`; locations
/// with a scheme are returned untouched; anything else yields `None`.
pub fn parse_cd(command: &str, home: &Path) -> Option<String> {
    let c = command.trim();
    let head = c.get(..2)?;
    if !head.eq_ignore_ascii_case("cd") {
        return None;
    }
    let rest = &c[2..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let x = rest.trim();
    if x.is_empty() {
        return None;
    }
    if has_scheme(x) {
        return Some(x.to_string());
    }
    if x == "~" {
        return Some(home.to_string_lossy().into_owned());
    }
    if let Some(r) = x.strip_prefix("~/") {
        return Some(home.join(r).to_string_lossy().into_owned());
    }
    Some(x.to_string())
}

/// Splits a menu title into (label without `&`, lowercase accelerator).
/// `&&` yields a literal `&` (and no accelerator by itself).
pub fn menu_label(title: &str) -> (String, Option<char>) {
    let mut out = String::new();
    let mut accel = None;
    let mut it = title.chars().peekable();
    while let Some(c) = it.next() {
        if c == '&' {
            match it.peek() {
                Some('&') => {
                    it.next();
                    out.push('&');
                }
                Some(&n) if accel.is_none() => {
                    accel = Some(n.to_lowercase().next().unwrap_or(n));
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    (out, accel)
}

impl Hotlist {
    /// Appends the current location: title = last component, command = `cd <loc>`
    /// with the home prefix abbreviated to `~`.
    pub fn add_current(&mut self, location: &str, home: &Path) {
        let trimmed = location.trim_end_matches('/');
        let title = match trimmed.rsplit('/').next() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => location.to_string(),
        };
        let mut shown = location.to_string();
        if !has_scheme(location) {
            let p = Path::new(location);
            if let Ok(rest) = p.strip_prefix(home) {
                let rest = rest.to_string_lossy();
                shown = if rest.is_empty() {
                    "~".to_string()
                } else {
                    format!("~/{rest}")
                };
            }
        }
        self.nodes.push(HotNode::Item(HotItem {
            title,
            command: format!("cd {shown}"),
            target: String::new(),
        }));
    }

    /// Nodes without entries whose title starts with `hide_char` (recursive).
    pub fn visible(&self) -> Vec<HotNode> {
        fn filter(nodes: &[HotNode], hc: Option<char>) -> Vec<HotNode> {
            nodes
                .iter()
                .filter_map(|n| {
                    let title = match n {
                        HotNode::Item(i) => &i.title,
                        HotNode::Submenu { title, .. } => title,
                    };
                    if hc.is_some_and(|h| title.starts_with(h)) {
                        return None;
                    }
                    Some(match n {
                        HotNode::Item(i) => HotNode::Item(i.clone()),
                        HotNode::Submenu { title, children } => HotNode::Submenu {
                            title: title.clone(),
                            children: filter(children, hc),
                        },
                    })
                })
                .collect()
        }
        filter(&self.nodes, self.hide_char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn home() -> PathBuf {
        PathBuf::from("/Users/x")
    }

    fn item(t: &str) -> HotNode {
        HotNode::Item(HotItem {
            title: t.into(),
            command: "cd /tmp".into(),
            target: String::new(),
        })
    }

    #[test]
    fn parse_cd_variants() {
        assert_eq!(
            parse_cd("cd ~/Downloads", &home()),
            Some("/Users/x/Downloads".into())
        );
        assert_eq!(parse_cd("CD /tmp ", &home()), Some("/tmp".into()));
        assert_eq!(
            parse_cd("cd sftp://web-1/var/www", &home()),
            Some("sftp://web-1/var/www".into())
        );
        assert_eq!(parse_cd("ls", &home()), None);
        assert_eq!(parse_cd("cdx /tmp", &home()), None);
        assert_eq!(parse_cd("cd ~", &home()), Some("/Users/x".into()));
    }

    #[test]
    fn menu_label_accelerator() {
        assert_eq!(menu_label("&Downloads"), ("Downloads".into(), Some('d')));
        assert_eq!(menu_label("A&&B"), ("A&B".into(), None));
    }

    #[test]
    fn add_current_uses_tilde() {
        let mut h = Hotlist::default();
        h.add_current("/Users/x/Documents/PVT", &home());
        assert_eq!(
            h.nodes,
            vec![HotNode::Item(HotItem {
                title: "PVT".into(),
                command: "cd ~/Documents/PVT".into(),
                target: String::new()
            })]
        );
    }

    #[test]
    fn visible_hides_prefixed() {
        let h = Hotlist {
            nodes: vec![
                item("?tajno"),
                item("javno"),
                HotNode::Submenu {
                    title: "sub".into(),
                    children: vec![item("?x"), item("y")],
                },
            ],
            hide_char: Some('?'),
        };
        assert_eq!(
            h.visible(),
            vec![
                item("javno"),
                HotNode::Submenu {
                    title: "sub".into(),
                    children: vec![item("y")]
                }
            ]
        );
        let h2 = Hotlist {
            hide_char: None,
            ..h
        };
        assert_eq!(h2.visible().len(), 3);
    }

    #[test]
    fn json_roundtrip() {
        let h = Hotlist {
            nodes: vec![
                item("a"),
                HotNode::Submenu {
                    title: "s".into(),
                    children: vec![item("b")],
                },
            ],
            hide_char: Some('?'),
        };
        let s = serde_json::to_string(&h).unwrap();
        assert!(s.contains(r#""kind":"item""#) && s.contains(r#""kind":"submenu""#));
        assert!(s.contains(r#""hideChar":"?""#));
        assert_eq!(serde_json::from_str::<Hotlist>(&s).unwrap(), h);
        let d = serde_json::to_string(&Hotlist::default()).unwrap();
        assert!(d.contains(r#""hideChar":null"#));
    }
}
