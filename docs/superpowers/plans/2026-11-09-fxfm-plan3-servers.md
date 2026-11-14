# fxfm plan 3: serveri (Ctrl+F) – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ctrl+F otvara TC dijalog "Connect to server" s listom servera. Spajanje radi preko SFTP, SCP, FTP i FTPS, s ključevima po serveru, provjerom host keya i lozinkama spremljenim u Keychain ili u trezor zaključan master lozinkom. Spojeni server je "disk" u traci, panel ga pregledava kao mapu i rade F3, F4 (upload natrag), F5, F6, F7, F8, F2 i chmod.

**Architecture:**
- Novi `fxfm-core` modul `remote/`:
  - trait `RemoteFs` (sinkroni API) s tri backenda: `sftp.rs` (russh + russh-sftp), `scp.rs` (russh exec: `scp -t/-f`, listing preko SFTP-a ili `ls`), `ftp.rs` (suppaftp, FTPS preko rustls);
  - `known_hosts.rs`, `transfer.rs` (upload/download stabla, `.part` pa zamjena);
  - registar veza `Sessions` u `vfs::Ctx`.
- **Uvodi se trait** (najavljeno u planu 2): `RemoteFs` za servere. `Local` i `Archive` ostaju kakvi jesu u `vfs`, a `vfs` dobiva treću granu `Loc::Remote`.
- Prijenosi u kojima sudjeluje server idu kao u planu 2 preko lokalnog staginga:
  - server → lokalno: preuzmi u `stage-<n>`, zatim `ops::copy`/`ops::move_` (isti konflikti i progress);
  - lokalno → server: direktan upload s pitanjem za svaku datoteku;
  - server ↔ server i arhiva ↔ server: preko staginga.
- Novi `fxfm-core` moduli `servers.rs` (lista servera, `servers.json`, parsiranje URL-a) i `secrets.rs` (trait `SecretStore`, `Vault` s master lozinkom). Keychain je tanki sloj u `src-tauri` (`security-framework`).
- Spajanje je **bez stanja** prema UI-ju: `connect` vraća ili `Connected` ili pitanje (host key, lozinka, passphrase, master lozinka). UI pita korisnika i ponovno zove `connect` s odgovorom.
- russh je async: `remote::rt()` je jedan zajednički tokio runtime (2 worker threada). Sinkrone metode backenda zovu `rt().block_on(...)` s vremenskim ograničenjem. Pozivatelji su uvijek job threadovi ili `spawn_blocking`, nikad tokio worker.

**Tech Stack:**
- sve iz plana 1 i 2;
- `russh` 0.65 (`default-features = false`, features `ring`, `rsa`, `flate2`, `des`; **ne** `aws-lc-rs`);
- `russh-sftp` (verzija koja odgovara russh 0.65; ako 3.x ne kompajlira s 0.65, uzmi par koji navodi README od russh-sftp);
- `tokio` 1 (`rt-multi-thread`, `time`, `sync`, `io-util`, `net`);
- `suppaftp` 12 (sinkroni, feature `rustls-ring`);
- `argon2` 0.5/0.6 i `chacha20poly1305` (XChaCha20-Poly1305), `rand` za sol i nonce, `base64`, `sha2`;
- `security-framework` 3 u `src-tauri`, samo `cfg(target_os = "macos")`;
- dev-dependencies u `fxfm-core`: `libunftp` 0.24 + `unftp-sbe-fs` (FTP testni server), `rcgen` (cert za FTPS test).

Sve je čisti Rust ili `ring` (cc), pa universal build ostaje bez Homebrewa, OpenSSL-a i cmakea.

**Spec:** `docs/superpowers/specs/2026-11-06-fxfm-design.md` (§2 cilj 5, §4 traka diskova i red s putanjom, §5 F3/F4/F5/F6/F8 i Ctrl+F, Ctrl+Shift+F, §6 remote stavke u hotlisti, §7 Serveri, §8 arhive na serveru, §9 master lozinka i Keychain, §10 ugrađene biblioteke). Plan 2: `docs/superpowers/plans/2026-11-07-fxfm-plan2-archives.md`, čije konvencije (lokacije kao `String`, `OpObserver`, staging, editwatch) vrijede i ovdje.

## Global Constraints

- Sve iz Global Constraints plana 1 i 2 vrijedi i dalje: Tauri izolacija, `fxfm-core` se builda i testira na Linuxu, commitovi samo kao `zbuzanic@gmail.com` s Co-Authored-By linijom, UI tekstovi na engleskom.
- Grana `feat/plan3-servers` odvaja se od `feat/plan2-archives`.
- **Lokacija na serveru** je `<scheme>://<ime servera>/<apsolutna putanja>`, npr. `sftp://web-1/var/www/html`. Korijen je `sftp://web-1/`.
  - Scheme je `sftp`, `scp`, `ftp` ili `ftps`. Server se traži **po imenu**, a scheme se pri formatiranju uvijek uzima iz protokola servera.
  - Ime servera ne smije sadržavati `/` i mora biti jedinstveno u cijeloj listi (ne samo unutar mape), jer ga koriste lokacije i hotlista.
  - Brza veza preko "New URL…" ima ime `user@host` ili `user@host:port` ako port nije zadani. Ne sprema se u listu.
  - Putanje u lokaciji su doslovne (bez percent-encodinga). Percent-encoding se dekodira samo u URL-u iz "New URL…".
- **Zadani portovi:** SFTP/SCP 22, FTP/FTPS 21.
- **Podatkovna mapa** (portable ili sistemska) dobiva:
  - `servers.json`: stablo servera, **bez lozinki i passphraseova**;
  - `known_hosts`: prihvaćeni host keyevi i FTPS certifikati;
  - `keys/`: importirani privatni ključevi, prava 0600;
  - `secrets.json`: samo u portable načinu, kriptirani trezor.
- **Lozinke:**
  - Sistemski način: macOS Keychain, servis `fxfm`, račun `<server id>/password` ili `<server id>/passphrase`.
  - Portable način: `secrets.json` kriptiran master lozinkom (Argon2id m=64 MiB, t=3, p=1; XChaCha20-Poly1305; nova sol pri svakoj promjeni master lozinke, novi nonce za svaki zapis). Bez master lozinke lozinke se ne spremaju.
  - Master lozinka se traži jednom po pokretanju, kad zatreba, i drži se samo u memoriji.
  - Unesene a nespremljene lozinke drže se u memoriji do izlaza, zbog ponovnog spajanja.
- **Testovi nikad ne pišu u pravi Keychain ni u pravu podatkovnu mapu** (pravilo vlasnika). Keychain kod se testira samo ručno. Mrežni testovi koriste samo `127.0.0.1` i servere pokrenute u samom testu.
- **Vremena:** spajanje 15 s; svaki zahtjev ili blok prijenosa 60 s bez napretka daje `RemoteError::Timeout`; SSH keepalive svakih 30 s. Operacija nikad ne visi zauvijek.
- **Upload na server nikad ne ostavlja skraćen original:**
  1. piše u `.<ime>.fxfm-<pid>-<n>.part` u istoj mapi;
  2. postavlja prava: ako cilj postoji, kopira njegova prava; inače prava lokalnog izvora (FTP preskače);
  3. zamjenjuje cilj (SFTP `posix-rename@openssh.com` ako ga server nudi, inače brisanje cilja pa rename; FTP `RNFR`/`RNTO`, a ako to ne uspije, `DELE` pa `RNTO`).

  Iznimke:
  - Ako je cilj symlink, ili se `.part` ne može stvoriti zbog prava, piše se direktno u cilj (truncate + write).
  - Kod greške ili Cancela `.part` se briše.
- **Brisanje na serveru** je uvijek trajno. Dijalog: `Delete N file(s) from the server? This cannot be undone.`
- **Prekid veze:** svaka operacija na mrtvoj vezi vraća `RemoteError::Disconnected`. Veza se tada izbacuje iz registra, a UI prikazuje `Connection to <ime> lost` s gumbima `Reconnect` i `Close`.
- **Provjera buildova:** `cargo tree -i openssl-sys` i `cargo tree -i aws-lc-sys` moraju biti prazni.

