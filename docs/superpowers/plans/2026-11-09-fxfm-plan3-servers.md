# fxfm plan 3: servers (Ctrl+F) – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ctrl+F opens the TC "Connect to server" dialog with a list of servers. Connecting works over SFTP, SCP, FTP and FTPS, with per-server keys, host key verification, and passwords stored in the Keychain or in a vault locked by a master password. A connected server is a "drive" in the drive bar, the panel browses it like a folder, and F3, F4 (upload back), F5, F6, F7, F8, F2 and chmod all work.

**Architecture:**
- New `fxfm-core` module `remote/`:
  - trait `RemoteFs` (synchronous API) with three backends: `sftp.rs` (russh + russh-sftp), `scp.rs` (russh exec: `scp -t/-f`, listing via SFTP or `ls`), `ftp.rs` (suppaftp, FTPS via rustls);
  - `known_hosts.rs`, `transfer.rs` (upload/download of trees, `.part` then replace);
  - connection registry `Sessions` in `vfs::Ctx`.
- **A trait is introduced** (announced in plan 2): `RemoteFs` for servers. `Local` and `Archive` stay as they are in `vfs`, and `vfs` gets a third branch `Loc::Remote`.
- Transfers involving a server go through local staging, as in plan 2:
  - server → local: download into `stage-<n>`, then `ops::copy`/`ops::move_` (same conflicts and progress);
  - local → server: direct upload, asking about each file;
  - server ↔ server and archive ↔ server: via staging.
- New `fxfm-core` modules `servers.rs` (server list, `servers.json`, URL parsing) and `secrets.rs` (trait `SecretStore`, `Vault` with a master password). Keychain is a thin layer in `src-tauri` (`security-framework`).
- Connecting is **stateless** with respect to the UI: `connect` returns either `Connected` or a question (host key, password, passphrase, master password). The UI asks the user and calls `connect` again with the answer.
- russh is async: `remote::rt()` is a single shared tokio runtime (2 worker threads). The synchronous backend methods call `rt().block_on(...)` with a timeout. Callers are always job threads or `spawn_blocking`, never a tokio worker.

**Tech Stack:**
- everything from plans 1 and 2;
- `russh` 0.65 (`default-features = false`, features `ring`, `rsa`, `flate2`, `des`; **not** `aws-lc-rs`);
- `russh-sftp` (the version matching russh 0.65; if 3.x does not compile with 0.65, use the pair given in the russh-sftp README);
- `tokio` 1 (`rt-multi-thread`, `time`, `sync`, `io-util`, `net`);
- `suppaftp` 12 (synchronous, feature `rustls-ring`);
- `argon2` 0.5/0.6 and `chacha20poly1305` (XChaCha20-Poly1305), `rand` for salt and nonce, `base64`, `sha2`;
- `security-framework` 3 in `src-tauri`, only `cfg(target_os = "macos")`;
- dev-dependencies in `fxfm-core`: `libunftp` 0.24 + `unftp-sbe-fs` (FTP test server), `rcgen` (cert for the FTPS test).

Everything is pure Rust or `ring` (cc), so the universal build stays free of Homebrew, OpenSSL and cmake.

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (§2 goal 5, §4 drive bar and path row, §5 F3/F4/F5/F6/F8 and Ctrl+F, Ctrl+Shift+F, §6 remote items in the hotlist, §7 Servers, §8 archives on a server, §9 master password and Keychain, §10 bundled libraries). Plan 2: `docs/superpowers/plans/2026-11-07-fxfm-plan2-archives.md`, whose conventions (locations as `String`, `OpObserver`, staging, editwatch) apply here too.

## Global Constraints

- Everything from Global Constraints of plans 1 and 2 still applies: Tauri isolation, `fxfm-core` is built and tested on Linux, commits only as `zbuzanic@gmail.com` with a Co-Authored-By line, UI texts in English.
- Branch `feat/plan3-servers` is cut from `feat/plan2-archives`.
- A **server location** is `<scheme>://<server name>/<absolute path>`, e.g. `sftp://web-1/var/www/html`. The root is `sftp://web-1/`.
  - The scheme is `sftp`, `scp`, `ftp` or `ftps`. The server is looked up **by name**, and when formatting, the scheme is always taken from the server's protocol.
  - A server name must not contain `/` and must be unique across the whole list (not just within a folder), because locations and the hotlist use it.
  - A quick connection via "New URL…" is named `user@host` or `user@host:port` if the port is not the default. It is not saved to the list.
  - Paths in a location are literal (no percent-encoding). Percent-encoding is decoded only in the URL from "New URL…".
- **Default ports:** SFTP/SCP 22, FTP/FTPS 21.
- **The data folder** (portable or system) gets:
  - `servers.json`: the server tree, **without passwords and passphrases**;
  - `known_hosts`: accepted host keys and FTPS certificates;
  - `keys/`: imported private keys, permissions 0600;
  - `secrets.json`: portable mode only, the encrypted vault.
- **Passwords:**
  - System mode: macOS Keychain, service `fxfm`, account `<server id>/password` or `<server id>/passphrase`.
  - Portable mode: `secrets.json` encrypted with the master password (Argon2id m=64 MiB, t=3, p=1; XChaCha20-Poly1305; new salt on every master password change, new nonce for every entry). Without a master password, passwords are not saved.
  - The master password is requested once per launch, when needed, and is kept in memory only.
  - Entered but unsaved passwords are kept in memory until exit, to allow reconnecting.
