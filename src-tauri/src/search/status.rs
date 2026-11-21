//! `search_status`: the index state and every volume, for the UI.

use super::*;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "off" | "notAsked" | "building" | "ready"
    pub state: &'static str,
    pub entries: u64,
    pub volumes: Vec<VolStatus>,
    /// Folders macOS privacy protection blocked: Full Disk Access opens them ("Retry" after
    /// granting it).
    pub denied: Vec<String>,
    /// Folders that cannot be read for other reasons (owner or mode bits, system protection);
    /// Full Disk Access does not help.
    pub unreadable: Vec<String>,
    /// `denied` is not empty.
    pub fda_needed: bool,
    /// The built-in excludes (`default_excludes()`), shown read-only in Options.
    pub exclude_defaults: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VolStatus {
    pub path: String,
    pub name: String,
    /// "startup" | "local" | "network"
    pub kind: &'static str,
    pub enabled: bool,
    pub mounted: bool,
    pub entries: u64,
    /// Unix seconds of the last scan; 0 = never.
    pub refreshed: i64,
    /// "building" | "ready" | "stale" | "off"
    pub state: &'static str,
    /// `mntfrom` of a network volume (`//user@host/share`), also while it is not mounted.
    pub source: Option<String>,
}

impl Inner {
    pub(super) fn status(&self) -> Status {
        let (settings, pending, counts, mounted) = {
            let run = lock(&self.run);
            (
                run.settings.clone(),
                run.pending.clone(),
                run.counts.clone(),
                run.mounted.clone(),
            )
        };
        let (entries, vols) = match self.read().as_ref() {
            Some(s) => (s.index.len() as u64, s.volumes.clone()),
            None => (0, Vec::new()),
        };
        let on = settings.index == Some(true);
        let state_of = |root: &str, enabled: bool, mounted: bool| -> &'static str {
            if !on || !enabled {
                "off"
            } else if pending.contains(root) {
                "building"
            } else if vols.iter().any(|v| v.root == root && v.complete) && mounted {
                "ready"
            } else {
                "stale"
            }
        };
        let refreshed = |root: &str| {
            vols.iter()
                .find(|v| v.root == root)
                .map_or(0, |v| v.refreshed)
        };
        let enabled = |p: &str| {
            settings
                .volumes
                .iter()
                .any(|v| v.enabled && trim(&v.path) == p)
        };
        let mut out = vec![VolStatus {
            path: self.root.clone(),
            name: mounted
                .iter()
                .find(|m| m.path == self.root)
                .map_or_else(|| last_name(&self.root), |m| m.name.clone()),
            kind: "startup",
            enabled: on,
            mounted: true,
            entries: counts.get(&self.root).copied().unwrap_or(0),
            refreshed: refreshed(&self.root),
            state: state_of(&self.root, true, true),
            source: None,
        }];
        for m in mounted
            .iter()
            .filter(|m| m.path != self.root && !STARTUP_ALIASES.contains(&m.path.as_str()))
        {
            let en = enabled(&m.path);
            out.push(VolStatus {
                path: m.path.clone(),
                name: m.name.clone(),
                kind: if m.network { "network" } else { "local" },
                enabled: en,
                mounted: true,
                entries: counts.get(&m.path).copied().unwrap_or(0),
                refreshed: refreshed(&m.path),
                state: state_of(&m.path, en, true),
                source: m.source.clone(),
            });
        }
        for v in settings.volumes.iter().filter(|v| v.enabled) {
            let p = trim(&v.path);
            if out.iter().any(|o| o.path == p) {
                continue;
            }
            let stored = vols.iter().find(|s| s.root == p);
            let network = stored.is_some_and(|s| s.network);
            out.push(VolStatus {
                name: last_name(&p),
                kind: if network { "network" } else { "local" },
                enabled: true,
                mounted: false,
                entries: counts.get(&p).copied().unwrap_or(0),
                refreshed: refreshed(&p),
                state: state_of(&p, true, false),
                source: stored.and_then(|s| s.source.clone()),
                path: p,
            });
        }
        let startup_done = vols.iter().any(|v| v.root == self.root && v.complete);
        let home = dirs::home_dir().map_or_else(String::new, |h| h.to_string_lossy().into_owned());
        let (fda, unreadable) = split_denied(&vols, &home);
        Status {
            state: match settings.index {
                Some(false) => "off",
                None => "notAsked",
                Some(true) if startup_done => "ready",
                Some(true) => "building",
            },
            entries,
            volumes: out,
            fda_needed: !fda.is_empty(),
            denied: fda,
            unreadable,
            exclude_defaults: default_excludes(),
        }
    }
}

/// Splits the unreadable folders of `vols` into those Full Disk Access would open (blocked by
/// privacy protection: EPERM in the home folder or on a volume under `/Volumes`) and the rest
/// (EACCES, and EPERM elsewhere: system protection, unreadable even with Full Disk Access).
pub(crate) fn split_denied(vols: &[VolumeState], home: &str) -> (Vec<String>, Vec<String>) {
    let mut fda = Vec::new();
    let mut other = Vec::new();
    for v in vols {
        other.extend(v.denied.iter().cloned());
        for b in &v.blocked {
            if needs_full_disk_access(b, home) {
                fda.push(b.clone());
            } else {
                other.push(b.clone());
            }
        }
    }
    (fda, other)
}