## Review Focus

1. **Veza pukne usred posla** (ugašen VPN, server stane, mreža visi bez RST-a):
   - operacija završi greškom najkasnije za 60 s, ne visi;
   - upload ostavi cilj netaknut, a `.part` se počisti ako se još može;
   - panel prikaže "Connection lost" i ponudi Reconnect.

   Testovi: Task 4 `upload_interrupted_leaves_target_intact`, Task 6 `stalled_server_times_out`, Task 9 `disconnected_error_drops_session`.
2. **Promijenjen host key**: spajanje se prekida **prije slanja lozinke ili ključa**, upozorenje je glasno, a novi ključ se upisuje tek nakon izričite potvrde. Testovi u Tasku 6: `changed_host_key_refuses_before_auth` i `accepting_requires_matching_fingerprint`.
3. **Imena sa razmakom, `'`, `$`, `;`, `*` i Unicodeom** na SCP-u (shell naredbe) i FTP-u: nikad se ne izvršava kao shell i nikad ne promaši datoteku. Testovi: Task 7 `scp_names_with_shell_chars` i Task 8 `ftp_names_with_spaces_and_unicode`.
4. **Lozinke na disku**: `servers.json` nikad ne sadrži lozinku, kriva master lozinka ne kvari trezor, a prekid pisanja trezora ostavlja stari trezor. Testovi u Tasku 2: `servers_json_never_contains_secret`, `vault_wrong_master_password_keeps_file` i `vault_tampered_file_is_rejected`.
5. **Pozivi iz Tauri async konteksta**: `block_on` unutar tokio konteksta paničari. Sinkroni API mora raditi iz `spawn_blocking` threada tuđeg runtimea. Test u Tasku 6: `sftp_calls_work_from_spawn_blocking`.

---

## File Structure

```
crates/core/Cargo.toml                  + russh, russh-sftp, tokio, suppaftp, argon2, chacha20poly1305, rand, base64, sha2; dev: libunftp, unftp-sbe-fs, rcgen
crates/core/src/lib.rs                  + pub mod remote; pub mod secrets; pub mod servers;
crates/core/src/servers.rs              ServerNode/ServerEntry/Protocol/Auth, servers.json, validate, find_by_name, parse_url, import_key
crates/core/src/secrets.rs              SecretStore trait, MemoryStore, Vault (secrets.json), migrate_secrets
crates/core/src/remote/mod.rs           RemoteFs, RemoteStat, RemoteError, RemoteLoc, parse/format, Sessions, rt(), connect()
crates/core/src/remote/known_hosts.rs   KnownHosts: check/trust, fingerprint format
crates/core/src/remote/transfer.rs      download_tree, upload_tree (.part + zamjena), remove_tree, walk_size
crates/core/src/remote/sftp.rs          SftpFs (russh + russh-sftp), ssh_connect (zajedničko za SCP)
crates/core/src/remote/scp.rs           ScpFs (exec: scp -t/-f, ls, mkdir, rm, mv, chmod; shell_quote)
crates/core/src/remote/ftp.rs           FtpFs (suppaftp, FTPS rustls s TOFU verifierom)
crates/core/src/remote/testutil.rs      #[cfg(test)] MemFs (RemoteFs u memoriji s ubacivanjem grešaka), SshTestServer, FtpTestServer
crates/core/src/vfs/mod.rs              Loc::Remote, routing list/nearest/dir_size/mkdir/rename/delete/materialize/entry_stamp/check_upload_target
crates/core/src/vfs/transfer.rs         matrica prijenosa s udaljenim izvorom/odredištem
src-tauri/Cargo.toml                    + security-framework (macos)
src-tauri/src/keychain.rs               KeychainStore: SecretStore preko security-framework
src-tauri/src/remote.rs                 Connector: servers, secrets, known_hosts, session password cache; komande connect/disconnect/…
src-tauri/src/state.rs                  + remote: Connector
src-tauri/src/commands.rs               edit_file/open_viewer za server, set_storage_mode s lozinkama, chmod
crates/core/src/keymap.rs               cm_connect/cm_disconnect available; + cm_chmod (Files, bez zadane tipke)
ui/lib.mjs                              parseRemote, remoteRoot, parentLocation za server, serverDefaults, validateServer, connectionLabel
ui/servers.js                           Ctrl+F dijalog, Edit dijalog, New URL, Encrypt, tok spajanja (host key, lozinke)
ui/app.js / panel.js / hotlist.js       ☁️ u traci diskova, red s putanjom, Ctrl+Shift+F, Connection lost, remote hotlist, chmod
README.md, docs/ROADMAP.md              odjeljak "Serveri"; ažurirani roadmap
```

## Zajednički tipovi (novi)

```rust
// remote/mod.rs
pub struct RemoteLoc { pub scheme: String, pub server: String, pub path: String /* apsolutna, "/" = korijen */ }
pub fn parse_remote(loc: &str) -> Option<RemoteLoc>;   // samo sftp|scp|ftp|ftps; "sftp://m" == "sftp://m/"
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

/// Sinkroni API. `progress(bytes_since_last)` vraća false za Cancel.
pub trait RemoteFs: Send + Sync {
    fn display(&self) -> String;                          // "sftp://user@host" (":port" ako nije zadani)
    fn home(&self) -> Result<String, RemoteError>;
    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError>;       // bez "." i ".."; hidden = počinje s '.'
    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError>; // lstat; None = ne postoji
    fn read_link(&self, path: &str) -> Result<String, RemoteError>;       // FTP: Unsupported
    fn read_to(&self, path: &str, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), RemoteError>;
    fn write_from(&self, path: &str, input: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), RemoteError>; // create/truncate
    fn mkdir(&self, path: &str) -> Result<(), RemoteError>;
    fn remove_file(&self, path: &str) -> Result<(), RemoteError>;        // briše i symlink (ne cilj)
    fn remove_dir(&self, path: &str) -> Result<(), RemoteError>;         // prazna mapa
    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError>;   // Exists ako `to` postoji
    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError>;  // rename preko postojećeg (pravila iz Global Constraints)
    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError>; // FTP: Unsupported
    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError>;   // FTP: Unsupported
    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError>;           // veza za dugi job (FTP: nova prijava; SSH: Arc klon sebe)
    fn close(&self);
}
```

`RemoteError` → `ListError`: `NotFound` → `NotFound`, `PermissionDenied` → `PermissionDenied`, ostalo → `Io("<loc>: <poruka>")`. → `OpError`: `Cancelled` → `Cancelled`, ostalo → `Io(poruka)`. UI prepoznaje poruke s prefiksom `connection lost` i `server not responding` (Task 10).

---

### Task 1: Lista servera, URL i ključevi

**Files:** Create `crates/core/src/servers.rs`; Modify `crates/core/src/lib.rs`, `crates/core/Cargo.toml` (`rand`)