- **Tests never write to the real Keychain or to the real data folder** (owner's rule). Keychain code is tested manually only. Network tests use only `127.0.0.1` and servers started inside the test itself.
- **Timeouts:** connecting 15 s; each request or transfer block with no progress for 60 s yields `RemoteError::Timeout`; SSH keepalive every 30 s. An operation never hangs forever.
- **Upload to a server never leaves a truncated original:**
  1. writes to `.<name>.fxfm-<pid>-<n>.part` in the same folder;
  2. sets permissions: if the target exists, copies its permissions; otherwise the permissions of the local source (FTP skips this);
  3. replaces the target (SFTP `posix-rename@openssh.com` if the server offers it, otherwise delete the target then rename; FTP `RNFR`/`RNTO`, and if that fails, `DELE` then `RNTO`).

  Exceptions:
  - If the target is a symlink, or the `.part` cannot be created because of permissions, write directly to the target (truncate + write).
  - On error or Cancel the `.part` is deleted.
- **Deleting on a server** is always permanent. Dialog: `Delete N file(s) from the server? This cannot be undone.`
- **Connection loss:** every operation on a dead connection returns `RemoteError::Disconnected`. The connection is then dropped from the registry, and the UI shows `Connection to <name> lost` with `Reconnect` and `Close` buttons.
- **Build check:** `cargo tree -i openssl-sys` and `cargo tree -i aws-lc-sys` must be empty.

## Review Focus

1. **Connection breaks mid-operation** (VPN turned off, server stops, network hangs without RST):
   - the operation ends with an error within 60 s at the latest, it does not hang;
   - an upload leaves the target untouched, and the `.part` is cleaned up if still possible;
   - the panel shows "Connection lost" and offers Reconnect.

   Tests: Task 4 `upload_interrupted_leaves_target_intact`, Task 6 `stalled_server_times_out`, Task 9 `disconnected_error_drops_session`.
2. **Changed host key**: connecting is aborted **before sending the password or key**, the warning is loud, and the new key is written only after explicit confirmation. Tests in Task 6: `changed_host_key_refuses_before_auth` and `accepting_requires_matching_fingerprint`.
3. **Names with a space, `'`, `$`, `;`, `*` and Unicode** over SCP (shell commands) and FTP: never executed as a shell and never miss the file. Tests: Task 7 `scp_names_with_shell_chars` and Task 8 `ftp_names_with_spaces_and_unicode`.
4. **Passwords on disk**: `servers.json` never contains a password, a wrong master password does not corrupt the vault, and an interrupted vault write leaves the old vault. Tests in Task 2: `servers_json_never_contains_secret`, `vault_wrong_master_password_keeps_file` and `vault_tampered_file_is_rejected`.
5. **Calls from the Tauri async context**: `block_on` inside a tokio context panics. The synchronous API must work from a `spawn_blocking` thread of someone else's runtime. Test in Task 6: `sftp_calls_work_from_spawn_blocking`.

---

## File Structure

```
crates/core/Cargo.toml                  + russh, russh-sftp, tokio, suppaftp, argon2, chacha20poly1305, rand, base64, sha2; dev: libunftp, unftp-sbe-fs, rcgen
crates/core/src/lib.rs                  + pub mod remote; pub mod secrets; pub mod servers;
crates/core/src/servers.rs              ServerNode/ServerEntry/Protocol/Auth, servers.json, validate, find_by_name, parse_url, import_key
crates/core/src/secrets.rs              SecretStore trait, MemoryStore, Vault (secrets.json), migrate_secrets
crates/core/src/remote/mod.rs           RemoteFs, RemoteStat, RemoteError, RemoteLoc, parse/format, Sessions, rt(), connect()
crates/core/src/remote/known_hosts.rs   KnownHosts: check/trust, fingerprint format
crates/core/src/remote/transfer.rs      download_tree, upload_tree (.part + replace), remove_tree, walk_size
crates/core/src/remote/sftp.rs          SftpFs (russh + russh-sftp), ssh_connect (shared with SCP)
crates/core/src/remote/scp.rs           ScpFs (exec: scp -t/-f, ls, mkdir, rm, mv, chmod; shell_quote)
crates/core/src/remote/ftp.rs           FtpFs (suppaftp, FTPS rustls with a TOFU verifier)
crates/core/src/remote/testutil.rs      #[cfg(test)] MemFs (in-memory RemoteFs with error injection), SshTestServer, FtpTestServer
crates/core/src/vfs/mod.rs              Loc::Remote, routing list/nearest/dir_size/mkdir/rename/delete/materialize/entry_stamp/check_upload_target
crates/core/src/vfs/transfer.rs         transfer matrix with a remote source/destination
src-tauri/Cargo.toml                    + security-framework (macos)
src-tauri/src/keychain.rs               KeychainStore: SecretStore over security-framework
src-tauri/src/remote.rs                 Connector: servers, secrets, known_hosts, session password cache; connect/disconnect/… commands
src-tauri/src/state.rs                  + remote: Connector
src-tauri/src/commands.rs               edit_file/open_viewer for a server, set_storage_mode with passwords, chmod
crates/core/src/keymap.rs               cm_connect/cm_disconnect available; + cm_chmod (Files, no default key)
ui/lib.mjs                              parseRemote, remoteRoot, parentLocation for a server, serverDefaults, validateServer, connectionLabel
ui/servers.js                           Ctrl+F dialog, Edit dialog, New URL, Encrypt, connect flow (host key, passwords)
ui/app.js / panel.js / hotlist.js       ☁️ in the drive bar, path row, Ctrl+Shift+F, Connection lost, remote hotlist, chmod
README.md, docs/ROADMAP.md              "Serveri" section; updated roadmap
```

## Shared types (new)

```rust
// remote/mod.rs
pub struct RemoteLoc { pub scheme: String, pub server: String, pub path: String /* absolute, "/" = root */ }
pub fn parse_remote(loc: &str) -> Option<RemoteLoc>;   // only sftp|scp|ftp|ftps; "sftp://m" == "sftp://m/"
pub fn format_remote(scheme: &str, server: &str, path: &str) -> String;

pub struct RemoteStat { pub is_dir: bool, pub is_symlink: bool, pub size: u64, pub mtime: i64, pub mode: u32 }

#[derive(Error, Debug, Clone, PartialEq)]
pub enum RemoteError {
    #[error("not found: {0}")] NotFound(String),
    #[error("permission denied: {0}")] PermissionDenied(String),
    #[error("already exists: {0}")] Exists(String),
    #[error("{0}")] Unsupported(String),
    #[error("connection lost")] Disconnected,
    #[error("server not responding")] Timeout,
    #[error("operation cancelled")] Cancelled,
    #[error("{0}")] Other(String),
}

/// Synchronous API. `progress(bytes_since_last)` returns false for Cancel.
pub trait RemoteFs: Send + Sync {
    fn display(&self) -> String;                          // "sftp://user@host" (":port" if not the default)
    fn home(&self) -> Result<String, RemoteError>;
    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError>;       // without "." and ".."; hidden = starts with '.'
    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError>; // lstat; None = does not exist
    fn read_link(&self, path: &str) -> Result<String, RemoteError>;       // FTP: Unsupported
    fn read_to(&self, path: &str, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), RemoteError>;
    fn write_from(&self, path: &str, input: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), RemoteError>; // create/truncate
    fn mkdir(&self, path: &str) -> Result<(), RemoteError>;
    fn remove_file(&self, path: &str) -> Result<(), RemoteError>;        // also deletes a symlink (not its target)
    fn remove_dir(&self, path: &str) -> Result<(), RemoteError>;         // empty folder
    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError>;   // Exists if `to` exists
    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError>;  // rename over an existing target (rules from Global Constraints)
    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError>; // FTP: Unsupported
    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError>;   // FTP: Unsupported
    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError>;           // connection for a long job (FTP: new login; SSH: Arc clone of itself)
    fn close(&self);
}
```

`RemoteError` → `ListError`: `NotFound` → `NotFound`, `PermissionDenied` → `PermissionDenied`, anything else → `Io("<loc>: <message>")`. → `OpError`: `Cancelled` → `Cancelled`, anything else → `Io(message)`. The UI recognizes messages with the prefix `connection lost` and `server not responding` (Task 10).

---

### Task 1: Server list, URL and keys

**Files:** Create `crates/core/src/servers.rs`; Modify `crates/core/src/lib.rs`, `crates/core/Cargo.toml` (`rand`)

**Interfaces:**
- Consumes: `storage::{load_json, save_json}`.
- Produces:
  ```rust
  pub const SERVERS_FILE: &str = "servers.json";
  pub enum Protocol { Sftp, Scp, Ftp, Ftps }                 // serde lowercase; .scheme() -> &str; .default_port() -> u16
  pub enum Auth { Password, Key, KeyPassphrase }            // serde camelCase
  pub enum FtpEncoding { Utf8, Windows1250, Latin1 }        // serde camelCase, default Utf8
  pub struct ServerEntry {                                  // serde camelCase, everything optional has #[serde(default)]
      pub id: String,            // 16 hex characters, stable (binds the passwords)
      pub name: String, pub protocol: Protocol, pub host: String, pub port: u16, pub user: String,
      pub auth: Auth, pub key_path: Option<String>,   // absolute or "keys/<name>" (relative to the data dir)
      pub save_password: bool, pub remote_dir: Option<String>, pub local_dir: Option<String>,
      pub legacy_algorithms: bool, pub ftp_passive: bool /* default true */, pub ftp_encoding: FtpEncoding,
  }
  pub enum ServerNode { Folder { name: String, children: Vec<ServerNode> }, Server(ServerEntry) } // serde tag "kind"
  pub struct ServerList { pub nodes: Vec<ServerNode> }      // Default, Serialize, Deserialize
  impl ServerList {
      pub fn find_by_name(&self, name: &str) -> Option<&ServerEntry>;
      pub fn find_by_id(&self, id: &str) -> Option<&ServerEntry>;
      pub fn all(&self) -> Vec<&ServerEntry>;
      pub fn validate(&self) -> Result<(), String>;        // first error, text for the UI
  }
  pub fn new_id() -> String;
  pub fn parse_url(url: &str) -> Result<ServerEntry, String>; // name = user@host[:port], remote_dir = path
  pub fn key_file(entry: &ServerEntry, data_dir: &Path) -> Option<PathBuf>;
  pub fn import_key(src: &Path, data_dir: &Path) -> io::Result<String>;   // returns "keys/<name>"
  ```

- [ ] **Step 1: tests** in `servers.rs`:
  - `servers_roundtrip_json`: a tree with a folder and two servers is returned equal after `save_json`/`load_json`. Missing fields get default values (`ftp_passive == true`, `ftp_encoding == Utf8`).
  - `validate_rejects_bad_entries`. Messages:
    - empty name → `Name is required`;
    - name with `/` → `Name cannot contain '/'`;
    - same name in two different folders → `Another server is already named 'x'`;
    - empty host → `Host is required`;
    - `Auth::Key` without `key_path` → `Choose a key file`.
  - `parse_url_variants`:
    - `sftp://zb@web-1:2222/var/www` → protocol Sftp, user `zb`, host `web-1`, port 2222, `remote_dir == Some("/var/www")`, name `zb@web-1:2222`;
    - `ftp://h` → port 21, user `anonymous`, name `anonymous@h`;
    - `ftps://u@h/a%20b` → `remote_dir == Some("/a b")`;
    - `http://h` → `Err("Unsupported protocol 'http'")`.
  - `servers_json_never_contains_secret`: the serialized `ServerList` has no `password` or `passphrase` field (checked against the JSON text). The structs **have no** such field, and the test enforces that.
  - `import_key_copies_with_0600`:
    - `import_key` copies to `<data>/keys/id_ed25519` with permissions 0600 and returns `keys/id_ed25519`;
    - a second import of the same name gives `keys/id_ed25519-2`;
    - `key_file` for `keys/...` returns `<data>/keys/...`, and for an absolute path returns the path itself.
- [ ] **Step 2:** Run `cargo test -p fxfm-core servers`. Expected: FAIL.
- [ ] **Step 3:** Implement. For `id` use `rand` (8 bytes, hex).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): server list, URL parsing, key import`.

### Task 2: Password vault and SecretStore

**Files:** Create `crates/core/src/secrets.rs`; Modify `crates/core/src/lib.rs`, `crates/core/Cargo.toml` (`argon2`, `chacha20poly1305`, `base64`)

**Interfaces:**
- Produces:
  ```rust
  pub trait SecretStore: Send + Sync {
      fn get(&self, account: &str) -> Result<Option<String>, String>;
      fn set(&self, account: &str, secret: &str) -> Result<(), String>;
      fn delete(&self, account: &str) -> Result<(), String>;   // nonexistent = Ok
  }
  pub fn account(server_id: &str, kind: SecretKind) -> String;  // "<id>/password" | "<id>/passphrase"
  pub enum SecretKind { Password, Passphrase }
  pub struct MemoryStore;                                       // HashMap behind a Mutex; for tests and "unsaved" passwords
  pub const VAULT_FILE: &str = "secrets.json";
  pub struct Vault;                                             // unlocked vault (key in memory)
  impl Vault {
      pub fn exists(data_dir: &Path) -> bool;
      pub fn create(data_dir: &Path, master: &str) -> io::Result<Vault>;          // error if it already exists
      pub fn unlock(data_dir: &Path, master: &str) -> Result<Vault, VaultError>;
      pub fn change_master(&mut self, new_master: &str) -> io::Result<()>;        // new salt, everything re-encrypted
  }
  impl SecretStore for Vault { … }                              // every set/delete atomically writes the whole file
  pub enum VaultError { WrongPassword, Corrupt(String), Io(String) }  // Display: "Wrong master password", "Password file is damaged: …"
  pub fn migrate_secrets(from: &dyn SecretStore, to: &dyn SecretStore, accounts: &[String]) -> Result<usize, String>; // returns the number copied
  ```
- Format of `secrets.json`: `{"version":1,"kdf":{"alg":"argon2id","m":65536,"t":3,"p":1,"salt":"<b64>"},"check":"<b64 nonce|ct od 'fxfm'>","items":{"<account>":"<b64 nonce|ct>"}}`. Account names are visible (not secret), and the values are encrypted. The AAD of each entry is the account name, so entries cannot be swapped.

- [ ] **Step 1: tests**:
  - `vault_roundtrip`: `create` → `set("a/password","x")` → a new `unlock` with the same password → `get == Some("x")`; `delete` → `None`.
  - `vault_wrong_master_password_keeps_file`: `unlock` with the wrong password gives `VaultError::WrongPassword`, and the file is byte-identical.
  - `vault_tampered_file_is_rejected`: a changed byte in an entry's ciphertext → `get` returns `Err` with `damaged`; swapped values of two accounts → `Err`.
  - `vault_change_master`: after `change_master("n")` the old password gives `WrongPassword`, the new one works, and the salt is different.
  - `vault_plaintext_not_in_file`: the text of `secrets.json` does not contain the password `hunter2-unique`.
  - `migrate_secrets_copies_existing_only`: from a `MemoryStore` with two of three accounts into another `MemoryStore` → returns 2.
  - Tests use `Params` with a small `m` (e.g. 8 MiB), via a `#[cfg(test)]` constant, so they do not take long.
