//! Server connections: the saved server list, passwords (system keychain or the portable
//! vault), connecting and disconnecting. The logic lives in the Tauri-free `Connector`;
//! the commands at the bottom wrap it.
//!
//! Events: `connections://changed {lost: [server name]}` whenever a connection is opened or
//! closed; `lost` names the connections that died during a command or job.
//!
//! Secrets never appear in logs, error texts or anything sent to the UI.

use crate::editwatch::EditWatch;
use crate::state::{lock, AppState};
use fxfm_core::remote::known_hosts::{KnownHosts, KNOWN_HOSTS_FILE};
use fxfm_core::remote::{
    self as core_remote, format_remote, parse_remote, ConnectError, ConnectOptions, Credentials,
    RemoteError, RemoteFs,
};
use fxfm_core::secrets::{self, account, migrate_secrets, MemoryStore, SecretStore, Vault};
use fxfm_core::servers::{self, Auth, Protocol, ServerEntry, ServerList, ServerNode};
use fxfm_core::storage::{self, DataLocation, StorageMode};
use fxfm_core::vfs::Ctx;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

pub const WRONG_LOGIN: &str = "Wrong user name, password or key";
pub const CONNECT_TIMEOUT_MSG: &str = "Server did not answer within 15 seconds";
pub const MASTER_REQUIRED: &str = "master password required";
pub const CHMOD_ONLY_REMOTE: &str = "Change attributes works on SFTP and SCP servers";
pub const CONNECTIONS_CHANGED: &str = "connections://changed";

const KINDS: [secrets::SecretKind; 2] = [
    secrets::SecretKind::Password,
    secrets::SecretKind::Passphrase,
];

/// Which secret of a server (`<id>/password` or `<id>/passphrase`).
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl From<SecretKind> for secrets::SecretKind {
    fn from(k: SecretKind) -> Self {
        match k {
            SecretKind::Password => secrets::SecretKind::Password,
            SecretKind::Passphrase => secrets::SecretKind::Passphrase,
        }
    }
}

/// No `Debug`: it carries secrets.
#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectRequest {
    /// Name of a saved server (or of an earlier quick connection).
    pub server: Option<String>,
    /// `scheme://[user@]host[:port][/path]` for a quick connection.
    pub url: Option<String>,
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// Store the entered secrets (keychain or vault).
    pub save: bool,
    pub accept_fingerprint: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConnectReply {
    Connected {
        location: String,
        local_dir: Option<String>,
    },
    HostKey {
        host: String,
        port: u16,
        key_type: String,
        fingerprint: String,
        changed_from: Option<String>,
    },
    NeedPassword {
        retry: bool,
    },
    NeedPassphrase {
        retry: bool,
    },
    NeedMaster {
        create: bool,
    },
}

/// Error of `server_secret_set`: the `needMaster` reply, or a message.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum SecretSetError {
    Reply(ConnectReply),
    Message(String),
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub name: String,
    /// `sftp://user@host` (with `:port` when not the default).
    pub display: String,
    /// `sftp://name/`.
    pub root: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MasterStatus {
    pub mode: StorageMode,
    pub exists: bool,
    pub unlocked: bool,
}

/// One connection attempt. No `Debug`: it carries secrets.
pub struct Attempt {
    pub server: ServerEntry,
    /// Saved in the server list (false for a quick connection).
    pub saved: bool,
    /// What is sent to the server.
    pub creds: Credentials,
    /// What the user typed into this request (cached for the session on success).
    pub typed: Credentials,
    /// Store the typed secrets too.
    pub save: bool,
    pub accept_fingerprint: Option<String>,
}

pub enum Outcome {
    Connected(Arc<dyn RemoteFs>),
    Reply(ConnectReply),
}

pub enum Step {
    Done(ConnectReply),
    Attempt(Attempt),
}

/// The server is asked for a password (SSH password login, every FTP login).
fn uses_password(s: &ServerEntry) -> bool {
    matches!(s.protocol, Protocol::Ftp | Protocol::Ftps) || s.auth == Auth::Password
}

/// What the UI gets for a failed or finished connection attempt. On success the typed
/// secrets are kept for the session (and stored with `save`); a rejected one is forgotten.
pub fn after_connect(
    result: Result<Arc<dyn RemoteFs>, ConnectError>,
    a: &Attempt,
    session: &dyn SecretStore,
    store: Option<&dyn SecretStore>,
) -> Result<Outcome, String> {
    let pw = account(&a.server.id, secrets::SecretKind::Password);
    let pp = account(&a.server.id, secrets::SecretKind::Passphrase);
    let reply = |r| Ok(Outcome::Reply(r));
    match result {
        Ok(fs) => {
            for (acct, typed) in [(&pw, &a.typed.password), (&pp, &a.typed.passphrase)] {
                let Some(secret) = typed else { continue };
                if let Err(e) = session.set(acct, secret) {
                    eprintln!("fxfm: keeping {acct}: {e}");
                }
                if let Some(st) = store.filter(|_| a.save) {
                    if let Err(e) = st.set(acct, secret) {
                        eprintln!("fxfm: saving {acct}: {e}");
                    }
                }
            }
            Ok(Outcome::Connected(fs))
        }
        Err(ConnectError::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed_from,
        }) => reply(ConnectReply::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            changed_from,
        }),
        Err(ConnectError::NeedPassword) => reply(ConnectReply::NeedPassword { retry: false }),
        Err(ConnectError::NeedPassphrase) => reply(ConnectReply::NeedPassphrase { retry: false }),
        Err(ConnectError::AuthFailed) if uses_password(&a.server) => {
            let _ = session.delete(&pw);
            reply(ConnectReply::NeedPassword { retry: true })
        }
        // The server refused the key: asking again cannot help, and the passphrase (which
        // did decrypt the key) stays.
        Err(ConnectError::AuthFailed) => Err(WRONG_LOGIN.to_string()),
        Err(ConnectError::BadPassphrase) => {
            let _ = session.delete(&pp);
            reply(ConnectReply::NeedPassphrase { retry: true })
        }
        Err(ConnectError::Timeout) => Err(CONNECT_TIMEOUT_MSG.to_string()),
        // Passed through as is (it carries the legacy-algorithm hint where there is one).
        Err(ConnectError::Failed(m)) => Err(m),
    }
}

/// Where a new or existing connection opens: the server's remote folder, else home.
fn start_reply(server: &ServerEntry, fs: &dyn RemoteFs) -> Result<ConnectReply, RemoteError> {
    let dir = match server
        .remote_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        Some(d) => d.to_string(),
        None => match fs.home() {
            Ok(h) => h,
            Err(e) if e.is_lost() => return Err(e),
            Err(_) => "/".to_string(),
        },
    };
    Ok(ConnectReply::Connected {
        location: format_remote(server.protocol.scheme(), &server.name, &dir),
        local_dir: server.local_dir.clone(),
    })
}

fn set_save_password(nodes: &mut [ServerNode], id: &str) {
    for n in nodes {
        match n {
            ServerNode::Folder { children, .. } => set_save_password(children, id),
            ServerNode::Server(e) if e.id == id => e.save_password = true,
            ServerNode::Server(_) => {}
        }
    }
}

pub struct Connector {
    pub servers: Mutex<ServerList>,
    /// The unlocked vault (portable mode).
    pub vault: Mutex<Option<Vault>>,
    /// Entered secrets, kept in memory until exit (for reconnecting).
    pub session_secrets: MemoryStore,
    /// Quick connections ("New URL…") by name; never saved.
    pub quick: Mutex<HashMap<String, ServerEntry>>,
    /// The store of system mode (the keychain; `MemoryStore` in tests).
    system: Arc<dyn SecretStore>,
    /// Secret changes waiting for the vault to be unlocked or created.
    pending: Mutex<Vec<(String, Option<String>)>>,
    /// F3 copies of server files: (location, copy folder).
    views: Mutex<Vec<(String, PathBuf)>>,
}

