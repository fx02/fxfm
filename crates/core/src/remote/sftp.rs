//! The SSH connection (russh) shared by SFTP and SCP, and the SFTP backend (russh-sftp).
//!
//! The host key is checked inside the russh handshake (`check_server_key`), so no
//! credentials ever reach a server whose key the user has not accepted.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{cipher, kex, mac, ChannelMsg, Disconnect, Preferred};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use tokio::task::JoinHandle;

use super::known_hosts::{fingerprint_sha256, verify_tofu, KnownHosts};
use super::{
    rt, ConnectError, ConnectOptions, Credentials, RemoteError, RemoteFs, RemoteStat,
    CONNECT_TIMEOUT, REQUEST_TIMEOUT,
};
use crate::listing::Entry;
use crate::servers::{key_file, Auth, ServerEntry};

pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
/// Bytes per SFTP read/write request.
const BLOCK: u32 = 32 * 1024;
/// Requests in flight per transfer.
const PIPELINE: usize = 16;
const POSIX_RENAME: &str = "posix-rename@openssh.com";
const RSA_SHA1_NEEDS_LEGACY: &str = "The server only accepts RSA keys signed with SHA-1 (ssh-rsa). Enable \"Allow legacy algorithms (ssh-rsa/SHA1)\" for this server.";

// ---------------------------------------------------------------------------------------
// SSH connection
// ---------------------------------------------------------------------------------------

/// russh client handler: decides on the host key before authentication starts.
pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    known: KnownHosts,
    accept: Option<String>,
    /// Why the handshake was aborted, when the handler aborted it.
    verdict: Arc<Mutex<Option<ConnectError>>>,
}

impl ClientHandler {
    fn reject(&self, e: ConnectError) -> Result<bool, russh::Error> {
        *self.verdict.lock().unwrap() = Some(e);
        Ok(false)
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return self.reject(ConnectError::Failed(
                "The server presented a host certificate; only plain host keys are supported"
                    .into(),
            ));
        };
        let blob = key.to_bytes()?;
        let fingerprint = fingerprint_sha256(&blob);
        let key_type = key.algorithm().as_str().to_string();
        match verify_tofu(
            &self.known,
            &self.host,
            self.port,
            &key_type,
            &fingerprint,
            self.accept.as_deref(),
        ) {
            Ok(()) => Ok(true),
            Err(e) => self.reject(e),
        }
    }
}

/// An authenticated SSH connection.
pub(crate) struct SshSession {
    pub(crate) handle: Handle<ClientHandler>,
    pub(crate) user: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) default_port: u16,
}

impl SshSession {
    /// `<scheme>://user@host` with `:port` when not the protocol's default.
    pub(crate) fn display(&self, scheme: &str) -> String {
        super::display_url(scheme, &self.user, &self.host, self.port, self.default_port)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Starts a subsystem on a new session channel and waits for the server's answer.
    pub(crate) async fn subsystem(
        &self,
        name: &str,
    ) -> Result<russh::Channel<client::Msg>, RemoteError> {
        let mut ch = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| self.ssh_err(e))?;
        ch.request_subsystem(true, name)
            .await
            .map_err(|e| self.ssh_err(e))?;
        loop {
            match ch.wait().await {
                Some(ChannelMsg::Success) => return Ok(ch),
                Some(ChannelMsg::Failure) => {
                    return Err(RemoteError::Unsupported(format!(
                        "The server does not offer the {name} subsystem"
                    )))
                }
                Some(_) => continue,
                None => return Err(RemoteError::Disconnected),
            }
        }
    }

    pub(crate) fn ssh_err(&self, e: russh::Error) -> RemoteError {
        if self.is_closed() {
            RemoteError::Disconnected
        } else {
            match e {
                russh::Error::Disconnect
                | russh::Error::SendError
                | russh::Error::ConnectionTimeout
                | russh::Error::HUP => RemoteError::Disconnected,
                e => RemoteError::Other(e.to_string()),
            }
        }
    }

    pub(crate) async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}

fn preferred(legacy: bool) -> Preferred {
    let d = Preferred::DEFAULT;
    if !legacy {
        // ssh-rsa (SHA-1) only with the legacy option.
        let key: Vec<Algorithm> = d
            .key
            .iter()
            .filter(|a| **a != Algorithm::Rsa { hash: None })
            .cloned()
            .collect();
        return Preferred {
            key: Cow::Owned(key),
            ..d
        };
    }
    let mut kex_list: Vec<kex::Name> = d.kex.to_vec();
    let at = kex_list
        .iter()
        .position(|n| *n == kex::EXTENSION_SUPPORT_AS_CLIENT)
        .unwrap_or(kex_list.len());
    kex_list.splice(at..at, [kex::DH_G14_SHA1, kex::DH_G1_SHA1]);
    let mut ciphers: Vec<cipher::Name> = d.cipher.to_vec();
    ciphers.extend([
        cipher::AES_256_CBC,
        cipher::AES_192_CBC,
        cipher::AES_128_CBC,
        cipher::TRIPLE_DES_CBC,
    ]);
    let mut macs: Vec<mac::Name> = d.mac.to_vec();
    macs.extend([mac::HMAC_SHA1_ETM, mac::HMAC_SHA1]);
    Preferred {
        kex: Cow::Owned(kex_list),
        cipher: Cow::Owned(ciphers),
        mac: Cow::Owned(macs),
        ..d
    }
}

fn client_config(legacy: bool) -> client::Config {
    client::Config {
        preferred: preferred(legacy),
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: 3,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    }
}

