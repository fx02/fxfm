//! Backend-independent layer for servers (SFTP/SCP/FTP/FTPS): location format, the
//! synchronous `RemoteFs` trait, the session registry and a shared tokio runtime.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use thiserror::Error;

use crate::listing::{Entry, ListError};
use crate::ops::OpError;

pub mod ftp;
pub mod known_hosts;
pub mod scp;
pub mod sftp;
#[cfg(test)]
pub(crate) mod testutil;
pub mod transfer;

/// A request or transfer block without progress for this long fails with `Timeout`.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteLoc {
    pub scheme: String,
    pub server: String,
    /// Absolute; `/` is the root.
    pub path: String,
}

/// `sftp://server/path` (`scp`, `ftp`, `ftps` too); `sftp://m` is `sftp://m/`.
pub fn parse_remote(loc: &str) -> Option<RemoteLoc> {
    let (scheme, rest) = loc.split_once("://")?;
    if !matches!(scheme, "sftp" | "scp" | "ftp" | "ftps") {
        return None;
    }
    let (server, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if server.is_empty() {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    let path = if trimmed.is_empty() { "/" } else { trimmed };
    Some(RemoteLoc {
        scheme: scheme.to_string(),
        server: server.to_string(),
        path: path.to_string(),
    })
}

pub fn format_remote(scheme: &str, server: &str, path: &str) -> String {
    if path.starts_with('/') {
        format!("{scheme}://{server}{path}")
    } else {
        format!("{scheme}://{server}/{path}")
    }
}

/// `<scheme>://user@host`, with `:port` when not the protocol's default (IPv6 in brackets).
pub(crate) fn display_url(
    scheme: &str,
    user: &str,
    host: &str,
    port: u16,
    default_port: u16,
) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        format!("{scheme}://{user}@{host}")
    } else {
        format!("{scheme}://{user}@{host}:{port}")
    }
}

/// `dir` + `name` as a remote path.
pub fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Parent of a remote path; the root is its own parent.
pub fn parent_path(path: &str) -> &str {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/",
        Some((p, _)) => p,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteStat {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mtime: i64,
    pub mode: u32,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum RemoteError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("already exists: {0}")]
    Exists(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("connection lost")]
    Disconnected,
    #[error("server not responding")]
    Timeout,
    #[error("operation cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl RemoteError {
    /// The connection is gone or hung (`Disconnected`, `Timeout`): the session is dropped from
    /// the registry and the UI offers to reconnect. One rule for every layer.
    pub fn is_lost(&self) -> bool {
        matches!(self, RemoteError::Disconnected | RemoteError::Timeout)
    }

    pub fn into_list_error(self, loc: &str) -> ListError {
        match self {
            RemoteError::NotFound(_) => ListError::NotFound(loc.to_string()),
            RemoteError::PermissionDenied(_) => ListError::PermissionDenied(loc.to_string()),
            other => ListError::Io(format!("{loc}: {other}")),
        }
    }

    pub fn into_op_error(self) -> OpError {
        match self {
            RemoteError::Cancelled => OpError::Cancelled,
            other => OpError::Io(other.to_string()),
        }
    }
}

/// Synchronous API of a server connection. `progress(bytes_since_last)` returns false to cancel.
pub trait RemoteFs: Send + Sync {
    /// `sftp://user@host` (with `:port` when not the default).
    fn display(&self) -> String;
    fn home(&self) -> Result<String, RemoteError>;
    /// Without `.` and `..`; `hidden` means the name starts with a dot.
    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError>;
    /// lstat; `None` when the path does not exist.
    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError>;
    fn read_link(&self, path: &str) -> Result<String, RemoteError>;
    fn read_to(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError>;
    /// Creates or truncates.
    fn write_from(
        &self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError>;
    fn mkdir(&self, path: &str) -> Result<(), RemoteError>;
    /// Also removes a symlink (not its target).
    fn remove_file(&self, path: &str) -> Result<(), RemoteError>;
    /// Empty folder.
    fn remove_dir(&self, path: &str) -> Result<(), RemoteError>;
    /// `Exists` when `to` exists.
    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError>;
    /// Rename over an existing target.
    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError>;
    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError>;
    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError>;
    /// Whether uploads copy permissions onto what they create (FTP: no; an explicit
    /// `set_mode` stays available).
    fn upload_sets_mode(&self) -> bool {
        true
    }
    /// A connection for a long job (FTP: new login; SSH: a clone of itself).
    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError>;
    fn close(&self);
}

/// Open connections by server name.
#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, Arc<dyn RemoteFs>>>,
}

impl Sessions {
    pub fn new() -> Sessions {
        Sessions::default()
    }