impl Connector {
    pub fn new(system: Arc<dyn SecretStore>, servers: ServerList) -> Connector {
        Connector {
            servers: Mutex::new(servers),
            vault: Mutex::new(None),
            session_secrets: MemoryStore::default(),
            quick: Mutex::new(HashMap::new()),
            system,
            pending: Mutex::new(Vec::new()),
            views: Mutex::new(Vec::new()),
        }
    }

    /// With `servers.json` from `data_dir`.
    pub fn load(system: Arc<dyn SecretStore>, data_dir: &Path) -> Connector {
        Connector::new(system, storage::load_json(data_dir, servers::SERVERS_FILE))
    }

    /// Runs `f` on the store of the current mode: the system store, or the vault when it is
    /// unlocked. None in portable mode with a locked (or missing) vault.
    fn with_store<R>(
        &self,
        data: &DataLocation,
        f: impl FnOnce(&dyn SecretStore) -> R,
    ) -> Option<R> {
        match data.mode {
            StorageMode::System => Some(f(self.system.as_ref())),
            StorageMode::Portable => lock(&self.vault).as_ref().map(|v| f(v)),
        }
    }

    /// Every account a server in the list can have.
    fn accounts(&self) -> Vec<String> {
        lock(&self.servers)
            .all()
            .iter()
            .flat_map(|e| KINDS.iter().map(|k| account(&e.id, *k)))
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<(ServerEntry, bool)> {
        if let Some(e) = lock(&self.servers).find_by_name(name) {
            return Some((e.clone(), true));
        }
        lock(&self.quick).get(name).map(|e| (e.clone(), false))
    }

    pub fn servers(&self) -> ServerList {
        lock(&self.servers).clone()
    }

    /// Validates and writes `servers.json`; the secrets of removed servers are deleted from
    /// the current store (keychain or unlocked vault) and from the session.
    pub fn save_servers(&self, data: &DataLocation, list: ServerList) -> Result<(), String> {
        list.validate()?;
        if list.all().iter().any(|e| e.id.trim().is_empty()) {
            return Err("Every server needs an id".into());
        }
        let keep: HashSet<String> = list.all().iter().map(|e| e.id.clone()).collect();
        let removed: Vec<String> = {
            // Read, write and replace in one critical section.
            let mut cur = lock(&self.servers);
            let removed = cur
                .all()
                .iter()
                .filter(|e| !keep.contains(&e.id))
                .map(|e| e.id.clone())
                .collect();
            storage::save_json(&data.dir, servers::SERVERS_FILE, &list)
                .map_err(|e| e.to_string())?;
            *cur = list;
            removed
        };
        for id in removed {
            for k in KINDS {
                let acct = account(&id, k);
                let _ = self.session_secrets.delete(&acct);
                lock(&self.pending).retain(|(a, _)| *a != acct);
                if let Some(Err(e)) = self.with_store(data, |st| st.delete(&acct)) {
                    eprintln!("fxfm: deleting {acct}: {e}");
                }
            }
        }
        Ok(())
    }

    /// Sets `savePassword` of server `id` and writes `servers.json`, under the servers lock
    /// (a concurrent `save_servers` is never reverted).
    fn mark_save_password(&self, data: &DataLocation, id: &str) -> Result<(), String> {
        let mut cur = lock(&self.servers);
        let mut list = cur.clone();
        set_save_password(&mut list.nodes, id);
        if list == *cur {
            return Ok(());
        }
        storage::save_json(&data.dir, servers::SERVERS_FILE, &list).map_err(|e| e.to_string())?;
        *cur = list;
        Ok(())
    }

    /// Stores (`Some`) or deletes (`None`) a server secret. In portable mode with a locked
    /// vault the change waits for the master password (`needMaster`); an entered secret is
    /// used for this session meanwhile.
    pub fn secret_set(
        &self,
        data: &DataLocation,
        id: &str,
        kind: SecretKind,
        secret: Option<String>,
    ) -> Result<(), SecretSetError> {
        let acct = account(id, kind.into());
        let done = self.with_store(data, |st| match &secret {
            Some(s) => st.set(&acct, s),
            None => st.delete(&acct),
        });
        match done {
            Some(r) => {
                r.map_err(SecretSetError::Message)?;
                // Stored: it wins over an older one typed this session.
                let _ = self.session_secrets.delete(&acct);
                lock(&self.pending).retain(|(a, _)| *a != acct);
                Ok(())
            }
            None => {
                let exists = Vault::exists(&data.dir);
                if secret.is_none() && !exists {
                    return Ok(());
                }
                if let Some(s) = &secret {
                    let _ = self.session_secrets.set(&acct, s);
                }
                let mut p = lock(&self.pending);
                p.retain(|(a, _)| *a != acct);
                p.push((acct, secret));
                Err(SecretSetError::Reply(ConnectReply::NeedMaster {
                    create: !exists,
                }))
            }
        }
    }

    /// Applies the secret changes that waited for the vault. Failed ones stay queued (unless
    /// a newer change of the same account was queued meanwhile); returns how many failed.
    fn flush_pending(&self, vault: &Vault) -> usize {
        let queued = std::mem::take(&mut *lock(&self.pending));
        let mut failed = Vec::new();
        for (acct, secret) in queued {
            let r = match &secret {
                Some(s) => vault.set(&acct, s),
                None => vault.delete(&acct),
            };
            if let Err(e) = r {
                eprintln!("fxfm: saving {acct}: {e}");
                failed.push((acct, secret));
            } else if secret.is_some() {
                let _ = self.session_secrets.delete(&acct);
            }
        }
        let n = failed.len();
        let mut p = lock(&self.pending);
        failed.retain(|(a, _)| !p.iter().any(|(b, _)| b == a));
        failed.append(&mut p);
        *p = failed;
        n
    }

    /// `flush_pending` on the vault now in use; failures become the error.
    fn unlocked_with(&self, v: Vault, what: &str) -> Result<(), String> {
        let failed = self.flush_pending(&v);
        *lock(&self.vault) = Some(v);
        match failed {
            0 => Ok(()),
            n => Err(format!("{what}, but {n} password(s) could not be saved")),
        }
    }

    fn require_portable(data: &DataLocation) -> Result<(), String> {
        match data.mode {
            StorageMode::Portable => Ok(()),
            StorageMode::System => Err("The master password is only used in portable mode".into()),
        }
    }

    pub fn master_unlock(&self, data: &DataLocation, password: &str) -> Result<(), String> {
        Self::require_portable(data)?;
        if !Vault::exists(&data.dir) {
            return Err("No master password is set yet".into());
        }
        let v = Vault::unlock(&data.dir, password).map_err(|e| e.to_string())?;
        self.unlocked_with(v, "Unlocked")
    }

    /// Creates the vault, or changes its master password (`old` must be the current one).
    pub fn master_set(
        &self,
        data: &DataLocation,
        old: Option<String>,
        new: &str,
    ) -> Result<(), String> {
        Self::require_portable(data)?;
        if new.is_empty() {
            return Err("The master password cannot be empty".into());
        }
        let v = if Vault::exists(&data.dir) {
            let old = old.ok_or("Enter the current master password")?;
            let mut v = Vault::unlock(&data.dir, &old).map_err(|e| e.to_string())?;
            v.change_master(new).map_err(|e| e.to_string())?;
            v
        } else {
            Vault::create(&data.dir, new).map_err(|e| e.to_string())?
        };
        self.unlocked_with(v, "Master password set")
    }

    pub fn master_status(&self, data: &DataLocation) -> MasterStatus {
        MasterStatus {
            mode: data.mode,
            exists: Vault::exists(&data.dir),
            unlocked: lock(&self.vault).is_some(),
        }
    }

    /// The server of a request: a saved one or an earlier quick connection by name, or a new
    /// quick connection from a URL (the same name keeps its id, so its session secrets).
    fn resolve(&self, req: &ConnectRequest) -> Result<(ServerEntry, bool), String> {
        if let Some(name) = req.server.as_deref() {
            return self
                .find(name)
                .ok_or_else(|| format!("Unknown server '{name}'"));
        }
        let url = req.url.as_deref().ok_or("No server or URL given")?;
        let mut e = servers::parse_url(url)?;
        let mut quick = lock(&self.quick);
        if let Some(old) = quick.get(&e.name) {
            e.id = old.id.clone();
        }
        quick.insert(e.name.clone(), e.clone());
        Ok((e, false))
    }

    /// Steps 1 and 2 of connecting: the server, an existing connection, and the secrets.
    pub fn prepare(
        &self,
        data: &DataLocation,
        ctx: &Ctx,
        req: &ConnectRequest,
    ) -> Result<Step, String> {
        let (server, saved) = self.resolve(req)?;
        if let Some(fs) = ctx.sessions.get(&server.name) {
            match start_reply(&server, fs.as_ref()) {
                Ok(r) => return Ok(Step::Done(r)),
                // Dead: connect again.
                Err(_) => {
                    ctx.sessions.remove(&server.name);
                }
            }
        }
        let save = req.save && saved;
        if data.mode == StorageMode::Portable
            && saved
            && (server.save_password || save)
            && lock(&self.vault).is_none()
        {
            return Ok(Step::Done(ConnectReply::NeedMaster {
                create: !Vault::exists(&data.dir),
            }));
        }
        let lookup = |kind: secrets::SecretKind, given: &Option<String>| -> Option<String> {
            if given.is_some() {
                return given.clone();
            }
            let acct = account(&server.id, kind);
            if let Ok(Some(s)) = self.session_secrets.get(&acct) {
                return Some(s);
            }
            if !saved {
                return None;
            }
            match self.with_store(data, |st| st.get(&acct)) {
                Some(Ok(s)) => s,
                Some(Err(e)) => {
                    eprintln!("fxfm: reading {acct}: {e}");
                    None
                }
                None => None,
            }
        };
        let creds = Credentials {
            password: lookup(secrets::SecretKind::Password, &req.password),
            passphrase: lookup(secrets::SecretKind::Passphrase, &req.passphrase),
        };
        Ok(Step::Attempt(Attempt {
            server,
            saved,
            creds,
            typed: Credentials {
                password: req.password.clone(),
                passphrase: req.passphrase.clone(),
            },
            save,
            accept_fingerprint: req.accept_fingerprint.clone(),
        }))
    }

    /// Steps 4 and 5: the reply, and a new connection registered under the server's name.
    pub fn finish(
        &self,
        data: &DataLocation,
        ctx: &Ctx,
        a: Attempt,
        result: Result<Arc<dyn RemoteFs>, ConnectError>,
    ) -> Result<ConnectReply, String> {
        let outcome = match self.with_store(data, |st| {
            after_connect(result.clone(), &a, &self.session_secrets, Some(st))
        }) {
            Some(o) => o,
            None => after_connect(result, &a, &self.session_secrets, None),
        }?;
        let fs = match outcome {
            Outcome::Reply(r) => return Ok(r),
            Outcome::Connected(fs) => fs,
        };
        if a.save && a.saved && !a.server.save_password {
            if let Err(e) = self.mark_save_password(data, &a.server.id) {
                eprintln!("fxfm: saving servers: {e}");
            }
        }
        // Another attempt may have connected meanwhile; keep that one.
        let fs = match ctx.sessions.get(&a.server.name) {
            Some(existing) => {
                fs.close();
                existing
            }
            None => {
                ctx.sessions.insert(&a.server.name, fs.clone());
                fs
            }
        };
        start_reply(&a.server, fs.as_ref()).map_err(|e| {
            ctx.sessions.remove(&a.server.name);
            e.to_string()
        })
    }

    /// The whole connection flow (blocking; the network part has its own timeouts).
    pub fn connect(
        &self,
        data: &DataLocation,
        ctx: &Ctx,
        req: &ConnectRequest,
    ) -> Result<ConnectReply, String> {
        let a = match self.prepare(data, ctx, req)? {
            Step::Done(r) => return Ok(r),
            Step::Attempt(a) => a,
        };
        let known = KnownHosts::new(data.dir.join(KNOWN_HOSTS_FILE));
        let opts = ConnectOptions {
            data_dir: &data.dir,
            known: &known,
            accept_fingerprint: a.accept_fingerprint.as_deref(),
        };
        let result = core_remote::connect(&a.server, &a.creds, &opts);
        self.finish(data, ctx, a, result)
    }

    pub fn list_connections(&self, ctx: &Ctx) -> Vec<ConnectionInfo> {
        ctx.sessions
            .names()
            .into_iter()
            .filter_map(|name| {
                let fs = ctx.sessions.get(&name)?;
                let scheme = self
                    .find(&name)
                    .map_or("sftp", |(e, _)| e.protocol.scheme());
                Some(ConnectionInfo {
                    root: format_remote(scheme, &name, "/"),
                    display: fs.display(),
                    name,
                })
            })
            .collect()
    }

    /// Remembers the F3 copy of a server file (removed when that connection is closed).
    pub fn record_view(&self, location: &str, local: &Path) {
        if parse_remote(location).is_none() {
            return;
        }
        if let Some(dir) = local.parent() {
            lock(&self.views).push((location.to_string(), dir.to_path_buf()));
        }
    }

    /// Closes the connection `name` (if open) and removes its temporary copies: the F3
    /// ones, and the F4 ones without changes that were not uploaded. Returns whether it was
    /// connected.
    pub fn disconnect(&self, ctx: &Ctx, edits: &EditWatch, name: &str) -> bool {
        let fs = ctx.sessions.remove(name);
        if let Some(fs) = &fs {
            fs.close();
        }
        let of_server = |loc: &str| parse_remote(loc).is_some_and(|r| r.server == name);
        let views: Vec<PathBuf> = {
            let mut v = lock(&self.views);
            let (gone, keep): (Vec<_>, Vec<_>) = v.drain(..).partition(|(l, _)| of_server(l));
            *v = keep;
            gone.into_iter().map(|(_, d)| d).collect()
        };
        for d in views {
            ctx.cleanup_dir(&d);
        }
        edits.remove_clean_copies(ctx, &of_server);
        fs.is_some()
    }

    /// On exit: closes every connection.
    pub fn close_all(&self, ctx: &Ctx) {
        for name in ctx.sessions.names() {
            if let Some(fs) = ctx.sessions.remove(&name) {
                fs.close();
            }
        }
    }
}

/// Moves the data folder (`storage::migrate`) and the saved secrets with it.
///
/// Portable → System: the vault must be unlocked (or `master` unlocks it); every secret is
/// written to the system store before the files move (and removed again if they cannot).
/// System → Portable: `master` creates the vault in the new folder; secrets are deleted from
/// the system store only after the vault was opened again and every value checked.
/// Returns the new location and how many secrets moved.
pub fn change_storage(
    conn: &Connector,
    from: &DataLocation,
    mode: StorageMode,
    master: Option<String>,
    app_folder: Option<&Path>,
    system_dir: &Path,
) -> Result<(DataLocation, usize), String> {
    if from.mode == mode {
        return Ok((from.clone(), 0));
    }
    let accounts = conn.accounts();
    let master = master.filter(|m| !m.is_empty());
    let moved = MemoryStore::default();
    match mode {
        StorageMode::System => {
            let mut vault = lock(&conn.vault);
            if Vault::exists(&from.dir) {
                if vault.is_none() {
                    let m = master.as_deref().ok_or(MASTER_REQUIRED)?;
                    *vault = Some(Vault::unlock(&from.dir, m).map_err(|e| e.to_string())?);
                }
                if let Some(v) = vault.as_ref() {
                    migrate_secrets(v, &moved, &accounts)?;
                }
            }
            // Changes still waiting for the vault move too (newest wins).
            let pending = lock(&conn.pending).clone();
            let mut accounts = accounts;
            for (acct, secret) in &pending {
                match secret {
                    Some(s) => moved.set(acct, s)?,
                    None => moved.delete(acct)?,
                }
                if !accounts.contains(acct) {
                    accounts.push(acct.clone());
                }
            }
            let undo = || {
                for a in &accounts {
                    if matches!(moved.get(a), Ok(Some(_))) {
                        let _ = conn.system.delete(a);
                    }
                }
            };
            let n = match migrate_secrets(&moved, conn.system.as_ref(), &accounts) {
                Ok(n) => n,
                Err(e) => {
                    undo();
                    return Err(e);
                }
            };
            let to = match storage::migrate(from, mode, app_folder, system_dir) {
                Ok(to) => to,
                Err(e) => {
                    undo();
                    return Err(e.to_string());
                }
            };
            // A queued delete also removes an older keychain entry.
            for (acct, secret) in &pending {
                if secret.is_none() {
                    if let Err(e) = conn.system.delete(acct) {
                        eprintln!("fxfm: deleting {acct} from the keychain: {e}");
                    }
                }
            }
            lock(&conn.pending).clear();
            // System mode keeps no password file (the old portable folder still has it).
            let _ = std::fs::remove_file(to.dir.join(secrets::VAULT_FILE));
            *vault = None;
            Ok((to, n))
        }
        StorageMode::Portable => {
            let master = master.ok_or(MASTER_REQUIRED)?;
            // Read first: a keychain that refuses stops the move before any file moves.
            let n = migrate_secrets(conn.system.as_ref(), &moved, &accounts)?;
            let to =
                storage::migrate(from, mode, app_folder, system_dir).map_err(|e| e.to_string())?;
            let filled = (|| -> Result<Vault, String> {
                // A stale copy from the system folder; the original stays there.
                let _ = std::fs::remove_file(to.dir.join(secrets::VAULT_FILE));
                let v = Vault::create(&to.dir, &master).map_err(|e| e.to_string())?;
                migrate_secrets(&moved, &v, &accounts)?;
                drop(v);
                let v = Vault::unlock(&to.dir, &master).map_err(|e| e.to_string())?;
                for a in &accounts {
                    if moved.get(a)? != v.get(a)? {
                        return Err("A moved password could not be read back".into());
                    }
                }
                Ok(v)
            })();
            match filled {
                Ok(v) => {
                    for a in &accounts {
                        if matches!(moved.get(a), Ok(Some(_))) {
                            if let Err(e) = conn.system.delete(a) {
                                eprintln!("fxfm: deleting {a} from the keychain: {e}");
                            }
                        }
                    }
                    *lock(&conn.vault) = Some(v);
                    Ok((to, n))
                }
                Err(e) => {
                    // The new folder is a fresh copy; System stays in use.
                    let _ = std::fs::remove_dir_all(&to.dir);
                    Err(e)
                }
            }
        }
    }
}

/// Connections in `before` that are gone now.
pub fn lost_sessions(before: &[String], ctx: &Ctx) -> Vec<String> {
    let now = ctx.sessions.names();
    before
        .iter()
        .filter(|n| !now.contains(n))
        .cloned()
        .collect()
}

pub fn emit_changed(app: &AppHandle, lost: Vec<String>) {
    if let Err(e) = app.emit(CONNECTIONS_CHANGED, serde_json::json!({ "lost": lost })) {
        eprintln!("fxfm: emit {CONNECTIONS_CHANGED}: {e}");
    }
}

/// Runs `f`; connections it found dead (and the core dropped) are announced.
pub fn watch<T>(app: &AppHandle, ctx: &Ctx, f: impl FnOnce() -> T) -> T {
    let before = ctx.sessions.names();
    let r = f();
    let lost = lost_sessions(&before, ctx);
    if !lost.is_empty() {
        emit_changed(app, lost);
    }
    r
}

/// `chmod` on a server file; local and archive locations are refused.
pub fn chmod_location(ctx: &Ctx, location: &str, mode: u32) -> Result<(), String> {
    let r = parse_remote(location).ok_or(CHMOD_ONLY_REMOTE)?;
    let fs = ctx
        .sessions
        .get(&r.server)
        .ok_or_else(|| RemoteError::Disconnected.to_string())?;
    match fs.set_mode(&r.path, mode & 0o7777) {
        Err(e) if e.is_lost() => {
            ctx.sessions.remove(&r.server);
            Err(e.to_string())
        }
        other => other.map_err(|e| e.to_string()),
    }
}

// ------------------------------------------------------------------ commands

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

fn data_of(state: &AppState) -> DataLocation {
    lock(&state.data).clone()
}

#[tauri::command]
pub async fn servers_load(app: AppHandle) -> Result<ServerList, String> {
    blocking(move || app.state::<AppState>().remote.servers()).await
}

#[tauri::command]
pub async fn servers_save(app: AppHandle, list: ServerList) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.save_servers(&data_of(&state), list)
    })
    .await?
}