**Interfaces:**
- Consumes: `storage::{load_json, save_json}`.
- Produces:
  ```rust
  pub const SERVERS_FILE: &str = "servers.json";
  pub enum Protocol { Sftp, Scp, Ftp, Ftps }                 // serde lowercase; .scheme() -> &str; .default_port() -> u16
  pub enum Auth { Password, Key, KeyPassphrase }            // serde camelCase
  pub enum FtpEncoding { Utf8, Windows1250, Latin1 }        // serde camelCase, default Utf8
  pub struct ServerEntry {                                  // serde camelCase, sve opcionalno ima #[serde(default)]
      pub id: String,            // 16 hex znakova, stabilan (veže lozinke)
      pub name: String, pub protocol: Protocol, pub host: String, pub port: u16, pub user: String,
      pub auth: Auth, pub key_path: Option<String>,   // apsolutna ili "keys/<ime>" (relativno na data dir)
      pub save_password: bool, pub remote_dir: Option<String>, pub local_dir: Option<String>,
      pub legacy_algorithms: bool, pub ftp_passive: bool /* default true */, pub ftp_encoding: FtpEncoding,
  }
  pub enum ServerNode { Folder { name: String, children: Vec<ServerNode> }, Server(ServerEntry) } // serde tag "kind"
  pub struct ServerList { pub nodes: Vec<ServerNode> }      // Default, Serialize, Deserialize
  impl ServerList {
      pub fn find_by_name(&self, name: &str) -> Option<&ServerEntry>;
      pub fn find_by_id(&self, id: &str) -> Option<&ServerEntry>;
      pub fn all(&self) -> Vec<&ServerEntry>;
      pub fn validate(&self) -> Result<(), String>;        // prva greška, tekst za UI
  }
  pub fn new_id() -> String;
  pub fn parse_url(url: &str) -> Result<ServerEntry, String>; // ime = user@host[:port], remote_dir = putanja
  pub fn key_file(entry: &ServerEntry, data_dir: &Path) -> Option<PathBuf>;
  pub fn import_key(src: &Path, data_dir: &Path) -> io::Result<String>;   // vraća "keys/<ime>"
  ```

- [ ] **Step 1: testovi** u `servers.rs`:
  - `servers_roundtrip_json`: stablo s mapom i dva servera se `save_json`/`load_json` vrati jednako. Nedostajuća polja daju zadane vrijednosti (`ftp_passive == true`, `ftp_encoding == Utf8`).
  - `validate_rejects_bad_entries`. Poruke:
    - prazno ime → `Name is required`;
    - ime s `/` → `Name cannot contain '/'`;
    - isto ime u dvije različite mape → `Another server is already named 'x'`;
    - prazan host → `Host is required`;
    - `Auth::Key` bez `key_path` → `Choose a key file`.
  - `parse_url_variants`:
    - `sftp://zb@web-1:2222/var/www` → protokol Sftp, user `zb`, host `web-1`, port 2222, `remote_dir == Some("/var/www")`, ime `zb@web-1:2222`;
    - `ftp://h` → port 21, user `anonymous`, ime `anonymous@h`;
    - `ftps://u@h/a%20b` → `remote_dir == Some("/a b")`;
    - `http://h` → `Err("Unsupported protocol 'http'")`.
  - `servers_json_never_contains_secret`: serijalizirani `ServerList` nema polje `password` ni `passphrase` (provjera nad tekstom JSON-a). Strukture **nemaju** takvo polje, a test to drži.
  - `import_key_copies_with_0600`:
    - `import_key` kopira u `<data>/keys/id_ed25519` s pravima 0600 i vraća `keys/id_ed25519`;
    - drugi import istog imena daje `keys/id_ed25519-2`;
    - `key_file` za `keys/...` vraća `<data>/keys/...`, a za apsolutnu putanju nju samu.
- [ ] **Step 2:** Run `cargo test -p fxfm-core servers`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj. Za `id` koristi `rand` (8 bajtova, hex).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): server list, URL parsing, key import`.

### Task 2: Trezor lozinki i SecretStore

**Files:** Create `crates/core/src/secrets.rs`; Modify `crates/core/src/lib.rs`, `crates/core/Cargo.toml` (`argon2`, `chacha20poly1305`, `base64`)

**Interfaces:**
- Produces:
  ```rust
  pub trait SecretStore: Send + Sync {
      fn get(&self, account: &str) -> Result<Option<String>, String>;
      fn set(&self, account: &str, secret: &str) -> Result<(), String>;
      fn delete(&self, account: &str) -> Result<(), String>;   // nepostojeći = Ok
  }
  pub fn account(server_id: &str, kind: SecretKind) -> String;  // "<id>/password" | "<id>/passphrase"
  pub enum SecretKind { Password, Passphrase }
  pub struct MemoryStore;                                       // HashMap iza Mutexa; za testove i "nespremljene" lozinke
  pub const VAULT_FILE: &str = "secrets.json";
  pub struct Vault;                                             // otključan trezor (ključ u memoriji)
  impl Vault {
      pub fn exists(data_dir: &Path) -> bool;
      pub fn create(data_dir: &Path, master: &str) -> io::Result<Vault>;          // greška ako već postoji
      pub fn unlock(data_dir: &Path, master: &str) -> Result<Vault, VaultError>;
      pub fn change_master(&mut self, new_master: &str) -> io::Result<()>;        // nova sol, sve ponovno kriptirano
  }
  impl SecretStore for Vault { … }                              // svaki set/delete atomski piše cijeli file
  pub enum VaultError { WrongPassword, Corrupt(String), Io(String) }  // Display: "Wrong master password", "Password file is damaged: …"
  pub fn migrate_secrets(from: &dyn SecretStore, to: &dyn SecretStore, accounts: &[String]) -> Result<usize, String>; // vraća broj kopiranih
  ```
- Format `secrets.json`: `{"version":1,"kdf":{"alg":"argon2id","m":65536,"t":3,"p":1,"salt":"<b64>"},"check":"<b64 nonce|ct od 'fxfm'>","items":{"<account>":"<b64 nonce|ct>"}}`. Imena računa su vidljiva (nisu tajna), a vrijednosti su kriptirane. AAD svakog zapisa je ime računa, pa se zapisi ne mogu zamijeniti.

- [ ] **Step 1: testovi**:
  - `vault_roundtrip`: `create` → `set("a/password","x")` → novi `unlock` istom lozinkom → `get == Some("x")`; `delete` → `None`.
  - `vault_wrong_master_password_keeps_file`: `unlock` s krivom daje `VaultError::WrongPassword`, a datoteka je bajt-identična.
  - `vault_tampered_file_is_rejected`: promijenjen bajt u ciphertextu zapisa → `get` vraća `Err` s `damaged`; zamijenjene vrijednosti dvaju računa → `Err`.
  - `vault_change_master`: nakon `change_master("n")` stara lozinka daje `WrongPassword`, nova radi, a sol je drugačija.
  - `vault_plaintext_not_in_file`: tekst `secrets.json` ne sadrži lozinku `hunter2-unique`.
  - `migrate_secrets_copies_existing_only`: iz `MemoryStore` s dva od tri računa u drugi `MemoryStore` → vraća 2.
  - Testovi koriste `Params` s malim `m` (npr. 8 MiB), preko `#[cfg(test)]` konstante, da ne traju dugo.
