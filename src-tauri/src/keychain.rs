//! Server passwords in the macOS Keychain: generic passwords of service `fxfm`, account
//! `<server id>/password` or `<server id>/passphrase`. Exercised manually only; tests use
//! `MemoryStore` instead. Other systems have no keychain: nothing can be stored there.

use fxfm_core::secrets::SecretStore;

pub const SERVICE: &str = "fxfm";

pub struct KeychainStore;

#[cfg(target_os = "macos")]
mod imp {
    use super::{KeychainStore, SERVICE};
    use fxfm_core::secrets::SecretStore;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    /// `errSecItemNotFound`.
    const ITEM_NOT_FOUND: i32 = -25300;

    // Error texts name the account (a server id), never the secret.
    impl SecretStore for KeychainStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            match get_generic_password(SERVICE, account) {
                Ok(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| format!("Keychain item {account} is not text")),
                Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
                Err(e) => Err(format!("Keychain: {e}")),
            }
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            set_generic_password(SERVICE, account, secret.as_bytes())
                .map_err(|e| format!("Keychain: {e}"))
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            match delete_generic_password(SERVICE, account) {
                Ok(()) => Ok(()),
                Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
                Err(e) => Err(format!("Keychain: {e}")),
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn get(&self, _account: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn set(&self, _account: &str, _secret: &str) -> Result<(), String> {
        Err("No system keychain on this platform".into())
    }

    fn delete(&self, _account: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The store of system mode.
pub fn system_store() -> std::sync::Arc<dyn SecretStore> {
    std::sync::Arc::new(KeychainStore)
}