- [ ] **Step 2:** Run `cargo test -p fxfm-core secrets`. Expected: FAIL.
- [ ] **Step 3:** Implement. Writing goes through `storage::save_json` (atomic).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): encrypted password vault and SecretStore`.

### Task 3: Remote types, known_hosts and vfs routing (no transfers)

**Files:** Create `crates/core/src/remote/{mod.rs,known_hosts.rs,testutil.rs}`; Modify `crates/core/src/vfs/mod.rs`, `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: `listing::{Entry, ListError, sort_entries}`, `ops::{OpError, OpObserver, OpSummary, valid_name}`.
- Produces:
  - the types from "Shared types (new)";
  - `pub struct Sessions` with methods `insert(name: &str, fs: Arc<dyn RemoteFs>)`, `get(name) -> Option<Arc<dyn RemoteFs>>`, `remove(name) -> Option<Arc<dyn RemoteFs>>`, `names() -> Vec<String>`; `vfs::Ctx` gets `pub sessions: Sessions`;
  - `vfs::Loc::Remote(RemoteLoc)`; `vfs::parse` checks `parse_remote` first;
  - existing `vfs` functions get a server branch:
    - `list`;
    - `nearest`: the nearest existing parent folder via `stat`; root `/`; a nonexistent connection returns the location unchanged;
    - `dir_size` (recursive, honors `cancel`);
    - `mkdir` and `rename` (with `valid_name`);
    - `delete` (recursive, permanent, `obs.error` per item, `to_trash` is ignored);
    - `local_dir_for` (server → the user's `home`; the signature gets `home: &Path`, and all callers are updated);
  - `fn session_for(ctx: &Ctx, r: &RemoteLoc) -> Result<Arc<dyn RemoteFs>, RemoteError>`: no connection → `Disconnected`. On `Disconnected` from any operation the connection is **dropped** from `sessions`.
  - `remote::rt() -> &'static tokio::runtime::Runtime` (OnceLock, multi-thread, 2 workers, `enable_all`);
  - `pub fn with_timeout<T>(fut: impl Future<Output = Result<T, RemoteError>>, secs: u64) -> Result<T, RemoteError>` (`rt().block_on(tokio::time::timeout(...))`, expired → `Timeout`);
  - `known_hosts.rs`:
    ```rust
    pub const KNOWN_HOSTS_FILE: &str = "known_hosts";
    pub enum HostCheck { Known, Unknown, Changed { old_fingerprint: String } }
    pub struct KnownHosts { /* file path */ }
    impl KnownHosts {
        pub fn new(path: PathBuf) -> KnownHosts;
        pub fn check(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> HostCheck;
        pub fn trust(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> io::Result<()>; // replaces the old entry
    }
    pub fn fingerprint_sha256(blob: &[u8]) -> String;   // "SHA256:" + base64 without padding, as `ssh-keygen -l`
    ```
    A line has the format `<host>:<port> <key_type> <fingerprint>`. Empty lines and `#` comments are preserved. An FTPS certificate uses `key_type` `x509`.
  - `remote::testutil::MemFs`: an in-memory `RemoteFs` (a tree of path → file/folder/symlink with mode and mtime), with `fail_after_bytes(n)`, which returns `Disconnected` after n bytes of writing (for Review Focus 1).

- [ ] **Step 1: tests**:
  - `parse_remote_variants`:
    - `sftp://m/var/www` → (`sftp`, `m`, `/var/www`);
    - `sftp://m` → path `/`;
    - `ftp://a b/x` → server `a b`;
    - `http://x/y` and `/Users/x` → `None`;
    - `format_remote("sftp","m","/")` → `sftp://m/`.
  - `known_hosts_check_and_trust`:
    - empty → `Unknown`;
    - after `trust` → `Known`;
    - a different fingerprint → `Changed { old_fingerprint }`;
    - `trust` of a new one replaces the line and keeps the comment;
    - the same host on a different port is `Unknown`.
  - `fingerprint_matches_ssh_keygen`: a known ed25519 public key (blob from a fixed test `.pub`, a constant in the test) gives the expected `SHA256:...` (value computed with `ssh-keygen -lf` and written into the test).
  - `vfs_routes_remote`: `Ctx` with a `MemFs` under the name `m`:
    - `list("sftp://m/d")` gives sorted entries, `.hidden` has `hidden == true`;
    - `mkdir("sftp://m/d","n")` returns `sftp://m/d/n`;
    - `rename` returns the new location;
    - `delete(["sftp://m/d"])` deletes recursively and `done` counts the files;
    - `nearest("sftp://m/d/nema/x")` → `sftp://m/d`;
    - `dir_size` gives (bytes, file count).
  - `vfs_remote_without_session`: `list("sftp://nema/")` → `ListError::Io` with `connection lost`; `nearest` returns the input unchanged.
  - `disconnected_drops_session`: a `MemFs` that returns `Disconnected` on `list` → after the call `ctx.sessions.get("m")` is `None`.
  - `remote_calls_from_inside_tokio_spawn_blocking`: inside a `#[tokio::test]` with its own runtime, `spawn_blocking(|| with_timeout(async { Ok(1) }, 1))` returns `Ok(1)` without panicking.
- [ ] **Step 2:** Run `cargo test -p fxfm-core remote vfs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS. Existing vfs and archive tests stay green.
- [ ] **Step 5:** Commit `feat(core): remote locations, known_hosts, vfs routing for servers`.

### Task 4: Transfers from and to a server

**Files:** Create `crates/core/src/remote/transfer.rs`; Modify `crates/core/src/vfs/transfer.rs`, `crates/core/src/vfs/mod.rs`

**Interfaces:**
- Consumes: Task 3, `ops::{copy, move_, resolve_dest}`, staging from plan 2 (`Ctx::new_temp_dir("stage")`, `cleanup_dir`), the existing `archive_to_local`/`local_to_archive`.
- Produces:
  - `remote::transfer::download_tree(fs: &dyn RemoteFs, items: &[String], to_dir: &Path, obs: &dyn OpObserver, progress: &mut ProgressState) -> Result<(), OpError>`:
    - creates folders locally;
    - creates symlinks as symlinks (`read_link`; if `Unsupported`, downloads the target);
    - sets mtime and permissions;
    - per-item errors go through `obs.error` (Retry/Skip/SkipAll/Cancel).
  - `remote::transfer::upload_tree(fs: &dyn RemoteFs, sources: &[PathBuf], to_dir: &str, obs: &dyn OpObserver, progress: &mut ProgressState) -> Result<OpSummary, OpError>`:
    - for each file, `stat` the target; if it exists, `obs.conflict` (all `ConflictAnswer`s, including `Rename` and `OverwriteAllOlder` by mtime);
    - an existing folder is merged without asking (as in `ops`);
    - a folder where a file is expected or vice versa gives `obs.error` with the `ops::type_clash` text;
    - writing follows the `.part` rule from Global Constraints.
  - `remote::transfer::remove_tree(fs, path, obs) -> Result<u64, OpError>` (number of deleted files; also used by `vfs::delete`).
  - `ProgressState`: accumulates `Progress` and calls `obs.progress` at most every 100 ms.
  - `vfs::copy`/`vfs::move_` (in `transfer.rs`) for each combination with a remote side:

    | source → destination | how |
    |---|---|
    | server → local | `download_tree` into `stage-<n>`, then `ops::copy` or `ops::move_` stage → destination (conflicts from `ops`) |
    | local → server | `upload_tree` directly |
    | archive → server | extract to stage (existing code), then `upload_tree` |
    | server → archive | `download_tree` to stage, then the existing `local_to_archive` |
    | server → server, different connections | stage, then `upload_tree` (the job uses `fork()` for both sides) |
    | same server, move | `fs.rename` per item (no transfer); `Exists` → `obs.conflict` (Overwrite = `remove_tree` of the target + `rename`) |
    | same server, copy | stage, then `upload_tree` |

    On **move** the source is deleted only after the destination has been written successfully. Skipped items are not deleted.
  - Total size for progress: `walk_size` on the server before starting. It is not counted twice when the transfer goes through stage (the stage → destination part is not shown as a new job).

- [ ] **Step 1: tests** (with `MemFs`):
  - `upload_new_tree`: local folder `d/{a.txt, sub/b.txt}` → `sftp://m/up`, content and permissions match, and no `.part` remains anywhere.
  - `upload_conflict_answers`: `Skip` leaves the target, `Overwrite` replaces it, and `Rename { new_name: "a(2).txt" }` creates a new file.
  - `upload_replace_keeps_target_mode`: target with permissions 0755 and local source 0644 → after overwriting the target has 0755.
  - `upload_interrupted_leaves_target_intact`: `fail_after_bytes(10)` while overwriting a 1 MB file → `OpError::Io` with `connection lost`, and the target is byte-identical to the old one.
  - `upload_over_symlink_writes_through`: the target is a symlink to `real.txt` → after the upload the link is still a link, and `real.txt` has the new content.
  - `upload_without_dir_write_permission_writes_directly`: a `MemFs` folder marked "no create" and an existing target → the content is written directly.
  - `download_to_local_with_conflicts`: server → local through `ops` conflicts (`SkipAll` skips all existing); the stage folder does not exist afterwards.
  - `move_remote_to_local_deletes_source_after_success`: source deleted; on `Skip` of one item, that item stays on the server.
  - `move_within_same_server_renames`: `move_(["sftp://m/a/x"], "sftp://m/b/")` does not call `read_to` (MemFs counts calls) and `x` is in `b`.
  - `cancel_during_upload_removes_part`: `obs.is_cancelled()` after the first file → `summary.cancelled`, no `.part`.
  - `archive_to_remote_and_back`: `a.zip/d` → `sftp://m/x` → `<t>/out`, content equal to the source.
- [ ] **Step 2:** Run `cargo test -p fxfm-core transfer`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy and move to, from and between servers`.

### Task 5: materialize, entry_stamp and upload back for a server

**Files:** Modify `crates/core/src/vfs/mod.rs`

**Interfaces:**
- Consumes: Task 3–4.
- Produces (extensions of existing signatures, no change for local and archive):
  - `vfs::materialize(ctx, "sftp://m/x/f.php", prefix)` downloads to `<prefix>-<n>/f.php`; a folder gives `Only files can be opened from a server`;
  - `vfs::entry_stamp(ctx, loc)` for a server returns `Some((size, mtime))` from `stat`, and `None` if the file does not exist;
  - `vfs::check_upload_target(loc)` for a server: `Ok` if the connection is in `sessions`, otherwise `Err("Not connected to <name>; your edited copy is kept")`. The signature gets `ctx: &Ctx` and callers are updated (`editwatch.rs`, `jobs.rs`);
  - `JobRequest::Upload { local, target }` for a server goes through `upload_tree` with `NullObserver` (overwrite), i.e. the same `.part` rule.

- [ ] **Step 1: tests**:
  - `materialize_remote_file`: the content of the downloaded copy equals the MemFs file.
  - `entry_stamp_remote_changes_after_write`: the stamp before and after a change on the server differs.
  - `check_upload_target_needs_session`: with a connection `Ok`, without one `Err` containing `your edited copy is kept`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS (including the `src-tauri` editwatch tests).
- [ ] **Step 5:** Commit `feat(core): F3/F4 materialize and upload back for server files`.

### Task 6: SFTP backend and connecting over SSH

**Files:** Create `crates/core/src/remote/sftp.rs`; Modify `crates/core/src/remote/{mod.rs,testutil.rs}`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: Task 1 (`ServerEntry`, `key_file`), Task 3 (`KnownHosts`, `with_timeout`, `rt`).
- Produces:
  ```rust
  pub struct Credentials { pub password: Option<String>, pub passphrase: Option<String> }
  #[derive(Debug, Clone, PartialEq)]
  pub enum ConnectError {
      HostKey { host: String, port: u16, key_type: String, fingerprint: String, changed_from: Option<String> },
      NeedPassword, NeedPassphrase,          // missing; the UI asks
      AuthFailed,                            // sent, rejected ("Wrong user name, password or key")
      Timeout,                               // "Server did not answer within 15 seconds"
      Failed(String),                        // DNS, refused connection, unsupported algorithms (the message says to enable the legacy option)
  }
  pub struct ConnectOptions<'a> { pub data_dir: &'a Path, pub known: &'a KnownHosts, pub accept_fingerprint: Option<&'a str> }
  pub fn connect(server: &ServerEntry, creds: &Credentials, opts: &ConnectOptions) -> Result<Arc<dyn RemoteFs>, ConnectError>; // in remote/mod.rs, picks the backend by protocol
  pub(crate) fn ssh_connect(server, creds, opts) -> Result<SshSession, ConnectError>;  // shared by SFTP and SCP
  ```
- The host key is checked in the russh handler's `check_server_key`:
  - `Known` → continue;
  - `Unknown` or `Changed` → if `accept_fingerprint == Some(fp)` and equal to the one presented, `trust` and continue; otherwise abort the connection **before authentication** and return `HostKey`.
- Authentication:
  - `Password`;
  - `Key`: a key without a passphrase; if it is encrypted and no passphrase was given → `NeedPassphrase`;
  - `KeyPassphrase`.

  Loading via `russh::keys::decode_secret_key` supports OpenSSH (RSA, ECDSA, Ed25519) and PEM (PKCS#1, PKCS#8, encrypted). For RSA, `best_supported_rsa_hash` is used.
- `legacy_algorithms`: `Preferred` adds `ssh-rsa` (SHA-1) for the host key and user key, and `diffie-hellman-group14-sha1` and `diffie-hellman-group1-sha1` for kex.
- `SftpFs` has `display()` `sftp://user@host[:port]` and `home()` = `canonicalize(".")`. `replace` uses `posix-rename@openssh.com` if the server offers it. `fork()` returns a clone of the same connection (SFTP multiplexes).
- `testutil::SshTestServer`: a russh server on `127.0.0.1:0` inside the test, with a generated ed25519 host key. It accepts the password `pw` or the user's given public key.
  - The `sftp` subsystem starts the system `sftp-server` (`/usr/libexec/sftp-server`, `/usr/lib/openssh/sftp-server`, `/usr/lib/ssh/sftp-server`) and redirects stdio.
  - `exec` runs `sh -c <cmd>` (for SCP, Task 7).
  - The `stall()` option stops responding without closing the connection.
  - If `sftp-server` is not found, the tests print `skipped: no sftp-server` and pass.