- [ ] **Step 2:** Run `cargo test -p fxfm-core secrets`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj. Pisanje ide preko `storage::save_json` (atomski).
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): encrypted password vault and SecretStore`.

### Task 3: Remote tipovi, known_hosts i vfs routing (bez prijenosa)

**Files:** Create `crates/core/src/remote/{mod.rs,known_hosts.rs,testutil.rs}`; Modify `crates/core/src/vfs/mod.rs`, `crates/core/src/lib.rs`

**Interfaces:**
- Consumes: `listing::{Entry, ListError, sort_entries}`, `ops::{OpError, OpObserver, OpSummary, valid_name}`.
- Produces:
  - tipovi iz "Zajednički tipovi";
  - `pub struct Sessions` s metodama `insert(name: &str, fs: Arc<dyn RemoteFs>)`, `get(name) -> Option<Arc<dyn RemoteFs>>`, `remove(name) -> Option<Arc<dyn RemoteFs>>`, `names() -> Vec<String>`; `vfs::Ctx` dobiva `pub sessions: Sessions`;
  - `vfs::Loc::Remote(RemoteLoc)`; `vfs::parse` prvo provjerava `parse_remote`;
  - postojeće `vfs` funkcije dobivaju granu za server:
    - `list`;
    - `nearest`: najbliža postojeća nadmapa preko `stat`; korijen `/`; nepostojeća veza vraća lokaciju nepromijenjenu;
    - `dir_size` (rekurzivno, poštuje `cancel`);
    - `mkdir` i `rename` (uz `valid_name`);
    - `delete` (rekurzivno, trajno, `obs.error` po stavci, `to_trash` se ignorira);
    - `local_dir_for` (server → `home` korisnika; potpis dobiva `home: &Path`, a svi pozivatelji se ažuriraju);
  - `fn session_for(ctx: &Ctx, r: &RemoteLoc) -> Result<Arc<dyn RemoteFs>, RemoteError>`: nema veze → `Disconnected`. Kod `Disconnected` iz bilo koje operacije veza se **izbacuje** iz `sessions`.
  - `remote::rt() -> &'static tokio::runtime::Runtime` (OnceLock, multi-thread, 2 workera, `enable_all`);
  - `pub fn with_timeout<T>(fut: impl Future<Output = Result<T, RemoteError>>, secs: u64) -> Result<T, RemoteError>` (`rt().block_on(tokio::time::timeout(...))`, isteklo → `Timeout`);
  - `known_hosts.rs`:
    ```rust
    pub const KNOWN_HOSTS_FILE: &str = "known_hosts";
    pub enum HostCheck { Known, Unknown, Changed { old_fingerprint: String } }
    pub struct KnownHosts { /* putanja datoteke */ }
    impl KnownHosts {
        pub fn new(path: PathBuf) -> KnownHosts;
        pub fn check(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> HostCheck;
        pub fn trust(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> io::Result<()>; // zamjenjuje stari zapis
    }
    pub fn fingerprint_sha256(blob: &[u8]) -> String;   // "SHA256:" + base64 bez paddinga, kao `ssh-keygen -l`
    ```
    Format retka je `<host>:<port> <key_type> <fingerprint>`. Prazni retci i `#` komentari se čuvaju. FTPS certifikat koristi `key_type` `x509`.
  - `remote::testutil::MemFs`: `RemoteFs` u memoriji (stablo putanja → datoteka/mapa/symlink s modeom i mtimeom), uz `fail_after_bytes(n)`, koji poslije n bajtova pisanja vraća `Disconnected` (za Review Focus 1).

- [ ] **Step 1: testovi**:
  - `parse_remote_variants`:
    - `sftp://m/var/www` → (`sftp`, `m`, `/var/www`);
    - `sftp://m` → putanja `/`;
    - `ftp://a b/x` → server `a b`;
    - `http://x/y` i `/Users/x` → `None`;
    - `format_remote("sftp","m","/")` → `sftp://m/`.
  - `known_hosts_check_and_trust`:
    - prazno → `Unknown`;
    - nakon `trust` → `Known`;
    - drugi fingerprint → `Changed { old_fingerprint }`;
    - `trust` novog zamijeni redak i sačuva komentar;
    - isti host na drugom portu je `Unknown`.
  - `fingerprint_matches_ssh_keygen`: poznati ed25519 javni ključ (blob iz fiksnog testnog `.pub`, u testu kao konstanta) daje očekivani `SHA256:...` (vrijednost izračunata `ssh-keygen -lf` i upisana u test).
  - `vfs_routes_remote`: `Ctx` s `MemFs` pod imenom `m`:
    - `list("sftp://m/d")` daje sortirane unose, `.hidden` ima `hidden == true`;
    - `mkdir("sftp://m/d","n")` vraća `sftp://m/d/n`;
    - `rename` vraća novu lokaciju;
    - `delete(["sftp://m/d"])` briše rekurzivno i `done` broji datoteke;
    - `nearest("sftp://m/d/nema/x")` → `sftp://m/d`;
    - `dir_size` daje (bajtovi, broj datoteka).
  - `vfs_remote_without_session`: `list("sftp://nema/")` → `ListError::Io` s `connection lost`; `nearest` vraća ulaz nepromijenjen.
  - `disconnected_drops_session`: `MemFs` koji na `list` vrati `Disconnected` → poslije poziva `ctx.sessions.get("m")` je `None`.
  - `remote_calls_from_inside_tokio_spawn_blocking`: unutar `#[tokio::test]` s vlastitim runtimeom, `spawn_blocking(|| with_timeout(async { Ok(1) }, 1))` vraća `Ok(1)` bez panike.
- [ ] **Step 2:** Run `cargo test -p fxfm-core remote vfs`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS. Postojeći vfs i archive testovi ostaju zeleni.
- [ ] **Step 5:** Commit `feat(core): remote locations, known_hosts, vfs routing for servers`.

### Task 4: Prijenosi sa servera i na server

**Files:** Create `crates/core/src/remote/transfer.rs`; Modify `crates/core/src/vfs/transfer.rs`, `crates/core/src/vfs/mod.rs`

**Interfaces:**
- Consumes: Task 3, `ops::{copy, move_, resolve_dest}`, staging iz plana 2 (`Ctx::new_temp_dir("stage")`, `cleanup_dir`), postojeće `archive_to_local`/`local_to_archive`.
- Produces:
  - `remote::transfer::download_tree(fs: &dyn RemoteFs, items: &[String], to_dir: &Path, obs: &dyn OpObserver, progress: &mut ProgressState) -> Result<(), OpError>`:
    - mape stvara lokalno;
    - symlinke stvara kao symlinke (`read_link`; ako je `Unsupported`, preuzima cilj);
    - postavlja mtime i prava;
    - greške po stavci idu kroz `obs.error` (Retry/Skip/SkipAll/Cancel).
  - `remote::transfer::upload_tree(fs: &dyn RemoteFs, sources: &[PathBuf], to_dir: &str, obs: &dyn OpObserver, progress: &mut ProgressState) -> Result<OpSummary, OpError>`:
    - za svaku datoteku `stat` cilja; ako postoji, `obs.conflict` (svi `ConflictAnswer`, uključujući `Rename` i `OverwriteAllOlder` po mtimeu);
    - postojeća mapa se spaja bez pitanja (kao `ops`);
    - mapa na mjestu datoteke ili obrnuto daje `obs.error` s `ops::type_clash` tekstom;
    - pisanje prema pravilu `.part` iz Global Constraints.
  - `remote::transfer::remove_tree(fs, path, obs) -> Result<u64, OpError>` (broj obrisanih datoteka; koristi ga i `vfs::delete`).
  - `ProgressState`: zbraja `Progress` i zove `obs.progress` najviše svakih 100 ms.
  - `vfs::copy`/`vfs::move_` (u `transfer.rs`) za svaku kombinaciju s udaljenom stranom:

    | izvor → odredište | kako |
    |---|---|
    | server → lokalno | `download_tree` u `stage-<n>`, pa `ops::copy` ili `ops::move_` stage → odredište (konflikti iz `ops`) |
    | lokalno → server | `upload_tree` direktno |
    | arhiva → server | izvuci u stage (postojeći kod), pa `upload_tree` |
    | server → arhiva | `download_tree` u stage, pa postojeći `local_to_archive` |
    | server → server, različite veze | stage, pa `upload_tree` (job koristi `fork()` za obje strane) |
    | isti server, move | `fs.rename` po stavci (bez prijenosa); `Exists` → `obs.conflict` (Overwrite = `remove_tree` cilja + `rename`) |
    | isti server, copy | stage, pa `upload_tree` |

    Kod **move** se izvor briše tek nakon što je odredište uspješno upisano. Preskočene stavke se ne brišu.
  - Ukupna veličina za progress: `walk_size` na serveru prije početka. Ne broji se dvaput kad prijenos ide kroz stage (dio stage → odredište se ne prikazuje kao novi posao).

