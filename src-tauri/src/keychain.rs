//! Server passwords in the system's keychain: on macOS the Keychain (generic passwords of
//! service `fxfm`, account `<server id>/password` or `<server id>/passphrase`), on Linux the
//! Secret Service (GNOME Keyring, KWallet…) through `secret-tool` with the attributes
//! `service fxfm` and `account <…>`. Exercised manually only; tests use `MemoryStore` instead.

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

/// Linux: `secret-tool` (libsecret-tools, a dependency of the package). The secret travels on
/// stdin and stdout only, never on a command line; error texts name the account, not the secret.
#[cfg(target_os = "linux")]
mod imp {
    use super::{KeychainStore, SERVICE};
    use fxfm_core::secrets::SecretStore;
    use std::io::Write;
    use std::process::{Command, Output, Stdio};

    fn tool(args: &[&str], input: Option<&str>) -> Result<Output, String> {
        let mut child = Command::new("secret-tool")
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    "Secret Service: secret-tool is missing (install libsecret-tools)".to_string()
                }
                _ => format!("Secret Service: {e}"),
            })?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("Secret Service: {e}"))?;
        }
        child
            .wait_with_output()
            .map_err(|e| format!("Secret Service: {e}"))
    }

    fn failure(out: &Output) -> String {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        format!(
            "Secret Service: {}",
            if msg.is_empty() { "failed".into() } else { msg }
        )
    }

    impl SecretStore for KeychainStore {
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            let out = tool(&["lookup", "service", SERVICE, "account", account], None)?;
            if out.status.success() {
                return String::from_utf8(out.stdout)
                    .map(Some)
                    .map_err(|_| format!("Secret Service item {account} is not text"));
            }
            // Not found: exit 1 without a message.
            if out.stderr.is_empty() {
                Ok(None)
            } else {
                Err(failure(&out))
            }
        }

        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            let label = format!("fxfm: {account}");
            let out = tool(
                &[
                    "store", "--label", &label, "service", SERVICE, "account", account,
                ],
                Some(secret),
            )?;
            if out.status.success() {
                Ok(())
            } else {
                Err(failure(&out))
            }
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            let out = tool(&["clear", "service", SERVICE, "account", account], None)?;
            // Clearing what is not there is fine.
            if out.status.success() || out.stderr.is_empty() {
                Ok(())
            } else {
                Err(failure(&out))
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