- [ ] **Step 1: tests** (`#[test]`, against `SshTestServer`):
  - `sftp_password_login_and_list`: the listing of a server temp folder matches `fs::read_dir` (names, `is_dir`, sizes).
  - `sftp_key_login_ed25519_and_encrypted_pem`: an OpenSSH ed25519 key works; an encrypted PKCS#8 without a passphrase gives `NeedPassphrase`, and works with the correct passphrase.
  - `unknown_host_key_asks_then_trusts`: first `HostKey { changed_from: None }`, then with `accept_fingerprint` = that fingerprint it connects and writes to `known_hosts`; the third time it connects without asking.
  - `changed_host_key_refuses_before_auth`: `known_hosts` has a different fingerprint → `HostKey { changed_from: Some(old) }`, and the server **received no** auth request (the counter in the test server is 0).
  - `accepting_requires_matching_fingerprint`: `accept_fingerprint` of a different key → `HostKey` again, `known_hosts` unchanged.
  - `wrong_password_is_auth_failed`, `missing_password_is_need_password`.
  - `sftp_file_ops_roundtrip`: `write_from`, `read_to`, `mkdir`, `rename` (`Exists` if the target exists), `replace`, `set_mode`, `symlink`/`read_link`, `remove_file` and `remove_dir` work against a real `sftp-server`.
  - `stalled_server_times_out`: after `stall()` a `list` call returns `Timeout` within ≤ 65 s. The test uses a `#[cfg(test)]` timeout of 2 s via the `with_timeout` parameter, so it takes ~2 s.
  - `sftp_calls_work_from_spawn_blocking`: `list` called from `tokio::task::spawn_blocking` inside another runtime returns `Ok`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core sftp`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings && cargo tree -p fxfm-core -i aws-lc-sys; cargo tree -p fxfm-core -i openssl-sys`. Expected: tests PASS, and both `cargo tree` commands report that the package was not found.