fn connect_err(e: russh::Error, legacy: bool) -> ConnectError {
    match e {
        russh::Error::NoCommonAlgo { .. } if !legacy => ConnectError::Failed(format!(
            "{e}. The server may only offer old algorithms; enable \"Allow legacy algorithms (ssh-rsa/SHA1)\" for this server."
        )),
        russh::Error::ConnectionTimeout => ConnectError::Timeout,
        russh::Error::IO(io) => ConnectError::Failed(io.to_string()),
        e => ConnectError::Failed(e.to_string()),
    }
}

fn key_is_encrypted(text: &str) -> bool {
    if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
        || text.contains("Proc-Type: 4,ENCRYPTED")
    {
        return true;
    }
    if text.trim_start().starts_with("PuTTY-User-Key-File-") {
        return text
            .lines()
            .find_map(|l| l.strip_prefix("Encryption:"))
            .is_some_and(|v| v.trim() != "none");
    }
    if text.contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
        return PrivateKey::from_openssh(text).is_ok_and(|k| k.is_encrypted());
    }
    false
}

fn load_key(
    server: &ServerEntry,
    creds: &Credentials,
    opts: &ConnectOptions,
) -> Result<PrivateKey, ConnectError> {
    let path = key_file(server, opts.data_dir)
        .ok_or_else(|| ConnectError::Failed("No private key file is set for this server".into()))?;
    let text = std::fs::read_to_string(&path).map_err(|e| {
        ConnectError::Failed(format!("Cannot read key file {}: {e}", path.display()))
    })?;
    let passphrase = creds.passphrase.as_deref().filter(|p| !p.is_empty());
    let encrypted = key_is_encrypted(&text);
    if encrypted && passphrase.is_none() {
        return Err(ConnectError::NeedPassphrase);
    }
    match russh::keys::decode_secret_key(&text, if encrypted { passphrase } else { None }) {
        Ok(k) => Ok(k),
        Err(russh::keys::Error::KeyIsEncrypted) => Err(ConnectError::NeedPassphrase),
        // A passphrase that does not decrypt the key.
        Err(_) if encrypted => Err(ConnectError::BadPassphrase),
        Err(e) => Err(ConnectError::Failed(format!(
            "Cannot read key file {}: {e}",
            path.display()
        ))),
    }
}

/// The hash for an RSA user key, from the server's best `server-sig-algs` entry
/// (`None` = the server did not say; `Some(None)` = only `ssh-rsa`, SHA-1).
fn rsa_hash_choice(
    server_offer: Option<Option<HashAlg>>,
    legacy: bool,
) -> Result<Option<HashAlg>, ConnectError> {
    match server_offer {
        Some(Some(h)) => Ok(Some(h)),
        // SHA-1 signatures for the user key only with the legacy option.
        Some(None) if legacy => Ok(None),
        Some(None) => Err(ConnectError::Failed(RSA_SHA1_NEEDS_LEGACY.into())),
        // No server-sig-algs: old servers only know ssh-rsa.
        None if legacy => Ok(None),
        None => Ok(Some(HashAlg::Sha256)),
    }
}

async fn ssh_connect_async(
    server: &ServerEntry,
    creds: &Credentials,
    opts: &ConnectOptions<'_>,
) -> Result<SshSession, ConnectError> {
    let legacy = server.legacy_algorithms;
    let verdict = Arc::new(Mutex::new(None));
    let handler = ClientHandler {
        host: server.host.clone(),
        port: server.port,
        known: opts.known.clone(),
        accept: opts.accept_fingerprint.map(str::to_string),
        verdict: verdict.clone(),
    };
    let config = Arc::new(client_config(legacy));
    let handle = match client::connect(config, (server.host.as_str(), server.port), handler).await {
        Ok(h) => h,
        Err(e) => {
            let v = verdict.lock().unwrap().take();
            return Err(v.unwrap_or_else(|| connect_err(e, legacy)));
        }
    };
    let mut ssh = SshSession {
        handle,
        user: server.user.clone(),
        host: server.host.clone(),
        port: server.port,
        default_port: server.protocol.default_port(),
    };
    match authenticate(&mut ssh, server, creds, opts, legacy).await {
        Ok(()) => Ok(ssh),
        Err(e) => {
            ssh.disconnect().await;
            Err(e)
        }
    }
}

async fn authenticate(
    ssh: &mut SshSession,
    server: &ServerEntry,
    creds: &Credentials,
    opts: &ConnectOptions<'_>,
    legacy: bool,
) -> Result<(), ConnectError> {
    let user = server.user.clone();
    let result = match server.auth {
        Auth::Password => {
            let Some(pw) = creds.password.as_deref() else {
                return Err(ConnectError::NeedPassword);
            };
            ssh.handle.authenticate_password(user, pw).await
        }
        Auth::Key | Auth::KeyPassphrase => {
            let key = load_key(server, creds, opts)?;
            let hash = if key.algorithm().is_rsa() {
                let offer = ssh.handle.best_supported_rsa_hash().await.ok().flatten();
                rsa_hash_choice(offer, legacy)?
            } else {
                None
            };
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            ssh.handle.authenticate_publickey(user, key).await
        }
    };
    match result {
        Ok(r) if r.success() => Ok(()),
        Ok(_) => Err(ConnectError::AuthFailed),
        Err(e) => Err(connect_err(e, legacy)),
    }
}

/// Connects, checks the host key and authenticates (`CONNECT_TIMEOUT` overall).
pub(crate) fn ssh_connect(
    server: &ServerEntry,
    creds: &Credentials,
    opts: &ConnectOptions,
) -> Result<SshSession, ConnectError> {
    rt().block_on(async {
        tokio::time::timeout(CONNECT_TIMEOUT, ssh_connect_async(server, creds, opts))
            .await
            .unwrap_or(Err(ConnectError::Timeout))
    })
}

// ---------------------------------------------------------------------------------------
// SFTP
// ---------------------------------------------------------------------------------------