- [ ] **Step 1: testovi** (s `MemFs`):
  - `upload_new_tree`: lokalna mapa `d/{a.txt, sub/b.txt}` → `sftp://m/up`, sadržaj i prava odgovaraju, a `.part` ne ostaje nigdje.
  - `upload_conflict_answers`: `Skip` ostavlja cilj, `Overwrite` ga zamijeni, a `Rename { new_name: "a(2).txt" }` napravi novu datoteku.
  - `upload_replace_keeps_target_mode`: cilj s pravima 0755 i lokalni izvor 0644 → nakon prepisivanja cilj ima 0755.
  - `upload_interrupted_leaves_target_intact`: `fail_after_bytes(10)` kod prepisivanja 1 MB datoteke → `OpError::Io` s `connection lost`, a cilj je bajt-identičan starom.
  - `upload_over_symlink_writes_through`: cilj je symlink na `real.txt` → poslije uploada link je i dalje link, a `real.txt` ima novi sadržaj.
  - `upload_without_dir_write_permission_writes_directly`: `MemFs` mapa označena "no create" i postojeći cilj → sadržaj se upiše direktno.
  - `download_to_local_with_conflicts`: server → lokalno kroz `ops` konflikte (`SkipAll` preskoči sve postojeće); stage mapa ne postoji poslije.
  - `move_remote_to_local_deletes_source_after_success`: izvor obrisan; kod `Skip` jedne stavke ta ostaje na serveru.
  - `move_within_same_server_renames`: `move_(["sftp://m/a/x"], "sftp://m/b/")` ne zove `read_to` (MemFs broji pozive) i `x` je u `b`.
  - `cancel_during_upload_removes_part`: `obs.is_cancelled()` nakon prve datoteke → `summary.cancelled`, bez `.part`.
  - `archive_to_remote_and_back`: `a.zip/d` → `sftp://m/x` → `<t>/out`, sadržaj jednak izvoru.
- [ ] **Step 2:** Run `cargo test -p fxfm-core transfer`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): copy and move to, from and between servers`.

### Task 5: materialize, entry_stamp i upload natrag za server

**Files:** Modify `crates/core/src/vfs/mod.rs`

**Interfaces:**
- Consumes: Task 3–4.
- Produces (proširenja postojećih potpisa, bez promjene za lokalno i arhivu):
  - `vfs::materialize(ctx, "sftp://m/x/f.php", prefix)` preuzima u `<prefix>-<n>/f.php`; mapa daje `Only files can be opened from a server`;
  - `vfs::entry_stamp(ctx, loc)` za server vraća `Some((size, mtime))` iz `stat`, a `None` ako datoteka ne postoji;
  - `vfs::check_upload_target(loc)` za server: `Ok` ako je veza u `sessions`, inače `Err("Not connected to <ime>; your edited copy is kept")`. Potpis dobiva `ctx: &Ctx` i pozivatelji se ažuriraju (`editwatch.rs`, `jobs.rs`);
  - `JobRequest::Upload { local, target }` za server ide kroz `upload_tree` s `NullObserver` (overwrite), tj. istim pravilom `.part`.

- [ ] **Step 1: testovi**:
  - `materialize_remote_file`: sadržaj preuzete kopije jednak je MemFs datoteci.
  - `entry_stamp_remote_changes_after_write`: stamp prije i poslije promjene na serveru se razlikuje.
  - `check_upload_target_needs_session`: s vezom `Ok`, bez nje `Err` koji sadrži `your edited copy is kept`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core vfs`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS (uključujući `src-tauri` editwatch testove).
- [ ] **Step 5:** Commit `feat(core): F3/F4 materialize and upload back for server files`.

### Task 6: SFTP backend i spajanje preko SSH-a

**Files:** Create `crates/core/src/remote/sftp.rs`; Modify `crates/core/src/remote/{mod.rs,testutil.rs}`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: Task 1 (`ServerEntry`, `key_file`), Task 3 (`KnownHosts`, `with_timeout`, `rt`).
- Produces:
  ```rust
  pub struct Credentials { pub password: Option<String>, pub passphrase: Option<String> }
  #[derive(Debug, Clone, PartialEq)]
  pub enum ConnectError {
      HostKey { host: String, port: u16, key_type: String, fingerprint: String, changed_from: Option<String> },
      NeedPassword, NeedPassphrase,          // nedostaje; UI pita
      AuthFailed,                            // poslano, odbijeno ("Wrong user name, password or key")
      Timeout,                               // "Server did not answer within 15 seconds"
      Failed(String),                        // DNS, odbijena veza, nepodržani algoritmi (poruka kaže da uključi legacy opciju)
  }
  pub struct ConnectOptions<'a> { pub data_dir: &'a Path, pub known: &'a KnownHosts, pub accept_fingerprint: Option<&'a str> }
  pub fn connect(server: &ServerEntry, creds: &Credentials, opts: &ConnectOptions) -> Result<Arc<dyn RemoteFs>, ConnectError>; // u remote/mod.rs, bira backend po protokolu
  pub(crate) fn ssh_connect(server, creds, opts) -> Result<SshSession, ConnectError>;  // dijele SFTP i SCP
  ```
- Host key se provjerava u `check_server_key` russh handlera:
  - `Known` → nastavi;
  - `Unknown` ili `Changed` → ako je `accept_fingerprint == Some(fp)` i jednak predočenom, `trust` pa nastavi; inače prekini vezu **prije autentikacije** i vrati `HostKey`.