- [ ] **Step 5:** Commit `feat(core): SFTP backend with host key check and key auth`.

### Task 7: SCP backend

**Files:** Create `crates/core/src/remote/scp.rs`; Modify `crates/core/src/remote/mod.rs`

**Interfaces:**
- Consumes: `ssh_connect` from Task 6.
- Produces:
  - `ScpFs: RemoteFs`. On connecting it tries to open the SFTP subsystem:
    - if that succeeds, `list`, `stat`, `read_link` and `home` go over SFTP;
    - otherwise `list` runs `LC_ALL=C ls -la --time-style=+%s -- <path>` (GNU), and if that fails, `LC_ALL=C ls -la -T -- <path>` (BSD);
    - `stat` is `ls -lad`; `home` is `pwd`.
  - Transfers always go through `scp -t` and `scp -f` (SCP protocol: `C<mode> <size> <name>\n`, acknowledgements `\0`, errors `\1`/`\2`).
  - `mkdir`, `remove_file`, `remove_dir`, `rename` (`mv -n`, then a check), `replace` (`mv -f`), `symlink` (`ln -s`) and `set_mode` (`chmod`) are exec commands. An exit code ≠ 0 gives a `RemoteError` based on stderr: `No such file` → `NotFound`, `Permission denied` → `PermissionDenied`, anything else `Other(stderr)`.
  - `pub(crate) fn shell_quote(s: &str) -> String`: POSIX single-quote (`'` → `'\''`). Every argument in every command goes through `shell_quote`, and paths come after `--`.
  - `pub(crate) fn parse_ls(line: &str, gnu: bool) -> Option<(String, RemoteStat, Option<String>)>` (name, stat, symlink target).

