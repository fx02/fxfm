//! In-memory `RemoteFs` for tests, with hooks for failure injection and call counting.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Clone, Debug)]
enum Kind {
    File(Vec<u8>),
    Dir,
    Link(String),
}

#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    mode: u32,
    mtime: i64,
}

#[derive(Default)]
struct State {
    nodes: BTreeMap<String, Node>,
    fail_after: Option<u64>,
    no_create: HashSet<String>,
    calls: HashMap<&'static str, usize>,
    next_err: HashMap<&'static str, RemoteError>,
    no_upload_modes: bool,
}

/// Clones share the same tree.
#[derive(Clone, Default)]
pub struct MemFs {
    st: Arc<Mutex<State>>,
}

fn norm(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        "/".into()
    } else {
        t.into()
    }
}

impl MemFs {
    pub fn new() -> MemFs {
        MemFs::default()
    }

    fn hit(&self, method: &'static str) -> Result<(), RemoteError> {
        let mut s = self.st.lock().unwrap();
        *s.calls.entry(method).or_default() += 1;
        match s.next_err.remove(method) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn ensure_parents(s: &mut State, path: &str) {
        let mut p = parent_path(path).to_string();
        while p != "/" {
            s.nodes.entry(p.clone()).or_insert(Node {
                kind: Kind::Dir,
                mode: 0o755,
                mtime: 0,
            });
            p = parent_path(&p).to_string();
        }
    }

    fn put(&self, path: &str, kind: Kind, mode: u32, mtime: i64) {
        let path = norm(path);
        let mut s = self.st.lock().unwrap();
        Self::ensure_parents(&mut s, &path);
        s.nodes.insert(path, Node { kind, mode, mtime });
    }

    pub fn put_file(&self, path: &str, data: &[u8], mode: u32, mtime: i64) {
        self.put(path, Kind::File(data.to_vec()), mode, mtime);
    }

    pub fn put_dir(&self, path: &str) {
        self.put(path, Kind::Dir, 0o755, 0);
    }

    pub fn put_symlink(&self, link: &str, target: &str, mode: u32, mtime: i64) {
        self.put(link, Kind::Link(target.to_string()), mode, mtime);
    }

    /// Content of a file (None when missing or not a file).
    pub fn data(&self, path: &str) -> Option<Vec<u8>> {
        match &self.st.lock().unwrap().nodes.get(&norm(path))?.kind {
            Kind::File(d) => Some(d.clone()),
            _ => None,
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.st.lock().unwrap().nodes.contains_key(&norm(path))
    }

    pub fn mode_of(&self, path: &str) -> Option<u32> {
        self.st
            .lock()
            .unwrap()
            .nodes
            .get(&norm(path))
            .map(|n| n.mode)
    }

    pub fn paths(&self) -> Vec<String> {
        self.st.lock().unwrap().nodes.keys().cloned().collect()
    }

    /// The next `write_from` that gets more than `n` bytes writes `n` of them and returns
    /// `Disconnected` (the partial data stays). One-shot.
    pub fn fail_after_bytes(&self, n: u64) {
        self.st.lock().unwrap().fail_after = Some(n);
    }

    /// Creating a new file in `dir` gives `PermissionDenied`; overwriting is allowed.
    pub fn set_no_create(&self, dir: &str) {
        self.st.lock().unwrap().no_create.insert(norm(dir));
    }

    /// The next call of `method` (trait method name) returns `err`.
    pub fn fail_next(&self, method: &'static str, err: RemoteError) {
        self.st.lock().unwrap().next_err.insert(method, err);
    }

    /// Report `upload_sets_mode() == false`, like FTP.
    pub fn set_upload_sets_mode(&self, on: bool) {
        self.st.lock().unwrap().no_upload_modes = !on;
    }

    pub fn calls(&self, method: &str) -> usize {
        self.st
            .lock()
            .unwrap()
            .calls
            .get(method)
            .copied()
            .unwrap_or(0)
    }

    /// `path` with symlinks in its parent folders resolved (the last component is kept).
    fn canon_parent(s: &State, path: &str) -> String {
        let p = norm(path);
        let parent = parent_path(&p);
        if parent == "/" || p == "/" {
            return p;
        }
        let name = p.rsplit('/').next().unwrap();
        match Self::resolve(s, parent) {
            Ok((rp, _)) => join_path(&rp, name),
            Err(_) => p,
        }
    }

    fn resolve(s: &State, path: &str) -> Result<(String, Node), RemoteError> {
        let mut p = Self::canon_parent(s, path);
        for _ in 0..8 {
            let n = s
                .nodes
                .get(&p)
                .cloned()
                .ok_or_else(|| RemoteError::NotFound(path.to_string()))?;
            match n.kind {
                Kind::Link(t) => {
                    p = Self::canon_parent(
                        s,
                        &if t.starts_with('/') {
                            norm(&t)
                        } else {
                            norm(&join_path(parent_path(&p), &t))
                        },
                    )
                }
                _ => return Ok((p, n)),
            }
        }
        Err(RemoteError::Other(
            "too many levels of symbolic links".into(),
        ))
    }

    fn parent_is_dir(s: &State, path: &str) -> Result<(), RemoteError> {
        let parent = parent_path(path);
        if parent == "/" {
            return Ok(());
        }
        match s.nodes.get(parent) {
            Some(Node {
                kind: Kind::Dir, ..
            }) => Ok(()),
            Some(_) => Err(RemoteError::Other(format!("not a directory: {parent}"))),
            None => Err(RemoteError::NotFound(parent.to_string())),
        }
    }
}

impl RemoteFs for MemFs {
    fn display(&self) -> String {
        "sftp://mem@test".into()
    }

    fn home(&self) -> Result<String, RemoteError> {
        self.hit("home")?;
        Ok("/home/mem".into())
    }

    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError> {
        self.hit("list")?;
        let path = norm(path);
        let s = self.st.lock().unwrap();
        let (rp, n) = if path == "/" {
            (
                "/".to_string(),
                Node {
                    kind: Kind::Dir,
                    mode: 0o755,
                    mtime: 0,
                },
            )
        } else {
            Self::resolve(&s, &path)?
        };
        if !matches!(n.kind, Kind::Dir) {
            return Err(RemoteError::Other(format!("not a directory: {path}")));
        }
        let mut out = Vec::new();
        for (p, n) in &s.nodes {
            if p != "/" && parent_path(p) == rp {
                let name = p.rsplit('/').next().unwrap().to_string();
                out.push(Entry {
                    hidden: name.starts_with('.'),
                    is_dir: matches!(n.kind, Kind::Dir),
                    is_symlink: matches!(n.kind, Kind::Link(_)),
                    size: match &n.kind {
                        Kind::File(d) => d.len() as u64,
                        Kind::Link(t) => t.len() as u64,
                        Kind::Dir => 0,
                    },
                    mtime: n.mtime,
                    mode: n.mode,
                    name,
                });
            }
        }
        Ok(out)
    }

    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        self.hit("stat")?;
        let path = norm(path);
        let s = self.st.lock().unwrap();
        if path == "/" {
            return Ok(Some(RemoteStat {
                is_dir: true,
                is_symlink: false,
                size: 0,
                mtime: 0,
                mode: 0o755,
            }));
        }
        let path = Self::canon_parent(&s, &path);
        Ok(s.nodes.get(&path).map(|n| RemoteStat {
            is_dir: matches!(n.kind, Kind::Dir),
            is_symlink: matches!(n.kind, Kind::Link(_)),
            size: match &n.kind {
                Kind::File(d) => d.len() as u64,
                Kind::Link(t) => t.len() as u64,
                Kind::Dir => 0,
            },
            mtime: n.mtime,
            mode: n.mode,
        }))
    }

    fn read_link(&self, path: &str) -> Result<String, RemoteError> {
        self.hit("read_link")?;
        match self.st.lock().unwrap().nodes.get(&norm(path)) {
            Some(Node {
                kind: Kind::Link(t),
                ..
            }) => Ok(t.clone()),
            Some(_) => Err(RemoteError::Other(format!("not a symlink: {path}"))),
            None => Err(RemoteError::NotFound(path.to_string())),
        }
    }

    fn read_to(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.hit("read_to")?;
        let data = {
            let s = self.st.lock().unwrap();
            match Self::resolve(&s, path)?.1.kind {
                Kind::File(d) => d,
                _ => return Err(RemoteError::Other(format!("not a file: {path}"))),
            }
        };
        for chunk in data.chunks(4096) {
            out.write_all(chunk)
                .map_err(|e| RemoteError::Other(e.to_string()))?;
            if !progress(chunk.len() as u64) {
                return Err(RemoteError::Cancelled);
            }
        }
        Ok(())
    }

    fn write_from(
        &self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.hit("write_from")?;
        let path = norm(path);
        let (limit, target) = {
            let mut s = self.st.lock().unwrap();
            Self::parent_is_dir(&s, &path)?;
            let existing = Self::resolve(&s, &path).ok();
            let target = match existing {
                Some((
                    p,
                    Node {
                        kind: Kind::File(_),
                        ..
                    },
                )) => p,
                Some(_) => return Err(RemoteError::Other(format!("not a file: {path}"))),
                None => {
                    if s.no_create.contains(parent_path(&path)) {
                        return Err(RemoteError::PermissionDenied(path));
                    }
                    path.clone()
                }
            };
            let prev = s.nodes.get(&target).map(|n| (n.mode, n.mtime));
            let (mode, mtime) = prev.unwrap_or((0o644, 0));
            s.nodes.insert(
                target.clone(),
                Node {
                    kind: Kind::File(Vec::new()),
                    mode,
                    mtime,
                },
            );
            (s.fail_after, target)
        };
        let mut written = 0u64;
        let mut buf = [0u8; 4096];
        loop {
            let mut want = buf.len();
            if let Some(l) = limit {
                // One byte past the limit tells whether there is more input.
                want = want.min((l - written.min(l)) as usize + 1);
            }
            let n = input
                .read(&mut buf[..want])
                .map_err(|e| RemoteError::Other(e.to_string()))?;
            if n == 0 {
                return Ok(());
            }
            let n = match limit {
                Some(l) if written + n as u64 > l => {
                    let keep = (l - written) as usize;
                    let mut s = self.st.lock().unwrap();
                    s.fail_after = None;
                    if let Some(Node {
                        kind: Kind::File(d),
                        ..
                    }) = s.nodes.get_mut(&target)
                    {
                        d.extend_from_slice(&buf[..keep]);
                    }
                    return Err(RemoteError::Disconnected);
                }
                _ => n,
            };
            {
                let mut s = self.st.lock().unwrap();
                if let Some(Node {
                    kind: Kind::File(d),
                    ..
                }) = s.nodes.get_mut(&target)
                {
                    d.extend_from_slice(&buf[..n]);
                }
            }
            written += n as u64;
            if !progress(n as u64) {
                return Err(RemoteError::Cancelled);
            }
        }
    }

    fn mkdir(&self, path: &str) -> Result<(), RemoteError> {
        self.hit("mkdir")?;
        let path = norm(path);
        let mut s = self.st.lock().unwrap();
        if path == "/" || s.nodes.contains_key(&path) {
            return Err(RemoteError::Exists(path));
        }
        Self::parent_is_dir(&s, &path)?;
        s.nodes.insert(
            path,
            Node {
                kind: Kind::Dir,
                mode: 0o755,
                mtime: 0,
            },
        );
        Ok(())
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        self.hit("remove_file")?;
        let path = norm(path);
        let mut s = self.st.lock().unwrap();
        match s.nodes.get(&path) {
            None => Err(RemoteError::NotFound(path)),
            Some(Node {
                kind: Kind::Dir, ..
            }) => Err(RemoteError::Other(format!("is a directory: {path}"))),
            Some(_) => {
                s.nodes.remove(&path);
                Ok(())
            }
        }
    }

    fn remove_dir(&self, path: &str) -> Result<(), RemoteError> {
        self.hit("remove_dir")?;
        let path = norm(path);
        let mut s = self.st.lock().unwrap();
        match s.nodes.get(&path) {
            None => return Err(RemoteError::NotFound(path)),
            Some(Node {
                kind: Kind::Dir, ..
            }) => {}
            Some(_) => return Err(RemoteError::Other(format!("not a directory: {path}"))),
        }
        if s.nodes.keys().any(|p| parent_path(p) == path && p != &path) {
            return Err(RemoteError::Other(format!("directory not empty: {path}")));
        }
        s.nodes.remove(&path);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        self.hit("rename")?;
        let (from, to) = (norm(from), norm(to));
        let mut s = self.st.lock().unwrap();
        if !s.nodes.contains_key(&from) {
            return Err(RemoteError::NotFound(from));
        }
        if s.nodes.contains_key(&to) {
            return Err(RemoteError::Exists(to));
        }
        Self::parent_is_dir(&s, &to)?;
        move_tree(&mut s, &from, &to);
        Ok(())
    }

    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        self.hit("replace")?;
        let (from, to) = (norm(from), norm(to));
        let mut s = self.st.lock().unwrap();
        if !s.nodes.contains_key(&from) {
            return Err(RemoteError::NotFound(from));
        }
        if matches!(
            s.nodes.get(&to),
            Some(Node {
                kind: Kind::Dir,
                ..
            })
        ) {
            return Err(RemoteError::Other(format!("is a directory: {to}")));
        }
        Self::parent_is_dir(&s, &to)?;
        s.nodes.remove(&to);
        move_tree(&mut s, &from, &to);
        Ok(())
    }

    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError> {
        self.hit("symlink")?;
        let link = norm(link);
        let mut s = self.st.lock().unwrap();
        if s.nodes.contains_key(&link) {
            return Err(RemoteError::Exists(link));
        }
        Self::parent_is_dir(&s, &link)?;
        s.nodes.insert(
            link,
            Node {
                kind: Kind::Link(target.to_string()),
                mode: 0o777,
                mtime: 0,
            },
        );
        Ok(())
    }

    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError> {
        self.hit("set_mode")?;
        let mut s = self.st.lock().unwrap();
        let (p, _) = Self::resolve(&s, path)?;
        s.nodes.get_mut(&p).unwrap().mode = mode;
        Ok(())
    }

