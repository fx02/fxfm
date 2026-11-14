//! Saved servers (SFTP/SCP/FTP/FTPS): a tree of entries persisted as `servers.json`.
//! Secrets (passwords, passphrases) are never part of these structures.

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
}

impl Protocol {
    pub fn scheme(&self) -> &'static str {
        match self {
            Protocol::Sftp => "sftp",
            Protocol::Scp => "scp",
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
        }
    }

    pub fn default_port(&self) -> u16 {
        match self {
            Protocol::Sftp | Protocol::Scp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
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
}