- [ ] **Step 1: tests**:
  - `shell_quote_cases`: `a b` → `'a b'`, `it's` → `'it'\''s'`, `$(rm -rf ~)` stays literal inside the quotes.
  - `parse_ls_gnu_and_bsd`: fixed lines (file, folder, symlink `a -> b`, a name with spaces, a file name containing `->` is not a symlink if the first character is not `l`) give the expected values.
  - `scp_roundtrip_against_test_server` (SshTestServer; exec runs the system `scp`): upload and download of a 1 MB file give the same content; `mkdir`, `rename`, `remove_*` and `set_mode` work.
  - `scp_names_with_shell_chars`: files `a b.txt`, `it's.php`, `$x;y*.sh` and `čćž.txt` survive upload, list, rename, download and delete. No other file in the folder disappears, and no file `y` was created.
  - `scp_list_without_sftp_uses_ls`: a test server with the SFTP subsystem disabled gives the same listing as with SFTP (names, `is_dir`, size).
- [ ] **Step 2:** Run `cargo test -p fxfm-core scp`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): SCP backend with ls fallback`.

### Task 8: FTP and FTPS backend

**Files:** Create `crates/core/src/remote/ftp.rs`; Modify `crates/core/src/remote/{mod.rs,testutil.rs}`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: Task 1, Task 3 (`KnownHosts`).
- Produces:
  - `FtpFs: RemoteFs`. A single control connection behind a `Mutex`. `fork()` opens a new login with the same credentials, so long jobs do not block panel listing.
  - Listing: `MLSD` if the server advertises it in `FEAT`, otherwise `LIST -a` parsed by suppaftp's `list::File` (Unix and DOS format).
  - Modes: passive (default) or active (`ftp_passive == false`).
  - FTPS is explicit TLS (`AUTH TLS`) with rustls. The certificate is verified with **TOFU** via `known_hosts` (`key_type` `x509`, SHA-256 of the DER), the same way as an SSH host key (`ConnectError::HostKey`). Passing the webpki check is not a requirement, because LAN servers have self-signed certificates.
  - `ftp_encoding`:
    - Utf8 → `OPTS UTF8 ON` if the server supports it;
    - Windows1250 or Latin1 → names are decoded and encoded via `encoding_rs`.

    **If suppaftp does not expose the raw bytes of the name** (check in the 12.x source), support only Utf8, hide the field in the UI (Task 11) and add an item to `docs/ROADMAP.md`. A name that cannot be decoded must never be silently altered.
  - `set_mode` → `SITE CHMOD`; if the server rejects it, `Unsupported("This server does not support changing permissions")`. `symlink` and `read_link` → `Unsupported`.
  - `replace` → `RNFR`/`RNTO`, and if that fails, `DELE` of the target then `RNTO`.
  - `testutil::FtpTestServer`: libunftp with `unftp-sbe-fs` over a temp folder on `127.0.0.1:0`, user `u`/`pw`, optionally FTPS with an rcgen self-signed certificate.

- [ ] **Step 1: tests**:
  - `ftp_login_list_roundtrip`: list, `write_from`/`read_to` of 1 MB, `mkdir`, `rename`, `replace`, `remove_file` and `remove_dir`.
  - `ftp_wrong_password_is_auth_failed`.
  - `ftps_unknown_cert_asks_then_trusts`: first `HostKey` with `key_type == "x509"`, then with the accepted fingerprint it connects; a changed cert gives `changed_from: Some(_)`.
  - `ftp_names_with_spaces_and_unicode`: `a b.txt` and `čćž.txt` survive upload, list, rename, download and delete.
  - `ftp_fork_is_independent`: while one `fork()` reads a 5 MB file in another thread, `list` on the original returns a result before the read finishes.
  - `ftp_active_mode_lists`: `ftp_passive = false` gives the same listing.
- [ ] **Step 2:** Run `cargo test -p fxfm-core ftp`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings && cargo tree -p fxfm-core -i openssl-sys`. Expected: PASS, and for `openssl-sys` the package is not found.
- [ ] **Step 5:** Commit `feat(core): FTP and FTPS backend`.

### Task 9: Tauri layer – connections, passwords, Keychain, F3/F4 and migration

**Files:** Create `src-tauri/src/remote.rs`, `src-tauri/src/keychain.rs`; Modify `src-tauri/src/{state.rs,commands.rs,jobs.rs,main.rs}`, `src-tauri/Cargo.toml`