    pub fn insert(&self, name: &str, fs: Arc<dyn RemoteFs>) {
        self.map.lock().unwrap().insert(name.to_string(), fs);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn RemoteFs>> {
        self.map.lock().unwrap().get(name).cloned()
    }

    pub fn remove(&self, name: &str) -> Option<Arc<dyn RemoteFs>> {
        self.map.lock().unwrap().remove(name)
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.map.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }
}

/// Secrets for one connection attempt; never stored in `ServerEntry`.
#[derive(Clone, Default)]
pub struct Credentials {
    pub password: Option<String>,
    pub passphrase: Option<String>,
}

/// Never prints the secrets themselves.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let set = |v: &Option<String>| if v.is_some() { "<set>" } else { "<none>" };
        f.debug_struct("Credentials")
            .field("password", &set(&self.password))
            .field("passphrase", &set(&self.passphrase))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectError {
    /// The host key is unknown (`changed_from: None`) or differs from the stored one.
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed_from: Option<String>,
    },
    /// A password is needed and was not given.
    NeedPassword,
    /// The key is encrypted and no passphrase was given.
    NeedPassphrase,
    /// Credentials were sent and rejected (a password, or a key the server does not accept).
    AuthFailed,
    /// The passphrase does not decrypt the private key (nothing was sent to the server).
    BadPassphrase,
    /// No answer within `CONNECT_TIMEOUT`.
    Timeout,
    /// DNS, refused connection, no common algorithm, unreadable key file, ...
    Failed(String),
}

pub struct ConnectOptions<'a> {
    pub data_dir: &'a Path,
    pub known: &'a known_hosts::KnownHosts,
    /// The fingerprint the user accepted for an unknown or changed host key.
    pub accept_fingerprint: Option<&'a str>,
}

/// Opens a connection with the backend of the server's protocol.
pub fn connect(
    server: &crate::servers::ServerEntry,
    creds: &Credentials,
    opts: &ConnectOptions,
) -> Result<Arc<dyn RemoteFs>, ConnectError> {
    use crate::servers::Protocol;
    match server.protocol {
        Protocol::Sftp => Ok(Arc::new(sftp::SftpFs::connect(server, creds, opts)?)),
        Protocol::Scp => Ok(Arc::new(scp::ScpFs::connect(server, creds, opts)?)),
        Protocol::Ftp | Protocol::Ftps => Ok(Arc::new(ftp::FtpFs::connect(server, creds, opts)?)),
    }
}

/// The runtime shared by all network backends.
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("fxfm-remote")
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `fut` on the shared runtime; blocks the calling (non-async) thread. Elapsed time
/// gives `Timeout`.
pub fn with_timeout<T>(
    fut: impl Future<Output = Result<T, RemoteError>>,
    timeout: Duration,
) -> Result<T, RemoteError> {
    rt().block_on(async {
        match tokio::time::timeout(timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(RemoteError::Timeout),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_variants() {
        let r = parse_remote("sftp://m/var/www").unwrap();
        assert_eq!(
            (r.scheme.as_str(), r.server.as_str(), r.path.as_str()),
            ("sftp", "m", "/var/www")
        );
        assert_eq!(parse_remote("sftp://m").unwrap().path, "/");
        assert_eq!(parse_remote("ftp://a b/x").unwrap().server, "a b");
        assert_eq!(parse_remote("http://x/y"), None);
        assert_eq!(parse_remote("/Users/x"), None);
        assert_eq!(parse_remote("sftp:///x"), None);
        assert_eq!(format_remote("sftp", "m", "/"), "sftp://m/");
        assert_eq!(parent_path("/a/b"), "/a");
        assert_eq!(parent_path("/a"), "/");
        assert_eq!(parent_path("/"), "/");
    }

    #[tokio::test]
    async fn remote_calls_from_inside_tokio_spawn_blocking() {
        let r =
            tokio::task::spawn_blocking(|| with_timeout(async { Ok(1) }, Duration::from_secs(1)))
                .await
                .unwrap();
        assert_eq!(r, Ok(1));
    }

    #[test]
    fn with_timeout_elapses() {
        let r: Result<(), _> = with_timeout(
            async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            },
            Duration::from_millis(50),
        );
        assert_eq!(r, Err(RemoteError::Timeout));
    }
}
