//! Mounted volumes and free space.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum VolumeKind {
    Root,
    Home,
    Volume,
    Network,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub path: String,
    pub kind: VolumeKind,
    pub total: u64,
    pub free: u64,
}

/// `(total, free)` bytes for the filesystem containing `path` (free = available to unprivileged users).
pub fn space(path: &Path) -> io::Result<(u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated string and `st` a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let frsize = st.f_frsize as u64;
    Ok((frsize * st.f_blocks as u64, frsize * st.f_bavail as u64))
}

fn volume(name: String, path: &Path, kind: VolumeKind) -> Volume {
    let (total, free) = space(path).unwrap_or((0, 0));
    Volume {
        name,
        path: path.to_string_lossy().into_owned(),
        kind,
        total,
        free,
    }
}

#[cfg(target_os = "macos")]
fn is_network_fs(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and out-pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    let bytes: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as u8)
        .collect();
    matches!(bytes.as_slice(), b"smbfs" | b"afpfs" | b"nfs" | b"webdav")
}

#[cfg(target_os = "macos")]
pub fn list_volumes(home: &Path) -> Vec<Volume> {
    let root = Path::new("/");
    let mut root_name = String::from("/");
    let mut others = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        for de in rd.flatten() {
            let p = de.path();
            let name = de.file_name().to_string_lossy().into_owned();
            // The boot volume appears in /Volumes as a symlink to "/".
            if std::fs::canonicalize(&p).is_ok_and(|c| c == root) {
                root_name = name;
                continue;
            }
            others.push((name, p));
        }
    }
    others.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = vec![
        volume(root_name, root, VolumeKind::Root),
        volume("~".into(), home, VolumeKind::Home),
    ];
    for (name, p) in others {
        let kind = if is_network_fs(&p) {
            VolumeKind::Network
        } else {
            VolumeKind::Volume
        };
        out.push(volume(name, &p, kind));
    }
    out
}

#[cfg(not(target_os = "macos"))]
pub fn list_volumes(home: &Path) -> Vec<Volume> {
    let mut out = vec![
        volume("/".into(), Path::new("/"), VolumeKind::Root),
        volume("~".into(), home, VolumeKind::Home),
    ];
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut found: Vec<(String, String)> = Vec::new();
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let (Some(_dev), Some(mp)) = (it.next(), it.next()) else {
            continue;
        };
        // /proc/mounts escapes spaces as \040.
        let mp = mp.replace("\\040", " ");
        if ["/media", "/mnt", "/run/media"]
            .iter()
            .any(|b| mp == *b || mp.starts_with(&format!("{b}/")))
            && mp != "/media"
            && mp != "/mnt"
            && mp != "/run/media"
        {
            let name = mp.rsplit('/').next().unwrap_or(&mp).to_string();
            found.push((name, mp));
        }
    }
    found.sort();
    found.dedup();
    for (name, mp) in found {
        out.push(volume(name, Path::new(&mp), VolumeKind::Volume));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_start_with_root_and_home() {
        let home = dirs::home_dir().unwrap();
        let v = list_volumes(&home);
        assert_eq!(v[0].kind, VolumeKind::Root);
        assert_eq!(v[0].path, "/");
        assert_eq!(v[1].kind, VolumeKind::Home);
        assert!(v[0].total > 0);
    }

    #[test]
    fn volume_kind_serializes_camel_case() {
        assert_eq!(
            serde_json::to_string(&VolumeKind::Network).unwrap(),
            "\"network\""
        );
    }
}
