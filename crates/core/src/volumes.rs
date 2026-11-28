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

/// Whether the item `path` lies on a network volume (SMB, AFP, NFS, WebDAV), which has no
/// Trash. Its folder is asked, so a local symlink to a share is still a local item.
pub fn on_network_volume(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .is_some_and(is_network_fs)
    }
    #[cfg(target_os = "linux")]
    {
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .is_some_and(is_network_fs_linux)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        false
    }
}

/// Linux: a CIFS/SMB, NFS or NCP filesystem (`statfs` magic), or a share mounted by GNOME's
/// gvfs (`/run/user/<uid>/gvfs/…`, a FUSE filesystem).
#[cfg(target_os = "linux")]
fn is_network_fs_linux(path: &Path) -> bool {
    if path.to_string_lossy().contains("/gvfs/") {
        return true;
    }
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and out-pointer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    // CIFS, SMB2, SMB, NFS, NCP.
    matches!(
        st.f_type as u64,
        0xFF53_4D42 | 0xFE53_4D42 | 0x517B | 0x6969 | 0x564C
    )
}

/// Filesystem types of network mounts in `/proc/self/mounts`.
const NETWORK_TYPES: &[&str] = &[
    "cifs",
    "smb3",
    "smbfs",
    "nfs",
    "nfs4",
    "ncpfs",
    "davfs",
    "fuse.sshfs",
];

/// `/proc/self/mounts` escapes space, tab, newline and backslash as octal (`\040`…).
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The drives of a Linux system from `/proc/self/mounts` (`mounts`) and the share folders of
/// gvfs (`gvfs`: names in `/run/user/<uid>/gvfs`): removable and other disks under `/media`,
/// `/run/media` and `/mnt`, network mounts anywhere, gvfs shares. `(name, path, network)`.
pub fn linux_drives(mounts: &str, gvfs_dir: &str, gvfs: &[String]) -> Vec<(String, String, bool)> {
    let mut found: Vec<(String, String, bool)> = Vec::new();
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let (Some(_dev), Some(mp), Some(fstype)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let mp = unescape_mount(mp);
        let network = NETWORK_TYPES.contains(&fstype);
        let removable = ["/media", "/mnt", "/run/media"]
            .iter()
            .any(|b| mp.starts_with(&format!("{b}/")));
        if network || removable {
            let name = mp.rsplit('/').next().unwrap_or(&mp).to_string();
            found.push((name, mp, network));
        }
    }
    for share in gvfs {
        // e.g. smb-share:server=nas.local,share=media → "media (nas.local)".
        let field = |k: &str| {
            share
                .split([':', ','])
                .find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_string))
        };
        let name = match (field("share"), field("server").or_else(|| field("host"))) {
            (Some(sh), Some(srv)) => format!("{sh} ({srv})"),
            (None, Some(srv)) => srv,
            _ => share.clone(),
        };
        found.push((name, format!("{gvfs_dir}/{share}"), true));
    }
    found.sort();
    found.dedup_by(|a, b| a.1 == b.1);
    found
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
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    // SAFETY: getuid cannot fail.
    let gvfs_dir = format!("/run/user/{}/gvfs", unsafe { libc::getuid() });
    let gvfs: Vec<String> = std::fs::read_dir(&gvfs_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    for (name, mp, network) in linux_drives(&mounts, &gvfs_dir, &gvfs) {
        let kind = if network {
            VolumeKind::Network
        } else {
            VolumeKind::Volume
        };
        out.push(volume(name, Path::new(&mp), kind));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_mounts_give_disks_and_shares() {
        let mounts = "\
/dev/nvme0n1p2 / ext4 rw 0 0
/dev/sdb1 /media/me/USB\\040Stick vfat rw 0 0
//nas/music /mnt/music cifs rw 0 0
nas:/export /srv/nfs nfs4 rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/sdc1 /media vfat rw 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw 0 0
";
        let gvfs = vec![
            "smb-share:server=nas.local,share=media".to_string(),
            "sftp:host=web1".to_string(),
        ];
        let d = linux_drives(mounts, "/run/user/1000/gvfs", &gvfs);
        let got: Vec<(&str, &str, bool)> = d
            .iter()
            .map(|(n, p, k)| (n.as_str(), p.as_str(), *k))
            .collect();
        assert_eq!(
            got,
            [
                ("USB Stick", "/media/me/USB Stick", false),
                (
                    "media (nas.local)",
                    "/run/user/1000/gvfs/smb-share:server=nas.local,share=media",
                    true
                ),
                ("music", "/mnt/music", true),
                ("nfs", "/srv/nfs", true),
                ("web1", "/run/user/1000/gvfs/sftp:host=web1", true),
            ]
        );
        assert_eq!(unescape_mount("a\\040b\\134c\\0"), "a b\\c\\0");
    }

    /// Local items (and odd paths) are never on a network volume; nothing is mounted for this.
    #[test]
    fn local_items_are_not_on_a_network_volume() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(!on_network_volume(&f));
        assert!(!on_network_volume(t.path()));
        assert!(!on_network_volume(Path::new("/")));
        assert!(!on_network_volume(Path::new("relative")));
        assert!(!on_network_volume(&t.path().join("missing/x")));
    }

    /// Read-only check on a real share: `FXFM_NET_ITEM=/Volumes/<share>/<item>`.
    #[test]
    #[ignore = "needs a mounted network share (FXFM_NET_ITEM)"]
    fn an_item_on_a_mounted_share_is_on_a_network_volume() {
        let item = std::env::var("FXFM_NET_ITEM").expect("FXFM_NET_ITEM");
        assert!(on_network_volume(Path::new(&item)));
    }

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