struct Inner {
    ssh: SshSession,
    sftp: RawSftpSession,
    display: String,
    posix_rename: AtomicBool,
    timeout_ms: AtomicU64,
    closed: AtomicBool,
}

/// SFTP over one SSH connection. Clones from `fork()` share the connection (SFTP
/// multiplexes requests); only the original closes it.
pub struct SftpFs {
    inner: Arc<Inner>,
    owner: bool,
}

type ReadTask = Task<Result<Vec<u8>, SftpError>>;
type WriteTask = Task<Result<(), SftpError>>;

/// Aborts the spawned request when dropped (cancel, error or timeout).
struct Task<T>(JoinHandle<T>);

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> Task<Result<T, SftpError>> {
    async fn join(&mut self) -> Result<T, SftpError> {
        match (&mut self.0).await {
            Ok(r) => r,
            Err(e) => Err(SftpError::UnexpectedBehavior(format!("request task: {e}"))),
        }
    }
}

fn sftp_config(timeout: Duration) -> russh_sftp::client::Config {
    russh_sftp::client::Config {
        // Our own timeout (`req`) fires first; this one only cleans up.
        request_timeout_secs: timeout.as_secs() + 5,
        ..Default::default()
    }
}

fn is_eof(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

fn is_not_found(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::NoSuchFile)
}

fn to_stat(a: &FileAttributes) -> RemoteStat {
    let mode = a.permissions.unwrap_or(0);
    RemoteStat {
        is_dir: mode & 0o170000 == 0o040000,
        is_symlink: mode & 0o170000 == 0o120000,
        size: a.size.unwrap_or(0),
        mtime: a.mtime.map(i64::from).unwrap_or(0),
        mode,
    }
}

/// Two SSH strings, as `posix-rename@openssh.com` expects.
fn encode_two_paths(a: &str, b: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + a.len() + b.len());
    for s in [a, b] {
        v.extend_from_slice(&(s.len() as u32).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
    }
    v
}