**Interfaces:**
- Consumes: Task 1–8; the existing `blocking`, `Jobs`, `EditWatch`, `storage::migrate`.
- Produces:
  - `KeychainStore: SecretStore` (`security_framework::passwords::{get_generic_password, set_generic_password, delete_generic_password}`, service `fxfm`), only `cfg(target_os = "macos")`. On other systems `set` returns `Err("No system keychain on this platform")`.
  - `pub struct Connector` in `AppState.remote`. It holds `servers: Mutex<ServerList>`, `vault: Mutex<Option<Vault>>`, `session_secrets: MemoryStore` (entered, unsaved passwords) and `quick: Mutex<HashMap<String, ServerEntry>>` (quick connections by name).
  - Commands (all `async` + `blocking`, like the existing ones):
    - `servers_load() -> ServerList`;
    - `servers_save(list: ServerList) -> Result<(), String>`: runs `validate`. For deleted servers it deletes their secrets from the current store (Keychain or unlocked vault) and from `session_secrets`;
    - `server_secret_set(id: String, kind: SecretKind, secret: Option<String>) -> Result<(), ConnectReply>`: `None` deletes. Portable without an unlocked vault returns `NeedMaster { create }`;
    - `connect(req: ConnectRequest) -> Result<ConnectReply, String>`;
    - `disconnect(name: String) -> Result<(), String>`: `close()` and removal from `sessions`; emits `connections://changed`. Deletes `view-*` copies from that connection and F4 copies with no unsaved changes (spec §5: "temporary copies are cleaned up when the connection is closed"). Copies with unsaved changes stay, and a later upload reports `Not connected…` (Task 5). Test: `disconnect_cleans_clean_copies_only`;
    - `list_connections() -> Vec<ConnectionInfo>`: `ConnectionInfo { name, display, root: String /* "sftp://name/" */ }`;
    - `master_unlock(password: String) -> Result<(), String>`, `master_set(old: Option<String>, new: String) -> Result<(), String>` (creates the vault or changes the password), `master_status() -> MasterStatus { mode: StorageMode, exists: bool, unlocked: bool }`;
    - `import_key(path: String) -> Result<String, String>`;
    - `chmod(location: String, mode: u32) -> Result<(), String>`: local and archive → `Change attributes works on SFTP and SCP servers`.
    ```rust
    #[serde(rename_all = "camelCase")]
    pub struct ConnectRequest { pub server: Option<String> /* name */, pub url: Option<String>,
        pub password: Option<String>, pub passphrase: Option<String>, pub save: bool, pub accept_fingerprint: Option<String> }
    #[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
    pub enum ConnectReply {
        Connected { location: String, local_dir: Option<String> },
        HostKey { host: String, port: u16, key_type: String, fingerprint: String, changed_from: Option<String> },
        NeedPassword { retry: bool }, NeedPassphrase { retry: bool },
        NeedMaster { create: bool },
    }
    ```
  - The `connect` flow:
    1. finds the server (a name in the list or `parse_url`); if already connected, returns `Connected` immediately;
    2. takes passwords from the request, then `session_secrets`, then the store. In portable mode with `save_password` and a locked vault it returns `NeedMaster { create: !Vault::exists }`;
    3. calls `remote::connect` (`blocking`);
    4. `NeedPassword`/`NeedPassphrase` → reply with `retry: false`; `AuthFailed` → `NeedPassword { retry: true }` (for a key with a passphrase `NeedPassphrase { retry: true }`), and the wrong password is deleted from `session_secrets`;
    5. on success: the entered password goes into `session_secrets`, and with `save` also into the store; the connection is registered in `sessions`; `connections://changed` is emitted; `Connected` is returned with `remote_dir` or `home()`.
  - `Disconnected` from any command or job emits `connections://changed` (the connection has already been dropped in core, Task 3).
  - `edit_file` and `open_viewer` for a server work as for an archive (`materialize`, `EditWatch`). The viewer title is the full location.
  - `set_storage_mode(mode, master: Option<String>)`:
    - Portable → System: needs an unlocked vault (otherwise `Err("master password required")`). After the file migration, all secrets go to the Keychain (`migrate_secrets`).
    - System → Portable: `master` is required and creates a new vault in the new `fxfm-data`. Secrets are copied from the Keychain, and deleted from the Keychain only after the vault is reopened and every value verified.
    - The confirmation message states the number of migrated passwords.
  - Exit: `close()` on all connections; temporary copies with unsaved changes stay (the existing editwatch rule).

- [ ] **Step 1: tests** (`src-tauri`, without Keychain: `Connector` takes an `Arc<dyn SecretStore>` for system mode, and tests supply a `MemoryStore`):
  - `connect_reply_serde`: JSON shapes of all `ConnectReply` variants (`{"kind":"hostKey","host":…,"changedFrom":null}` …).
  - `servers_save_removes_secrets_of_deleted`: a deleted server → its `id/password` disappears from the store.
  - `portable_save_password_needs_master`: portable, no vault → `server_secret_set` returns `NeedMaster { create: true }`; after `master_set` the password is in `secrets.json`.
  - `wrong_password_not_cached`: `AuthFailed` leaves no password in `session_secrets` (logic in the pure function `after_connect(result, entered, store)`, tested without a network).
  - `migrate_portable_to_system_moves_secrets` and `migrate_system_to_portable_requires_master` (MemoryStore instead of Keychain, temp folders instead of real ones).
  - `disconnected_error_drops_session`: a job that gets `Disconnected` → `list_connections` no longer returns it.
- [ ] **Step 2:** Run `cargo test -p fxfm`. Expected: FAIL.
- [ ] **Step 3:** Implement. Register the commands in `main.rs`.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: connections, password storage and server commands`.

### Task 10: UI – a server in the panels

**Files:** Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/app.js`, `ui/panel.js`, `ui/hotlist.js`, `ui/dialogs.js`, `crates/core/src/keymap.rs`

**Interfaces:**
- Consumes: commands from Task 9; the `connections://changed` event.
- Produces (`lib.mjs`, pure functions):
  - `parseRemote(loc) -> {scheme, server, path} | null` (same rules as `parse_remote`);
  - `remoteRoot(loc) -> "sftp://m/"`;
  - `parentLocation` for a server: `sftp://m/var` → `sftp://m/`, and `sftp://m/` → `null` (Backspace at the root does nothing);
  - `joinLocation("sftp://m/", "x")` → `sftp://m/x`;
  - `connectionLost(message) -> boolean` (prefixes `connection lost`, `server not responding`);
  - `deleteConfirmText(location, n)`: on a server `Delete N file(s) from the server? This cannot be undone.`, otherwise the existing texts.
- Behavior:
  - The drive bar gets a button `☁️ <name>` for each connection from `list_connections`. It refreshes on `connections://changed`. It is active when the panel is on that connection.
  - Path row: on a server, instead of free space, `[sftp://user@host]` (`display`).
  - `cm_disconnect` (Ctrl+Shift+F) closes the connection of the active panel. All panels on that connection go to the server's `local_dir` or `~`.
  - An error with `connectionLost` in a panel or job shows the dialog `Connection to <name> lost` with the buttons `Reconnect` (flow from Task 11, then the same path or `nearest`) and `Close` (the panel goes to `~`).
  - Hotlist: `cd sftp://name/path` and a Target path on a server connect (if the connection does not exist) and then open the path. `REMOTE_MSG` is removed.
  - `cm_chmod` (Files menu, title `Change attributes…`, no default key): a dialog with an octal field and 9 rwx checkboxes, kept in sync. For multiple selected items it applies to all. Folders are not processed recursively.
  - `keymap.rs`: `cm_connect` and `cm_disconnect` get `available: true`; `cm_chmod` is added.
  - Session restore stays as it is (server → `~`, `restore_path`).