#[tauri::command]
pub async fn server_secret_set(
    app: AppHandle,
    id: String,
    kind: SecretKind,
    secret: Option<String>,
) -> Result<(), SecretSetError> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.secret_set(&data_of(&state), &id, kind, secret)
    })
    .await
    .map_err(SecretSetError::Message)?
}

#[tauri::command]
pub async fn connect(app: AppHandle, req: ConnectRequest) -> Result<ConnectReply, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let before = state.vfs.sessions.names();
        let r = state.remote.connect(&data_of(&state), &state.vfs, &req);
        if matches!(r, Ok(ConnectReply::Connected { .. })) || state.vfs.sessions.names() != before {
            emit_changed(&app, Vec::new());
        }
        r
    })
    .await?
}

#[tauri::command]
pub async fn disconnect(app: AppHandle, name: String) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.disconnect(&state.vfs, &state.edits, &name);
        emit_changed(&app, Vec::new());
    })
    .await
}

#[tauri::command]
pub fn list_connections(app: AppHandle) -> Vec<ConnectionInfo> {
    let state = app.state::<AppState>();
    state.remote.list_connections(&state.vfs)
}

#[tauri::command]
pub async fn master_unlock(app: AppHandle, password: String) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.master_unlock(&data_of(&state), &password)
    })
    .await?
}

