//! Secret storage: an in-memory store and an encrypted vault file (portable mode).
//!
//! The vault file (`secrets.json`) keeps account names in the clear and encrypts every
//! value with XChaCha20-Poly1305 under a key derived from the master password with
//! Argon2id. The account name is the AAD of its record, so records cannot be swapped.

use crate::storage;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use zeroize::Zeroizing;

pub const VAULT_FILE: &str = "secrets.json";

#[cfg(not(test))]
const KDF_M_KIB: u32 = 65536;
#[cfg(test)]
const KDF_M_KIB: u32 = 8 * 1024;
const KDF_T: u32 = 3;
const KDF_P: u32 = 1;

const NONCE_LEN: usize = 24;
const CHECK_AAD: &[u8] = b"check";
const CHECK_PLAINTEXT: &[u8] = b"fxfm";

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// Deleting a nonexistent account is Ok.
    fn delete(&self, account: &str) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    Passphrase,
}

/// Account name for a server secret: `<id>/password` or `<id>/passphrase`.
pub fn account(server_id: &str, kind: SecretKind) -> String {
    let k = match kind {
        SecretKind::Password => "password",
        SecretKind::Passphrase => "passphrase",
    };
    format!("{server_id}/{k}")
}

/// Secrets held in memory only (tests, and entered-but-unsaved passwords).
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

/// Copy the listed accounts that exist in `from` into `to`; returns how many were copied.
pub fn migrate_secrets(
    from: &dyn SecretStore,
    to: &dyn SecretStore,
    accounts: &[String],
) -> Result<usize, String> {
    let mut n = 0;
    for a in accounts {
        if let Some(s) = from.get(a)? {
            to.set(a, &s)?;
            n += 1;
        }
    }
    Ok(n)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    #[error("Wrong master password")]
    WrongPassword,
    #[error("Password file is damaged: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Io(String),
}

#[derive(Serialize, Deserialize, Clone)]
struct Kdf {
    alg: String,
    m: u32,
    t: u32,
    p: u32,
    salt: String,
}

#[derive(Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    kdf: Kdf,
    check: String,
    items: BTreeMap<String, String>,
}

/// Derives the key with the parameters stored in `kdf`.
fn derive_key(master: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>, String> {
    if kdf.alg != "argon2id" {
        return Err(format!("unsupported KDF {}", kdf.alg));
    }
    let salt = B64
        .decode(&kdf.salt)
        .map_err(|e| format!("bad salt: {e}"))?;
    let params = Params::new(kdf.m, kdf.t, kdf.p, Some(32))
        .map_err(|e| format!("bad KDF parameters: {e}"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(master.as_bytes(), &salt, &mut *key)
        .map_err(|e| format!("key derivation failed: {e}"))?;
    Ok(key)
}

fn encrypt(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<String, String> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = XChaCha20Poly1305::new(key.into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| "encryption failed".to_string())?;
    let mut out = nonce.to_vec();
    out.extend(ct);
    Ok(B64.encode(out))
}

/// `Err(None)` = authentication failed; `Err(Some(_))` = malformed encoding.
fn decrypt(key: &[u8; 32], aad: &[u8], encoded: &str) -> Result<Vec<u8>, Option<String>> {
    let raw = B64
        .decode(encoded)
        .map_err(|e| Some(format!("bad base64: {e}")))?;
    if raw.len() < NONCE_LEN {
        return Err(Some("record too short".into()));
    }
    let (nonce, ct) = raw.split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| None)
}

fn read_file(dir: &Path) -> Result<VaultFile, VaultError> {
    let text =
        std::fs::read_to_string(dir.join(VAULT_FILE)).map_err(|e| VaultError::Io(e.to_string()))?;
    let f: VaultFile =
        serde_json::from_str(&text).map_err(|e| VaultError::Corrupt(e.to_string()))?;
    if f.version != 1 {
        return Err(VaultError::Corrupt(format!(
            "unsupported version {}",
            f.version
        )));
    }
    Ok(f)
}

fn new_kdf(master: &str) -> io::Result<(Kdf, Zeroizing<[u8; 32]>)> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let kdf = Kdf {
        alg: "argon2id".into(),
        m: KDF_M_KIB,
        t: KDF_T,
        p: KDF_P,
        salt: B64.encode(salt),
    };
    let key = derive_key(master, &kdf).map_err(io::Error::other)?;
    Ok((kdf, key))
}

/// An unlocked vault. The derived key lives in memory only (zeroized on drop); the master
/// password is never stored. Every operation re-reads the file, so on-disk damage is noticed.
pub struct Vault {
    dir: PathBuf,
    key: Zeroizing<[u8; 32]>,
    io: Mutex<()>,
}

impl Vault {
    pub fn exists(data_dir: &Path) -> bool {
        data_dir.join(VAULT_FILE).is_file()
    }

    pub fn create(data_dir: &Path, master: &str) -> io::Result<Vault> {
        if Self::exists(data_dir) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "password file already exists",
            ));
        }
        let (kdf, key) = new_kdf(master)?;
        let check = encrypt(&key, CHECK_AAD, CHECK_PLAINTEXT).map_err(io::Error::other)?;
        let file = VaultFile {
            version: 1,
            kdf,
            check,
            items: BTreeMap::new(),
        };
        storage::save_json(data_dir, VAULT_FILE, &file)?;
        Ok(Vault {
            dir: data_dir.to_path_buf(),
            key,
            io: Mutex::new(()),
        })
    }

    pub fn unlock(data_dir: &Path, master: &str) -> Result<Vault, VaultError> {
        let file = read_file(data_dir)?;
        let key = derive_key(master, &file.kdf).map_err(VaultError::Corrupt)?;
        match decrypt(&key, CHECK_AAD, &file.check) {
            Ok(p) if p == CHECK_PLAINTEXT => {}
            Ok(_) => return Err(VaultError::Corrupt("check value mismatch".into())),
            Err(None) => return Err(VaultError::WrongPassword),
            Err(Some(e)) => return Err(VaultError::Corrupt(e)),
        }
        Ok(Vault {
            dir: data_dir.to_path_buf(),
            key,
            io: Mutex::new(()),
        })
    }

    /// New salt and key; every item is re-encrypted; the whole file is written atomically.
    pub fn change_master(&mut self, new_master: &str) -> io::Result<()> {
        let _g = self.io.lock().unwrap();
        let old = self.read_current().map_err(io::Error::other)?;
        let (kdf, key) = new_kdf(new_master)?;
        let mut items = BTreeMap::new();
        for (acct, enc) in &old.items {
            let plain = decrypt(&self.key, acct.as_bytes(), enc).map_err(|_| {
                io::Error::other(format!("Password file is damaged: record {acct}"))
            })?;
            items.insert(
                acct.clone(),
                encrypt(&key, acct.as_bytes(), &plain).map_err(io::Error::other)?,
            );
        }
        let check = encrypt(&key, CHECK_AAD, CHECK_PLAINTEXT).map_err(io::Error::other)?;
        let file = VaultFile {
            version: 1,
            kdf,
            check,
            items,
        };
        storage::save_json(&self.dir, VAULT_FILE, &file)?;
        self.key = key;
        Ok(())
    }
}