    fn upload_sets_mode(&self) -> bool {
        !self.st.lock().unwrap().no_upload_modes
    }

    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError> {
        self.hit("fork")?;
        Ok(Arc::new(self.clone()))
    }

    fn close(&self) {}
}

fn move_tree(s: &mut State, from: &str, to: &str) {
    let prefix = format!("{from}/");
    let moved: Vec<String> = s
        .nodes
        .keys()
        .filter(|p| *p == from || p.starts_with(&prefix))
        .cloned()
        .collect();
    for p in moved {
        let n = s.nodes.remove(&p).unwrap();
        s.nodes.insert(format!("{to}{}", &p[from.len()..]), n);
    }
}

// ---------------------------------------------------------------------------------------
// In-process SSH server (russh) for the SFTP and SCP backends.
// ---------------------------------------------------------------------------------------

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth as SrvAuth, Msg, PublicKeyOffer, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, ChannelMsg};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The system `sftp-server`, if any.
pub fn sftp_server_path() -> Option<PathBuf> {
    [
        "/usr/libexec/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/lib/ssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

/// True when SFTP tests can run; otherwise prints the skip notice.
pub fn have_sftp_server() -> bool {
    if sftp_server_path().is_some() {
        true
    } else {
        eprintln!("skipped: no sftp-server");
        false
    }
}

/// A random ed25519 key.
pub fn random_ed25519() -> PrivateKey {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    PrivateKey::from(Ed25519Keypair::from_seed(&seed))
}

/// Writes `key` in OpenSSH format (unencrypted) to `path`, mode 0600.
pub fn write_openssh_key(key: &PrivateKey, path: &Path) {
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(path, text.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

struct SrvState {
    auth_requests: AtomicUsize,
    sftp_enabled: AtomicBool,
    stalled: AtomicBool,
    user_keys: Mutex<Vec<PublicKey>>,
    cwd: PathBuf,
}

/// russh server on `127.0.0.1:<random>`: password `pw` or an allowed public key (any user),
/// `sftp` subsystem = the system `sftp-server`, `exec` = `sh -c <cmd>`. Runs on its own
/// runtime so client calls blocking on `rt()` never starve it.
pub struct SshTestServer {
    rt: Option<tokio::runtime::Runtime>,
    pub port: u16,
    /// The server's working directory (what `home()` resolves to); canonical.
    pub root: PathBuf,
    _root_dir: tempfile::TempDir,
    host_key: PrivateKey,
    st: Arc<SrvState>,
}

impl SshTestServer {
    pub fn start() -> SshTestServer {
        Self::start_with_host_key(random_ed25519())
    }

    pub fn start_with_host_key(host_key: PrivateKey) -> SshTestServer {
        let root_dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root_dir.path()).unwrap();
        let st = Arc::new(SrvState {
            auth_requests: AtomicUsize::new(0),
            sftp_enabled: AtomicBool::new(true),
            stalled: AtomicBool::new(false),
            user_keys: Mutex::new(Vec::new()),
            cwd: root.clone(),
        });
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let config = Arc::new(russh::server::Config {
            keys: vec![host_key.clone()],
            auth_rejection_time: std::time::Duration::from_millis(10),
            auth_rejection_time_initial: Some(std::time::Duration::from_millis(0)),
            inactivity_timeout: None,
            ..Default::default()
        });
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let st2 = st.clone();
        rt.spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let handler = SrvHandler {
                    st: st2.clone(),
                    channels: HashMap::new(),
                };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(running) = russh::server::run_stream(config, sock, handler).await {
                        let _ = running.await;
                    }
                });
            }
        });
        SshTestServer {
            rt: Some(rt),
            port,
            root,
            _root_dir: root_dir,
            host_key,
            st,
        }
    }

    /// `SHA256:...` of the host key, as `known_hosts` stores it.
    pub fn host_fingerprint(&self) -> String {
        let blob = self.host_key.public_key().to_bytes().unwrap();
        super::known_hosts::fingerprint_sha256(&blob)
    }

    pub fn host_key_type(&self) -> String {
        self.host_key.algorithm().as_str().to_string()
    }

    /// Authentication requests received so far (any method).
    pub fn auth_requests(&self) -> usize {
        self.st.auth_requests.load(Ordering::SeqCst)
    }

    pub fn allow_key(&self, key: PublicKey) {
        self.st.user_keys.lock().unwrap().push(key);
    }

    /// Refuse the `sftp` subsystem (for SCP-only servers).
    pub fn disable_sftp(&self) {
        self.st.sftp_enabled.store(false, Ordering::SeqCst);
    }

    /// Stop answering channel data without closing anything.
    pub fn stall(&self) {
        self.st.stalled.store(true, Ordering::SeqCst);
    }

    /// A server entry for this server (password auth, user `tester`).
    pub fn entry(&self) -> crate::servers::ServerEntry {
        crate::servers::ServerEntry {
            id: "0123456789abcdef".into(),
            name: "test".into(),
            host: "127.0.0.1".into(),
            port: self.port,
            user: "tester".into(),
            ..Default::default()
        }
    }
}