- Autentikacija:
  - `Password`;
  - `Key`: ključ bez passphrasea; ako je kriptiran, a passphrase nije dan → `NeedPassphrase`;
  - `KeyPassphrase`.

  Učitavanje preko `russh::keys::decode_secret_key` podržava OpenSSH (RSA, ECDSA, Ed25519) i PEM (PKCS#1, PKCS#8, kriptirani). Za RSA se koristi `best_supported_rsa_hash`.
- `legacy_algorithms`: `Preferred` dodaje `ssh-rsa` (SHA-1) za host key i korisnički ključ te `diffie-hellman-group14-sha1` i `diffie-hellman-group1-sha1` za kex.
- `SftpFs` ima `display()` `sftp://user@host[:port]` i `home()` = `canonicalize(".")`. `replace` koristi `posix-rename@openssh.com` ako ga server nudi. `fork()` vraća klon iste veze (SFTP multipleksira).
- `testutil::SshTestServer`: russh server na `127.0.0.1:0` u testu, s generiranim ed25519 host keyem. Prihvaća lozinku `pw` ili zadani javni ključ korisnika.
  - Subsystem `sftp` pokreće sistemski `sftp-server` (`/usr/libexec/sftp-server`, `/usr/lib/openssh/sftp-server`, `/usr/lib/ssh/sftp-server`) i preusmjerava stdio.
  - `exec` pokreće `sh -c <cmd>` (za SCP, Task 7).
  - Opcija `stall()` prestaje odgovarati bez zatvaranja veze.
  - Ako `sftp-server` nije pronađen, testovi ispisuju `skipped: no sftp-server` i prolaze.

- [ ] **Step 1: testovi** (`#[test]`, protiv `SshTestServer`):
  - `sftp_password_login_and_list`: lista privremene mape servera odgovara `fs::read_dir` (imena, `is_dir`, veličine).
  - `sftp_key_login_ed25519_and_encrypted_pem`: OpenSSH ed25519 ključ radi; kriptirani PKCS#8 bez passphrasea daje `NeedPassphrase`, a s ispravnim passphraseom radi.
  - `unknown_host_key_asks_then_trusts`: prvo `HostKey { changed_from: None }`, zatim s `accept_fingerprint` = taj fingerprint spaja i upisuje u `known_hosts`; treći put spaja bez pitanja.
  - `changed_host_key_refuses_before_auth`: `known_hosts` ima drugi fingerprint → `HostKey { changed_from: Some(old) }`, a server **nije primio** nijedan auth zahtjev (brojač u test serveru je 0).
  - `accepting_requires_matching_fingerprint`: `accept_fingerprint` drugog ključa → opet `HostKey`, `known_hosts` nepromijenjen.
  - `wrong_password_is_auth_failed`, `missing_password_is_need_password`.
  - `sftp_file_ops_roundtrip`: `write_from`, `read_to`, `mkdir`, `rename` (`Exists` ako cilj postoji), `replace`, `set_mode`, `symlink`/`read_link`, `remove_file` i `remove_dir` rade na pravom `sftp-server`u.
  - `stalled_server_times_out`: nakon `stall()` poziv `list` vraća `Timeout` za ≤ 65 s. Test koristi `#[cfg(test)]` timeout 2 s preko `with_timeout` parametra, pa traje ~2 s.
  - `sftp_calls_work_from_spawn_blocking`: `list` pozvan iz `tokio::task::spawn_blocking` unutar drugog runtimea vraća `Ok`.
- [ ] **Step 2:** Run `cargo test -p fxfm-core sftp`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings && cargo tree -p fxfm-core -i aws-lc-sys; cargo tree -p fxfm-core -i openssl-sys`. Expected: testovi PASS, a oba `cargo tree` ispisuju da paket nije pronađen.
- [ ] **Step 5:** Commit `feat(core): SFTP backend with host key check and key auth`.

### Task 7: SCP backend

**Files:** Create `crates/core/src/remote/scp.rs`; Modify `crates/core/src/remote/mod.rs`

**Interfaces:**
- Consumes: `ssh_connect` iz Taska 6.
- Produces:
  - `ScpFs: RemoteFs`. Pri spajanju pokušava otvoriti SFTP subsystem:
    - ako uspije, `list`, `stat`, `read_link` i `home` idu preko SFTP-a;
    - inače `list` izvršava `LC_ALL=C ls -la --time-style=+%s -- <path>` (GNU), a ako to ne uspije, `LC_ALL=C ls -la -T -- <path>` (BSD);
    - `stat` je `ls -lad`; `home` je `pwd`.
  - Prijenos ide uvijek preko `scp -t` i `scp -f` (SCP protokol: `C<mode> <size> <name>\n`, potvrde `\0`, greške `\1`/`\2`).
  - `mkdir`, `remove_file`, `remove_dir`, `rename` (`mv -n`, zatim provjera), `replace` (`mv -f`), `symlink` (`ln -s`) i `set_mode` (`chmod`) su exec naredbe. Izlazni kod ≠ 0 daje `RemoteError` po stderru: `No such file` → `NotFound`, `Permission denied` → `PermissionDenied`, ostalo `Other(stderr)`.
  - `pub(crate) fn shell_quote(s: &str) -> String`: POSIX single-quote (`'` → `'\''`). Svaki argument u svakoj naredbi prolazi kroz `shell_quote`, a putanje idu iza `--`.
  - `pub(crate) fn parse_ls(line: &str, gnu: bool) -> Option<(String, RemoteStat, Option<String>)>` (ime, stat, cilj symlinka).

- [ ] **Step 1: testovi**:
  - `shell_quote_cases`: `a b` → `'a b'`, `it's` → `'it'\''s'`, `$(rm -rf ~)` ostaje doslovan unutar navodnika.
  - `parse_ls_gnu_and_bsd`: fiksni retci (datoteka, mapa, symlink `a -> b`, ime s razmacima, ime s `->` u imenu datoteke nije symlink ako prvi znak nije `l`) daju očekivane vrijednosti.
  - `scp_roundtrip_against_test_server` (SshTestServer; exec pokreće sistemski `scp`): upload i download 1 MB datoteke daju isti sadržaj; `mkdir`, `rename`, `remove_*` i `set_mode` rade.
  - `scp_names_with_shell_chars`: datoteke `a b.txt`, `it's.php`, `$x;y*.sh` i `čćž.txt` prežive upload, list, rename, download i delete. Nijedna druga datoteka u mapi ne nestane, a nije nastala datoteka `y`.
  - `scp_list_without_sftp_uses_ls`: test server s isključenim SFTP subsystemom daje isti listing kao sa SFTP-om (imena, `is_dir`, veličina).
- [ ] **Step 2:** Run `cargo test -p fxfm-core scp`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): SCP backend with ls fallback`.

### Task 8: FTP i FTPS backend

**Files:** Create `crates/core/src/remote/ftp.rs`; Modify `crates/core/src/remote/{mod.rs,testutil.rs}`, `crates/core/Cargo.toml`

**Interfaces:**
- Consumes: Task 1, Task 3 (`KnownHosts`).
- Produces:
  - `FtpFs: RemoteFs`. Jedna kontrolna veza iza `Mutex`a. `fork()` otvara novu prijavu s istim podacima, pa dugi jobovi ne blokiraju listanje panela.
  - Listanje: `MLSD` ako ga server oglašava u `FEAT`, inače `LIST -a` parsiran suppaftpovim `list::File` (Unix i DOS format).
  - Načini: pasivni (zadano) ili aktivni (`ftp_passive == false`).
  - FTPS je explicit TLS (`AUTH TLS`) s rustls. Certifikat se provjerava **TOFU** preko `known_hosts` (`key_type` `x509`, SHA-256 DER-a), na isti način kao SSH host key (`ConnectError::HostKey`). Prolazak webpki provjere nije uvjet, jer LAN serveri imaju self-signed certifikate.
  - `ftp_encoding`:
    - Utf8 → `OPTS UTF8 ON` ako ga server podržava;
    - Windows1250 ili Latin1 → imena se dekodiraju i kodiraju preko `encoding_rs`.

    **Ako suppaftp ne izlaže sirove bajtove imena** (provjeri u izvoru 12.x), podrži samo Utf8, sakrij polje u UI-ju (Task 11) i dodaj stavku u `docs/ROADMAP.md`. Ne smije se dogoditi da se ime koje se ne može dekodirati tiho promijeni.
  - `set_mode` → `SITE CHMOD`; ako ga server odbije, `Unsupported("This server does not support changing permissions")`. `symlink` i `read_link` → `Unsupported`.
  - `replace` → `RNFR`/`RNTO`, a ako to ne uspije, `DELE` cilja pa `RNTO`.
  - `testutil::FtpTestServer`: libunftp s `unftp-sbe-fs` nad privremenom mapom na `127.0.0.1:0`, korisnik `u`/`pw`, opcionalno FTPS s rcgen self-signed certifikatom.

- [ ] **Step 1: testovi**:
  - `ftp_login_list_roundtrip`: list, `write_from`/`read_to` 1 MB, `mkdir`, `rename`, `replace`, `remove_file` i `remove_dir`.
  - `ftp_wrong_password_is_auth_failed`.
  - `ftps_unknown_cert_asks_then_trusts`: prvo `HostKey` s `key_type == "x509"`, zatim s prihvaćenim fingerprintom spaja; promijenjeni cert daje `changed_from: Some(_)`.
  - `ftp_names_with_spaces_and_unicode`: `a b.txt` i `čćž.txt` prežive upload, list, rename, download i delete.
  - `ftp_fork_is_independent`: dok jedan `fork()` čita 5 MB datoteku u drugom threadu, `list` na originalu vraća rezultat prije kraja čitanja.
  - `ftp_active_mode_lists`: `ftp_passive = false` daje isti listing.
- [ ] **Step 2:** Run `cargo test -p fxfm-core ftp`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `cargo test -p fxfm-core && cargo clippy -p fxfm-core -- -D warnings && cargo tree -p fxfm-core -i openssl-sys`. Expected: PASS, a za `openssl-sys` paket nije pronađen.
- [ ] **Step 5:** Commit `feat(core): FTP and FTPS backend`.

### Task 9: Tauri sloj – veze, lozinke, Keychain, F3/F4 i migracija

**Files:** Create `src-tauri/src/remote.rs`, `src-tauri/src/keychain.rs`; Modify `src-tauri/src/{state.rs,commands.rs,jobs.rs,main.rs}`, `src-tauri/Cargo.toml`

**Interfaces:**
- Consumes: Task 1–8; postojeći `blocking`, `Jobs`, `EditWatch`, `storage::migrate`.
- Produces:
  - `KeychainStore: SecretStore` (`security_framework::passwords::{get_generic_password, set_generic_password, delete_generic_password}`, servis `fxfm`), samo `cfg(target_os = "macos")`. Na drugim sustavima `set` vraća `Err("No system keychain on this platform")`.
  - `pub struct Connector` u `AppState.remote`. Sadrži `servers: Mutex<ServerList>`, `vault: Mutex<Option<Vault>>`, `session_secrets: MemoryStore` (unesene, nespremljene lozinke) i `quick: Mutex<HashMap<String, ServerEntry>>` (brze veze po imenu).
  - Komande (sve `async` + `blocking`, kao postojeće):
    - `servers_load() -> ServerList`;
    - `servers_save(list: ServerList) -> Result<(), String>`: radi `validate`. Za obrisane servere briše njihove tajne iz trenutnog storea (Keychain ili otključan trezor) i iz `session_secrets`;
    - `server_secret_set(id: String, kind: SecretKind, secret: Option<String>) -> Result<(), ConnectReply>`: `None` briše. Portable bez otključanog trezora vraća `NeedMaster { create }`;
    - `connect(req: ConnectRequest) -> Result<ConnectReply, String>`;
    - `disconnect(name: String) -> Result<(), String>`: `close()` i uklanjanje iz `sessions`; emitira `connections://changed`. Briše `view-*` kopije s te veze i F4 kopije bez neučitanih promjena (spec §5: "privremene kopije čiste se pri zatvaranju veze"). Kopije s neučitanim promjenama ostaju, a upload kasnije javlja `Not connected…` (Task 5). Test: `disconnect_cleans_clean_copies_only`;
    - `list_connections() -> Vec<ConnectionInfo>`: `ConnectionInfo { name, display, root: String /* "sftp://name/" */ }`;
    - `master_unlock(password: String) -> Result<(), String>`, `master_set(old: Option<String>, new: String) -> Result<(), String>` (stvara trezor ili mijenja lozinku), `master_status() -> MasterStatus { mode: StorageMode, exists: bool, unlocked: bool }`;
    - `import_key(path: String) -> Result<String, String>`;
    - `chmod(location: String, mode: u32) -> Result<(), String>`: lokalno i arhiva → `Change attributes works on SFTP and SCP servers`.
    ```rust
    #[serde(rename_all = "camelCase")]
    pub struct ConnectRequest { pub server: Option<String> /* ime */, pub url: Option<String>,
        pub password: Option<String>, pub passphrase: Option<String>, pub save: bool, pub accept_fingerprint: Option<String> }
    #[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
    pub enum ConnectReply {
        Connected { location: String, local_dir: Option<String> },
        HostKey { host: String, port: u16, key_type: String, fingerprint: String, changed_from: Option<String> },
        NeedPassword { retry: bool }, NeedPassphrase { retry: bool },
        NeedMaster { create: bool },
    }
    ```
  - Tok `connect`:
    1. pronađe server (ime u listi ili `parse_url`); ako je već spojen, odmah `Connected`;
    2. lozinke uzima iz zahtjeva, zatim `session_secrets`, zatim storea. U portable načinu s `save_password` i zaključanim trezorom vraća `NeedMaster { create: !Vault::exists }`;
    3. zove `remote::connect` (`blocking`);
    4. `NeedPassword`/`NeedPassphrase` → reply s `retry: false`; `AuthFailed` → `NeedPassword { retry: true }` (za ključ s passphraseom `NeedPassphrase { retry: true }`), a pogrešna lozinka se briše iz `session_secrets`;
    5. kod uspjeha: unesena lozinka ide u `session_secrets`, a uz `save` i u store; veza se registrira u `sessions`; emitira se `connections://changed`; vraća se `Connected` s `remote_dir` ili `home()`.
  - `Disconnected` iz bilo koje komande ili joba emitira `connections://changed` (veza je već izbačena u core-u, Task 3).
  - `edit_file` i `open_viewer` za server rade kao za arhivu (`materialize`, `EditWatch`). Naslov viewera je puna lokacija.
  - `set_storage_mode(mode, master: Option<String>)`:
    - Portable → System: treba otključan trezor (inače `Err("master password required")`). Nakon migracije datoteka sve tajne idu u Keychain (`migrate_secrets`).
    - System → Portable: `master` je obavezan i stvara novi trezor u novom `fxfm-data`. Tajne se kopiraju iz Keychaina, a iz Keychaina se brišu tek nakon što se trezor ponovno otvori i svaka vrijednost provjeri.
    - Poruka potvrde navodi broj preseljenih lozinki.
  - Izlaz: `close()` svih veza; privremene kopije s neučitanim promjenama ostaju (postojeće pravilo editwatcha).

