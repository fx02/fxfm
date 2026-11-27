//! Saved servers (SFTP/SCP/FTP/FTPS, SMB shares): a tree of entries persisted as
//! `servers.json`. Secrets (passwords, passphrases) are never part of these structures.
//!
//! SMB entries are not connected by fxfm: macOS mounts the share (see `smb_url`) and the
//! panels browse the mount point as local files.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const SERVERS_FILE: &str = "servers.json";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Sftp,
    Scp,
    Ftp,
    Ftps,
    /// A Windows/Samba share, mounted by macOS.
    Smb,
}

impl Protocol {
    pub fn scheme(&self) -> &'static str {
        match self {
            Protocol::Sftp => "sftp",
            Protocol::Scp => "scp",
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
            Protocol::Smb => "smb",
        }
    }

    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Sftp | Protocol::Scp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
            Protocol::Smb => 445,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Auth {
    #[default]
    Password,
    Key,
    KeyPassphrase,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum FtpEncoding {
    #[default]
    Utf8,
    Windows1250,
    Latin1,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerEntry {
    /// 16 hex chars, stable; ties stored passwords to the entry.
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    /// Absolute, or `keys/<name>` relative to the data dir.
    pub key_path: Option<String>,
    pub save_password: bool,
    pub remote_dir: Option<String>,
    pub local_dir: Option<String>,
    pub legacy_algorithms: bool,
    #[serde(default = "default_true")]
    pub ftp_passive: bool,
    pub ftp_encoding: FtpEncoding,
    /// SMB only: the share to mount (required there; empty for other protocols).
    pub share: String,
    /// SMB only: mount the share when fxfm starts (with the saved password; never asks).
    pub auto_connect: bool,
    /// SMB only: log in with the system's Kerberos ticket (`kinit` / Ticket Viewer): fxfm
    /// never asks for, stores or sends a password for this entry.
    pub kerberos: bool,
}

impl Default for ServerEntry {
    fn default() -> Self {
        ServerEntry {
            id: String::new(),
            name: String::new(),
            protocol: Protocol::Sftp,
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: Auth::Password,
            key_path: None,
            save_password: false,
            remote_dir: None,
            local_dir: None,
            legacy_algorithms: false,
            ftp_passive: true,
            ftp_encoding: FtpEncoding::Utf8,
            share: String::new(),
            auto_connect: false,
            kerberos: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServerNode {
    Folder {
        name: String,
        children: Vec<ServerNode>,
    },
    Server(ServerEntry),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerList {
    pub nodes: Vec<ServerNode>,
}

fn collect<'a>(nodes: &'a [ServerNode], out: &mut Vec<&'a ServerEntry>) {
    for n in nodes {
        match n {
            ServerNode::Folder { children, .. } => collect(children, out),
            ServerNode::Server(e) => out.push(e),
        }
    }
}

impl ServerList {
    pub fn find_by_name(&self, name: &str) -> Option<&ServerEntry> {
        self.all().into_iter().find(|e| e.name == name)
    }

    pub fn find_by_id(&self, id: &str) -> Option<&ServerEntry> {
        self.all().into_iter().find(|e| e.id == id)
    }

    /// All servers, depth-first in tree order.
    pub fn all(&self) -> Vec<&ServerEntry> {
        let mut out = Vec::new();
        collect(&self.nodes, &mut out);
        out
    }

    /// First problem found, as text for the UI.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen: Vec<&str> = Vec::new();
        for e in self.all() {
            if e.name.trim().is_empty() {
                return Err("Name is required".into());
            }
            if e.name.contains('/') {
                return Err("Name cannot contain '/'".into());
            }
            if seen.contains(&e.name.as_str()) {
                return Err(format!("Another server is already named '{}'", e.name));
            }
            seen.push(&e.name);
            if e.host.trim().is_empty() {
                return Err("Host is required".into());
            }
            if e.protocol == Protocol::Smb {
                if e.share.trim().is_empty() {
                    return Err("Share is required".into());
                }
                if e.share.contains('/') {
                    return Err("Share cannot contain '/'".into());
                }
                continue;
            }
            if matches!(e.auth, Auth::Key | Auth::KeyPassphrase)
                && e.key_path.as_deref().is_none_or(|p| p.trim().is_empty())
            {
                return Err("Choose a key file".into());
            }
        }
        Ok(())
    }
}

/// New random server id: 8 random bytes as 16 hex chars.
pub fn new_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `scheme://[user@]host[:port][/path]` into an unsaved quick-connect entry.
/// Name is `user@host` or `user@host:port` (port only when not the default).
///
/// Error texts never repeat parts of the URL that may hold the password, and the password
/// never reaches the host, the port or the name (see `split_userinfo`).
pub fn parse_url(url: &str) -> Result<ServerEntry, String> {
    let (scheme, rest) = url.trim().split_once("://").ok_or("Invalid URL")?;
    let protocol = match scheme.to_ascii_lowercase().as_str() {
        "sftp" => Protocol::Sftp,
        "scp" => Protocol::Scp,
        "ftp" => Protocol::Ftp,
        "ftps" => Protocol::Ftps,
        // Only a real scheme name is repeated (`user:pass://` is not one).
        _ if !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric()) => {
            return Err(format!("Unsupported protocol '{scheme}'"))
        }
        _ => return Err("Unsupported protocol".into()),
    };
    let (userinfo, after) = split_userinfo(rest);
    let (hostport, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, ""),
    };
    // A password in the URL is deliberately ignored: secrets are never stored here.
    let user = userinfo.map_or("", |u| u.split(':').next().unwrap_or(""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => {
            let port: u16 = p
                .parse()
                .map_err(|_| "Invalid port in the URL".to_string())?;
            (h, port)
        }
        _ => (hostport, protocol.default_port()),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err("Host is required".into());
    }
    let user = if user.is_empty() {
        "anonymous".to_string()
    } else {
        percent_decode(user)
    };
    let name = if port == protocol.default_port() {
        format!("{user}@{host}")
    } else {
        format!("{user}@{host}:{port}")
    };
    Ok(ServerEntry {
        id: new_id(),
        name,
        protocol,
        host: host.to_string(),
        port,
        user,
        remote_dir: (!path.is_empty()).then(|| percent_decode(path)),
        ..ServerEntry::default()
    })
}

/// Splits `rest` (after `scheme://`) into `(userinfo, host[:port][/path])`.
/// When the userinfo may hold a password (a `:` before the first `@`, or a `:` and no `@`,
/// before the first `/`), it ends at the LAST `@` of the URL: a password may contain `/`, `@`
/// or digits. Otherwise it ends at the last `@` before the first `/`, so a path may contain
/// `@` (`sftp://u@h/home/a@b`). Same rule as `urlDisplayName` in ui/lib.mjs. The price: with a
/// `:` before the first `/` (port or password), an `@` in the path must be written `%40`.
fn split_userinfo(rest: &str) -> (Option<&str>, &str) {
    let head = &rest[..rest.find('/').unwrap_or(rest.len())];
    // `[::1]:22` is an IPv6 host, not a password.
    let password = !head.starts_with('[')
        && match (head.find(':'), head.find('@')) {
            (Some(c), Some(a)) => c < a,
            (Some(_), None) => true,
            (None, _) => false,
        };
    let at = if password {
        rest.rfind('@')
    } else {
        head.rfind('@')
    };
    match at {
        Some(i) => (Some(&rest[..i]), &rest[i + 1..]),
        None => (None, rest),
    }
}

// ------------------------------------------------------------------ SMB shares

/// Percent-encodes everything but the URL's unreserved characters (`A-Z a-z 0-9 - . _ ~`).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The SMB login of a user field: a Windows domain typed as `DOMAIN\user` or `DOMAIN;user`
/// becomes `DOMAIN;user` (what smbfs expects); a plain user stays as it is. Not encoded.
pub fn smb_user(user: &str) -> String {
    let user = user.trim();
    match user.split_once(['\\', ';']) {
        Some((domain, name)) => format!("{domain};{name}"),
        None => user.to_string(),
    }
}

/// The user name handed to NetFS next to the URL. With a domain this is the bare name: the
/// domain travels in the URL (`DOMAIN;user@host`), and a `DOMAIN;user` user argument
/// overrides the URL's user and is not taken as a domain login (owner's test 2026-11-17:
/// `DOMAIN\user` failed, the plain name worked).
pub fn netfs_user(user: &str) -> String {
    let user = smb_user(user);
    match user.split_once(';') {
        Some((_, name)) => name.to_string(),
        None => user,
    }
}

/// `smb://[user@]host[:port]/share` for NetFS. The user (domain and name separately) and
/// the share are percent-encoded; port 445 is left out. Never holds a password: NetFS gets
/// it as a separate argument.
pub fn smb_url(entry: &ServerEntry) -> String {
    format!(
        "smb:{}/{}",
        smb_server_url(entry),
        percent_encode(entry.share.trim())
    )
}

/// `//[user@]host[:port]` of the entry (as in `smb_url`, without the share): the server
/// argument of `smbutil view`. Never holds a password.
pub fn smb_server_url(entry: &ServerEntry) -> String {
    let user = smb_user(&entry.user);
    let userinfo = if user.is_empty() {
        String::new()
    } else {
        let enc = match user.split_once(';') {
            Some((domain, name)) => format!("{};{}", percent_encode(domain), percent_encode(name)),
            None => percent_encode(&user),
        };
        format!("{enc}@")
    };
    let host = entry.host.trim();
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let port = if entry.port == 0 || entry.port == Protocol::Smb.default_port() {
        String::new()
    } else {
        format!(":{}", entry.port)
    };
    format!("//{userinfo}{host}{port}")
}

/// `(host, share)` of an smbfs `f_mntfromname` (`//[user@]host[:port]/share`, the share
/// percent-decoded). None for anything else, also for a mount of a folder inside a share.
pub fn parse_mntfrom(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix("//")?;
    let (authority, path) = rest.split_once('/')?;
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match hostport.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => hostport.split(':').next().unwrap_or(""),
    };
    let share = path.strip_suffix('/').unwrap_or(path);
    if host.is_empty() || share.is_empty() || share.contains('/') {
        return None;
    }
    Some((host.to_string(), percent_decode(share)))
}

/// The same item on a mounted SMB share as an `smb://` link (Finder, other Macs) and as a
/// Windows UNC path (`\\host\share\…`): `path` is a local path at or below `mount_point`,
/// the share mounted from `mntfrom` (`//[user@]host/share`). None for anything else. The user
/// name is never part of either.
pub fn share_links(mntfrom: &str, mount_point: &str, path: &str) -> Option<(String, String)> {
    let (host, share) = parse_mntfrom(mntfrom)?;
    let mount = mount_point.trim_end_matches('/');
    let rest = if path == mount {
        ""
    } else {
        path.strip_prefix(mount)?
            .strip_prefix('/')
            .map(|_| &path[mount.len()..])?
    };
    // A share from Finder's sidebar is mounted as `name._smb._tcp.local`.
    let host = host.strip_suffix("._smb._tcp.local").unwrap_or(&host);
    let url_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let enc = |seg: &str| -> String {
        seg.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let mut smb = format!("smb://{url_host}/{}", enc(&share));
    let mut unc = format!("\\\\{host}\\{share}");
    for seg in rest.split('/').filter(|s| !s.is_empty()) {
        smb.push('/');
        smb.push_str(&enc(seg));
        unc.push('\\');
        unc.push_str(seg);
    }
    Some((smb, unc))
}

/// A host name for comparing: lower case, without a trailing dot and without the Bonjour
/// (`._smb._tcp.local`) or `.local` suffix, so a share mounted from Finder's sidebar
/// (`nas._smb._tcp.local`) matches an entry for `nas.local` or `nas`.
fn host_key(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    let h = h.strip_suffix("._smb._tcp.local").unwrap_or(&h);
    h.strip_suffix(".local").unwrap_or(h).to_string()
}

/// Whether the smbfs mount from `mntfrom` is the entry's share: host (see `host_key`) and
/// share compared case-insensitively; the user and the port do not matter.
pub fn matches_mount(entry: &ServerEntry, mntfrom: &str) -> bool {
    let Some((host, share)) = parse_mntfrom(mntfrom) else {
        return false;
    };
    entry.protocol == Protocol::Smb
        && host_key(&host) == host_key(&entry.host)
        && share.to_lowercase() == entry.share.trim().to_lowercase()
}

/// `matches_mount`, also when the two sides name the host differently: one by an IP address
/// and the other by name, or two names (an alias) that resolve to a shared address.
/// `resolve(name)` gives the name's addresses; it is only asked when the share names already
/// match, so a mount of another share never waits on DNS.
pub fn matches_mount_resolving(
    entry: &ServerEntry,
    mntfrom: &str,
    resolve: &dyn Fn(&str) -> Vec<std::net::IpAddr>,
) -> bool {
    if matches_mount(entry, mntfrom) {
        return true;
    }
    let Some((host, share)) = parse_mntfrom(mntfrom) else {
        return false;
    };
    if entry.protocol != Protocol::Smb || share.to_lowercase() != entry.share.trim().to_lowercase()
    {
        return false;
    }
    let ip = |h: &str| {
        h.trim()
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .ok()
    };
    match (ip(&host), ip(&entry.host)) {
        (Some(a), None) => resolve(entry.host.trim()).contains(&a),
        (None, Some(b)) => resolve(&host).contains(&b),
        // Two different addresses.
        (Some(_), Some(_)) => false,
        (None, None) => {
            let a = resolve(&host);
            !a.is_empty() && resolve(entry.host.trim()).iter().any(|x| a.contains(x))
        }
    }
}

/// Resolves the entry's key path: absolute as is, `keys/<name>` under `data_dir`.
pub fn key_file(entry: &ServerEntry, data_dir: &Path) -> Option<PathBuf> {
    let p = entry.key_path.as_deref().filter(|p| !p.is_empty())?;
    let p = Path::new(p);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_dir.join(p)
    })
}

/// Copies a private key into `<data_dir>/keys/` with mode 0600 (never overwriting;
/// a taken name gets `-2`, `-3`, ...). Returns the `keys/<name>` path to store.
pub fn import_key(src: &Path, data_dir: &Path) -> io::Result<String> {
    let base = src
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad key file name"))?;
    let bytes = std::fs::read(src)?;
    let keys = data_dir.join("keys");
    std::fs::create_dir_all(&keys)?;
    for n in 1u32.. {
        let name = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        match opts.open(keys.join(&name)) {
            Ok(mut f) => {
                f.write_all(&bytes)?;
                return Ok(format!("keys/{name}"));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{load_json, save_json};

    fn srv(name: &str, host: &str) -> ServerEntry {
        ServerEntry {
            id: new_id(),
            name: name.into(),
            protocol: Protocol::Sftp,
            host: host.into(),
            port: 22,
            user: "zb".into(),
            auth: Auth::Password,
            key_path: None,
            save_password: false,
            remote_dir: None,
            local_dir: None,
            legacy_algorithms: false,
            ftp_passive: true,
            ftp_encoding: FtpEncoding::Utf8,
            share: String::new(),
            auto_connect: false,
            kerberos: false,
        }
    }

    fn list_of(nodes: Vec<ServerNode>) -> ServerList {
        ServerList { nodes }
    }

    #[test]
    fn servers_roundtrip_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = srv("b", "hb");
        b.protocol = Protocol::Ftps;
        b.ftp_encoding = FtpEncoding::Windows1250;
        let list = list_of(vec![
            ServerNode::Folder {
                name: "prod".into(),
                children: vec![ServerNode::Server(srv("a", "ha"))],
            },
            ServerNode::Server(b),
        ]);
        save_json(dir.path(), SERVERS_FILE, &list).unwrap();
        let back: ServerList = load_json(dir.path(), SERVERS_FILE);
        assert_eq!(back, list);
        assert_eq!(back.all().len(), 2);

        let minimal: ServerList = serde_json::from_str(
            r#"{"nodes":[{"kind":"server","id":"0123456789abcdef","name":"m","protocol":"sftp","host":"h","port":22,"user":"u","auth":"password"}]}"#,
        )
        .unwrap();
        let e = minimal.find_by_name("m").unwrap();
        assert!(e.ftp_passive);
        assert_eq!(e.ftp_encoding, FtpEncoding::Utf8);
        assert_eq!(minimal.find_by_id("0123456789abcdef").unwrap().name, "m");
    }

    #[test]
    fn validate_rejects_bad_entries() {
        let check = |nodes: Vec<ServerNode>| list_of(nodes).validate();
        assert!(check(vec![ServerNode::Server(srv("ok", "h"))]).is_ok());
        assert_eq!(
            check(vec![ServerNode::Server(srv("", "h"))]),
            Err("Name is required".into())
        );
        assert_eq!(
            check(vec![ServerNode::Server(srv("a/b", "h"))]),
            Err("Name cannot contain '/'".into())
        );
        assert_eq!(
            check(vec![
                ServerNode::Folder {
                    name: "f1".into(),
                    children: vec![ServerNode::Server(srv("x", "h"))]
                },
                ServerNode::Folder {
                    name: "f2".into(),
                    children: vec![ServerNode::Server(srv("x", "h2"))]
                },
            ]),
            Err("Another server is already named 'x'".into())
        );
        assert_eq!(
            check(vec![ServerNode::Server(srv("a", ""))]),
            Err("Host is required".into())
        );
        let mut k = srv("a", "h");
        k.auth = Auth::Key;
        assert_eq!(
            check(vec![ServerNode::Server(k)]),
            Err("Choose a key file".into())
        );
    }

    #[test]
    fn parse_url_variants() {
        let e = parse_url("sftp://zb@web-1:2222/var/www").unwrap();
        assert_eq!(e.protocol, Protocol::Sftp);
        assert_eq!(e.user, "zb");
        assert_eq!(e.host, "web-1");
        assert_eq!(e.port, 2222);
        assert_eq!(e.remote_dir.as_deref(), Some("/var/www"));
        assert_eq!(e.name, "zb@web-1:2222");

        let e = parse_url("ftp://h").unwrap();
        assert_eq!(e.protocol, Protocol::Ftp);
        assert_eq!(e.port, 21);
        assert_eq!(e.user, "anonymous");
        assert_eq!(e.name, "anonymous@h");
        assert_eq!(e.remote_dir, None);

        let e = parse_url("ftps://u@h/a%20b").unwrap();
        assert_eq!(e.remote_dir.as_deref(), Some("/a b"));

        assert_eq!(
            parse_url("http://h"),
            Err("Unsupported protocol 'http'".into())
        );
    }

    #[test]
    fn parse_url_errors_never_show_the_password() {
        // Each URL carries the password "pa/55"; none of its parts may be in an error.
        for url in [
            "sftp://u:pa/55@host:x",
            "sftp://u:pa/55@host:2222x/srv",
            "ftp://zb:pa/55@h:",
            "sftp://u:pa:55@",
            "pa/55://u@h",
            "u:pa/55://h",
            "u:pa/55@host",
        ] {
            let e = parse_url(url).expect_err(url);
            for part in ["pa", "55", "/"] {
                assert!(!e.contains(part), "{url} -> {e}");
            }
        }
        assert_eq!(
            parse_url("sftp://h:x1"),
            Err("Invalid port in the URL".into())
        );
        assert_eq!(
            parse_url("sftp://h:99999"),
            Err("Invalid port in the URL".into())
        );
        // A valid URL with a password: the password is dropped.
        let e = parse_url("sftp://u:pa55@h").unwrap();
        assert_eq!((e.user.as_str(), e.host.as_str(), e.port), ("u", "h", 22));
        assert!(!serde_json::to_string(&e).unwrap().contains("pa55"));
    }

    #[test]
    fn parse_url_userinfo_ends_at_the_last_at() {
        // The password (with `/`, `@`, digits) never reaches host, port or name.
        for (url, host, port, dir) in [
            ("sftp://zb:2024/x@h", "h", 22, None),
            ("sftp://zb:pa/55@host", "host", 22, None),
            ("sftp://zb:p@ss/w0rd@h:2222/srv", "h", 2222, Some("/srv")),
            ("sftp://zb:22@h", "h", 22, None),
        ] {
            let e = parse_url(url).unwrap();
            assert_eq!(
                (
                    e.user.as_str(),
                    e.host.as_str(),
                    e.port,
                    e.remote_dir.as_deref()
                ),
                ("zb", host, port, dir),
                "{url}"
            );
            for part in ["2024", "pa", "55", "ss", "w0rd"] {
                assert!(
                    !e.name.contains(part) || host.contains(part),
                    "{url} -> {}",
                    e.name
                );
            }
        }
        // Without a possible password an `@` may stay in the path.
        let e = parse_url("sftp://zb@h/home/a@b").unwrap();
        assert_eq!((e.user.as_str(), e.host.as_str()), ("zb", "h"));
        assert_eq!(e.remote_dir.as_deref(), Some("/home/a@b"));
        let e = parse_url("sftp://zb@h:2222/a@b").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("h", 2222));
        assert_eq!(e.remote_dir.as_deref(), Some("/a@b"));
        let e = parse_url("sftp://h/a@b").unwrap();
        assert_eq!((e.user.as_str(), e.host.as_str()), ("anonymous", "h"));
        let e = parse_url("sftp://[::1]:2222/a@b").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("::1", 2222));
    }

    #[test]
    fn parse_url_percent_decode_survives_non_ascii() {
        let e = parse_url("sftp://h/%a\u{e9}").unwrap();
        assert_eq!(e.remote_dir.as_deref(), Some("/%a\u{e9}"));
        let e = parse_url("sftp://h/x%").unwrap();
        assert_eq!(e.remote_dir.as_deref(), Some("/x%"));
        let e = parse_url("sftp://h/%zz%41").unwrap();
        assert_eq!(e.remote_dir.as_deref(), Some("/%zzA"));
    }

    #[test]
    fn servers_json_never_contains_secret() {
        let list = list_of(vec![ServerNode::Server(srv("a", "h"))]);
        let text = serde_json::to_string(&list).unwrap();
        for key in ["\"password\":", "\"passphrase\":"] {
            assert!(!text.contains(key), "{key} must not be serialized");
        }
    }

    #[test]
    fn import_key_copies_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("id_ed25519");
        std::fs::write(&src, "KEY").unwrap();

        let rel = import_key(&src, data.path()).unwrap();
        assert_eq!(rel, "keys/id_ed25519");
        let dst = data.path().join("keys/id_ed25519");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "KEY");
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(import_key(&src, data.path()).unwrap(), "keys/id_ed25519-2");

        let mut e = srv("a", "h");
        e.key_path = Some(rel);
        assert_eq!(key_file(&e, data.path()), Some(dst));
        e.key_path = Some("/abs/key".into());
        assert_eq!(key_file(&e, data.path()), Some(PathBuf::from("/abs/key")));
        e.key_path = None;
        assert_eq!(key_file(&e, data.path()), None);
    }
    fn smb(host: &str, user: &str, share: &str, port: u16) -> ServerEntry {
        ServerEntry {
            name: "nas".into(),
            protocol: Protocol::Smb,
            host: host.into(),
            port,
            user: user.into(),
            share: share.into(),
            ..ServerEntry::default()
        }
    }

    #[test]
    fn netfs_user_leaves_the_domain_to_the_url() {
        assert_eq!(netfs_user("CORP\\ana"), "ana");
        assert_eq!(netfs_user("CORP;ana"), "ana");
        assert_eq!(netfs_user(" ana "), "ana");
        assert_eq!(netfs_user(""), "");
    }

    #[test]
    fn smb_user_domain_forms() {
        assert_eq!(smb_user("zb"), "zb");
        assert_eq!(smb_user(" CORP\\zb "), "CORP;zb");
        assert_eq!(smb_user("CORP;zb"), "CORP;zb");
        assert_eq!(smb_user(""), "");
    }

    #[test]
    fn smb_url_variants() {
        assert_eq!(smb_url(&smb("nas", "", "Public", 445)), "smb://nas/Public");
        assert_eq!(smb_url(&smb("nas", "zb", "data", 445)), "smb://zb@nas/data");
        assert_eq!(
            smb_url(&smb("nas", "CORP\\zb", "data", 445)),
            "smb://CORP;zb@nas/data"
        );
        assert_eq!(
            smb_url(&smb("nas", "CORP;zb", "data", 445)),
            "smb://CORP;zb@nas/data"
        );
        assert_eq!(
            smb_url(&smb("10.0.0.5", "zb", "data", 1445)),
            "smb://zb@10.0.0.5:1445/data"
        );
        assert_eq!(
            smb_url(&smb("nas", "Žana Ivić", "My Files", 445)),
            "smb://%C5%BDana%20Ivi%C4%87@nas/My%20Files"
        );
        assert_eq!(
            smb_url(&smb("nas", "a@b", "x#y?z%", 445)),
            "smb://a%40b@nas/x%23y%3Fz%25"
        );
        assert_eq!(smb_url(&smb("fe80::1", "", "s", 445)), "smb://[fe80::1]/s");
        assert_eq!(smb_server_url(&smb("nas", "", "", 445)), "//nas");
        assert_eq!(
            smb_server_url(&smb("10.0.0.5", "CORP\\zb", "data", 1445)),
            "//CORP;zb@10.0.0.5:1445"
        );
    }

    #[test]
    fn smb_url_never_holds_the_password() {
        // The entry has no password field; a password typed into the user field by mistake
        // is still encoded as a user, never as `user:password`.
        let url = smb_url(&smb("nas", "zb:secret", "data", 445));
        assert_eq!(url, "smb://zb%3Asecret@nas/data");
        assert!(!url.contains(":secret"));
    }

    #[test]
    fn share_links_smb_and_windows() {
        let l = share_links(
            "//zb@nas.example.com/Team%20Docs",
            "/Volumes/Team Docs",
            "/Volumes/Team Docs/Plans/Q4 plan.docx",
        )
        .unwrap();
        assert_eq!(
            l.0,
            "smb://nas.example.com/Team%20Docs/Plans/Q4%20plan.docx"
        );
        assert_eq!(l.1, r"\\nas.example.com\Team Docs\Plans\Q4 plan.docx");
        let root = share_links(
            "//nas._smb._tcp.local/data",
            "/Volumes/data",
            "/Volumes/data",
        )
        .unwrap();
        assert_eq!(root, ("smb://nas/data".into(), r"\\nas\data".into()));
        assert!(share_links("//nas/data", "/Volumes/data", "/Volumes/data-2/x").is_none());
        assert!(share_links("//nas/data", "/Volumes/data", "/Users/x").is_none());
        assert!(share_links("/dev/disk3s1", "/Volumes/usb", "/Volumes/usb/x").is_none());
        let v6 = share_links("//[fe80::1]/d", "/Volumes/d", "/Volumes/d/č.txt").unwrap();
        assert_eq!(v6.0, "smb://[fe80::1]/d/%C4%8D.txt");
    }

    #[test]
    fn parse_mntfrom_variants() {
        let p = |s: &str| parse_mntfrom(s).map(|(h, s)| format!("{h}|{s}"));
        assert_eq!(p("//zb@nas/data").as_deref(), Some("nas|data"));
        assert_eq!(p("//nas/data/").as_deref(), Some("nas|data"));
        assert_eq!(
            p("//CORP;zb@nas:1445/My%20Files").as_deref(),
            Some("nas|My Files")
        );
        assert_eq!(p("//GUEST:@nas/Public").as_deref(), Some("nas|Public"));
        assert_eq!(p("//zb@[fe80::1]/s").as_deref(), Some("fe80::1|s"));
        assert_eq!(p("//nas/data/sub"), None);
        assert_eq!(p("/dev/disk3s1"), None);
        assert_eq!(p("//nas/"), None);
        assert_eq!(p("map auto_home"), None);
    }

    #[test]
    fn matches_mount_ignores_case_user_and_encoding() {
        let e = smb("NAS.local", "zb", "My Files", 445);
        assert!(matches_mount(&e, "//zb@nas.local/My%20Files"));
        assert!(matches_mount(&e, "//other@NAS.LOCAL/my%20files"));
        assert!(matches_mount(&e, "//nas.local/MY FILES"));
        assert!(matches_mount(&e, "//zb@nas._smb._tcp.local/My%20Files"));
        assert!(matches_mount(&e, "//zb@nas/My%20Files"));
        assert!(!matches_mount(&e, "//zb@nas2.local/My%20Files"));
        assert!(!matches_mount(&e, "//zb@nas.local/Other"));
        assert!(!matches_mount(&e, "//zb@nas.local/My%20Files/sub"));
        let mut sftp = e.clone();
        sftp.protocol = Protocol::Sftp;
        assert!(!matches_mount(&sftp, "//zb@nas.local/My%20Files"));
    }

    #[test]
    fn mounts_by_ip_match_entries_by_name_and_back() {
        let ip: std::net::IpAddr = "192.168.1.5".parse().unwrap();
        let resolve = move |h: &str| {
            if h == "nas.local" {
                vec![ip]
            } else {
                Vec::new()
            }
        };
        let by_name = smb("nas.local", "zb", "data", 445);
        assert!(matches_mount_resolving(
            &by_name,
            "//zb@192.168.1.5/data",
            &resolve
        ));
        assert!(!matches_mount_resolving(
            &by_name,
            "//zb@192.168.1.6/data",
            &resolve
        ));
        assert!(!matches_mount_resolving(
            &by_name,
            "//zb@192.168.1.5/other",
            &resolve
        ));
        let by_ip = smb("192.168.1.5", "zb", "data", 445);
        assert!(matches_mount_resolving(
            &by_ip,
            "//zb@nas.local/data",
            &resolve
        ));
        // Two names for one host (an alias) match through a shared address; DNS is never
        // asked for a mount of another share.
        let ip6: std::net::IpAddr = "fd00::5".parse().unwrap();
        let alias = move |h: &str| match h {
            "nas.local" => vec![ip],
            "files.example.com" => vec![ip6, ip],
            "other.example.com" => vec!["192.168.1.9".parse().unwrap()],
            _ => Vec::new(),
        };
        assert!(matches_mount_resolving(
            &by_name,
            "//zb@files.example.com/data",
            &alias
        ));
        assert!(!matches_mount_resolving(
            &by_name,
            "//zb@other.example.com/data",
            &alias
        ));
        assert!(!matches_mount_resolving(
            &by_name,
            "//zb@unknown.example.com/data",
            &alias
        ));
        let unresolved = smb("gone.example.com", "zb", "data", 445);
        assert!(!matches_mount_resolving(
            &unresolved,
            "//zb@unknown.example.com/data",
            &alias
        ));
        let never = |_: &str| -> Vec<std::net::IpAddr> { panic!("no DNS for another share") };
        assert!(!matches_mount_resolving(
            &by_name,
            "//zb@other.local/music",
            &never
        ));
        assert!(!matches_mount_resolving(
            &by_ip,
            "//zb@192.168.1.6/data",
            &never
        ));
    }

    #[test]
    fn smb_entries_validate_and_old_files_load() {
        let check = |e: ServerEntry| list_of(vec![ServerNode::Server(e)]).validate();
        assert!(check(smb("nas", "", "data", 445)).is_ok());
        assert_eq!(
            check(smb("nas", "", " ", 445)),
            Err("Share is required".into())
        );
        assert_eq!(
            check(smb("nas", "", "a/b", 445)),
            Err("Share cannot contain '/'".into())
        );
        // A file written before SMB existed has no `share`.
        let old: ServerList = serde_json::from_str(
            r#"{"nodes":[{"kind":"server","id":"0123456789abcdef","name":"m","protocol":"ftp","host":"h","port":21,"user":"u","auth":"password"}]}"#,
        )
        .unwrap();
        assert_eq!(old.find_by_name("m").unwrap().share, "");
        assert!(!old.find_by_name("m").unwrap().kerberos);
        let e = smb("nas", "zb", "data", 445);
        let text = serde_json::to_string(&e).unwrap();
        assert!(text.contains(r#""protocol":"smb""#) && text.contains(r#""share":"data""#));
        assert_eq!(serde_json::from_str::<ServerEntry>(&text).unwrap(), e);
        assert_eq!(Protocol::Smb.scheme(), "smb");
        assert_eq!(Protocol::Smb.default_port(), 445);
    }
}