- [ ] **Step 1: tests** in `ui/lib.test.mjs` for all the new functions in the list, with the stated values, plus the Rust `keymap` test `connect_commands_available`.
- [ ] **Step 2:** Run `node --test ui/ && cargo test -p fxfm-core keymap`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `node --test ui/ && cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): servers as drives, disconnect, connection lost, remote hotlist, chmod`.

### Task 11: UI – Ctrl+F dialog, server editing and connect flow

**Files:** Create `ui/servers.js`; Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/app.js`, `ui/style.css`, `ui/index.html`

**Interfaces:**
- Consumes: commands from Task 9; style and tree as in `ui/hotlist.js`.
- Produces:
  - `lib.mjs`:
    - `serverDefaults(protocol) -> {port, auth}` (sftp/scp → 22/`password`, ftp/ftps → 21/`password`);
    - `validateServer(entry, allNames) -> string|null`, with the messages from Task 1 (the UI checks before saving; the backend checks once more);
    - `fieldsFor(protocol) -> Set<string>`: SSH has `auth`, `keyPath`, `legacyAlgorithms`; FTP has `ftpPassive`, `ftpEncoding`;
    - `connectionLabel(info) -> "☁️ " + name`;
    - `hostKeyText(reply)`.

    `hostKeyText` for a new key:
    ```
    The authenticity of host '<host>:<port>' can't be established.
    <keyType> key fingerprint is <fp>.
    Connect and remember this key?
    ```
    For a changed key:
    ```
    WARNING: THE HOST KEY FOR '<host>:<port>' HAS CHANGED!
    Someone may be intercepting the connection, or the server was reinstalled.
    Old: <old>
    New: <fp>
    Replace the stored key and connect?
    ```
    A changed key has a red title, and the default button is `Cancel`.
  - The `Connect to server` dialog (Ctrl+F):
    - on the left a tree of folders and servers, on the right the buttons `Connect`, `New connection…`, `New URL…`, `Duplicate entry…`, `New folder…`, `Edit…`, `Delete`, `Encrypt…`, `Cancel`, `Help`;
    - Enter and double-click connect, Esc closes, ↑/↓ and ←/→ (collapse and expand a folder) work, and drag & drop and Ctrl+↑/↓ reorder (logic from the hotlist: `moveNode`, `relocateNode`);
    - `Delete` asks for confirmation `Delete '<name>' and its saved password?`;
    - `Duplicate entry…` gives a new `id` and the name `<name> (2)`, but **without** the password (message in the status);
    - `Encrypt…`: in portable mode it sets or changes the master password (two fields, plus the old field if a vault exists); in system mode the message `In system mode passwords are stored in the macOS Keychain.`;
    - `Help` shows a short text about the keys and the URL format.
  - The `Edit connection` dialog:
    - fields: Name, Protocol, Host, Port, User, Authentication (Password / Key file / Key file + passphrase), Key file (`Choose…` and `Import…`), Save password (with a password or passphrase field), Remote dir, Local dir (`Choose…`), Allow legacy algorithms (ssh-rsa/SHA1), Passive mode, Name encoding;
    - only the fields from `fieldsFor` are visible;
    - changing the protocol changes the port only if it was the default for the old protocol;
    - OK saves via `servers_save` and, if a password was entered, `server_secret_set`. An empty field with "Save password" on does not delete the saved password, and turning "Save password" off deletes it.
  - `New URL…`: a single field (`sftp://user@host:port/path`), then the connect flow.
  - The connect flow in `servers.js`, `async function connectFlow(request) -> location|null`, a loop over `connect`:
    - `hostKey` → dialog with `hostKeyText` → repeat with `acceptFingerprint`;
    - `needPassword`/`needPassphrase` → password field (with the `retry` text `Wrong password, try again`) and a `Save password` checkbox (only for a server from the list) → repeat;
    - `needMaster` → master password field (`create` asks for two fields) → `master_unlock` or `master_set` → repeat;
    - `connected` → the active panel goes to `location`, and if `local_dir` is set, the other panel goes there.

    The hotlist and Reconnect from Task 10 use this flow too.

- [ ] **Step 1: tests** in `ui/lib.test.mjs`:
  - `serverDefaults`;
  - `validateServer` (all five messages);
  - `fieldsFor` for all four protocols;
  - `hostKeyText` (new and changed key, exact text);
  - changing the protocol changes the port only when it was the default (pure function `portAfterProtocolChange(oldProto, newProto, port)`).
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implement. `Ctrl+F` is `cm_connect` in the `app.js` dispatch table.
- [ ] **Step 4:** Run `node --test ui/ && cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): Connect to server dialog, connection editor, connect flow`.

### Task 12: Archive on a server, README, build and manual check

**Files:** Modify `src-tauri/src/commands.rs`, `ui/panel.js`, `README.md`, `docs/ROADMAP.md`

**Interfaces:**
- Consumes: Task 5 (`materialize`), `EditWatch` from plan 2.
- Produces:
  - Enter on an archive on a server (`archiveKindOf` of the name): the new command `open_remote_archive(location) -> String` downloads the archive as an F4 copy (`EditWatch::open_for_edit`, tracked) and returns the local path. The panel enters it.
  - A change to the archive (F5 inside, F8, F2…) modifies the tracked copy, so the existing `edit://changed` prompt offers an upload back to the server.
  - `[..]` at the root of such an archive returns to the folder on the server, not to the temp folder: the panel remembers `{tempArchive → remote parent}` in memory.
  - README gets a section `Serveri` (Ctrl+F, protocols, keys and import, host key, where passwords are stored in portable and system mode, Ctrl+Shift+F, Connection lost).
  - ROADMAP: plan 3 is removed from `Next`, and open items are recorded (e.g. FTP encoding if only UTF-8 remained).

- [ ] **Step 1: test** `remote_archive_parent_returns_to_server` in `ui/lib.test.mjs` (pure function `archiveParent(location, remoteArchives)`).
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL; then implementation and PASS.
- [ ] **Step 3:** Run the whole suite: `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 4:** Universal build: `cargo tauri build --target universal-apple-darwin`, then `lipo -archs` on the binary. Expected: `x86_64 arm64`. The new `.dmg` goes to `~/Documents/PVT/fxfm-builds/macos/`, and the old one to Trash.
- [ ] **Step 5:** Manual check (user; with a real server on the LAN). No writing to the user's production folders, only to a test folder on the server.
  1. Ctrl+F → New connection (SFTP, key) → Connect → host key prompt → panel on the server, ☁️ in the bar.
  2. F5 a php file to the server and back, F6, F7, F8, F2; chmod +x on a `.sh`.
  3. F4 on a file on the server → edit in the editor → upload prompt → content on the server changed, permissions preserved.
  4. Turn off the VPN in the middle of a copy → error within 60 s at the latest, Connection lost → Reconnect.
  5. Portable mode: Save password → master password → restart → the password asks only for the master password.
  6. Hotlist `cd sftp://<name>/var/www` connects and opens.
- [ ] **Step 6:** Commit `docs: servers in README, roadmap update`.