/// Fills `buf` as far as the reader allows; 0 means end of input.
fn read_full(input: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

impl SftpFs {
    /// Connects over SSH and starts the SFTP subsystem.
    pub fn connect(
        server: &ServerEntry,
        creds: &Credentials,
        opts: &ConnectOptions,
    ) -> Result<SftpFs, ConnectError> {
        let ssh = ssh_connect(server, creds, opts)?;
        SftpFs::from_session(ssh, "sftp").map_err(|e| match e {
            RemoteError::Timeout => ConnectError::Timeout,
            e => ConnectError::Failed(e.to_string()),
        })
    }

    /// SFTP on an authenticated connection (also used by SCP for listings).
    pub(crate) fn from_session(ssh: SshSession, scheme: &str) -> Result<SftpFs, RemoteError> {
        SftpFs::try_from_session(ssh, scheme).map_err(|(ssh, e)| {
            rt().block_on(ssh.disconnect());
            e
        })
    }

    /// Like `from_session`, but hands the connection back when SFTP does not start.
    pub(crate) fn try_from_session(
        ssh: SshSession,
        scheme: &str,
    ) -> Result<SftpFs, (Box<SshSession>, RemoteError)> {
        let opened = rt().block_on(async {
            tokio::time::timeout(CONNECT_TIMEOUT, async {
                let ch = ssh.subsystem("sftp").await?;
                let sftp =
                    RawSftpSession::new_with_config(ch.into_stream(), sftp_config(REQUEST_TIMEOUT));
                let version = sftp.init().await.map_err(|e| {
                    if ssh.is_closed() {
                        RemoteError::Disconnected
                    } else {
                        RemoteError::Other(format!("SFTP: {e}"))
                    }
                })?;
                let posix = version
                    .extensions
                    .get(POSIX_RENAME)
                    .is_some_and(|v| v == "1");
                Ok((sftp, posix))
            })
            .await
            .unwrap_or(Err(RemoteError::Timeout))
        });
        let (sftp, posix_rename) = match opened {
            Ok(o) => o,
            Err(e) => return Err((Box::new(ssh), e)),
        };
        Ok(SftpFs {
            inner: Arc::new(Inner {
                display: ssh.display(scheme),
                ssh,
                sftp,
                posix_rename: AtomicBool::new(posix_rename),
                timeout_ms: AtomicU64::new(REQUEST_TIMEOUT.as_millis() as u64),
                closed: AtomicBool::new(false),
            }),
            owner: true,
        })
    }

    /// The SSH connection under this SFTP session.
    pub(crate) fn ssh(&self) -> &SshSession {
        &self.inner.ssh
    }

    #[cfg(test)]
    pub(crate) fn set_request_timeout(&self, t: Duration) {
        self.inner
            .timeout_ms
            .store(t.as_millis() as u64, Ordering::SeqCst);
        self.inner.sftp.set_timeout(t.as_secs() + 5);
    }

    /// Forces the remove-then-rename fallback of `replace`.
    #[cfg(test)]
    pub(crate) fn disable_posix_rename(&self) {
        self.inner.posix_rename.store(false, Ordering::SeqCst);
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.inner.timeout_ms.load(Ordering::SeqCst))
    }

    /// Runs one request on the shared runtime with the request timeout.
    fn req<T>(&self, fut: impl Future<Output = Result<T, SftpError>>) -> Result<T, SftpError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(SftpError::UnexpectedBehavior("session closed".into()));
        }
        let t = self.timeout();
        rt().block_on(async {
            tokio::time::timeout(t, fut)
                .await
                .unwrap_or(Err(SftpError::Timeout))
        })
    }

    fn err(&self, e: SftpError, path: &str) -> RemoteError {
        let dead = self.inner.closed.load(Ordering::SeqCst) || self.inner.ssh.is_closed();
        match e {
            SftpError::Timeout => RemoteError::Timeout,
            SftpError::Status(s) => match s.status_code {
                StatusCode::NoSuchFile => RemoteError::NotFound(path.to_string()),
                StatusCode::PermissionDenied => RemoteError::PermissionDenied(path.to_string()),
                StatusCode::NoConnection | StatusCode::ConnectionLost => RemoteError::Disconnected,
                code => {
                    let msg = if s.error_message.is_empty() {
                        code.to_string()
                    } else {
                        s.error_message
                    };
                    RemoteError::Other(format!("{path}: {msg}"))
                }
            },
            // The channel or the connection is gone (send/receive on closed pipes).
            SftpError::IO(_) | SftpError::UnexpectedBehavior(_) if dead => {
                RemoteError::Disconnected
            }
            SftpError::UnexpectedBehavior(m)
                if m.contains("SendError")
                    || m.contains("RecvError")
                    || m.contains("session closed") =>
            {
                RemoteError::Disconnected
            }
            e => RemoteError::Other(format!("{path}: {e}")),
        }
    }

    fn call<T>(
        &self,
        path: &str,
        fut: impl Future<Output = Result<T, SftpError>>,
    ) -> Result<T, RemoteError> {
        self.req(fut).map_err(|e| self.err(e, path))
    }

    /// Closes a file handle in the background (error paths).
    fn close_handle_later(&self, handle: String) {
        let inner = self.inner.clone();
        rt().spawn(async move {
            let _ = inner.sftp.close(handle).await;
        });
    }

    fn spawn_read(&self, handle: &str, off: u64) -> ReadTask {
        let inner = self.inner.clone();
        let handle = handle.to_string();
        Task(rt().spawn(async move { inner.sftp.read(handle, off, BLOCK).await.map(|d| d.data) }))
    }

    fn spawn_write(&self, handle: &str, off: u64, data: Vec<u8>) -> WriteTask {
        let inner = self.inner.clone();
        let handle = handle.to_string();
        Task(rt().spawn(async move { inner.sftp.write(handle, off, data).await.map(|_| ()) }))
    }

    fn read_blocks(
        &self,
        path: &str,
        handle: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let mut queue: VecDeque<(u64, ReadTask)> = VecDeque::new();
        let mut next = 0u64;
        loop {
            while queue.len() < PIPELINE {
                queue.push_back((next, self.spawn_read(handle, next)));
                next += BLOCK as u64;
            }
            let (off, mut task) = queue.pop_front().expect("queue is filled above");
            let data = match self.req(task.join()) {
                Ok(d) => d,
                Err(e) if is_eof(&e) => return Ok(()),
                Err(e) => return Err(self.err(e, path)),
            };
            if data.is_empty() {
                return Ok(());
            }
            out.write_all(&data)
                .map_err(|e| RemoteError::Other(format!("write: {e}")))?;
            if !progress(data.len() as u64) {
                return Err(RemoteError::Cancelled);
            }
            if data.len() < BLOCK as usize {
                // Short read: requests already sent for later offsets assumed full
                // blocks; drop them and continue right after what arrived.
                queue.clear();
                next = off + data.len() as u64;
            }
        }
    }

    fn write_blocks(
        &self,
        path: &str,
        handle: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let mut queue: VecDeque<(u64, WriteTask)> = VecDeque::new();
        let mut off = 0u64;
        let mut eof = false;
        loop {
            while !eof && queue.len() < PIPELINE {
                let mut buf = vec![0u8; BLOCK as usize];
                let n = read_full(input, &mut buf)
                    .map_err(|e| RemoteError::Other(format!("read: {e}")))?;
                if n == 0 {
                    eof = true;
                    break;
                }
                buf.truncate(n);
                queue.push_back((n as u64, self.spawn_write(handle, off, buf)));
                off += n as u64;
                if n < BLOCK as usize {
                    eof = true;
                }
            }
            let Some((n, mut task)) = queue.pop_front() else {
                return Ok(());
            };
            self.req(task.join()).map_err(|e| self.err(e, path))?;
            if !progress(n) {
                return Err(RemoteError::Cancelled);
            }
        }
    }

    fn lstat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        match self.req(self.inner.sftp.lstat(path)) {
            Ok(a) => Ok(Some(to_stat(&a.attrs))),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(self.err(e, path)),
        }
    }

    fn plain_rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        self.call(from, self.inner.sftp.rename(from, to))
            .map(|_| ())
    }
}

impl RemoteFs for SftpFs {
    fn display(&self) -> String {
        self.inner.display.clone()
    }