- [ ] **Step 1: testovi** (`src-tauri`, bez Keychaina: `Connector` prima `Arc<dyn SecretStore>` za sistemski način, a testovi daju `MemoryStore`):
  - `connect_reply_serde`: JSON oblici svih `ConnectReply` varijanti (`{"kind":"hostKey","host":…,"changedFrom":null}` …).
  - `servers_save_removes_secrets_of_deleted`: obrisan server → njegov `id/password` nestaje iz storea.
  - `portable_save_password_needs_master`: portable, trezor ne postoji → `server_secret_set` vraća `NeedMaster { create: true }`; poslije `master_set` lozinka je u `secrets.json`.
  - `wrong_password_not_cached`: `AuthFailed` ne ostavlja lozinku u `session_secrets` (logika u čistoj funkciji `after_connect(result, entered, store)`, koja se testira bez mreže).
  - `migrate_portable_to_system_moves_secrets` i `migrate_system_to_portable_requires_master` (MemoryStore umjesto Keychaina, privremene mape umjesto pravih).
  - `disconnected_error_drops_session`: job koji dobije `Disconnected` → `list_connections` ga više ne vraća.
- [ ] **Step 2:** Run `cargo test -p fxfm`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj. Registriraj komande u `main.rs`.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat: connections, password storage and server commands`.

### Task 10: UI – server u panelima

**Files:** Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/app.js`, `ui/panel.js`, `ui/hotlist.js`, `ui/dialogs.js`, `crates/core/src/keymap.rs`

**Interfaces:**
- Consumes: komande iz Taska 9; događaj `connections://changed`.
- Produces (`lib.mjs`, čiste funkcije):
  - `parseRemote(loc) -> {scheme, server, path} | null` (ista pravila kao `parse_remote`);
  - `remoteRoot(loc) -> "sftp://m/"`;
  - `parentLocation` za server: `sftp://m/var` → `sftp://m/`, a `sftp://m/` → `null` (Backspace u korijenu ne radi ništa);
  - `joinLocation("sftp://m/", "x")` → `sftp://m/x`;
  - `connectionLost(message) -> boolean` (prefiksi `connection lost`, `server not responding`);
  - `deleteConfirmText(location, n)`: na serveru `Delete N file(s) from the server? This cannot be undone.`, a inače postojeći tekstovi.
- Ponašanje:
  - Traka diskova dobiva gumb `☁️ <ime>` za svaku vezu iz `list_connections`. Osvježava se na `connections://changed`. Aktivan je kad je panel na toj vezi.
  - Red s putanjom: na serveru umjesto slobodnog prostora `[sftp://user@host]` (`display`).
  - `cm_disconnect` (Ctrl+Shift+F) prekida vezu aktivnog panela. Svi paneli na toj vezi idu na `local_dir` servera ili `~`.
  - Greška s `connectionLost` u panelu ili jobu prikazuje dijalog `Connection to <ime> lost` s gumbima `Reconnect` (tok iz Taska 11, zatim ista putanja ili `nearest`) i `Close` (panel ide na `~`).
  - Hotlista: `cd sftp://ime/putanja` i Target path na serveru spajaju (ako veza ne postoji) pa otvaraju putanju. `REMOTE_MSG` se uklanja.
  - `cm_chmod` (meni Files, naslov `Change attributes…`, bez zadane tipke): dijalog s oktalnim poljem i 9 kvačica rwx, sinkroniziranih. Za više označenih stavki primjenjuje se na sve. Mape se ne obrađuju rekurzivno.
  - `keymap.rs`: `cm_connect` i `cm_disconnect` dobivaju `available: true`; dodaje se `cm_chmod`.
  - Session restore ostaje kakav jest (server → `~`, `restore_path`).

