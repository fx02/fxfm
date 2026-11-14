//! Accepted host keys and FTPS certificates. One line per `host:port`:
//! `<host>:<port> <key_type> <fingerprint>`. Blank lines and `#` comments are kept.

use std::fs;
use std::io;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use super::ConnectError;

pub const KNOWN_HOSTS_FILE: &str = "known_hosts";

#[derive(Debug, Clone, PartialEq)]
pub enum HostCheck {
    Known,
    Unknown,
    Changed { old_fingerprint: String },
}

#[derive(Clone, Debug)]
pub struct KnownHosts {
    path: PathBuf,
}

/// `(host:port, key_type, fingerprint)` of a data line.
fn parse_line(line: &str) -> Option<(&str, &str, &str)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let mut it = t.split_whitespace();
    Some((it.next()?, it.next()?, it.next()?))
}

impl KnownHosts {
    pub fn new(path: PathBuf) -> KnownHosts {
        KnownHosts { path }
    }

    fn read(&self) -> String {
        fs::read_to_string(&self.path).unwrap_or_default()
    }

    pub fn check(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> HostCheck {
        let id = format!("{host}:{port}");
        let content = self.read();
        for line in content.lines() {
            if let Some((h, t, f)) = parse_line(line) {
                if h == id {
                    return if t == key_type && f == fingerprint {
                        HostCheck::Known
                    } else {
                        HostCheck::Changed {
                            old_fingerprint: f.to_string(),
                        }
                    };
                }
            }
        }
        HostCheck::Unknown
    }

    /// Records the key, replacing an older entry of the same `host:port`.
    pub fn trust(
        &self,
        host: &str,
        port: u16,
        key_type: &str,
        fingerprint: &str,
    ) -> io::Result<()> {
        let id = format!("{host}:{port}");
        let new_line = format!("{id} {key_type} {fingerprint}");
        let content = self.read();
        let mut out = String::new();
        let mut replaced = false;
        for line in content.lines() {
            match parse_line(line) {
                Some((h, _, _)) if h == id => {
                    if !replaced {
                        out.push_str(&new_line);
                        out.push('\n');
                        replaced = true;
                    }
                }
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if !replaced {
            out.push_str(&new_line);
            out.push('\n');
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, out)?;
        fs::rename(&tmp, &self.path)
    }
}

/// Trust-on-first-use decision for a host key or certificate: `Ok` when it is known, or
/// when the user accepted exactly this fingerprint (then it is recorded); otherwise the
/// `HostKey` question for the user (`changed_from` set when another key was stored).
pub(crate) fn verify_tofu(
    known: &KnownHosts,
    host: &str,
    port: u16,
    key_type: &str,
    fingerprint: &str,
    accept: Option<&str>,
) -> Result<(), ConnectError> {
    let changed_from = match known.check(host, port, key_type, fingerprint) {
        HostCheck::Known => return Ok(()),
        HostCheck::Unknown => None,
        HostCheck::Changed { old_fingerprint } => Some(old_fingerprint),
    };
    if accept == Some(fingerprint) {
        return known.trust(host, port, key_type, fingerprint).map_err(|e| {
            ConnectError::Failed(format!("Cannot save the host key to known_hosts: {e}"))
        });
    }
    Err(ConnectError::HostKey {
        host: host.to_string(),
        port,
        key_type: key_type.to_string(),
        fingerprint: fingerprint.to_string(),
        changed_from,
    })
}

/// `SHA256:` + unpadded base64 of the SHA-256 of `blob`, like `ssh-keygen -l`.
pub fn fingerprint_sha256(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_check_and_trust() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join(KNOWN_HOSTS_FILE);
        let kh = KnownHosts::new(p.clone());
        assert_eq!(
            kh.check("h", 22, "ssh-ed25519", "SHA256:a"),
            HostCheck::Unknown
        );
        fs::write(&p, "# my hosts\n\n").unwrap();
        kh.trust("h", 22, "ssh-ed25519", "SHA256:a").unwrap();
        assert_eq!(
            kh.check("h", 22, "ssh-ed25519", "SHA256:a"),
            HostCheck::Known
        );
        assert_eq!(
            kh.check("h", 22, "ssh-ed25519", "SHA256:b"),
            HostCheck::Changed {
                old_fingerprint: "SHA256:a".into()
            }
        );
        assert_eq!(
            kh.check("h", 2222, "ssh-ed25519", "SHA256:a"),
            HostCheck::Unknown
        );
        kh.trust("h", 22, "ssh-ed25519", "SHA256:b").unwrap();
        kh.trust("x", 21, "x509", "SHA256:c").unwrap();
        assert_eq!(
            kh.check("h", 22, "ssh-ed25519", "SHA256:b"),
            HostCheck::Known
        );
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "# my hosts\n\nh:22 ssh-ed25519 SHA256:b\nx:21 x509 SHA256:c\n"
        );
    }

    #[test]
    fn fingerprint_matches_ssh_keygen() {
        // Throwaway ed25519 key; expected value from `ssh-keygen -lf`.
        let b64 = "AAAAC3NzaC1lZDI1NTE5AAAAID05m9r+bGDJZaelKipMuoO7lfFrKefujUUWgtVHFfqP";
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(
            fingerprint_sha256(&blob),
            "SHA256:SHFUxnyqpoLHFfXDe9l/D6KMPC9CkmzuiSx7trf5khY"
        );
    }
}