impl Vault {
    /// Reads the file and checks that it still belongs to this vault's key.
    fn read_current(&self) -> Result<VaultFile, String> {
        let file = read_file(&self.dir).map_err(|e| e.to_string())?;
        match decrypt(&self.key, CHECK_AAD, &file.check) {
            Ok(p) if p == CHECK_PLAINTEXT => Ok(file),
            _ => Err("Password file was changed; unlock again".to_string()),
        }
    }
}

impl SecretStore for Vault {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let _g = self.io.lock().unwrap();
        let file = self.read_current()?;
        let Some(enc) = file.items.get(account) else {
            return Ok(None);
        };
        let plain = decrypt(&self.key, account.as_bytes(), enc).map_err(|e| {
            let why = e.unwrap_or_else(|| format!("record {account} failed authentication"));
            VaultError::Corrupt(why).to_string()
        })?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|_| VaultError::Corrupt(format!("record {account} is not text")).to_string())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        let _g = self.io.lock().unwrap();
        let mut file = self.read_current()?;
        let enc = encrypt(&self.key, account.as_bytes(), secret.as_bytes())?;
        file.items.insert(account.into(), enc);
        storage::save_json(&self.dir, VAULT_FILE, &file).map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let _g = self.io.lock().unwrap();
        let mut file = self.read_current()?;
        if file.items.remove(account).is_none() {
            return Ok(());
        }
        storage::save_json(&self.dir, VAULT_FILE, &file).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::fs;

    fn read_json(dir: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(dir.join(VAULT_FILE)).unwrap()).unwrap()
    }

    #[test]
    fn vault_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        assert!(!Vault::exists(d.path()));
        let v = Vault::create(d.path(), "master").unwrap();
        assert!(Vault::exists(d.path()));
        assert!(Vault::create(d.path(), "master").is_err());
        v.set("a/password", "x").unwrap();
        let v2 = Vault::unlock(d.path(), "master").unwrap();
        assert_eq!(v2.get("a/password").unwrap(), Some("x".to_string()));
        assert_eq!(v2.get("b/password").unwrap(), None);
        v2.delete("a/password").unwrap();
        v2.delete("missing").unwrap();
        assert_eq!(v2.get("a/password").unwrap(), None);
    }

    #[test]
    fn vault_wrong_master_password_keeps_file() {
        let d = tempfile::tempdir().unwrap();
        Vault::create(d.path(), "master")
            .unwrap()
            .set("a/password", "x")
            .unwrap();
        let before = fs::read(d.path().join(VAULT_FILE)).unwrap();
        let err = Vault::unlock(d.path(), "wrong").err().unwrap();
        assert!(matches!(err, VaultError::WrongPassword));
        assert_eq!(err.to_string(), "Wrong master password");
        assert_eq!(before, fs::read(d.path().join(VAULT_FILE)).unwrap());
    }

    #[test]
    fn vault_tampered_file_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let v = Vault::create(d.path(), "master").unwrap();
        v.set("a/password", "x").unwrap();
        v.set("b/password", "y").unwrap();
        let orig = read_json(d.path());

        // Flip a byte of a ciphertext.
        let mut j = orig.clone();
        let mut raw = B64
            .decode(j["items"]["a/password"].as_str().unwrap())
            .unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        j["items"]["a/password"] = B64.encode(raw).into();
        fs::write(d.path().join(VAULT_FILE), j.to_string()).unwrap();
        let e = v.get("a/password").unwrap_err();
        assert!(e.contains("damaged"), "{e}");

        // Swap two items: AAD (account name) must catch it.
        let mut j = orig.clone();
        let (a, b) = (
            j["items"]["a/password"].clone(),
            j["items"]["b/password"].clone(),
        );
        j["items"]["a/password"] = b;
        j["items"]["b/password"] = a;
        fs::write(d.path().join(VAULT_FILE), j.to_string()).unwrap();
        assert!(v.get("a/password").is_err());
        assert!(v.get("b/password").is_err());

        // Garbage file: unlock reports Corrupt.
        fs::write(d.path().join(VAULT_FILE), "not json").unwrap();
        let e = Vault::unlock(d.path(), "master").err().unwrap();
        assert!(matches!(e, VaultError::Corrupt(_)));
        assert!(e.to_string().starts_with("Password file is damaged: "));
    }

    #[test]
    fn vault_change_master() {
        let d = tempfile::tempdir().unwrap();
        let mut v = Vault::create(d.path(), "old").unwrap();
        v.set("a/password", "x").unwrap();
        let salt_before = read_json(d.path())["kdf"]["salt"].clone();
        v.change_master("n").unwrap();
        assert_ne!(salt_before, read_json(d.path())["kdf"]["salt"]);
        assert!(matches!(
            Vault::unlock(d.path(), "old").err().unwrap(),
            VaultError::WrongPassword
        ));
        let v2 = Vault::unlock(d.path(), "n").unwrap();
        assert_eq!(v2.get("a/password").unwrap(), Some("x".to_string()));
        // The old handle keeps working with the new key.
        v.set("b/password", "y").unwrap();
        assert_eq!(v2.get("b/password").unwrap(), Some("y".to_string()));
    }

    #[test]
    fn vault_plaintext_not_in_file() {
        let d = tempfile::tempdir().unwrap();
        let v = Vault::create(d.path(), "master-unique-pw").unwrap();
        v.set("a/password", "hunter2-unique").unwrap();
        let text = fs::read_to_string(d.path().join(VAULT_FILE)).unwrap();
        assert!(!text.contains("hunter2-unique"));
        assert!(!text.contains("master-unique-pw"));
        assert!(text.contains("a/password"));
        assert!(text.contains("argon2id"));
    }

    #[test]
    fn migrate_secrets_copies_existing_only() {
        let from = MemoryStore::default();
        from.set("a/password", "1").unwrap();
        from.set("b/passphrase", "2").unwrap();
        let to = MemoryStore::default();
        let accounts: Vec<String> = ["a/password", "b/passphrase", "c/password"]
            .map(String::from)
            .into();
        assert_eq!(migrate_secrets(&from, &to, &accounts).unwrap(), 2);
        assert_eq!(to.get("b/passphrase").unwrap(), Some("2".to_string()));
        assert_eq!(to.get("c/password").unwrap(), None);
    }

    #[test]
    fn account_names() {
        assert_eq!(account("id1", SecretKind::Password), "id1/password");
        assert_eq!(account("id1", SecretKind::Passphrase), "id1/passphrase");
    }

    #[test]
    fn vault_detects_file_replaced_by_other_handle() {
        let d = tempfile::tempdir().unwrap();
        let mut a = Vault::create(d.path(), "old").unwrap();
        a.set("a/password", "x").unwrap();
        let mut b = Vault::unlock(d.path(), "old").unwrap();
        b.change_master("new").unwrap();
        let before = fs::read(d.path().join(VAULT_FILE)).unwrap();
        let msg = "Password file was changed; unlock again";
        assert_eq!(a.set("c/password", "z").unwrap_err(), msg);
        assert_eq!(a.delete("a/password").unwrap_err(), msg);
        assert_eq!(a.get("a/password").unwrap_err(), msg);
        assert_eq!(a.change_master("other").unwrap_err().to_string(), msg);
        assert_eq!(before, fs::read(d.path().join(VAULT_FILE)).unwrap());
    }
}