- [ ] **Step 1: testovi** u `ui/lib.test.mjs` za sve nove funkcije iz liste, s navedenim vrijednostima, te Rust test `keymap` `connect_commands_available`.
- [ ] **Step 2:** Run `node --test ui/ && cargo test -p fxfm-core keymap`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj.
- [ ] **Step 4:** Run `node --test ui/ && cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): servers as drives, disconnect, connection lost, remote hotlist, chmod`.

### Task 11: UI – Ctrl+F dijalog, uređivanje servera i tok spajanja

**Files:** Create `ui/servers.js`; Modify `ui/lib.mjs`, `ui/lib.test.mjs`, `ui/app.js`, `ui/style.css`, `ui/index.html`

**Interfaces:**
- Consumes: komande iz Taska 9; stil i stablo kao `ui/hotlist.js`.
- Produces:
  - `lib.mjs`:
    - `serverDefaults(protocol) -> {port, auth}` (sftp/scp → 22/`password`, ftp/ftps → 21/`password`);
    - `validateServer(entry, allNames) -> string|null`, s porukama iz Taska 1 (UI provjerava prije spremanja; backend još jednom);
    - `fieldsFor(protocol) -> Set<string>`: SSH ima `auth`, `keyPath`, `legacyAlgorithms`; FTP ima `ftpPassive`, `ftpEncoding`;
    - `connectionLabel(info) -> "☁️ " + name`;
    - `hostKeyText(reply)`.

    `hostKeyText` za novi ključ:
    ```
    The authenticity of host '<host>:<port>' can't be established.
    <keyType> key fingerprint is <fp>.
    Connect and remember this key?
    ```
    Za promijenjen ključ:
    ```
    WARNING: THE HOST KEY FOR '<host>:<port>' HAS CHANGED!
    Someone may be intercepting the connection, or the server was reinstalled.
    Old: <old>
    New: <fp>
    Replace the stored key and connect?
    ```
    Promijenjen ključ ima crveni naslov, a zadani gumb je `Cancel`.
  - Dijalog `Connect to server` (Ctrl+F):
    - lijevo stablo mapa i servera, desno gumbi `Connect`, `New connection…`, `New URL…`, `Duplicate entry…`, `New folder…`, `Edit…`, `Delete`, `Encrypt…`, `Cancel`, `Help`;
    - Enter i dvoklik spajaju, Esc zatvara, ↑/↓ i ←/→ (sklopi i rasklopi mapu) rade, a drag & drop i Ctrl+↑/↓ mijenjaju redoslijed (logika iz hotliste: `moveNode`, `relocateNode`);
    - `Delete` traži potvrdu `Delete '<ime>' and its saved password?`;
    - `Duplicate entry…` daje novi `id` i ime `<ime> (2)`, ali **bez** lozinke (poruka u statusu);
    - `Encrypt…`: u portable načinu postavlja ili mijenja master lozinku (dva polja i staro polje ako trezor postoji); u sistemskom načinu poruka `In system mode passwords are stored in the macOS Keychain.`;
    - `Help` prikazuje kratak tekst o tipkama i formatu URL-a.
  - Dijalog `Edit connection`:
    - polja: Name, Protocol, Host, Port, User, Authentication (Password / Key file / Key file + passphrase), Key file (`Choose…` i `Import…`), Save password (s poljem lozinke ili passphrasea), Remote dir, Local dir (`Choose…`), Allow legacy algorithms (ssh-rsa/SHA1), Passive mode, Name encoding;
    - vidljiva su samo polja iz `fieldsFor`;
    - promjena protokola mijenja port samo ako je bio zadani za stari protokol;
    - OK sprema preko `servers_save` i, ako je lozinka unesena, `server_secret_set`. Prazno polje uz uključen "Save password" ne briše spremljenu lozinku, a isključen "Save password" je briše.
  - `New URL…`: jedno polje (`sftp://user@host:port/path`), zatim tok spajanja.
  - Tok spajanja u `servers.js`, `async function connectFlow(request) -> location|null`, petlja nad `connect`:
    - `hostKey` → dijalog s `hostKeyText` → ponovi s `acceptFingerprint`;
    - `needPassword`/`needPassphrase` → polje lozinke (s `retry` tekst `Wrong password, try again`) i kvačica `Save password` (samo za server iz liste) → ponovi;
    - `needMaster` → polje master lozinke (`create` traži dva polja) → `master_unlock` ili `master_set` → ponovi;
    - `connected` → aktivni panel ide na `location`, a ako je zadan `local_dir`, drugi panel ide tamo.

    Tok koriste i hotlista i Reconnect iz Taska 10.

- [ ] **Step 1: testovi** u `ui/lib.test.mjs`:
  - `serverDefaults`;
  - `validateServer` (svih pet poruka);
  - `fieldsFor` za sva četiri protokola;
  - `hostKeyText` (novi i promijenjeni ključ, točan tekst);
  - promjena protokola mijenja port samo kad je bio zadani (čista funkcija `portAfterProtocolChange(oldProto, newProto, port)`).
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL.
- [ ] **Step 3:** Implementiraj. `Ctrl+F` je `cm_connect` u `app.js` dispatch tablici.
- [ ] **Step 4:** Run `node --test ui/ && cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(ui): Connect to server dialog, connection editor, connect flow`.

### Task 12: Arhiva na serveru, README, build i ručna provjera

**Files:** Modify `src-tauri/src/commands.rs`, `ui/panel.js`, `README.md`, `docs/ROADMAP.md`

**Interfaces:**
- Consumes: Task 5 (`materialize`), `EditWatch` iz plana 2.
- Produces:
  - Enter na arhivi na serveru (`archiveKindOf` imena): nova komanda `open_remote_archive(location) -> String` preuzme arhivu kao F4 kopiju (`EditWatch::open_for_edit`, praćena) i vraća lokalnu putanju. Panel ulazi u nju.
  - Promjena arhive (F5 unutra, F8, F2…) mijenja praćenu kopiju, pa postojeći `edit://changed` upit nudi upload natrag na server.
  - `[..]` u korijenu takve arhive vraća na mapu na serveru, ne u temp mapu: panel pamti `{tempArchive → remote parent}` u memoriji.
  - README dobiva odjeljak "Serveri" (Ctrl+F, protokoli, ključevi i import, host key, gdje se spremaju lozinke u portable i sistemskom načinu, Ctrl+Shift+F, Connection lost).
  - ROADMAP: plan 3 se miče iz "Sljedeće", a otvorene stavke se upisuju (npr. FTP encoding ako je ostao samo UTF-8).

- [ ] **Step 1: test** `remote_archive_parent_returns_to_server` u `ui/lib.test.mjs` (čista funkcija `archiveParent(location, remoteArchives)`).
- [ ] **Step 2:** Run `node --test ui/`. Expected: FAIL; zatim implementacija i PASS.
- [ ] **Step 3:** Run cijeli paket: `cargo test --workspace && cargo clippy --workspace -- -D warnings && node --test ui/`. Expected: PASS.
- [ ] **Step 4:** Universal build: `cargo tauri build --target universal-apple-darwin`, zatim `lipo -archs` na binaryju. Expected: `x86_64 arm64`. Novi `.dmg` ide u `~/Documents/PVT/fxfm-builds/macos/`, a stari u Trash.
- [ ] **Step 5:** Ručna provjera (korisnik; s pravim serverom u LAN-u). Bez pisanja u korisnikove produkcijske mape, samo u testnu mapu na serveru.
  1. Ctrl+F → New connection (SFTP, ključ) → Connect → host key upit → panel na serveru, ☁️ u traci.
  2. F5 php datoteke na server i natrag, F6, F7, F8, F2; chmod +x na `.sh`.
  3. F4 na datoteci na serveru → izmjena u editoru → upit za upload → sadržaj na serveru promijenjen, prava ostala.
  4. Ugasiti VPN usred kopiranja → greška najkasnije za 60 s, Connection lost → Reconnect.
  5. Portable način: Save password → master lozinka → restart → lozinka traži samo master lozinku.
  6. Hotlista `cd sftp://<ime>/var/www` spaja i otvara.
- [ ] **Step 6:** Commit `docs: servers in README, roadmap update`.