    fn home(&self) -> Result<String, RemoteError> {
        let name = self.call(".", self.inner.sftp.realpath("."))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| RemoteError::Other("empty realpath reply".into()))
    }

    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError> {
        let handle = self.call(path, self.inner.sftp.opendir(path))?.handle;
        let mut out = Vec::new();
        loop {
            match self.req(self.inner.sftp.readdir(handle.as_str())) {
                Ok(name) => {
                    for f in name.files {
                        if f.filename == "." || f.filename == ".." {
                            continue;
                        }
                        let st = to_stat(&f.attrs);
                        out.push(Entry {
                            hidden: f.filename.starts_with('.'),
                            name: f.filename,
                            is_dir: st.is_dir,
                            is_symlink: st.is_symlink,
                            size: st.size,
                            mtime: st.mtime,
                            mode: st.mode,
                        });
                    }
                }
                Err(e) if is_eof(&e) => break,
                Err(e) => {
                    self.close_handle_later(handle);
                    return Err(self.err(e, path));
                }
            }
        }
        self.call(path, self.inner.sftp.close(handle))?;
        Ok(out)
    }

    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        self.lstat(path)
    }

    fn read_link(&self, path: &str) -> Result<String, RemoteError> {
        let name = self.call(path, self.inner.sftp.readlink(path))?;
        name.files
            .into_iter()
            .next()
            .map(|f| f.filename)
            .ok_or_else(|| RemoteError::Other(format!("{path}: empty readlink reply")))
    }

    fn read_to(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let handle = self
            .call(
                path,
                self.inner
                    .sftp
                    .open(path, OpenFlags::READ, FileAttributes::empty()),
            )?
            .handle;
        match self.read_blocks(path, &handle, out, progress) {
            Ok(()) => self.call(path, self.inner.sftp.close(handle)).map(|_| ()),
            Err(e) => {
                self.close_handle_later(handle);
                Err(e)
            }
        }
    }

    fn write_from(
        &self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE;
        let handle = self
            .call(
                path,
                self.inner.sftp.open(path, flags, FileAttributes::empty()),
            )?
            .handle;
        match self.write_blocks(path, &handle, input, progress) {
            Ok(()) => self.call(path, self.inner.sftp.close(handle)).map(|_| ()),
            Err(e) => {
                self.close_handle_later(handle);
                Err(e)
            }
        }
    }

    fn mkdir(&self, path: &str) -> Result<(), RemoteError> {
        self.call(path, self.inner.sftp.mkdir(path, FileAttributes::empty()))
            .map(|_| ())
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        self.call(path, self.inner.sftp.remove(path)).map(|_| ())
    }

    fn remove_dir(&self, path: &str) -> Result<(), RemoteError> {
        self.call(path, self.inner.sftp.rmdir(path)).map(|_| ())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        // SFTP v3 (OpenSSH) answers a taken target with a generic failure.
        if self.lstat(to)?.is_some() {
            return Err(RemoteError::Exists(to.to_string()));
        }
        self.plain_rename(from, to)
    }

    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        if self.inner.posix_rename.load(Ordering::SeqCst) {
            let reply = self.call(
                from,
                self.inner
                    .sftp
                    .extended(POSIX_RENAME, encode_two_paths(from, to)),
            )?;
            return match reply {
                Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(()),
                Packet::Status(s) => Err(self.err(SftpError::Status(s), from)),
                _ => Err(RemoteError::Other(format!(
                    "{from}: unexpected rename reply"
                ))),
            };
        }
        // Without posix-rename: move the target aside first, so a failing rename can put it
        // back (removing it first would lose it if the rename then fails).
        if self.lstat(to)?.is_some() {
            let bak = super::ftp::backup_path(to);
            if self.plain_rename(to, &bak).is_ok() {
                if let Err(e) = self.plain_rename(from, to) {
                    return match self.plain_rename(&bak, to) {
                        Ok(()) => Err(e),
                        Err(_) => Err(RemoteError::Other(format!(
                            "{to}: the replace failed and the original is kept as {bak}"
                        ))),
                    };
                }
                if self.req(self.inner.sftp.remove(&bak)).is_err() {
                    log::warn!("cannot delete the replace backup {bak}");
                }
                return Ok(());
            }
        }
        // The target cannot be moved aside (or is gone): the old way.
        match self.req(self.inner.sftp.remove(to)) {
            Ok(_) => {}
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(self.err(e, to)),
        }
        self.plain_rename(from, to)
    }

    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError> {
        // OpenSSH's sftp-server takes (target, link), the reverse of the draft's
        // (linkpath, targetpath); every common server follows OpenSSH.
        self.call(link, self.inner.sftp.symlink(target, link))
            .map(|_| ())
    }

    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError> {
        let attrs = FileAttributes {
            permissions: Some(mode & 0o7777),
            ..FileAttributes::empty()
        };
        self.call(path, self.inner.sftp.setstat(path, attrs))
            .map(|_| ())
    }

    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError> {
        if self.inner.closed.load(Ordering::SeqCst) || self.inner.ssh.is_closed() {
            return Err(RemoteError::Disconnected);
        }
        Ok(Arc::new(SftpFs {
            inner: self.inner.clone(),
            owner: false,
        }))
    }

    fn close(&self) {
        if !self.owner || self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.inner.sftp.close_session();
        let inner = self.inner.clone();
        rt().spawn(async move {
            inner.ssh.disconnect().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::known_hosts::KNOWN_HOSTS_FILE;
    use crate::remote::testutil::{
        have_sftp_server, random_ed25519, write_openssh_key, SshTestServer,
    };
    use crate::remote::{connect, Credentials};
    use crate::servers::{Auth, Protocol};
    use std::fs;
    use std::time::Instant;

    struct Ctx {
        srv: SshTestServer,
        data: tempfile::TempDir,
        known: KnownHosts,
    }

    impl Ctx {
        fn opts(&self, accept: Option<&'static str>) -> ConnectOptions<'_> {
            ConnectOptions {
                data_dir: self.data.path(),
                known: &self.known,
                accept_fingerprint: accept,
            }
        }

        fn trust(&self) {
            self.known
                .trust(
                    "127.0.0.1",
                    self.srv.port,
                    &self.srv.host_key_type(),
                    &self.srv.host_fingerprint(),
                )
                .unwrap();
        }

        fn root(&self) -> String {
            self.srv.root.to_str().unwrap().to_string()
        }

        fn known_hosts_text(&self) -> Option<String> {
            fs::read_to_string(self.data.path().join(KNOWN_HOSTS_FILE)).ok()
        }

        /// Connected (trusted host key, password `pw`).
        fn fs(&self) -> SftpFs {
            self.trust();
            SftpFs::connect(&self.srv.entry(), &pw(), &self.opts(None)).unwrap()
        }
    }

    fn setup() -> Option<Ctx> {
        if !have_sftp_server() {
            return None;
        }
        let data = tempfile::tempdir().unwrap();
        let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
        Some(Ctx {
            srv: SshTestServer::start(),
            data,
            known,
        })
    }

    fn pw() -> Credentials {
        Credentials {
            password: Some("pw".into()),
            passphrase: None,
        }
    }

    fn noop(_: u64) -> bool {
        true
    }

    fn put(fs: &dyn RemoteFs, path: &str, data: &[u8]) {
        fs.write_from(path, &mut &data[..], &mut noop).unwrap();
    }

    fn get(fs: &dyn RemoteFs, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        fs.read_to(path, &mut out, &mut noop).unwrap();
        out
    }

    #[test]
    fn sftp_password_login_and_list() {
        let Some(c) = setup() else { return };
        fs::write(c.srv.root.join("a.txt"), b"hello").unwrap();
        fs::write(c.srv.root.join(".hidden"), b"x").unwrap();
        fs::create_dir(c.srv.root.join("sub")).unwrap();
        fs::write(c.srv.root.join("big.bin"), vec![7u8; 100_000]).unwrap();
        let sftp = c.fs();
        assert_eq!(
            sftp.display(),
            format!("sftp://tester@127.0.0.1:{}", c.srv.port)
        );
        assert_eq!(sftp.home().unwrap(), c.root());

        let mut got: Vec<(String, bool, u64, bool)> = sftp
            .list(&c.root())
            .unwrap()
            .into_iter()
            .map(|e| {
                (
                    e.name,
                    e.is_dir,
                    if e.is_dir { 0 } else { e.size },
                    e.hidden,
                )
            })
            .collect();
        got.sort();
        let mut want: Vec<(String, bool, u64, bool)> = fs::read_dir(&c.srv.root)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let md = e.metadata().unwrap();
                let name = e.file_name().into_string().unwrap();
                let hidden = name.starts_with('.');
                (
                    name,
                    md.is_dir(),
                    if md.is_dir() { 0 } else { md.len() },
                    hidden,
                )
            })
            .collect();
        want.sort();
        assert_eq!(got, want);
        assert_eq!(
            sftp.list(&format!("{}/nope", c.root())),
            Err(RemoteError::NotFound(format!("{}/nope", c.root())))
        );
        sftp.close();
        sftp.close();
    }

    #[test]
    fn sftp_key_login_ed25519_and_encrypted_pem() {
        let Some(c) = setup() else { return };
        // OpenSSH ed25519 key, imported under keys/.
        let key = random_ed25519();
        fs::create_dir_all(c.data.path().join("keys")).unwrap();
        write_openssh_key(&key, &c.data.path().join("keys/id_ed25519"));
        c.srv.allow_key(key.public_key().clone());
        c.trust();
        let mut e = c.srv.entry();
        e.auth = Auth::Key;
        e.key_path = Some("keys/id_ed25519".into());
        let fs1 = SftpFs::connect(&e, &Credentials::default(), &c.opts(None)).unwrap();
        assert!(fs1.list(&c.root()).is_ok());
        fs1.close();

        // Encrypted PKCS#8 (PEM) from ssh-keygen. OpenSSH writes ed25519 keys in its own
        // format even with `-m PKCS8`, so this one is RSA.
        let kp = c.data.path().join("rsa_pkcs8");
        let made = std::process::Command::new("ssh-keygen")
            .args([
                "-q", "-t", "rsa", "-b", "2048", "-m", "PKCS8", "-N", "secret", "-C", "", "-f",
            ])
            .arg(&kp)
            .status();
        if !matches!(made, Ok(s) if s.success()) {
            eprintln!("skipped: no ssh-keygen");
            return;
        }
        let text = fs::read_to_string(&kp).unwrap();
        assert!(
            text.starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----"),
            "{text}"
        );
        let pubkey = fs::read_to_string(kp.with_extension("pub")).unwrap();
        let mut e = c.srv.entry();
        e.auth = Auth::KeyPassphrase;
        e.key_path = Some(kp.to_str().unwrap().into());
        // The right passphrase, but the server does not accept the key: a rejected login,
        // not a wrong passphrase.
        let right = Credentials {
            password: None,
            passphrase: Some("secret".into()),
        };
        assert_eq!(
            SftpFs::connect(&e, &right, &c.opts(None)).err(),
            Some(ConnectError::AuthFailed)
        );
        c.srv
            .allow_key(russh::keys::PublicKey::from_openssh(pubkey.trim()).unwrap());
        assert_eq!(
            SftpFs::connect(&e, &Credentials::default(), &c.opts(None)).err(),
            Some(ConnectError::NeedPassphrase)
        );
        let wrong = Credentials {
            password: None,
            passphrase: Some("wrong".into()),
        };
        assert_eq!(
            SftpFs::connect(&e, &wrong, &c.opts(None)).err(),
            Some(ConnectError::BadPassphrase)
        );
        let fs2 = SftpFs::connect(&e, &right, &c.opts(None)).unwrap();
        assert!(fs2.list(&c.root()).is_ok());
    }

    #[test]
    fn unknown_host_key_asks_then_trusts() {
        let Some(c) = setup() else { return };
        let fp = c.srv.host_fingerprint();
        let first = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(None)).err();
        assert_eq!(
            first,
            Some(ConnectError::HostKey {
                host: "127.0.0.1".into(),
                port: c.srv.port,
                key_type: "ssh-ed25519".into(),
                fingerprint: fp.clone(),
                changed_from: None,
            })
        );
        assert_eq!(c.srv.auth_requests(), 0);
        assert_eq!(c.known_hosts_text(), None);

        let accept: &'static str = Box::leak(fp.clone().into_boxed_str());
        let fs2 = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(Some(accept))).unwrap();
        fs2.close();
        assert_eq!(
            c.known_hosts_text().unwrap(),
            format!("127.0.0.1:{} ssh-ed25519 {fp}\n", c.srv.port)
        );
        let fs3 = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(None)).unwrap();
        assert!(fs3.home().is_ok());
    }

    #[test]
    fn changed_host_key_refuses_before_auth() {
        let Some(c) = setup() else { return };
        let old = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        c.known
            .trust("127.0.0.1", c.srv.port, "ssh-ed25519", old)
            .unwrap();
        let r = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(None)).err();
        assert_eq!(
            r,
            Some(ConnectError::HostKey {
                host: "127.0.0.1".into(),
                port: c.srv.port,
                key_type: "ssh-ed25519".into(),
                fingerprint: c.srv.host_fingerprint(),
                changed_from: Some(old.into()),
            })
        );
        assert_eq!(c.srv.auth_requests(), 0);
    }

    #[test]
    fn accepting_requires_matching_fingerprint() {
        let Some(c) = setup() else { return };
        let other = SshTestServer::start();
        let other_fp: &'static str = Box::leak(other.host_fingerprint().into_boxed_str());
        let r = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(Some(other_fp))).err();
        assert!(
            matches!(
                r,
                Some(ConnectError::HostKey {
                    changed_from: None,
                    ..
                })
            ),
            "{r:?}"
        );
        assert_eq!(c.known_hosts_text(), None);
        assert_eq!(c.srv.auth_requests(), 0);
    }

    #[test]
    fn wrong_password_is_auth_failed() {
        let Some(c) = setup() else { return };
        c.trust();
        let bad = Credentials {
            password: Some("nope".into()),
            passphrase: None,
        };
        assert_eq!(
            SftpFs::connect(&c.srv.entry(), &bad, &c.opts(None)).err(),
            Some(ConnectError::AuthFailed)
        );
    }

    #[test]
    fn missing_password_is_need_password() {
        let Some(c) = setup() else { return };
        c.trust();
        assert_eq!(
            SftpFs::connect(&c.srv.entry(), &Credentials::default(), &c.opts(None)).err(),
            Some(ConnectError::NeedPassword)
        );
        assert_eq!(c.srv.auth_requests(), 0);
    }

    #[test]
    fn sftp_file_ops_roundtrip() {
        let Some(c) = setup() else { return };
        let sftp = c.fs();
        let r = c.root();
        let p = |n: &str| format!("{r}/{n}");

        // 300 KB so reads and writes span many pipelined blocks.
        let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        put(&sftp, &p("f.bin"), &data);
        assert_eq!(fs::read(c.srv.root.join("f.bin")).unwrap(), data);
        assert_eq!(get(&sftp, &p("f.bin")), data);
        put(&sftp, &p("f.bin"), b"short");
        assert_eq!(get(&sftp, &p("f.bin")), b"short");
        put(&sftp, &p("empty"), b"");
        assert_eq!(get(&sftp, &p("empty")), b"");
        assert!(matches!(
            sftp.read_to(&p("missing"), &mut Vec::new(), &mut noop),
            Err(RemoteError::NotFound(_))
        ));

        // Cancel from progress.
        let mut cancel = |_: u64| false;
        assert_eq!(
            sftp.read_to(&p("f.bin"), &mut Vec::new(), &mut cancel),
            Err(RemoteError::Cancelled)
        );

        let st = sftp.stat(&p("f.bin")).unwrap().unwrap();
        assert!(!st.is_dir && !st.is_symlink);
        assert_eq!(st.size, 5);
        assert_eq!(sftp.stat(&p("missing")).unwrap(), None);

        sftp.mkdir(&p("d")).unwrap();
        assert!(sftp.stat(&p("d")).unwrap().unwrap().is_dir);
        assert!(c.srv.root.join("d").is_dir());

        // rename: Exists when the target exists, nothing changes.
        put(&sftp, &p("g.txt"), b"g");
        assert_eq!(
            sftp.rename(&p("f.bin"), &p("g.txt")),
            Err(RemoteError::Exists(p("g.txt")))
        );
        assert_eq!(get(&sftp, &p("f.bin")), b"short");
        sftp.rename(&p("f.bin"), &p("d/f2.bin")).unwrap();
        assert!(!c.srv.root.join("f.bin").exists());
        assert_eq!(fs::read(c.srv.root.join("d/f2.bin")).unwrap(), b"short");

        // replace over an existing file.
        sftp.replace(&p("d/f2.bin"), &p("g.txt")).unwrap();
        assert_eq!(fs::read(c.srv.root.join("g.txt")).unwrap(), b"short");
        assert!(!c.srv.root.join("d/f2.bin").exists());
        // replace onto a new name works too.
        sftp.replace(&p("g.txt"), &p("h.txt")).unwrap();
        assert_eq!(fs::read(c.srv.root.join("h.txt")).unwrap(), b"short");
        // Without posix-rename: remove the target, then rename.
        assert!(sftp.inner.posix_rename.load(Ordering::SeqCst));
        sftp.disable_posix_rename();
        put(&sftp, &p("i.txt"), b"i");
        sftp.replace(&p("i.txt"), &p("h.txt")).unwrap();
        assert_eq!(fs::read(c.srv.root.join("h.txt")).unwrap(), b"i");
        assert!(!c.srv.root.join("i.txt").exists());
        // The target is moved aside first: a rename that fails puts it back, and no backup
        // is left behind either way.
        assert!(sftp.replace(&p("missing.txt"), &p("h.txt")).is_err());
        assert_eq!(fs::read(c.srv.root.join("h.txt")).unwrap(), b"i");
        let leftovers = |root: &std::path::Path| {
            fs::read_dir(root)
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".bak")
                })
                .count()
        };
        assert_eq!(leftovers(&c.srv.root), 0);
        sftp.replace(&p("h.txt"), &p("j.txt")).unwrap();
        sftp.replace(&p("j.txt"), &p("h.txt")).unwrap();

        sftp.set_mode(&p("h.txt"), 0o640).unwrap();
        assert_eq!(
            sftp.stat(&p("h.txt")).unwrap().unwrap().mode & 0o7777,
            0o640
        );
        {
            use std::os::unix::fs::PermissionsExt;
            let m = fs::metadata(c.srv.root.join("h.txt"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o7777, 0o640);
        }

        sftp.symlink("h.txt", &p("link")).unwrap();
        assert_eq!(
            fs::read_link(c.srv.root.join("link")).unwrap().to_str(),
            Some("h.txt")
        );
        assert_eq!(sftp.read_link(&p("link")).unwrap(), "h.txt");
        let ls = sftp.stat(&p("link")).unwrap().unwrap();
        assert!(ls.is_symlink && !ls.is_dir);
        let listed = sftp.list(&r).unwrap();
        assert!(listed.iter().any(|e| e.name == "link" && e.is_symlink));

        sftp.remove_file(&p("link")).unwrap();
        assert!(fs::symlink_metadata(c.srv.root.join("link")).is_err());
        assert!(c.srv.root.join("h.txt").exists());
        sftp.remove_file(&p("h.txt")).unwrap();
        assert!(matches!(
            sftp.remove_file(&p("h.txt")),
            Err(RemoteError::NotFound(_))
        ));
        sftp.remove_dir(&p("d")).unwrap();
        assert!(!c.srv.root.join("d").exists());

        // fork shares the session; closing the fork leaves the original usable.
        let f = sftp.fork().unwrap();
        assert!(f.list(&r).is_ok());
        f.close();
        assert!(sftp.list(&r).is_ok());
        sftp.close();
        assert_eq!(sftp.list(&r), Err(RemoteError::Disconnected));
        sftp.close();
    }

    #[test]
    fn stalled_server_times_out() {
        let Some(c) = setup() else { return };
        let sftp = c.fs();
        sftp.set_request_timeout(Duration::from_secs(2));
        assert!(sftp.list(&c.root()).is_ok());
        c.srv.stall();
        let t = Instant::now();
        assert_eq!(sftp.list(&c.root()), Err(RemoteError::Timeout));
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    }

    #[test]
    fn sftp_calls_work_from_spawn_blocking() {
        let Some(c) = setup() else { return };
        let sftp: Arc<dyn RemoteFs> = Arc::new(c.fs());
        let root = c.root();
        let other = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let r = other.block_on(async move {
            tokio::task::spawn_blocking(move || sftp.list(&root))
                .await
                .unwrap()
        });
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn rsa_hash_choice_cases() {
        let sha1_refused = Err(ConnectError::Failed(RSA_SHA1_NEEDS_LEGACY.into()));
        for legacy in [false, true] {
            for h in [HashAlg::Sha256, HashAlg::Sha512] {
                assert_eq!(rsa_hash_choice(Some(Some(h)), legacy), Ok(Some(h)));
            }
        }
        // Server offers only ssh-rsa (SHA-1).
        assert_eq!(rsa_hash_choice(Some(None), false), sha1_refused);
        assert_eq!(rsa_hash_choice(Some(None), true), Ok(None));
        // Server did not send server-sig-algs.
        assert_eq!(rsa_hash_choice(None, false), Ok(Some(HashAlg::Sha256)));
        assert_eq!(rsa_hash_choice(None, true), Ok(None));
    }

    #[test]
    fn legacy_algorithms_extend_the_offer() {
        let plain = preferred(false);
        let legacy = preferred(true);
        let rsa_sha1 = Algorithm::Rsa { hash: None };
        assert!(!plain.key.contains(&rsa_sha1));
        assert!(legacy.key.contains(&rsa_sha1));
        assert!(!plain.kex.contains(&kex::DH_G14_SHA1));
        assert!(legacy.kex.contains(&kex::DH_G14_SHA1));
        assert!(legacy.kex.contains(&kex::DH_G1_SHA1));
        let Some(c) = setup() else { return };
        c.trust();
        let mut e = c.srv.entry();
        e.legacy_algorithms = true;
        let fs = SftpFs::connect(&e, &pw(), &c.opts(None)).unwrap();
        assert!(fs.home().is_ok());
    }

    #[test]
    fn missing_sftp_subsystem_fails_cleanly() {
        let Some(c) = setup() else { return };
        c.trust();
        c.srv.disable_sftp();
        let r = SftpFs::connect(&c.srv.entry(), &pw(), &c.opts(None)).err();
        assert_eq!(
            r,
            Some(ConnectError::Failed(
                "The server does not offer the sftp subsystem".into()
            ))
        );
    }

    #[test]
    fn connect_dispatches_by_protocol() {
        let data = tempfile::tempdir().unwrap();
        let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
        let opts = ConnectOptions {
            data_dir: data.path(),
            known: &known,
            accept_fingerprint: None,
        };
        let mut e = crate::servers::ServerEntry {
            host: "127.0.0.1".into(),
            port: 1,
            ..Default::default()
        };
        e.protocol = Protocol::Scp;
        assert!(matches!(
            connect(&e, &pw(), &opts).err(),
            Some(ConnectError::Failed(_))
        ));
        e.protocol = Protocol::Ftps;
        assert!(matches!(
            connect(&e, &pw(), &opts).err(),
            Some(ConnectError::Failed(_))
        ));
        // Port 1 on localhost: refused, not a hang.
        e.protocol = Protocol::Sftp;
        assert!(matches!(
            connect(&e, &pw(), &opts).err(),
            Some(ConnectError::Failed(_))
        ));
    }
}