#[tauri::command]
pub async fn master_set(app: AppHandle, old: Option<String>, new: String) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.master_set(&data_of(&state), old, &new)
    })
    .await?
}

#[tauri::command]
pub async fn master_status(app: AppHandle) -> Result<MasterStatus, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        state.remote.master_status(&data_of(&state))
    })
    .await
}

/// Copies a private key into `<data dir>/keys/` (0600); returns the `keys/<name>` to store.
#[tauri::command]
pub async fn import_key(app: AppHandle, path: String) -> Result<String, String> {
    blocking(move || {
        let state = app.state::<AppState>();
        servers::import_key(Path::new(&path), &data_of(&state).dir).map_err(|e| e.to_string())
    })
    .await?
}

#[tauri::command]
pub async fn chmod(app: AppHandle, location: String, mode: u32) -> Result<(), String> {
    blocking(move || {
        let state = app.state::<AppState>();
        watch(&app, &state.vfs, || {
            chmod_location(&state.vfs, &location, mode)
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{run_job, JobRequest};
    use fxfm_core::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
    use fxfm_core::remote::RemoteStat;
    use fxfm_core::secrets::{account, VAULT_FILE};
    use fxfm_core::servers::{Auth, Protocol, ServerNode, SERVERS_FILE};
    use fxfm_core::storage::DATA_DIR_NAME;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};

    // ------------------------------------------------------------ test server

    /// An in-memory server with files only; `dead` makes every call `Disconnected`, `hung`
    /// makes it `Timeout`. Forks share everything.
    #[derive(Default, Clone)]
    struct FakeFs {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        dead: Arc<AtomicBool>,
        hung: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
    }

    impl FakeFs {
        fn with(files: &[(&str, &[u8])]) -> Arc<FakeFs> {
            let fs = FakeFs::default();
            for (p, d) in files {
                fs.files.lock().unwrap().insert(p.to_string(), d.to_vec());
            }
            Arc::new(fs)
        }

        fn alive(&self) -> Result<(), RemoteError> {
            if self.dead.load(Ordering::SeqCst) {
                Err(RemoteError::Disconnected)
            } else if self.hung.load(Ordering::SeqCst) {
                Err(RemoteError::Timeout)
            } else {
                Ok(())
            }
        }
    }

    impl RemoteFs for FakeFs {
        fn display(&self) -> String {
            "sftp://zb@fake".into()
        }
        fn home(&self) -> Result<String, RemoteError> {
            self.alive()?;
            Ok("/home/zb".into())
        }
        fn list(&self, _path: &str) -> Result<Vec<fxfm_core::listing::Entry>, RemoteError> {
            self.alive()?;
            Ok(Vec::new())
        }
        fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
            self.alive()?;
            if path == "/" {
                return Ok(Some(RemoteStat {
                    is_dir: true,
                    is_symlink: false,
                    size: 0,
                    mtime: 0,
                    mode: 0o755,
                }));
            }
            Ok(self.files.lock().unwrap().get(path).map(|d| RemoteStat {
                is_dir: false,
                is_symlink: false,
                size: d.len() as u64,
                mtime: 1_700_000_000,
                mode: 0o644,
            }))
        }
        fn read_link(&self, _path: &str) -> Result<String, RemoteError> {
            Err(RemoteError::Unsupported("no links".into()))
        }
        fn read_to(
            &self,
            path: &str,
            out: &mut dyn Write,
            _progress: &mut dyn FnMut(u64) -> bool,
        ) -> Result<(), RemoteError> {
            self.alive()?;
            let d = self
                .files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| RemoteError::NotFound(path.into()))?;
            out.write_all(&d)
                .map_err(|e| RemoteError::Other(e.to_string()))
        }
        fn write_from(
            &self,
            path: &str,
            input: &mut dyn Read,
            _progress: &mut dyn FnMut(u64) -> bool,
        ) -> Result<(), RemoteError> {
            self.alive()?;
            let mut d = Vec::new();
            input
                .read_to_end(&mut d)
                .map_err(|e| RemoteError::Other(e.to_string()))?;
            self.files.lock().unwrap().insert(path.into(), d);
            Ok(())
        }
        fn mkdir(&self, _path: &str) -> Result<(), RemoteError> {
            Err(RemoteError::Unsupported("no folders".into()))
        }
        fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
            self.alive()?;
            self.files.lock().unwrap().remove(path);
            Ok(())
        }
        fn remove_dir(&self, _path: &str) -> Result<(), RemoteError> {
            Err(RemoteError::Unsupported("no folders".into()))
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
            self.replace(from, to)
        }
        fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
            self.alive()?;
            let mut f = self.files.lock().unwrap();
            let d = f
                .remove(from)
                .ok_or_else(|| RemoteError::NotFound(from.into()))?;
            f.insert(to.into(), d);
            Ok(())
        }
        fn symlink(&self, _target: &str, _link: &str) -> Result<(), RemoteError> {
            Err(RemoteError::Unsupported("no links".into()))
        }
        fn set_mode(&self, _path: &str, _mode: u32) -> Result<(), RemoteError> {
            self.alive()
        }
        fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError> {
            self.alive()?;
            Ok(Arc::new(self.clone()))
        }
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }

    struct Quiet;
    impl OpObserver for Quiet {
        fn progress(&self, _p: &Progress) {}
        fn conflict(&self, _c: &ConflictInfo) -> ConflictAnswer {
            ConflictAnswer::Overwrite
        }
        fn error(&self, _path: &str, _msg: &str) -> ErrorAnswer {
            ErrorAnswer::Skip
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    // ------------------------------------------------------------ helpers

    fn srv(name: &str) -> ServerEntry {
        ServerEntry {
            id: fxfm_core::servers::new_id(),
            name: name.into(),
            protocol: Protocol::Sftp,
            host: "127.0.0.1".into(),
            port: 2222,
            user: "zb".into(),
            save_password: true,
            ..ServerEntry::default()
        }
    }

    fn list_of(entries: &[&ServerEntry]) -> ServerList {
        ServerList {
            nodes: entries
                .iter()
                .map(|e| ServerNode::Server((*e).clone()))
                .collect(),
        }
    }

    fn loc(mode: StorageMode, dir: &Path) -> DataLocation {
        DataLocation {
            mode,
            dir: dir.to_path_buf(),
        }
    }

    fn connector() -> (Connector, Arc<MemoryStore>) {
        let sys = Arc::new(MemoryStore::default());
        (Connector::new(sys.clone(), ServerList::default()), sys)
    }

    fn attempt(server: ServerEntry, typed_pw: Option<&str>) -> Attempt {
        let creds = Credentials {
            password: typed_pw.map(str::to_string),
            passphrase: None,
        };
        Attempt {
            server,
            saved: true,
            typed: creds.clone(),
            creds,
            save: false,
            accept_fingerprint: None,
        }
    }

    // ------------------------------------------------------------ tests

    #[test]
    fn connect_reply_serde() {
        let j = |r: ConnectReply| serde_json::to_value(r).unwrap();
        assert_eq!(
            j(ConnectReply::Connected {
                location: "sftp://m/home/zb".into(),
                local_dir: None
            }),
            json!({"kind": "connected", "location": "sftp://m/home/zb", "localDir": null})
        );
        assert_eq!(
            j(ConnectReply::HostKey {
                host: "h".into(),
                port: 22,
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:x".into(),
                changed_from: None
            }),
            json!({"kind": "hostKey", "host": "h", "port": 22, "keyType": "ssh-ed25519",
                   "fingerprint": "SHA256:x", "changedFrom": null})
        );
        assert_eq!(
            j(ConnectReply::NeedPassword { retry: true }),
            json!({"kind": "needPassword", "retry": true})
        );
        assert_eq!(
            j(ConnectReply::NeedPassphrase { retry: false }),
            json!({"kind": "needPassphrase", "retry": false})
        );
        assert_eq!(
            j(ConnectReply::NeedMaster { create: true }),
            json!({"kind": "needMaster", "create": true})
        );
        // server_secret_set's error is the reply itself, or a plain message.
        assert_eq!(
            serde_json::to_value(SecretSetError::Reply(ConnectReply::NeedMaster {
                create: false
            }))
            .unwrap(),
            json!({"kind": "needMaster", "create": false})
        );
        assert_eq!(
            serde_json::to_value(SecretSetError::Message("x".into())).unwrap(),
            json!("x")
        );
        let r: ConnectRequest = serde_json::from_value(json!({
            "server": "m", "password": "pw", "save": true, "acceptFingerprint": "SHA256:x"
        }))
        .unwrap();
        assert_eq!(r.server.as_deref(), Some("m"));
        assert!(r.save && r.url.is_none() && r.passphrase.is_none());
        assert_eq!(r.accept_fingerprint.as_deref(), Some("SHA256:x"));
        let k: SecretKind = serde_json::from_value(json!("passphrase")).unwrap();
        assert_eq!(k, SecretKind::Passphrase);
    }

    #[test]
    fn servers_save_removes_secrets_of_deleted() {
        let t = tempfile::tempdir().unwrap();
        let data = loc(StorageMode::System, t.path());
        let (c, sys) = connector();
        let (a, b) = (srv("a"), srv("b"));
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-pa".into()))
            .unwrap();
        c.secret_set(&data, &b.id, SecretKind::Password, Some("secret-pb".into()))
            .unwrap();
        c.secret_set(
            &data,
            &b.id,
            SecretKind::Passphrase,
            Some("secret-kb".into()),
        )
        .unwrap();
        let pb = account(&b.id, fxfm_core::secrets::SecretKind::Password);
        c.session_secrets.set(&pb, "typed").unwrap();
        assert_eq!(sys.get(&pb).unwrap().as_deref(), Some("secret-pb"));

        c.save_servers(&data, list_of(&[&a])).unwrap();
        assert_eq!(sys.get(&pb).unwrap(), None);
        assert_eq!(
            sys.get(&account(&b.id, fxfm_core::secrets::SecretKind::Passphrase))
                .unwrap(),
            None
        );
        assert_eq!(c.session_secrets.get(&pb).unwrap(), None);
        let pa = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        assert_eq!(sys.get(&pa).unwrap().as_deref(), Some("secret-pa"));
        // servers.json holds the list, never a secret.
        let text = std::fs::read_to_string(t.path().join(SERVERS_FILE)).unwrap();
        assert!(text.contains("\"a\"") && !text.contains("\"b\"") && !text.contains("secret"));
        // An invalid list is refused and nothing changes.
        let bad = list_of(&[&a, &a]);
        assert!(c.save_servers(&data, bad).is_err());
        assert_eq!(c.servers.lock().unwrap().all().len(), 1);
    }

    #[test]
    fn portable_save_password_needs_master() {
        let t = tempfile::tempdir().unwrap();
        let data = loc(StorageMode::Portable, t.path());
        let (c, sys) = connector();
        let a = srv("a");
        c.save_servers(&data, list_of(&[&a])).unwrap();
        let r = c.secret_set(&data, &a.id, SecretKind::Password, Some("pw-a".into()));
        assert_eq!(
            r,
            Err(SecretSetError::Reply(ConnectReply::NeedMaster {
                create: true
            }))
        );
        assert!(!t.path().join(VAULT_FILE).exists());
        c.master_set(&data, None, "master").unwrap();
        let acct = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        let v = Vault::unlock(t.path(), "master").unwrap();
        assert_eq!(v.get(&acct).unwrap().as_deref(), Some("pw-a"));
        // Never in the system store, never in clear text on disk.
        assert_eq!(sys.get(&acct).unwrap(), None);
        let text = std::fs::read_to_string(t.path().join(VAULT_FILE)).unwrap();
        assert!(!text.contains("pw-a"));
        // Unlocked now: a change goes straight in.
        c.secret_set(&data, &a.id, SecretKind::Password, Some("pw-2".into()))
            .unwrap();
        assert_eq!(v.get(&acct).unwrap().as_deref(), Some("pw-2"));
        // A locked, existing vault asks for the master password (create: false).
        *c.vault.lock().unwrap() = None;
        assert_eq!(
            c.secret_set(&data, &a.id, SecretKind::Password, None),
            Err(SecretSetError::Reply(ConnectReply::NeedMaster {
                create: false
            }))
        );
        // Changing it needs the current one.
        assert!(c.master_set(&data, None, "new").is_err());
        assert!(c.master_set(&data, Some("wrong".into()), "new").is_err());
        c.master_set(&data, Some("master".into()), "new").unwrap();
        // The pending delete was applied on unlock.
        let v = Vault::unlock(t.path(), "new").unwrap();
        assert_eq!(v.get(&acct).unwrap(), None);
    }

    #[test]
    fn wrong_password_not_cached() {
        let session = MemoryStore::default();
        let store = MemoryStore::default();
        let a = srv("a");
        let acct = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        // A password cached earlier that the server now rejects.
        session.set(&acct, "old").unwrap();
        let mut at = attempt(a.clone(), Some("bad"));
        at.save = true;
        match after_connect(Err(ConnectError::AuthFailed), &at, &session, Some(&store)) {
            Ok(Outcome::Reply(r)) => assert_eq!(r, ConnectReply::NeedPassword { retry: true }),
            _ => panic!("expected needPassword"),
        }
        assert_eq!(session.get(&acct).unwrap(), None);
        assert_eq!(store.get(&acct).unwrap(), None);

        // A passphrase that does not decrypt the key: ask for it again, forget it.
        let mut k = srv("k");
        k.auth = Auth::KeyPassphrase;
        k.key_path = Some("/k".into());
        let pacct = account(&k.id, fxfm_core::secrets::SecretKind::Passphrase);
        session.set(&pacct, "bad").unwrap();
        let mut at = attempt(k.clone(), None);
        at.creds.passphrase = Some("bad".into());
        match after_connect(Err(ConnectError::BadPassphrase), &at, &session, None) {
            Ok(Outcome::Reply(r)) => {
                assert_eq!(r, ConnectReply::NeedPassphrase { retry: true })
            }
            _ => panic!("expected needPassphrase"),
        }
        assert_eq!(session.get(&pacct).unwrap(), None);

        // The passphrase decrypted the key but the server refused the key: a message (no
        // endless passphrase prompt), and the passphrase stays.
        session.set(&pacct, "good").unwrap();
        let mut at = attempt(k, None);
        at.creds.passphrase = Some("good".into());
        assert_eq!(
            after_connect(Err(ConnectError::AuthFailed), &at, &session, None).err(),
            Some(WRONG_LOGIN.to_string())
        );
        assert_eq!(session.get(&pacct).unwrap().as_deref(), Some("good"));

        // A key without passphrase that the server refuses: a message.
        let mut k = srv("k2");
        k.auth = Auth::Key;
        let at = attempt(k, None);
        assert_eq!(
            after_connect(Err(ConnectError::AuthFailed), &at, &session, None).err(),
            Some(WRONG_LOGIN.to_string())
        );

        // Every other ConnectError.
        let at = attempt(a.clone(), None);
        let reply = |e| match after_connect(Err(e), &at, &session, None) {
            Ok(Outcome::Reply(r)) => Ok(r),
            Ok(Outcome::Connected(_)) => panic!("not connected"),
            Err(m) => Err(m),
        };
        assert_eq!(
            reply(ConnectError::NeedPassword),
            Ok(ConnectReply::NeedPassword { retry: false })
        );
        assert_eq!(
            reply(ConnectError::NeedPassphrase),
            Ok(ConnectReply::NeedPassphrase { retry: false })
        );
        assert_eq!(
            reply(ConnectError::Timeout),
            Err(CONNECT_TIMEOUT_MSG.to_string())
        );
        let legacy = "The server only accepts RSA keys signed with SHA-1 (ssh-rsa). Enable \"Allow legacy algorithms (ssh-rsa/SHA1)\" for this server.";
        assert_eq!(
            reply(ConnectError::Failed(legacy.into())),
            Err(legacy.to_string())
        );
        assert_eq!(
            reply(ConnectError::HostKey {
                host: "h".into(),
                port: 22,
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:n".into(),
                changed_from: Some("SHA256:o".into()),
            }),
            Ok(ConnectReply::HostKey {
                host: "h".into(),
                port: 22,
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:n".into(),
                changed_from: Some("SHA256:o".into()),
            })
        );

        // Success: the typed password is cached, and stored with `save`.
        let mut at = attempt(a, Some("good"));
        at.save = true;
        let fs: Arc<dyn RemoteFs> = FakeFs::with(&[]);
        assert!(matches!(
            after_connect(Ok(fs), &at, &session, Some(&store)),
            Ok(Outcome::Connected(_))
        ));
        assert_eq!(session.get(&acct).unwrap().as_deref(), Some("good"));
        assert_eq!(store.get(&acct).unwrap().as_deref(), Some("good"));
    }

    /// A portable data folder with servers `a` (password saved in the vault) and `b`.
    fn portable_setup(app: &Path) -> (DataLocation, ServerEntry, ServerEntry) {
        let dir = app.join(DATA_DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let data = loc(StorageMode::Portable, &dir);
        (data, srv("a"), srv("b"))
    }

    #[test]
    fn migrate_portable_to_system_moves_secrets() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let sysdir = t.path().join("system");
        let (data, a, b) = portable_setup(&app);
        let (c, sys) = connector();
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.master_set(&data, None, "m").unwrap();
        c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-pa".into()))
            .unwrap();
        // Locked: refused, nothing moves.
        *c.vault.lock().unwrap() = None;
        let e = change_storage(&c, &data, StorageMode::System, None, Some(&app), &sysdir);
        assert_eq!(e.err().as_deref(), Some(MASTER_REQUIRED));
        assert!(!sysdir.exists() || std::fs::read_dir(&sysdir).unwrap().next().is_none());
        // With the master password it is unlocked and moved.
        let (to, n) = change_storage(
            &c,
            &data,
            StorageMode::System,
            Some("m".into()),
            Some(&app),
            &sysdir,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(to.mode, StorageMode::System);
        assert_eq!(to.dir, sysdir);
        let pa = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        assert_eq!(sys.get(&pa).unwrap().as_deref(), Some("secret-pa"));
        assert!(to.dir.join(SERVERS_FILE).is_file());
        assert!(!to.dir.join(VAULT_FILE).exists());
        assert!(c.vault.lock().unwrap().is_none());
    }

    #[test]
    fn migrate_system_to_portable_requires_master() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let sysdir = t.path().join("system");
        std::fs::create_dir_all(&sysdir).unwrap();
        let data = loc(StorageMode::System, &sysdir);
        let (c, sys) = connector();
        let (a, b) = (srv("a"), srv("b"));
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-pa".into()))
            .unwrap();
        c.secret_set(
            &data,
            &b.id,
            SecretKind::Passphrase,
            Some("secret-kb".into()),
        )
        .unwrap();
        let e = change_storage(&c, &data, StorageMode::Portable, None, Some(&app), &sysdir);
        assert_eq!(e.err().as_deref(), Some(MASTER_REQUIRED));
        assert!(!app.join(DATA_DIR_NAME).exists());
        let (to, n) = change_storage(
            &c,
            &data,
            StorageMode::Portable,
            Some("m".into()),
            Some(&app),
            &sysdir,
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(to.dir, app.join(DATA_DIR_NAME));
        let v = Vault::unlock(&to.dir, "m").unwrap();
        let pa = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        let kb = account(&b.id, fxfm_core::secrets::SecretKind::Passphrase);
        assert_eq!(v.get(&pa).unwrap().as_deref(), Some("secret-pa"));
        assert_eq!(v.get(&kb).unwrap().as_deref(), Some("secret-kb"));
        // Gone from the system store only after the check.
        assert_eq!(sys.get(&pa).unwrap(), None);
        assert_eq!(sys.get(&kb).unwrap(), None);
        assert!(c.vault.lock().unwrap().is_some());
    }

    #[test]
    fn disconnected_error_drops_session() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let (c, _) = connector();
        let fs = FakeFs::with(&[("/f.txt", b"x")]);
        ctx.sessions.insert("m", fs.clone());
        ctx.sessions.insert("other", FakeFs::with(&[]));
        assert_eq!(c.list_connections(&ctx).len(), 2);
        let before = ctx.sessions.names();
        fs.dead.store(true, Ordering::SeqCst);
        let out = t.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let req = JobRequest::Copy {
            sources: vec!["sftp://m/f.txt".into()],
            dest: out.to_string_lossy().into_owned(),
            cwd: "sftp://m/".into(),
        };
        let _ = run_job(&ctx, &req, &Quiet);
        let names: Vec<String> = c
            .list_connections(&ctx)
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, vec!["other".to_string()]);
        assert_eq!(lost_sessions(&before, &ctx), vec!["m".to_string()]);
    }

    #[test]
    fn timeout_error_drops_session() {
        // "server not responding" is a lost connection in every layer: the job drops the
        // session (the UI gets it in `lost`), and a hung session is never reused on connect.
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let (c, _) = connector();
        let fs = FakeFs::with(&[("/f.txt", b"x")]);
        ctx.sessions.insert("m", fs.clone());
        ctx.sessions.insert("other", FakeFs::with(&[]));
        let before = ctx.sessions.names();
        fs.hung.store(true, Ordering::SeqCst);
        let out = t.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let req = JobRequest::Copy {
            sources: vec!["sftp://m/f.txt".into()],
            dest: out.to_string_lossy().into_owned(),
            cwd: "sftp://m/".into(),
        };
        let r = run_job(&ctx, &req, &Quiet);
        assert!(r.is_err(), "{r:?}");
        let names: Vec<String> = c
            .list_connections(&ctx)
            .into_iter()
            .map(|i| i.name)
            .collect();
        assert_eq!(names, vec!["other".to_string()]);
        assert_eq!(lost_sessions(&before, &ctx), vec!["m".to_string()]);

        // A panel listing on a hung session drops it the same way.
        ctx.sessions.insert("m", fs.clone());
        let before = ctx.sessions.names();
        assert!(fxfm_core::vfs::list(&ctx, "sftp://m/").is_err());
        assert_eq!(lost_sessions(&before, &ctx), vec!["m".to_string()]);

        // An existing session whose home() hangs counts as dead, not as "start at /".
        let server = srv("m");
        assert_eq!(
            start_reply(&server, fs.as_ref()).err(),
            Some(RemoteError::Timeout)
        );
    }

    #[test]
    fn list_connections_shape() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let (c, _) = connector();
        let mut m = srv("web-1");
        m.protocol = Protocol::Ftps;
        *c.servers.lock().unwrap() = list_of(&[&m]);
        ctx.sessions.insert("web-1", FakeFs::with(&[]));
        assert_eq!(
            c.list_connections(&ctx),
            vec![ConnectionInfo {
                name: "web-1".into(),
                display: "sftp://zb@fake".into(),
                root: "ftps://web-1/".into(),
            }]
        );
    }

    #[test]
    fn disconnect_cleans_clean_copies_only() {
        let t = tempfile::tempdir().unwrap();
        let temp_root = t.path().join("tmp-root");
        let ctx = Ctx::new(temp_root.clone());
        let (c, _) = connector();
        let fs = FakeFs::with(&[("/a.txt", b"a"), ("/b.txt", b"b"), ("/c.txt", b"c")]);
        ctx.sessions.insert("m", fs.clone());
        ctx.sessions.insert("n", FakeFs::with(&[("/d.txt", b"d")]));
        let edits = EditWatch::new(Arc::new(|_, _: &str, _: &str| {}), temp_root);
        let (_, clean) = edits.open_for_edit(&ctx, "sftp://m/a.txt").unwrap();
        let (_, dirty) = edits.open_for_edit(&ctx, "sftp://m/b.txt").unwrap();
        let (_, other) = edits.open_for_edit(&ctx, "sftp://n/d.txt").unwrap();
        std::fs::write(&dirty, b"edited").unwrap();
        let view = fxfm_core::vfs::materialize(&ctx, "sftp://m/c.txt", "view").unwrap();
        c.record_view("sftp://m/c.txt", &view);
        let other_view = fxfm_core::vfs::materialize(&ctx, "sftp://n/d.txt", "view").unwrap();
        c.record_view("sftp://n/d.txt", &other_view);

        assert!(c.disconnect(&ctx, &edits, "m"));
        assert!(fs.closed.load(Ordering::SeqCst));
        assert!(ctx.sessions.get("m").is_none());
        assert!(!clean.exists() && !clean.parent().unwrap().exists());
        assert!(!view.exists());
        assert_eq!(std::fs::read(&dirty).unwrap(), b"edited");
        assert!(other.exists() && other_view.exists());
        let pending: Vec<String> = edits.pending().into_iter().map(|p| p.location).collect();
        assert_eq!(pending, vec!["sftp://m/b.txt".to_string()]);
        // Not connected any more: still Ok, nothing else to clean.
        assert!(!c.disconnect(&ctx, &edits, "m"));
    }

    #[test]
    fn server_edit_conflict_mentions_server() {
        let t = tempfile::tempdir().unwrap();
        let temp_root = t.path().join("tmp-root");
        let ctx = Ctx::new(temp_root.clone());
        let fs = FakeFs::with(&[("/x.txt", b"one")]);
        ctx.sessions.insert("m", fs.clone());
        let edits = EditWatch::new(Arc::new(|_, _: &str, _: &str| {}), temp_root);
        let (id, local) = edits.open_for_edit(&ctx, "sftp://m/x.txt").unwrap();
        std::fs::write(&local, b"mine").unwrap();
        fs.files
            .lock()
            .unwrap()
            .insert("/x.txt".into(), b"theirs!".to_vec());
        assert_eq!(
            edits.prepare_upload(&ctx, id, false).err().as_deref(),
            Some("conflict: x.txt was changed on the server after you opened it")
        );
    }

    #[test]
    fn chmod_only_on_servers() {
        let t = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(t.path().join("tmp-root"));
        let f = t.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        let local = f.to_string_lossy().into_owned();
        assert_eq!(
            chmod_location(&ctx, &local, 0o600).err().as_deref(),
            Some(CHMOD_ONLY_REMOTE)
        );
        assert_eq!(
            chmod_location(&ctx, &format!("{}/a.zip/x", t.path().display()), 0o600)
                .err()
                .as_deref(),
            Some(CHMOD_ONLY_REMOTE)
        );
        let fs = FakeFs::with(&[("/x", b"x")]);
        ctx.sessions.insert("m", fs.clone());
        chmod_location(&ctx, "sftp://m/x", 0o600).unwrap();
        fs.dead.store(true, Ordering::SeqCst);
        assert!(chmod_location(&ctx, "sftp://m/x", 0o600).is_err());
        assert!(ctx.sessions.get("m").is_none());
    }

    /// A store whose writes fail (a keychain that refuses).
    #[derive(Default)]
    struct FailingStore(MemoryStore);

    impl SecretStore for FailingStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            self.0.get(account)
        }
        fn set(&self, _account: &str, _secret: &str) -> Result<(), String> {
            Err("Keychain: refused".into())
        }
        fn delete(&self, _account: &str) -> Result<(), String> {
            Err("Keychain: refused".into())
        }
    }

    #[test]
    fn failed_store_write_keeps_session_secret() {
        let t = tempfile::tempdir().unwrap();
        let data = loc(StorageMode::System, t.path());
        let c = Connector::new(Arc::new(FailingStore::default()), ServerList::default());
        let a = srv("a");
        c.save_servers(&data, list_of(&[&a])).unwrap();
        let acct = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        c.session_secrets.set(&acct, "typed").unwrap();
        assert_eq!(
            c.secret_set(&data, &a.id, SecretKind::Password, Some("new".into())),
            Err(SecretSetError::Message("Keychain: refused".into()))
        );
        assert_eq!(
            c.session_secrets.get(&acct).unwrap().as_deref(),
            Some("typed")
        );
    }

    #[test]
    fn queued_password_survives_failed_save() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        let data = loc(StorageMode::Portable, &dir);
        let (c, _) = connector();
        let a = srv("a");
        c.save_servers(&data, list_of(&[&a])).unwrap();
        c.master_set(&data, None, "m").unwrap();
        *c.vault.lock().unwrap() = None;
        assert_eq!(
            c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-a".into())),
            Err(SecretSetError::Reply(ConnectReply::NeedMaster {
                create: false
            }))
        );
        // The vault file cannot be rewritten: unlocked, but the password is not saved.
        let mode = |m| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(m));
        mode(0o500).unwrap();
        let r = c.master_unlock(&data, "m");
        mode(0o700).unwrap();
        assert_eq!(
            r.err().as_deref(),
            Some("Unlocked, but 1 password(s) could not be saved")
        );
        assert!(c.master_status(&data).unlocked);
        let acct = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        assert_eq!(Vault::unlock(&dir, "m").unwrap().get(&acct).unwrap(), None);
        // Still queued (and usable this session): the next unlock saves it.
        assert_eq!(
            c.session_secrets.get(&acct).unwrap().as_deref(),
            Some("secret-a")
        );
        c.master_unlock(&data, "m").unwrap();
        assert_eq!(
            Vault::unlock(&dir, "m")
                .unwrap()
                .get(&acct)
                .unwrap()
                .as_deref(),
            Some("secret-a")
        );
    }

    #[test]
    fn migrate_portable_to_system_takes_queued_changes() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let sysdir = t.path().join("system");
        let (data, a, b) = portable_setup(&app);
        let (c, sys) = connector();
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.master_set(&data, None, "m").unwrap();
        c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-a".into()))
            .unwrap();
        *c.vault.lock().unwrap() = None;
        // Queued while locked: a new password for b, and a delete for a.
        let pb = account(&b.id, fxfm_core::secrets::SecretKind::Password);
        let pa = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        sys.set(&pa, "stale").unwrap();
        assert!(c
            .secret_set(&data, &b.id, SecretKind::Password, Some("secret-b".into()))
            .is_err());
        assert!(c
            .secret_set(&data, &a.id, SecretKind::Password, None)
            .is_err());
        let (_, n) = change_storage(
            &c,
            &data,
            StorageMode::System,
            Some("m".into()),
            Some(&app),
            &sysdir,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(sys.get(&pb).unwrap().as_deref(), Some("secret-b"));
        assert_eq!(sys.get(&pa).unwrap(), None);
        assert!(c.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn migrate_system_to_portable_failure_keeps_system_secrets() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let sysdir = t.path().join("system");
        std::fs::create_dir_all(&sysdir).unwrap();
        let data = loc(StorageMode::System, &sysdir);
        let (c, sys) = connector();
        let (a, b) = (srv("a"), srv("b"));
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.secret_set(&data, &a.id, SecretKind::Password, Some("secret-a".into()))
            .unwrap();
        c.secret_set(
            &data,
            &b.id,
            SecretKind::Passphrase,
            Some("secret-b".into()),
        )
        .unwrap();
        // A folder where the password file should go: creating the vault fails.
        std::fs::create_dir(sysdir.join(VAULT_FILE)).unwrap();
        let r = change_storage(
            &c,
            &data,
            StorageMode::Portable,
            Some("m".into()),
            Some(&app),
            &sysdir,
        );
        assert!(r.is_err());
        assert!(!app.join(DATA_DIR_NAME).exists());
        let pa = account(&a.id, fxfm_core::secrets::SecretKind::Password);
        let kb = account(&b.id, fxfm_core::secrets::SecretKind::Passphrase);
        assert_eq!(sys.get(&pa).unwrap().as_deref(), Some("secret-a"));
        assert_eq!(sys.get(&kb).unwrap().as_deref(), Some("secret-b"));
        assert!(sysdir.join(SERVERS_FILE).is_file());
        assert!(c.vault.lock().unwrap().is_none());
    }

    #[test]
    fn connect_save_marks_server_without_reverting_others() {
        let t = tempfile::tempdir().unwrap();
        let data = loc(StorageMode::System, t.path());
        let (c, _) = connector();
        let mut a = srv("a");
        a.save_password = false;
        c.save_servers(&data, list_of(&[&a])).unwrap();
        // Meanwhile the list gained a server.
        let b = srv("b");
        c.save_servers(&data, list_of(&[&a, &b])).unwrap();
        c.mark_save_password(&data, &a.id).unwrap();
        let list = c.servers();
        assert_eq!(list.all().len(), 2);
        assert!(list.find_by_name("a").unwrap().save_password);
        let disk: ServerList = storage::load_json(t.path(), SERVERS_FILE);
        assert_eq!(disk, list);
    }
}