impl Drop for SshTestServer {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

struct SrvHandler {
    st: Arc<SrvState>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl SrvHandler {
    fn count(&self) {
        self.st.auth_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn key_allowed(&self, key: &PublicKey) -> bool {
        self.st
            .user_keys
            .lock()
            .unwrap()
            .iter()
            .any(|k| k.key_data() == key.key_data())
    }
}

impl russh::server::Handler for SrvHandler {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<SrvAuth, Self::Error> {
        self.count();
        Ok(SrvAuth::reject())
    }

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<SrvAuth, Self::Error> {
        self.count();
        Ok(if password == "pw" {
            SrvAuth::Accept
        } else {
            SrvAuth::reject()
        })
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        offer: PublicKeyOffer<'_>,
    ) -> Result<SrvAuth, Self::Error> {
        self.count();
        Ok(if self.key_allowed(offer.public_key) {
            SrvAuth::Accept
        } else {
            SrvAuth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<SrvAuth, Self::Error> {
        self.count();
        Ok(if self.key_allowed(key.public_key) {
            SrvAuth::Accept
        } else {
            SrvAuth::reject()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        _response: Option<russh::server::Response<'a>>,
    ) -> Result<SrvAuth, Self::Error> {
        self.count();
        Ok(SrvAuth::reject())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let server = sftp_server_path();
        match (self.channels.remove(&id), server) {
            (Some(ch), Some(server))
                if name == "sftp" && self.st.sftp_enabled.load(Ordering::SeqCst) =>
            {
                let mut cmd = tokio::process::Command::new(server);
                cmd.current_dir(&self.st.cwd);
                session.channel_success(id)?;
                tokio::spawn(pump(ch, cmd, self.st.clone()));
            }
            (ch, _) => {
                if let Some(ch) = ch {
                    self.channels.insert(id, ch);
                }
                session.channel_failure(id)?;
            }
        }
        Ok(())
    }

    async fn exec_request(
        &mut self,
        id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(ch) = self.channels.remove(&id) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(String::from_utf8_lossy(data).into_owned())
            .current_dir(&self.st.cwd);
        session.channel_success(id)?;
        tokio::spawn(pump(ch, cmd, self.st.clone()));
        Ok(())
    }
}

/// Connects a channel to a child process: data -> stdin, stdout -> data, stderr ->
/// extended data 1; then exit status, EOF and close. Once stalled, nothing more flows.
async fn pump(ch: Channel<Msg>, mut cmd: tokio::process::Command, st: Arc<SrvState>) {
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let Ok(mut child) = cmd.spawn() else {
        let _ = ch.exit_status(127).await;
        let _ = ch.close().await;
        return;
    };
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut rd, wr) = ch.split();
    let wr = Arc::new(wr);

    let st_in = st.clone();
    let input = tokio::spawn(async move {
        while let Some(msg) = rd.wait().await {
            if st_in.stalled.load(Ordering::SeqCst) {
                continue;
            }
            match msg {
                ChannelMsg::Data { data } => {
                    if let Some(s) = stdin.as_mut() {
                        if s.write_all(&data).await.is_err() {
                            stdin = None;
                        }
                    }
                }
                ChannelMsg::Eof => stdin = None,
                _ => {}
            }
        }
    });

    let (w1, s1) = (wr.clone(), st.clone());
    let out = tokio::spawn(async move {
        let mut buf = vec![0u8; 32768];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if s1.stalled.load(Ordering::SeqCst) {
                        std::future::pending::<()>().await;
                    }
                    if w1.data_bytes(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let w2 = wr.clone();
    let err = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if w2.extended_data_bytes(1, buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let status = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(1);
    let _ = out.await;
    let _ = err.await;
    input.abort();
    let _ = wr.exit_status(status as u32).await;
    let _ = wr.eof().await;
    let _ = wr.close().await;
}

// ---------------------------------------------------------------------------------------
// In-process FTP(S) server (libunftp) for the FTP backend.
// ---------------------------------------------------------------------------------------

use libunftp::options::ActivePassiveMode;
use unftp_core::auth::{
    AuthenticationError, Authenticator, Credentials as FtpCreds, DefaultUser, Principal,
};

/// User `u`, password `pw`.
#[derive(Debug)]
struct FtpAuth;

#[async_trait::async_trait]
impl Authenticator for FtpAuth {
    async fn authenticate(
        &self,
        username: &str,
        creds: &FtpCreds,
    ) -> Result<Principal, AuthenticationError> {
        if username != "u" {
            return Err(AuthenticationError::BadUser);
        }
        if creds.password.as_deref() != Some("pw") {
            return Err(AuthenticationError::BadPassword);
        }
        Ok(Principal {
            username: username.to_string(),
        })
    }
}

/// Counts sessions that ended (QUIT or closed control connection).
#[derive(Debug)]
struct FtpLogouts(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl libunftp::notification::PresenceListener for FtpLogouts {
    async fn receive_presence_event(
        &self,
        e: libunftp::notification::PresenceEvent,
        _m: libunftp::notification::EventMeta,
    ) {
        if matches!(e, libunftp::notification::PresenceEvent::LoggedOut) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

struct FtpCert {
    cert_file: PathBuf,
    key_file: PathBuf,
    der: Vec<u8>,
}

/// libunftp over a temporary folder on `127.0.0.1:<random>`, passive and active mode,
/// optionally with explicit TLS (rcgen self-signed certificate). A fresh libunftp server
/// is built per connection, so `rotate_cert` affects the next login.
pub struct FtpTestServer {
    rt: Option<tokio::runtime::Runtime>,
    pub port: u16,
    /// What the FTP root `/` maps to.
    pub root: PathBuf,
    _root_dir: tempfile::TempDir,
    _cert_dir: tempfile::TempDir,
    cert: Option<Arc<Mutex<FtpCert>>>,
    cert_dir: PathBuf,
    logins: Arc<AtomicUsize>,
    logouts: Arc<AtomicUsize>,
}

fn new_cert(dir: &Path, n: usize) -> FtpCert {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_file = dir.join(format!("cert{n}.pem"));
    let key_file = dir.join(format!("key{n}.pem"));
    std::fs::write(&cert_file, ck.cert.pem()).unwrap();
    std::fs::write(&key_file, ck.signing_key.serialize_pem()).unwrap();
    FtpCert {
        cert_file,
        key_file,
        der: ck.cert.der().to_vec(),
    }
}

impl FtpTestServer {
    pub fn start() -> FtpTestServer {
        Self::start_with(false, None)
    }

    pub fn start_tls() -> FtpTestServer {
        Self::start_with(true, None)
    }

    /// A server that closes a control connection idle for `secs` seconds.
    pub fn start_idle(secs: u64) -> FtpTestServer {
        Self::start_with(false, Some(secs))
    }

    fn start_with(tls: bool, idle_secs: Option<u64>) -> FtpTestServer {
        let root_dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root_dir.path()).unwrap();
        let cert_dir = tempfile::tempdir().unwrap();
        let cert = tls.then(|| Arc::new(Mutex::new(new_cert(cert_dir.path(), 0))));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let logins = Arc::new(AtomicUsize::new(0));
        let logouts = Arc::new(AtomicUsize::new(0));
        let (root2, cert2, logins2, logouts2) =
            (root.clone(), cert.clone(), logins.clone(), logouts.clone());
        rt.spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                logins2.fetch_add(1, Ordering::SeqCst);
                let r = root2.clone();
                let mut b = libunftp::ServerBuilder::<_, DefaultUser>::with_authenticator(
                    Box::new(move || unftp_sbe_fs::Filesystem::new(r.clone()).unwrap()),
                    Arc::new(FtpAuth),
                )
                .active_passive_mode(ActivePassiveMode::ActiveAndPassive)
                .notify_presence(FtpLogouts(logouts2.clone()));
                if let Some(secs) = idle_secs {
                    b = b.idle_session_timeout(secs);
                }
                if let Some(c) = &cert2 {
                    let c = c.lock().unwrap();
                    b = b.ftps(c.cert_file.clone(), c.key_file.clone());
                }
                let server = b.build().unwrap();
                tokio::spawn(async move {
                    let _ = server.service(sock).await;
                });
            }
        });
        FtpTestServer {
            rt: Some(rt),
            port,
            root,
            _root_dir: root_dir,
            cert_dir: cert_dir.path().to_path_buf(),
            _cert_dir: cert_dir,
            cert,
            logins,
            logouts,
        }
    }

    /// `SHA256:...` of the current certificate's DER.
    pub fn cert_fingerprint(&self) -> String {
        let c = self.cert.as_ref().expect("TLS server").lock().unwrap();
        super::known_hosts::fingerprint_sha256(&c.der)
    }

    /// A new self-signed certificate for the following connections.
    pub fn rotate_cert(&self) {
        let c = self.cert.as_ref().expect("TLS server");
        *c.lock().unwrap() = new_cert(&self.cert_dir, rand::random::<u32>() as usize);
    }

    /// Control connections accepted so far.
    pub fn connections(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }

    /// Sessions that ended so far.
    pub fn logouts(&self) -> usize {
        self.logouts.load(Ordering::SeqCst)
    }

    /// A server entry for this server (user `u`, password auth, passive mode).
    pub fn entry(&self) -> crate::servers::ServerEntry {
        crate::servers::ServerEntry {
            id: "0123456789abcdef".into(),
            name: "ftptest".into(),
            protocol: if self.cert.is_some() {
                crate::servers::Protocol::Ftps
            } else {
                crate::servers::Protocol::Ftp
            },
            host: "127.0.0.1".into(),
            port: self.port,
            user: "u".into(),
            ..Default::default()
        }
    }
}

impl Drop for FtpTestServer {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}
