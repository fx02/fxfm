//! FTP and FTPS backend (suppaftp, blocking sockets).
//!
//! One control connection behind a `Mutex`; `fork()` logs in again, because FTP has a
//! single control channel and a long transfer would otherwise block panel listings.
//! FTPS is explicit TLS (`AUTH TLS`) with rustls (ring). The server certificate is trusted
//! on first use through `known_hosts` (`x509`, SHA-256 of the DER), like an SSH host key,
//! so self-signed LAN certificates work; it is checked before any credentials are sent.
//! Every socket carries read/write timeouts, so no call waits longer than
//! `REQUEST_TIMEOUT` without progress, and connecting is bounded by `CONNECT_TIMEOUT`.

#[cfg(test)]
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, TryLockError, Weak};
use std::time::{Duration, Instant, UNIX_EPOCH};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use suppaftp::list::{File as ListFile, ListParser, PosixPexQuery};
use suppaftp::types::{FileType, Response};
use suppaftp::{
    Encoding, FtpError, Mode, RustlsConnector, RustlsFtpStream, Status, TlsStream, TransferStream,
};

use super::known_hosts::{fingerprint_sha256, verify_tofu, KnownHosts};
use super::{
    display_url, parent_path, ConnectError, ConnectOptions, Credentials, RemoteError, RemoteFs,
    RemoteStat, CONNECT_TIMEOUT, REQUEST_TIMEOUT,
};
use crate::listing::Entry;
use crate::servers::{FtpEncoding, Protocol, ServerEntry};

/// Bytes per read/write call on a data connection.
const BLOCK: usize = 64 * 1024;
/// How long `close` waits for the server to answer `QUIT`.
const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
/// A `NOOP` is sent when the control connection was idle this long, so servers do not close it.
const KEEPALIVE: Duration = Duration::from_secs(30);
/// Wait for the keepalive `NOOP` reply: short, because the connection is locked meanwhile.
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(5);
const KEY_TYPE_X509: &str = "x509";
const NO_CHMOD: &str = "This server does not support changing permissions";
const NO_SYMLINKS: &str = "Symbolic links are not supported over FTP";

// ---------------------------------------------------------------------------------------
// Name encoding
// ---------------------------------------------------------------------------------------

/// Encoding of names on the control and data connections. suppaftp hands every command
/// line and listing line through it as raw bytes. The single-byte encodings map all 256
/// bytes, so every name round-trips; a name the encoding cannot represent is refused
/// (`InvalidInput`) before anything is sent, never altered.
#[derive(Debug, Clone, Copy)]
struct Codec(Option<&'static encoding_rs::Encoding>);

impl Codec {
    fn new(e: FtpEncoding) -> Codec {
        Codec(match e {
            FtpEncoding::Utf8 => None,
            FtpEncoding::Windows1250 => Some(encoding_rs::WINDOWS_1250),
            // What "Latin-1" means in practice (WHATWG): Windows-1252, a superset.
            FtpEncoding::Latin1 => Some(encoding_rs::WINDOWS_1252),
        })
    }
}

impl Encoding for Codec {
    fn decode(&self, bytes: &[u8]) -> String {
        match self.0 {
            None => String::from_utf8_lossy(bytes).into_owned(),
            Some(enc) => enc.decode_without_bom_handling(bytes).0.into_owned(),
        }
    }

    fn encode(&self, text: &str) -> io::Result<Vec<u8>> {
        match self.0 {
            None => Ok(text.as_bytes().to_vec()),
            Some(enc) => {
                let (bytes, _, unmappable) = enc.encode(text);
                if unmappable {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("the name cannot be written in {}", enc.name()),
                    ))
                } else {
                    Ok(bytes.into_owned())
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// FTPS certificate check
// ---------------------------------------------------------------------------------------

/// Trust on first use for the server certificate (control and data connections).
/// Signatures are still verified, so the server must own the certificate's key.
#[derive(Debug)]
struct TofuVerifier {
    host: String,
    port: u16,
    known: KnownHosts,
    accept: Option<String>,
    provider: Arc<CryptoProvider>,
    /// Why the handshake was refused, when this verifier refused it.
    verdict: Arc<Mutex<Option<ConnectError>>>,
    /// Set once a certificate was presented: TLS is up, `AUTH TLS` was accepted.
    handshake: Arc<AtomicBool>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.handshake.store(true, Ordering::SeqCst);
        let fingerprint = fingerprint_sha256(end_entity.as_ref());
        match verify_tofu(
            &self.known,
            &self.host,
            self.port,
            KEY_TYPE_X509,
            &fingerprint,
            self.accept.as_deref(),
        ) {
            Ok(()) => Ok(ServerCertVerified::assertion()),
            Err(e) => {
                *self.verdict.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                Err(rustls::Error::General(
                    "the server certificate is not trusted".into(),
                ))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_connector(verifier: TofuVerifier) -> Result<RustlsConnector, ConnectError> {
    let config = ClientConfig::builder_with_provider(verifier.provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ConnectError::Failed(format!("TLS: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(RustlsConnector::from(Arc::new(config)))
}

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

/// The three-digit code of a reply (also codes suppaftp does not name).
fn reply_code(r: &Response) -> u32 {
    let b = &r.body;
    if b.len() >= 3 && b[..3].iter().all(u8::is_ascii_digit) {
        b[..3].iter().fold(0, |n, d| n * 10 + u32::from(d - b'0'))
    } else {
        r.status.code()
    }
}

/// The text of a reply's first line, without the code.
fn reply_text(r: &Response) -> String {
    let text = String::from_utf8_lossy(&r.body);
    let line = text.lines().next().unwrap_or("").trim();
    let line = match line.get(..4) {
        Some(p) if p[..3].bytes().all(|b| b.is_ascii_digit()) => &line[4..],
        _ => line,
    };
    line.trim().to_string()
}

/// `(code, text)` when the server refused a command; `None` for anything else
/// (including 421, which closes the connection).
fn refusal(e: &FtpError) -> Option<(u32, String)> {
    match e {
        FtpError::UnexpectedResponse(r) if reply_code(r) != 421 => {
            Some((reply_code(r), reply_text(r)))
        }
        _ => None,
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn looks_denied(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    ["permission", "denied", "access", "not allowed"]
        .iter()
        .any(|w| m.contains(w))
}

fn connect_err(e: FtpError) -> ConnectError {
    match e {
        FtpError::ConnectionError(io) if is_timeout(&io) => ConnectError::Timeout,
        FtpError::UnexpectedResponse(r) => {
            ConnectError::Failed(format!("The server refused: {}", reply_text(&r)))
        }
        e => ConnectError::Failed(e.to_string()),
    }
}

fn connect_error_text(e: ConnectError) -> RemoteError {
    match e {
        ConnectError::Timeout => RemoteError::Timeout,
        ConnectError::Failed(m) => RemoteError::Other(m),
        ConnectError::AuthFailed => RemoteError::Other("The server refused the login".into()),
        ConnectError::HostKey { .. } => {
            RemoteError::Other("The server certificate is no longer trusted".into())
        }
        ConnectError::NeedPassword | ConnectError::NeedPassphrase | ConnectError::BadPassphrase => {
            RemoteError::Other("A password is needed to log in again".into())
        }
    }
}

// ---------------------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------------------

/// True when an MLSD/MLST line carries a `UNIX.mode` fact (otherwise suppaftp reports
/// 0777, which must not be taken for the real mode).
fn mlsx_has_mode(line: &str) -> bool {
    match line.find("; ") {
        Some(end) if !line.starts_with(' ') => line[..end]
            .split(';')
            .any(|f| f.trim().to_ascii_lowercase().starts_with("unix.mode=")),
        _ => false,
    }
}

/// `type=cdir`/`type=pdir` entries of an MLSD listing (the folder and its parent).
fn mlsx_is_self_or_parent(line: &str) -> bool {
    let facts = line.split("; ").next().unwrap_or("").to_ascii_lowercase();
    facts
        .split(';')
        .any(|f| f == "type=cdir" || f == "type=pdir")
}

/// Parses an MLSD (or, with `mlst`, an MLST) line and says whether it carried a real
/// mode. Facts suppaftp rejects (`type=OS.unix=symlink`, an unusual `UNIX.mode`) are
/// reduced to type, size and time instead of losing the entry.
fn parse_mlsx(line: &str, mlst: bool) -> Option<(ListFile, bool)> {
    let parse = |l: &str| {
        if mlst {
            ListParser::parse_mlst(l)
        } else {
            ListParser::parse_mlsd(l)
        }
    };
    if let Ok(f) = parse(line) {
        return Some((f, mlsx_has_mode(line)));
    }
    let (facts, name) = line.trim_start().split_once("; ")?;
    let kept: Vec<String> = facts
        .split(';')
        .filter_map(|fact| {
            let (k, v) = fact.split_once('=')?;
            match k.to_ascii_lowercase().as_str() {
                "type" => {
                    let v = v.to_ascii_lowercase();
                    Some(if v.contains("link") {
                        "type=link".to_string()
                    } else if v.contains("dir") {
                        "type=dir".to_string()
                    } else {
                        "type=file".to_string()
                    })
                }
                "size" | "modify" => Some(fact.to_string()),
                _ => None,
            }
        })
        .collect();
    let simple = if kept.is_empty() {
        format!(" {name}")
    } else {
        format!("{}; {name}", kept.join(";"))
    };
    parse(&simple).ok().map(|f| (f, false))
}

fn stat_of(f: &ListFile, mode_known: bool) -> RemoteStat {
    let mode = if mode_known {
        let mut m = 0;
        for (shift, who) in [
            (6, PosixPexQuery::Owner),
            (3, PosixPexQuery::Group),
            (0, PosixPexQuery::Others),
        ] {
            m |= (u32::from(f.can_read(who)) << 2
                | u32::from(f.can_write(who)) << 1
                | u32::from(f.can_execute(who)))
                << shift;
        }
        m | if f.is_directory() {
            0o040000
        } else if f.is_symlink() {
            0o120000
        } else {
            0o100000
        }
    } else {
        0
    };
    RemoteStat {
        is_dir: f.is_directory(),
        is_symlink: f.is_symlink(),
        size: f.size() as u64,
        mtime: f
            .modified()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        mode,
    }
}

fn entry_of(f: &ListFile, mode_known: bool) -> Entry {
    let st = stat_of(f, mode_known);
    Entry {
        name: f.name().to_string(),
        hidden: f.name().starts_with('.'),
        is_dir: st.is_dir,
        is_symlink: st.is_symlink,
        size: st.size,
        mtime: st.mtime,
        mode: st.mode,
    }
}

/// Parses MLSD lines, or `LIST` lines (Unix `ls -l` or DOS format).
fn parse_listing(lines: &[String], mlsd: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in lines {
        let parsed = if mlsd {
            if mlsx_is_self_or_parent(line) {
                continue;
            }
            parse_mlsx(line, false)
        } else {
            ListParser::parse_posix(line)
                .map(|f| (f, true))
                .or_else(|_| ListParser::parse_dos(line).map(|f| (f, false)))
                .ok()
        };
        match parsed {
            Some((f, _)) if f.name() == "." || f.name() == ".." || f.name().is_empty() => {}
            Some((f, mode_known)) => out.push(entry_of(&f, mode_known)),
            // `total 12` and other lines that are not entries.
            None => log::debug!("skipping listing line {line:?}"),
        }
    }
    out
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

// ---------------------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------------------

fn set_timeouts(s: &TcpStream, t: Duration) {
    let _ = s.set_read_timeout(Some(t));
    let _ = s.set_write_timeout(Some(t));
}

/// What `FtpFs` needs to log in again (`fork`).
struct Config {
    server: ServerEntry,
    creds: Credentials,
    known: KnownHosts,
}

/// One logged-in control connection.
struct Conn {
    ftp: RustlsFtpStream,
    codec: Codec,
    mlsd: bool,
    mlst: bool,
    /// The server accepts `LIST -a` (hidden files).
    list_a: bool,
    /// `SITE CHMOD` is known to be unimplemented.
    no_chmod: bool,
    /// Test seam: each RNTO pops one entry; `true` answers it with a refusal
    /// without sending it.
    #[cfg(test)]
    rnto_script: VecDeque<bool>,
    /// Test: `set_mode` calls.
    #[cfg(test)]
    chmod_calls: usize,
    /// The connection state is unknown (timeout, I/O error, 421): nothing more is sent.
    broken: bool,
    /// End of the last command (keepalive: only an idle connection gets a `NOOP`).
    last_used: Instant,
}

enum RenameErr {
    From(FtpError),
    To(FtpError),
}

static BAK_N: AtomicU64 = AtomicU64::new(0);

/// `.<name>.fxfm-<pid>-<n>.bak` next to `target`.
pub(super) fn backup_path(target: &str) -> String {
    let n = BAK_N.fetch_add(1, Ordering::Relaxed);
    super::join_path(
        parent_path(target),
        &format!(".{}.fxfm-{}-{n}.bak", base_name(target), std::process::id()),
    )
}

/// A data command failed while opening (`Open`, server refusal or I/O) or later (`Done`,
/// already mapped).
enum DataErr {
    Open(FtpError),
    Done(RemoteError),
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, ConnectError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| ConnectError::Failed(format!("{host}: {e}")))?
        .collect();
    let mut last = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(t) => return Ok(t),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) if is_timeout(&e) => ConnectError::Timeout,
        Some(e) => ConnectError::Failed(format!("{host}:{port}: {e}")),
        None => ConnectError::Failed(format!("{host}: no address")),
    })
}

/// The FEAT labels (upper case). suppaftp's `feat()` waits for a second line after a
/// one-line `211 ` reply (a server without extensions), so the reply is read here.
fn features(ftp: &mut RustlsFtpStream) -> Result<Vec<String>, FtpError> {
    // 211 is left out of the expected codes on purpose: with it, suppaftp stops reading
    // after the `211-` opener. Any other code arrives complete as `UnexpectedResponse`.
    let r = match ftp.custom_command("FEAT", &[Status::CommandOk]) {
        Ok(r) => r,
        Err(FtpError::UnexpectedResponse(r)) => r,
        Err(e) => return Err(e),
    };
    if reply_code(&r) != 211 {
        // No FEAT: an old server without extensions.
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&r.body)
        .lines()
        .skip(1)
        .filter(|l| l.starts_with(' '))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_ascii_uppercase)
        .collect())
}

/// Connects, secures (FTPS), logs in and prepares the session. Blocking; the caller
/// bounds it with `CONNECT_TIMEOUT`.
fn login(cfg: &Config, accept: Option<&str>) -> Result<Conn, ConnectError> {
    let s = &cfg.server;
    let password = cfg
        .creds
        .password
        .clone()
        .ok_or(ConnectError::NeedPassword)?;
    let tcp = connect_tcp(&s.host, s.port)?;
    set_timeouts(&tcp, CONNECT_TIMEOUT);
    let _ = tcp.set_nodelay(true);
    let peer = tcp
        .peer_addr()
        .map_err(|e| ConnectError::Failed(e.to_string()))?;
    let mut ftp = RustlsFtpStream::connect_with_stream(tcp).map_err(connect_err)?;
    let codec = Codec::new(s.ftp_encoding);
    ftp.set_encoding(codec);
    if s.ftp_passive {
        ftp.set_mode(if peer.is_ipv6() {
            Mode::ExtendedPassive
        } else {
            Mode::Passive
        });
        // Servers behind NAT announce their private address; connect to the one we use.
        ftp.set_passive_nat_workaround(true);
        ftp = ftp.passive_stream_builder(|addr| {
            let t = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
                .map_err(FtpError::ConnectionError)?;
            set_timeouts(&t, REQUEST_TIMEOUT);
            Ok(t)
        });
    } else {
        ftp = ftp.active_mode(REQUEST_TIMEOUT);
    }
    if s.protocol == Protocol::Ftps {
        let verdict = Arc::new(Mutex::new(None));
        let handshake = Arc::new(AtomicBool::new(false));
        let connector = tls_connector(TofuVerifier {
            host: s.host.clone(),
            port: s.port,
            known: cfg.known.clone(),
            accept: accept.map(str::to_string),
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            verdict: verdict.clone(),
            handshake: handshake.clone(),
        })?;
        // SNI only; the certificate itself is checked by TOFU.
        let domain = if ServerName::try_from(s.host.as_str()).is_ok() {
            s.host.clone()
        } else {
            peer.ip().to_string()
        };
        ftp = ftp.into_secure(connector, &domain).map_err(|e| {
            if let Some(v) = verdict.lock().unwrap_or_else(|p| p.into_inner()).take() {
                return v;
            }
            match e {
                // After the handshake only PBSZ and PROT remain.
                FtpError::UnexpectedResponse(r) if handshake.load(Ordering::SeqCst) => {
                    ConnectError::Failed(format!(
                        "The server refused to protect the data connection (PBSZ/PROT): {}",
                        reply_text(&r)
                    ))
                }
                FtpError::UnexpectedResponse(r) => ConnectError::Failed(format!(
                    "The server does not support FTPS (AUTH TLS): {}",
                    reply_text(&r)
                )),
                FtpError::ConnectionError(io) if is_timeout(&io) => ConnectError::Timeout,
                e => ConnectError::Failed(format!("TLS: {e}")),
            }
        })?;
    }
    match ftp.login(s.user.as_str(), password.as_str()) {
        Ok(()) => {}
        Err(FtpError::UnexpectedResponse(r)) if matches!(reply_code(&r), 430 | 530) => {
            return Err(ConnectError::AuthFailed)
        }
        Err(e) => return Err(connect_err(e)),
    }
    ftp.transfer_type(FileType::Binary).map_err(connect_err)?;
    let feats = features(&mut ftp).map_err(connect_err)?;
    let has = |k: &str| feats.iter().any(|f| f == k);
    if s.ftp_encoding == FtpEncoding::Utf8 && has("UTF8") {
        // Some servers answer 202 (always on) or refuse; names stay UTF-8 either way.
        if let Err(FtpError::ConnectionError(io)) = ftp.opts("UTF8", Some("ON")) {
            return Err(connect_err(FtpError::ConnectionError(io)));
        }
    }
    set_timeouts(&ftp.get_ref(), REQUEST_TIMEOUT);
    Ok(Conn {
        ftp,
        codec,
        // RFC 3659: MLST in FEAT announces MLSD as well.
        mlsd: has("MLST") || has("MLSD"),
        mlst: has("MLST"),
        list_a: true,
        no_chmod: false,
        #[cfg(test)]
        rnto_script: VecDeque::new(),
        #[cfg(test)]
        chmod_calls: 0,
        broken: false,
        last_used: Instant::now(),
    })
}

impl Conn {
    /// Maps an error that is not interpreted per command; marks the connection broken
    /// when its state is unknown.
    fn fatal(&mut self, e: FtpError, path: &str) -> RemoteError {
        match e {
            // Refused by the codec (or a CR/LF in the name) before anything was sent.
            FtpError::ConnectionError(io) if io.kind() == io::ErrorKind::InvalidInput => {
                RemoteError::Other(format!("{path}: {io}"))
            }
            FtpError::ConnectionError(io) => {
                self.broken = true;
                if is_timeout(&io) {
                    RemoteError::Timeout
                } else {
                    RemoteError::Disconnected
                }
            }
            FtpError::UnexpectedResponse(r) => match reply_code(&r) {
                421 => {
                    self.broken = true;
                    RemoteError::Disconnected
                }
                530 => RemoteError::PermissionDenied(path.to_string()),
                _ => RemoteError::Other(format!("{path}: {}", reply_text(&r))),
            },
            FtpError::SecureError(m) => {
                self.broken = true;
                RemoteError::Other(format!("TLS: {m}"))
            }
            // Bad reply syntax and the like: the control channel may be out of step.
            e => {
                self.broken = true;
                RemoteError::Other(format!("{path}: {e}"))
            }
        }
    }

    /// A refused command on a path that should exist (RETR, DELE, RMD, RNFR, MLSD).
    /// FTP answers most failures with a generic 550, so the path is looked up.
    fn refused_existing(&mut self, e: FtpError, path: &str) -> RemoteError {
        let Some((code, msg)) = refusal(&e) else {
            return self.fatal(e, path);
        };
        if matches!(code, 450 | 550) {
            match self.stat_path(path) {
                Ok(None) => return RemoteError::NotFound(path.to_string()),
                Err(e) => return e,
                Ok(Some(_)) => {}
            }
        }
        if code == 530 || looks_denied(&msg) {
            RemoteError::PermissionDenied(path.to_string())
        } else {
            RemoteError::Other(format!("{path}: {msg}"))
        }
    }

    /// A refused command that creates `path` (STOR, MKD): a missing folder is `NotFound`,
    /// otherwise the folder does not allow it.
    fn refused_create(&mut self, e: FtpError, path: &str) -> RemoteError {
        let Some((code, msg)) = refusal(&e) else {
            return self.fatal(e, path);
        };
        if matches!(code, 450 | 530 | 532 | 550 | 553) {
            return match self.stat_path(parent_path(path)) {
                Ok(Some(p)) if p.is_dir => RemoteError::PermissionDenied(path.to_string()),
                Ok(_) => RemoteError::NotFound(path.to_string()),
                Err(e) => e,
            };
        }
        RemoteError::Other(format!("{path}: {msg}"))
    }

    /// Maps a refusal of a transfer command; `create` for STOR.
    fn refused(&mut self, e: FtpError, path: &str, create: bool) -> RemoteError {
        if create {
            self.refused_create(e, path)
        } else {
            self.refused_existing(e, path)
        }
    }

    /// Completes a transfer. Some servers (libunftp) accept the command with 150 and
    /// report a missing file or a refused folder only in the completion reply.
    fn finish<S: TlsStream>(
        &mut self,
        stream: TransferStream<S>,
        path: &str,
        create: bool,
    ) -> Result<(), RemoteError> {
        stream.finish().map_err(|e| self.refused(e, path, create))
    }

    /// Ends a transfer whose data connection failed with `io`.
    fn end_failed<S: TlsStream>(
        &mut self,
        stream: TransferStream<S>,
        io: io::Error,
        path: &str,
        create: bool,
    ) -> RemoteError {
        if is_timeout(&io) {
            // Do not wait another timeout: neither for TLS close_notify on the data
            // connection (dropping the stream flushes) nor for the completion reply.
            let _ = stream.get_ref().get_ref().shutdown(Shutdown::Both);
            let _ = self.ftp.get_ref().shutdown(Shutdown::Both);
            drop(stream);
            self.broken = true;
            return RemoteError::Timeout;
        }
        // The server's reply usually says why (disk full, aborted, ...).
        match self.finish(stream, path, create) {
            Ok(()) => RemoteError::Other(format!("{path}: {io}")),
            Err(e) => e,
        }
    }

    /// Abandons a transfer (cancel or local error); the control connection stays usable.
    fn cancel<S: TlsStream>(&mut self, stream: TransferStream<S>) {
        if let Err(e) = stream.finish() {
            if refusal(&e).is_none() {
                self.fatal(e, "");
            }
        }
    }

    /// Runs a listing command and returns its decoded lines.
    fn data_lines(&mut self, cmd: &str, path: &str) -> Result<Vec<String>, DataErr> {
        let (_, mut stream) = self
            .ftp
            .custom_data_command(cmd, &[Status::AboutToSend, Status::AlreadyOpen])
            .map_err(DataErr::Open)?;
        set_timeouts(stream.get_ref().get_ref(), REQUEST_TIMEOUT);
        let mut raw: Vec<Vec<u8>> = Vec::new();
        let read = {
            let mut rd = BufReader::new(&mut stream);
            loop {
                let mut line = Vec::new();
                match rd.read_until(b'\n', &mut line) {
                    Ok(0) => break Ok(()),
                    Ok(_) => raw.push(line),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => break Err(e),
                }
            }
        };
        if let Err(io) = read {
            return Err(DataErr::Done(self.end_failed(stream, io, path, false)));
        }
        self.finish(stream, path, false).map_err(DataErr::Done)?;
        Ok(raw
            .iter()
            .map(|l| {
                let l = l.strip_suffix(b"\n").unwrap_or(l);
                self.codec.decode(l.strip_suffix(b"\r").unwrap_or(l))
            })
            .filter(|l| !l.is_empty())
            .collect())
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<Entry>, RemoteError> {
        let refused = |c: &mut Conn, r: DataErr| match r {
            DataErr::Open(e) => c.refused_existing(e, path),
            DataErr::Done(e) => e,
        };
        if self.mlsd {
            return match self.data_lines(&format!("MLSD {path}"), path) {
                Ok(lines) => Ok(parse_listing(&lines, true)),
                Err(e) => Err(refused(self, e)),
            };
        }
        if self.list_a {
            match self.data_lines(&format!("LIST -a {path}"), path) {
                Ok(lines) => return Ok(parse_listing(&lines, false)),
                Err(DataErr::Open(e)) if refusal(&e).is_some() => {}
                Err(e) => return Err(refused(self, e)),
            }
        }
        match self.data_lines(&format!("LIST {path}"), path) {
            Ok(lines) => {
                // `-a` was refused but the folder lists: the server does not know `-a`.
                self.list_a = false;
                Ok(parse_listing(&lines, false))
            }
            Err(e) => Err(refused(self, e)),
        }
    }

    /// `None` when the path does not exist. MLST when offered, else a listing of the parent.
    /// A symlink that is a folder (FTP cannot read link targets, so `CWD` is tried) keeps
    /// `is_symlink` and gets `is_dir`, so a delete still removes only the link.
    fn stat_path(&mut self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        let mut st = self.lstat_path(path)?;
        if let Some(s) = st.as_mut().filter(|s| s.is_symlink && !s.is_dir) {
            s.is_dir = self.is_folder(path)?;
        }
        Ok(st)
    }

    /// Whether `path` can be entered: `CWD` into it, then back to the previous folder (only
    /// `home` reads the working folder; every other command uses absolute paths). A refusal or
    /// a name the codec cannot send means "not a folder"; a lost connection is an error.
    fn is_folder(&mut self, path: &str) -> Result<bool, RemoteError> {
        let back = match self.ftp.pwd() {
            Ok(b) => b,
            Err(e) => return Err(self.fatal(e, path)),
        };
        match self.ftp.cwd(path) {
            Ok(()) => {
                if let Err(e) = self.ftp.cwd(&back) {
                    let err = self.fatal(e, &back);
                    if self.broken {
                        return Err(err);
                    }
                    log::warn!("FTP: cannot return to the working folder: {err}");
                }
                Ok(true)
            }
            Err(e) => {
                let err = self.fatal(e, path);
                if self.broken {
                    Err(err)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// `stat_path` without following a symlink.
    fn lstat_path(&mut self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        if path.trim_end_matches('/').is_empty() {
            return Ok(Some(RemoteStat {
                is_dir: true,
                is_symlink: false,
                size: 0,
                mtime: 0,
                mode: 0,
            }));
        }
        if self.mlst {
            match self.ftp.mlst(Some(path)) {
                Ok(line) => {
                    return parse_mlsx(&line, true)
                        .map(|(f, mode_known)| Some(stat_of(&f, mode_known)))
                        .ok_or_else(|| {
                            RemoteError::Other(format!("{path}: unreadable MLST reply"))
                        })
                }
                Err(e) => match refusal(&e) {
                    Some((450 | 550, _)) => return Ok(None),
                    Some((500 | 502 | 504, _)) => self.mlst = false,
                    Some(_) => {}
                    None => return Err(self.fatal(e, path)),
                },
            }
        }
        let name = base_name(path);
        match self.list_dir(parent_path(path)) {
            Ok(entries) => Ok(entries
                .into_iter()
                .find(|e| e.name == name)
                .map(|e| RemoteStat {
                    is_dir: e.is_dir,
                    is_symlink: e.is_symlink,
                    size: e.size,
                    mtime: e.mtime,
                    mode: e.mode,
                })),
            Err(RemoteError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read_to(
        &mut self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let mut stream = match self.ftp.retr_as_stream(path) {
            Ok(s) => s,
            Err(e) => return Err(self.refused_existing(e, path)),
        };
        set_timeouts(stream.get_ref().get_ref(), REQUEST_TIMEOUT);
        let mut buf = vec![0u8; BLOCK];
        loop {
            let n = match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(self.end_failed(stream, e, path, false)),
            };
            if let Err(e) = out.write_all(&buf[..n]) {
                self.cancel(stream);
                return Err(RemoteError::Other(format!("write: {e}")));
            }
            if !progress(n as u64) {
                self.cancel(stream);
                return Err(RemoteError::Cancelled);
            }
        }
        self.finish(stream, path, false)
    }

    fn write_from(
        &mut self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let mut stream = match self.ftp.put_with_stream(path) {
            Ok(s) => s,
            Err(e) => return Err(self.refused_create(e, path)),
        };
        set_timeouts(stream.get_ref().get_ref(), REQUEST_TIMEOUT);
        let mut buf = vec![0u8; BLOCK];
        loop {
            let n = match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.cancel(stream);
                    return Err(RemoteError::Other(format!("read: {e}")));
                }
            };
            if let Err(e) = stream.write_all(&buf[..n]) {
                return Err(self.end_failed(stream, e, path, true));
            }
            if !progress(n as u64) {
                self.cancel(stream);
                return Err(RemoteError::Cancelled);
            }
        }
        if let Err(e) = stream.flush() {
            return Err(self.end_failed(stream, e, path, true));
        }
        self.finish(stream, path, true)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), RemoteError> {
        if self.stat_path(path)?.is_some() {
            return Err(RemoteError::Exists(path.to_string()));
        }
        match self.ftp.mkdir(path) {
            Ok(()) => Ok(()),
            Err(e) => {
                if refusal(&e).is_some() && matches!(self.stat_path(path), Ok(Some(_))) {
                    return Err(RemoteError::Exists(path.to_string()));
                }
                Err(self.refused_create(e, path))
            }
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), RemoteError> {
        // Servers answer a taken target with a generic 550, or overwrite it.
        if self.stat_path(to)?.is_some() {
            return Err(RemoteError::Exists(to.to_string()));
        }
        self.ftp
            .rename(from, to)
            .map_err(|e| self.refused_existing(e, from))
    }

    /// RNFR then RNTO, telling which of the two failed.
    fn rnfr_rnto(&mut self, from: &str, to: &str) -> Result<(), RenameErr> {
        self.ftp
            .custom_command(format!("RNFR {from}"), &[Status::RequestFilePending])
            .map_err(RenameErr::From)?;
        #[cfg(test)]
        if self.rnto_script.pop_front() == Some(true) {
            return Err(RenameErr::To(FtpError::UnexpectedResponse(Response::new(
                Status::FileUnavailable,
                b"550 Refused by the test".to_vec(),
            ))));
        }
        self.ftp
            .custom_command(
                format!("RNTO {to}"),
                // Some servers (bftpd) answer 200 instead of 250.
                &[Status::RequestedFileActionOk, Status::CommandOk],
            )
            .map(|_| ())
            .map_err(RenameErr::To)
    }

    /// Maps a failed `rnfr_rnto(from, ..)`.
    fn rename_err(&mut self, e: RenameErr, from: &str) -> RemoteError {
        match e {
            RenameErr::From(e) | RenameErr::To(e) => self.refused_existing(e, from),
        }
    }

    /// Rename over an existing target. FTP has no atomic replace: when the server refuses
    /// RNTO onto a taken name, the target is moved to a backup name first and only
    /// deleted once the source took its place; a failure puts the backup back. Only when
    /// even the backup rename is refused: DELE the target, then RNTO.
    fn replace(&mut self, from: &str, to: &str) -> Result<(), RemoteError> {
        match self.rnfr_rnto(from, to) {
            Ok(()) => return Ok(()),
            // The source is missing or locked: leave the target alone.
            Err(RenameErr::From(e)) => return Err(self.refused_existing(e, from)),
            Err(RenameErr::To(e)) if refusal(&e).is_none() => return Err(self.fatal(e, from)),
            Err(RenameErr::To(_)) => {}
        }
        if self.stat_path(to)?.is_none() {
            // Refused for another reason than the taken name.
            return self
                .rnfr_rnto(from, to)
                .map_err(|e| self.rename_err(e, from));
        }
        let bak = backup_path(to);
        match self.rnfr_rnto(to, &bak) {
            Ok(()) => {}
            Err(RenameErr::From(e) | RenameErr::To(e)) if refusal(&e).is_some() => {
                return self.delete_then_rename(from, to)
            }
            Err(RenameErr::From(e) | RenameErr::To(e)) => return Err(self.fatal(e, to)),
        }
        if let Err(e) = self.rnfr_rnto(from, to) {
            let err = self.rename_err(e, from);
            if self.broken {
                return Err(err);
            }
            return match self.rnfr_rnto(&bak, to) {
                Ok(()) => Err(err),
                Err(_) => Err(RemoteError::Other(format!(
                    "{to}: the replace failed and the original is kept as {bak}"
                ))),
            };
        }
        if let Err(e) = self.ftp.rm(&bak) {
            if refusal(&e).is_none() {
                self.fatal(e, &bak);
            } else {
                log::warn!("cannot delete the replace backup {bak}");
            }
        }
        Ok(())
    }

    /// The spec's last resort: DELE the target, then RNTO.
    fn delete_then_rename(&mut self, from: &str, to: &str) -> Result<(), RemoteError> {
        if let Err(e) = self.ftp.rm(to) {
            if refusal(&e).is_none() || self.stat_path(to)?.is_some() {
                return Err(self.refused_existing(e, to));
            }
        }
        self.rnfr_rnto(from, to)
            .map_err(|e| self.rename_err(e, from))
    }

    fn set_mode(&mut self, path: &str, mode: u32) -> Result<(), RemoteError> {
        #[cfg(test)]
        {
            self.chmod_calls += 1;
        }
        if self.no_chmod {
            return Err(RemoteError::Unsupported(NO_CHMOD.into()));
        }
        let cmd = format!("SITE CHMOD {:o} {path}", mode & 0o7777);
        match self
            .ftp
            .custom_command(cmd, &[Status::CommandOk, Status::RequestedFileActionOk])
        {
            Ok(_) => Ok(()),
            Err(e) => match refusal(&e) {
                Some((code, _)) => {
                    if matches!(code, 500 | 501 | 502 | 504) {
                        self.no_chmod = true;
                    }
                    Err(RemoteError::Unsupported(NO_CHMOD.into()))
                }
                None => Err(self.fatal(e, path)),
            },
        }
    }

    /// Logs out, best effort and briefly.
    fn quit(mut self) {
        if !self.broken {
            set_timeouts(&self.ftp.get_ref(), QUIT_TIMEOUT);
            let _ = self.ftp.quit();
        }
    }
}

// ---------------------------------------------------------------------------------------
// FtpFs
// ---------------------------------------------------------------------------------------

/// The control connection; `None` once closed. Shared (weakly) with the keepalive thread.
type SharedConn = Arc<Mutex<Option<Conn>>>;

/// An FTP or FTPS session. Dropping it logs out and stops its keepalive.
pub struct FtpFs {
    conn: SharedConn,
    cfg: Arc<Config>,
    display: String,
    /// Stops the keepalive thread when dropped (taken by `close`).
    keepalive: Mutex<Option<mpsc::Sender<()>>>,
    /// Test: `NOOP`s the keepalive sent.
    #[cfg(test)]
    noops: Arc<AtomicUsize>,
}

/// Every `every`: a `NOOP` when the connection was idle that long. A tick while a command or a
/// transfer holds the connection is skipped (`try_lock`). Ends when stopped, when the `FtpFs`
/// is closed or dropped, or when a `NOOP` fails (the connection is then marked broken, so the
/// next operation reports `Disconnected`). The `NOOP` waits at most `wait` for its reply, so a
/// hung server holds the lock (and `close`, Drop or the next operation) only that long.
fn keepalive(
    conn: Weak<Mutex<Option<Conn>>>,
    stop: mpsc::Receiver<()>,
    every: Duration,
    wait: Duration,
    #[cfg(test)] noops: Arc<AtomicUsize>,
) {
    loop {
        match stop.recv_timeout(every) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Stopped, or the `FtpFs` is gone.
            _ => return,
        }
        let Some(conn) = conn.upgrade() else {
            return;
        };
        let mut guard = match conn.try_lock() {
            Ok(g) => g,
            Err(TryLockError::WouldBlock) => continue,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
        };
        let Some(c) = guard.as_mut().filter(|c| !c.broken) else {
            return;
        };
        if c.last_used.elapsed() < every {
            continue;
        }
        #[cfg(test)]
        noops.fetch_add(1, Ordering::SeqCst);
        set_timeouts(&c.ftp.get_ref(), wait);
        let r = c.ftp.noop();
        set_timeouts(&c.ftp.get_ref(), REQUEST_TIMEOUT);
        if let Err(e) = r {
            let e = c.fatal(e, "NOOP");
            c.broken = true;
            log::info!("FTP keepalive: {e}; the connection is closed");
            return;
        }
        c.last_used = Instant::now();
    }
}

impl FtpFs {
    /// Connects, checks the FTPS certificate and logs in (`CONNECT_TIMEOUT` overall).
    pub fn connect(
        server: &ServerEntry,
        creds: &Credentials,
        opts: &ConnectOptions,
    ) -> Result<FtpFs, ConnectError> {
        let cfg = Arc::new(Config {
            server: server.clone(),
            creds: creds.clone(),
            known: opts.known.clone(),
        });
        FtpFs::open(cfg, opts.accept_fingerprint.map(str::to_string))
    }

    fn open(cfg: Arc<Config>, accept: Option<String>) -> Result<FtpFs, ConnectError> {
        let (tx, rx) = mpsc::channel();
        let cfg2 = cfg.clone();
        // DNS and the TLS handshake have no timeout of their own: bound the whole login.
        std::thread::Builder::new()
            .name("fxfm-ftp-connect".into())
            .spawn(move || {
                if let Err(mpsc::SendError(Ok(conn))) = tx.send(login(&cfg2, accept.as_deref())) {
                    // Too late: the caller already gave up.
                    conn.quit();
                }
            })
            .map_err(|e| ConnectError::Failed(e.to_string()))?;
        let conn = match rx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(r) => r?,
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(ConnectError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(ConnectError::Failed("The connection attempt failed".into()))
            }
        };
        let s = &cfg.server;
        let display = display_url(
            s.protocol.scheme(),
            &s.user,
            &s.host,
            s.port,
            s.protocol.default_port(),
        );
        let fs = FtpFs {
            conn: Arc::new(Mutex::new(Some(conn))),
            cfg,
            display,
            keepalive: Mutex::new(None),
            #[cfg(test)]
            noops: Arc::new(AtomicUsize::new(0)),
        };
        fs.start_keepalive(KEEPALIVE, KEEPALIVE_TIMEOUT);
        Ok(fs)
    }

    /// Starts the keepalive thread (a running one stops); `wait`: see `keepalive`.
    fn start_keepalive(&self, every: Duration, wait: Duration) {
        let (tx, rx) = mpsc::channel();
        let conn = Arc::downgrade(&self.conn);
        #[cfg(test)]
        let noops = self.noops.clone();
        let started = std::thread::Builder::new()
            .name("fxfm-ftp-keepalive".into())
            .spawn(move || {
                keepalive(
                    conn,
                    rx,
                    every,
                    wait,
                    #[cfg(test)]
                    noops,
                )
            });
        match started {
            Ok(_) => *self.keepalive.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx),
            Err(e) => log::warn!("FTP keepalive not started: {e}"),
        }
    }

    /// Runs `f` on the live connection; `Disconnected` once it is closed or broken.
    fn with<T>(
        &self,
        f: impl FnOnce(&mut Conn) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        let mut guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(c) if !c.broken => {
                let r = f(c);
                c.last_used = Instant::now();
                r
            }
            _ => Err(RemoteError::Disconnected),
        }
    }

    /// Scripts the next RNTO replies (`true` = refused without sending).
    #[cfg(test)]
    fn script_rnto(&self, refuse: &[bool]) {
        let _ = self.with(|c| {
            c.rnto_script = refuse.iter().copied().collect();
            Ok(())
        });
    }

    #[cfg(test)]
    fn chmod_calls(&self) -> usize {
        self.with(|c| Ok(c.chmod_calls)).unwrap()
    }

    #[cfg(test)]
    fn set_request_timeout(&self, t: Duration) {
        let _ = self.with(|c| {
            set_timeouts(&c.ftp.get_ref(), t);
            Ok(())
        });
    }

    /// `(MLSD, MLST, LIST -a)` in use.
    #[cfg(test)]
    fn listing_modes(&self) -> (bool, bool, bool) {
        self.with(|c| Ok((c.mlsd, c.mlst, c.list_a))).unwrap()
    }

    /// Lists with `LIST` as for a server without MLSD/MLST.
    #[cfg(test)]
    fn disable_mlsd(&self) {
        let _ = self.with(|c| {
            c.mlsd = false;
            c.mlst = false;
            Ok(())
        });
    }
}

impl Drop for FtpFs {
    fn drop(&mut self) {
        self.close();
    }
}

impl RemoteFs for FtpFs {
    fn display(&self) -> String {
        self.display.clone()
    }

    fn home(&self) -> Result<String, RemoteError> {
        self.with(|c| c.ftp.pwd().map_err(|e| c.fatal(e, ".")))
    }

    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError> {
        self.with(|c| c.list_dir(path))
    }

    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        self.with(|c| c.stat_path(path))
    }

    fn read_link(&self, _path: &str) -> Result<String, RemoteError> {
        Err(RemoteError::Unsupported(NO_SYMLINKS.into()))
    }

    fn read_to(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.with(|c| c.read_to(path, out, progress))
    }

    fn write_from(
        &self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.with(|c| c.write_from(path, input, progress))
    }

    fn mkdir(&self, path: &str) -> Result<(), RemoteError> {
        self.with(|c| c.mkdir(path))
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        self.with(|c| c.ftp.rm(path).map_err(|e| c.refused_existing(e, path)))
    }

    fn remove_dir(&self, path: &str) -> Result<(), RemoteError> {
        self.with(|c| c.ftp.rmdir(path).map_err(|e| c.refused_existing(e, path)))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        self.with(|c| c.rename(from, to))
    }

    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        self.with(|c| c.replace(from, to))
    }

    fn symlink(&self, _target: &str, _link: &str) -> Result<(), RemoteError> {
        Err(RemoteError::Unsupported(NO_SYMLINKS.into()))
    }

    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError> {
        self.with(|c| c.set_mode(path, mode))
    }

    /// FTP uploads keep the server's default permissions.
    fn upload_sets_mode(&self) -> bool {
        false
    }

    /// A new login with the same server and credentials: FTP has one control channel.
    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError> {
        // A busy session (a long transfer) is alive; do not wait for it.
        let dead = match self.conn.try_lock() {
            Ok(g) => g.as_ref().is_none_or(|c| c.broken),
            Err(TryLockError::Poisoned(p)) => p.into_inner().as_ref().is_none_or(|c| c.broken),
            Err(TryLockError::WouldBlock) => false,
        };
        if dead {
            return Err(RemoteError::Disconnected);
        }
        let fs = FtpFs::open(self.cfg.clone(), None).map_err(connect_error_text)?;
        Ok(Arc::new(fs))
    }

    fn close(&self) {
        // Dropping the sender wakes the keepalive thread, which ends.
        drop(
            self.keepalive
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take(),
        );
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(c) = conn {
            c.quit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{ConflictAnswer, ConflictInfo, ErrorAnswer, OpObserver, Progress};
    use crate::remote::known_hosts::KNOWN_HOSTS_FILE;
    use crate::remote::testutil::FtpTestServer;
    use crate::remote::transfer::{upload_tree, ProgressState};
    use crate::remote::{connect, Credentials};
    use crate::servers::FtpEncoding;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    struct Ctx {
        srv: FtpTestServer,
        data: tempfile::TempDir,
        known: KnownHosts,
    }

    impl Ctx {
        fn new(srv: FtpTestServer) -> Ctx {
            let data = tempfile::tempdir().unwrap();
            let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
            Ctx { srv, data, known }
        }

        fn opts<'a>(&'a self, accept: Option<&'a str>) -> ConnectOptions<'a> {
            ConnectOptions {
                data_dir: self.data.path(),
                known: &self.known,
                accept_fingerprint: accept,
            }
        }

        fn connect_entry(&self, e: &ServerEntry) -> FtpFs {
            FtpFs::connect(e, &pw(), &self.opts(None)).unwrap()
        }

        fn fs(&self) -> FtpFs {
            self.connect_entry(&self.srv.entry())
        }

        fn known_hosts_text(&self) -> Option<String> {
            fs::read_to_string(self.data.path().join(KNOWN_HOSTS_FILE)).ok()
        }
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

    fn names(fs: &dyn RemoteFs, path: &str) -> Vec<String> {
        let mut v: Vec<String> = fs.list(path).unwrap().into_iter().map(|e| e.name).collect();
        v.sort();
        v
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    #[test]
    fn ftp_login_list_roundtrip() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        assert_eq!(fs.display(), format!("ftp://u@127.0.0.1:{}", c.srv.port));
        assert_eq!(fs.home().unwrap(), "/");
        assert_eq!(fs.listing_modes(), (true, true, true));
        assert_eq!(fs.list("/").unwrap(), vec![]);

        fs.mkdir("/d").unwrap();
        assert_eq!(fs.mkdir("/d"), Err(RemoteError::Exists("/d".into())));
        let big = pattern(1024 * 1024);
        put(&fs, "/d/f.bin", &big);
        assert_eq!(fs::read(c.srv.root.join("d/f.bin")).unwrap(), big);
        assert_eq!(get(&fs, "/d/f.bin"), big);

        let list = fs.list("/").unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].is_dir && list[0].name == "d" && !list[0].hidden);
        let list = fs.list("/d").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].name.as_str(), list[0].size, list[0].is_dir),
            ("f.bin", big.len() as u64, false)
        );
        assert!(list[0].mtime > 1_600_000_000);
        let st = fs.stat("/d/f.bin").unwrap().unwrap();
        assert_eq!((st.is_dir, st.size), (false, big.len() as u64));
        assert!(fs.stat("/d").unwrap().unwrap().is_dir);
        assert!(fs.stat("/").unwrap().unwrap().is_dir);
        assert_eq!(fs.stat("/d/nope").unwrap(), None);

        // Missing paths.
        let mut sink = Vec::new();
        assert_eq!(
            fs.read_to("/d/nope", &mut sink, &mut noop),
            Err(RemoteError::NotFound("/d/nope".into()))
        );
        assert_eq!(fs.list("/nope"), Err(RemoteError::NotFound("/nope".into())));
        assert_eq!(
            fs.remove_file("/nope"),
            Err(RemoteError::NotFound("/nope".into()))
        );

        // rename refuses a taken target; replace takes it.
        fs.rename("/d/f.bin", "/d/g.bin").unwrap();
        put(&fs, "/d/h.txt", b"hello");
        assert_eq!(
            fs.rename("/d/h.txt", "/d/g.bin"),
            Err(RemoteError::Exists("/d/g.bin".into()))
        );
        fs.replace("/d/h.txt", "/d/g.bin").unwrap();
        assert_eq!(get(&fs, "/d/g.bin"), b"hello");
        assert_eq!(fs.stat("/d/h.txt").unwrap(), None);
        // RNTO onto the taken name refused: through a backup name.
        put(&fs, "/d/i.txt", b"again");
        fs.script_rnto(&[true]);
        fs.replace("/d/i.txt", "/d/g.bin").unwrap();
        assert_eq!(get(&fs, "/d/g.bin"), b"again");
        assert_eq!(names(&fs, "/d"), vec!["g.bin"]);
        // Even the backup rename refused: DELE + RNTO.
        put(&fs, "/d/j.txt", b"third");
        fs.script_rnto(&[true, true]);
        fs.replace("/d/j.txt", "/d/g.bin").unwrap();
        assert_eq!(get(&fs, "/d/g.bin"), b"third");
        assert_eq!(names(&fs, "/d"), vec!["g.bin"]);

        // LIST fallback gives the same listing as MLSD.
        let mlsd = fs.list("/d").unwrap();
        fs.disable_mlsd();
        let list = fs.list("/d").unwrap();
        assert_eq!(
            (list[0].name.as_str(), list[0].size, list[0].is_dir),
            (mlsd[0].name.as_str(), mlsd[0].size, mlsd[0].is_dir)
        );
        assert!(fs.list("/").unwrap()[0].is_dir);
        assert_eq!(fs.listing_modes(), (false, false, true));
        assert_eq!(fs.stat("/d/g.bin").unwrap().unwrap().size, 5);
        assert_eq!(fs.stat("/d/nope").unwrap(), None);
        assert_eq!(fs.list("/nope"), Err(RemoteError::NotFound("/nope".into())));

        assert_eq!(
            fs.set_mode("/d/g.bin", 0o600),
            Err(RemoteError::Unsupported(
                "This server does not support changing permissions".into()
            ))
        );
        assert!(matches!(
            fs.symlink("/d/g.bin", "/d/l"),
            Err(RemoteError::Unsupported(_))
        ));
        assert!(matches!(
            fs.read_link("/d/g.bin"),
            Err(RemoteError::Unsupported(_))
        ));

        fs.remove_file("/d/g.bin").unwrap();
        fs.remove_dir("/d").unwrap();
        assert_eq!(fs.list("/").unwrap(), vec![]);
        fs.close();
        assert_eq!(fs.list("/"), Err(RemoteError::Disconnected));
    }

    #[test]
    fn ftp_replace_never_loses_the_target() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        put(&fs, "/t.txt", b"target");
        // A refused RNFR (missing source) does not touch the target.
        assert_eq!(
            fs.replace("/missing.txt", "/t.txt"),
            Err(RemoteError::NotFound("/missing.txt".into()))
        );
        assert_eq!(get(&fs, "/t.txt"), b"target");
        // Direct RNTO refused, backup taken, then the source rename refused too:
        // the backup goes back to the target.
        put(&fs, "/s.txt", b"source");
        fs.script_rnto(&[true, false, true]);
        assert!(fs.replace("/s.txt", "/t.txt").is_err());
        assert_eq!(get(&fs, "/t.txt"), b"target");
        assert_eq!(get(&fs, "/s.txt"), b"source");
        assert_eq!(names(&fs, "/"), vec!["s.txt", "t.txt"]);
    }

    #[test]
    fn ftp_wrong_password_is_auth_failed() {
        let c = Ctx::new(FtpTestServer::start());
        let bad = Credentials {
            password: Some("nope".into()),
            passphrase: None,
        };
        assert_eq!(
            FtpFs::connect(&c.srv.entry(), &bad, &c.opts(None)).err(),
            Some(ConnectError::AuthFailed)
        );
        assert_eq!(
            FtpFs::connect(&c.srv.entry(), &Credentials::default(), &c.opts(None)).err(),
            Some(ConnectError::NeedPassword)
        );
        // The dispatcher reaches the FTP backend too.
        assert!(connect(&c.srv.entry(), &pw(), &c.opts(None)).is_ok());
    }

    #[test]
    fn ftps_unknown_cert_asks_then_trusts() {
        let c = Ctx::new(FtpTestServer::start_tls());
        let e = c.srv.entry();
        let fp = c.srv.cert_fingerprint();
        let err = FtpFs::connect(&e, &pw(), &c.opts(None)).err();
        assert_eq!(
            err,
            Some(ConnectError::HostKey {
                host: "127.0.0.1".into(),
                port: c.srv.port,
                key_type: "x509".into(),
                fingerprint: fp.clone(),
                changed_from: None,
            })
        );
        assert_eq!(c.known_hosts_text(), None);

        // A different fingerprint is not an acceptance.
        assert!(matches!(
            FtpFs::connect(&e, &pw(), &c.opts(Some("SHA256:other"))).err(),
            Some(ConnectError::HostKey { .. })
        ));

        let fs = FtpFs::connect(&e, &pw(), &c.opts(Some(&fp))).unwrap();
        assert_eq!(fs.display(), format!("ftps://u@127.0.0.1:{}", c.srv.port));
        assert_eq!(
            c.known_hosts_text().unwrap(),
            format!("127.0.0.1:{} x509 {fp}\n", c.srv.port)
        );
        // Data connections are protected too.
        put(&fs, "/t.txt", b"secret");
        assert_eq!(get(&fs, "/t.txt"), b"secret");
        assert_eq!(names(&fs, "/"), vec!["t.txt"]);
        fs.close();

        // Trusted now: no question.
        let fs = c.fs();
        assert_eq!(names(&fs, "/"), vec!["t.txt"]);
        fs.close();

        c.srv.rotate_cert();
        let new_fp = c.srv.cert_fingerprint();
        match FtpFs::connect(&e, &pw(), &c.opts(None)).err() {
            Some(ConnectError::HostKey {
                key_type,
                fingerprint,
                changed_from,
                ..
            }) => {
                assert_eq!(key_type, "x509");
                assert_eq!(fingerprint, new_fp);
                assert_eq!(changed_from, Some(fp));
            }
            other => panic!("expected HostKey, got {other:?}"),
        }
    }

    #[test]
    fn ftp_symlinked_folder_is_a_folder() {
        // libunftp shows links as links only in LIST (MLST/MLSD say `type=file`), so this
        // runs on the LIST path, as for a server without MLSD/MLST.
        let c = Ctx::new(FtpTestServer::start());
        fs::create_dir(c.srv.root.join("real")).unwrap();
        fs::write(c.srv.root.join("real/a.txt"), b"aaa").unwrap();
        fs::write(c.srv.root.join("f.txt"), b"ff").unwrap();
        std::os::unix::fs::symlink("real", c.srv.root.join("ld")).unwrap();
        std::os::unix::fs::symlink("f.txt", c.srv.root.join("lf")).unwrap();
        let fs = Arc::new(c.fs());
        fs.disable_mlsd();
        let home = fs.home().unwrap();
        let ld = fs.stat("/ld").unwrap().unwrap();
        assert!(ld.is_symlink && ld.is_dir, "{ld:?}");
        let lf = fs.stat("/lf").unwrap().unwrap();
        assert!(lf.is_symlink && !lf.is_dir, "{lf:?}");
        assert_eq!(
            fs.home().unwrap(),
            home,
            "the probe goes back to the working folder"
        );

        // Through the VFS: Enter's open is refused as for any folder (the UI then enters it),
        // the link to a file opens, and deleting the folder link removes only the link.
        let t = tempfile::tempdir().unwrap();
        let v = crate::vfs::Ctx::new(t.path().join("tmp-root"));
        v.sessions.insert("f", fs.clone());
        assert_eq!(
            crate::vfs::materialize(&v, "ftp://f/ld", "view").unwrap_err(),
            "Only files can be opened from a server"
        );
        let p = crate::vfs::materialize(&v, "ftp://f/lf", "view").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"ff");
        let names: Vec<String> = crate::vfs::list(&v, "ftp://f/ld")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["a.txt".to_string()]);
        let obs = crate::vfs::NullObserver;
        let r = crate::vfs::delete(&v, &["ftp://f/ld".to_string()], true, &obs).unwrap();
        assert_eq!(r.done, 1);
        assert!(
            fs::symlink_metadata(c.srv.root.join("ld")).is_err(),
            "link gone"
        );
        assert_eq!(fs::read(c.srv.root.join("real/a.txt")).unwrap(), b"aaa");
    }

    #[test]
    fn ftp_names_with_spaces_and_unicode() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        for (name, renamed) in [("a b.txt", "c d.txt"), ("čćž.txt", "šđ ž.txt")] {
            let from = format!("/{name}");
            let to = format!("/{renamed}");
            put(&fs, &from, name.as_bytes());
            assert_eq!(names(&fs, "/"), vec![name.to_string()]);
            fs.rename(&from, &to).unwrap();
            assert_eq!(names(&fs, "/"), vec![renamed.to_string()]);
            assert_eq!(get(&fs, &to), name.as_bytes());
            assert!(c.srv.root.join(renamed).is_file());
            fs.remove_file(&to).unwrap();
            assert_eq!(fs.list("/").unwrap(), vec![]);
        }
        // Same through the LIST parser.
        fs.disable_mlsd();
        put(&fs, "/a b.txt", b"1");
        put(&fs, "/čćž.txt", b"2");
        assert_eq!(names(&fs, "/"), vec!["a b.txt", "čćž.txt"]);
    }

    #[test]
    fn ftp_fork_is_independent() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        let big = pattern(5 * 1024 * 1024);
        fs::write(c.srv.root.join("big.bin"), &big).unwrap();
        let before = c.srv.connections();
        let job = fs.fork().unwrap();
        assert_eq!(c.srv.connections(), before + 1);

        let listed = Arc::new(AtomicBool::new(false));
        let read_done = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (l2, d2) = (listed.clone(), read_done.clone());
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut first = true;
            job.read_to("/big.bin", &mut out, &mut |_| {
                if first {
                    first = false;
                    started_tx.send(()).unwrap();
                    // Hold the transfer open until the panel has listed (bounded).
                    let t = Instant::now();
                    while !l2.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(10) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                true
            })
            .unwrap();
            d2.store(true, Ordering::SeqCst);
            out
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(names(&fs, "/"), vec!["big.bin"]);
        assert!(!read_done.load(Ordering::SeqCst));
        listed.store(true, Ordering::SeqCst);
        assert_eq!(reader.join().unwrap(), big);
    }

    #[test]
    fn ftp_fork_logs_out_on_drop() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        let job = fs.fork().unwrap();
        assert_eq!(names(job.as_ref(), "/"), Vec::<String>::new());
        let before = c.srv.logouts();
        drop(job);
        let t = Instant::now();
        while c.srv.logouts() == before && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(c.srv.logouts(), before + 1);
        // The original keeps working.
        assert_eq!(fs.list("/").unwrap(), vec![]);
    }

    #[test]
    fn ftp_active_mode_lists() {
        let c = Ctx::new(FtpTestServer::start());
        fs::create_dir(c.srv.root.join("sub")).unwrap();
        fs::write(c.srv.root.join("a.txt"), b"abc").unwrap();
        fs::write(c.srv.root.join(".hidden"), b"h").unwrap();
        let passive = c.fs();
        let mut e = c.srv.entry();
        e.ftp_passive = false;
        let active = c.connect_entry(&e);
        let key = |fs: &FtpFs| {
            let mut v: Vec<(String, bool, u64, bool)> = fs
                .list("/")
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
            v.sort();
            v
        };
        let p = key(&passive);
        assert_eq!(p.len(), 3);
        assert_eq!(key(&active), p);
        assert!(p.contains(&(".hidden".into(), false, 1, true)));
        assert_eq!(get(&active, "/a.txt"), b"abc");
        put(&active, "/b.txt", b"active");
        assert_eq!(fs::read(c.srv.root.join("b.txt")).unwrap(), b"active");
    }

    #[derive(Default)]
    struct Obs;

    impl OpObserver for Obs {
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

    #[test]
    fn ftp_upload_replaces_through_part_and_skips_modes() {
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        let local = tempfile::tempdir().unwrap();
        let src = local.path().join("f.txt");
        fs::write(&src, b"new content").unwrap();
        fs::write(c.srv.root.join("f.txt"), b"old").unwrap();
        let mut p = ProgressState::new(11, 1);
        let r = upload_tree(&fs, std::slice::from_ref(&src), "/", &Obs, &mut p).unwrap();
        assert_eq!((r.done, r.skipped), (1, 0));
        assert_eq!(fs::read(c.srv.root.join("f.txt")).unwrap(), b"new content");
        assert_eq!(names(&fs, "/"), vec!["f.txt"]);
        // FTP uploads never chmod (neither the `.part` nor new folders).
        fs::create_dir(local.path().join("dir")).unwrap();
        fs::write(local.path().join("dir/x"), b"x").unwrap();
        let mut p = ProgressState::new(1, 1);
        upload_tree(&fs, &[local.path().join("dir")], "/", &Obs, &mut p).unwrap();
        assert_eq!(fs::read(c.srv.root.join("dir/x")).unwrap(), b"x");
        assert_eq!(fs.chmod_calls(), 0);

        // A folder that refuses new files: the `.part` cannot be created, so the existing
        // target is written directly.
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores folder permissions
        }
        fs::create_dir(c.srv.root.join("ro")).unwrap();
        fs::write(c.srv.root.join("ro/f.txt"), b"old").unwrap();
        fs::set_permissions(c.srv.root.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
        let denied = fs.write_from("/ro/x.part", &mut &b"x"[..], &mut noop);
        assert_eq!(
            denied,
            Err(RemoteError::PermissionDenied("/ro/x.part".into()))
        );
        let mut p = ProgressState::new(11, 1);
        let r = upload_tree(&fs, &[src], "/ro", &Obs, &mut p);
        fs::set_permissions(c.srv.root.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.unwrap().done, 1);
        assert_eq!(
            fs::read(c.srv.root.join("ro/f.txt")).unwrap(),
            b"new content"
        );
        assert_eq!(names(&fs, "/ro"), vec!["f.txt"]);
    }

    #[test]
    fn ftp_single_byte_encodings_round_trip() {
        for enc in [FtpEncoding::Windows1250, FtpEncoding::Latin1] {
            let codec = Codec::new(enc);
            for b in 0u8..=255 {
                let s = codec.decode(&[b]);
                assert_eq!(codec.encode(&s).unwrap(), vec![b], "{enc:?} byte {b:#x}");
            }
        }
        let w = Codec::new(FtpEncoding::Windows1250);
        assert_eq!(w.decode(b"\x9a\xe8\x9e"), "ščž");
        assert_eq!(w.encode("ščž").unwrap(), b"\x9a\xe8\x9e");
        let err = w.encode("日本").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let l = Codec::new(FtpEncoding::Latin1);
        assert_eq!(l.decode(b"caf\xe9"), "café");
        assert!(l.encode("č").is_err());
    }

    #[test]
    fn ftp_unencodable_name_fails_without_dropping_the_connection() {
        let c = Ctx::new(FtpTestServer::start());
        let mut e = c.srv.entry();
        e.ftp_encoding = FtpEncoding::Latin1;
        let fs = c.connect_entry(&e);
        assert!(matches!(
            fs.write_from("/č.txt", &mut &b"x"[..], &mut noop),
            Err(RemoteError::Other(_))
        ));
        put(&fs, "/ok.txt", b"x");
        assert_eq!(names(&fs, "/"), vec!["ok.txt"]);
    }

    #[test]
    fn ftp_mlsd_lines_suppaftp_rejects_are_kept() {
        let lines: Vec<String> = [
            "type=OS.unix=symlink;size=4;modify=20260101120000; l nk",
            "type=OS.unix=slink:/x;UNIX.mode=0120777; s",
            "type=file;size=3;UNIX.mode=0100644; odd mode",
            "type=cdir;modify=20260101120000; .",
            "type=file;size=3;UNIX.mode=0640; ok",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let e = parse_listing(&lines, true);
        let got: Vec<(&str, bool, u64, u32)> = e
            .iter()
            .map(|e| (e.name.as_str(), e.is_symlink, e.size, e.mode))
            .collect();
        assert_eq!(
            got,
            vec![
                ("l nk", true, 4, 0),
                ("s", true, 0, 0),
                ("odd mode", false, 3, 0),
                ("ok", false, 3, 0o100640),
            ]
        );
        assert_eq!(e[0].mtime, 1_767_268_800);
    }

    #[test]
    fn ftp_mlsx_mode_only_when_the_server_sends_it() {
        assert!(mlsx_has_mode("type=file;size=3;UNIX.mode=0644; a b"));
        assert!(!mlsx_has_mode("type=file;size=3; UNIX.mode=0644"));
        assert!(!mlsx_has_mode(" name only"));
    }

    #[test]
    fn ftp_connect_refused_is_failed_not_a_hang() {
        let data = tempfile::tempdir().unwrap();
        let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
        let opts = ConnectOptions {
            data_dir: data.path(),
            known: &known,
            accept_fingerprint: None,
        };
        let e = ServerEntry {
            host: "127.0.0.1".into(),
            port: 1,
            protocol: crate::servers::Protocol::Ftp,
            ..Default::default()
        };
        let t = Instant::now();
        assert!(matches!(
            FtpFs::connect(&e, &pw(), &opts).err(),
            Some(ConnectError::Failed(_))
        ));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    /// A minimal server that logs anyone in and then never answers `PWD`.
    fn stalling_server() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((sock, _)) = l.accept() else { return };
            let mut w = sock.try_clone().unwrap();
            let mut r = std::io::BufReader::new(sock);
            w.write_all(b"220 ready\r\n").unwrap();
            let mut line = String::new();
            while std::io::BufRead::read_line(&mut r, &mut line).unwrap_or(0) > 0 {
                let reply: &[u8] = match line.split_whitespace().next().unwrap_or("") {
                    "USER" => b"331 password\r\n",
                    "PASS" => b"230 ok\r\n",
                    "TYPE" => b"200 ok\r\n",
                    "FEAT" => b"211 no features\r\n",
                    // PWD and anything else: silence.
                    _ => b"",
                };
                if w.write_all(reply).is_err() {
                    return;
                }
                line.clear();
            }
        });
        port
    }

    #[test]
    fn ftp_keepalive_noop_on_a_hung_server_times_out_quickly() {
        // The server answers nothing after login: the NOOP gives up after its own short wait
        // (not the 60 s request timeout), marks the session broken and frees the connection.
        let data = tempfile::tempdir().unwrap();
        let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
        let opts = ConnectOptions {
            data_dir: data.path(),
            known: &known,
            accept_fingerprint: None,
        };
        let e = ServerEntry {
            host: "127.0.0.1".into(),
            port: stalling_server(),
            protocol: crate::servers::Protocol::Ftp,
            user: "u".into(),
            ..Default::default()
        };
        let fs = FtpFs::connect(&e, &pw(), &opts).unwrap();
        fs.start_keepalive(Duration::from_millis(100), Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(fs.noops.load(Ordering::SeqCst), 1);
        let t = Instant::now();
        assert_eq!(fs.list("/"), Err(RemoteError::Disconnected));
        fs.close();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn ftp_keepalive_keeps_an_idle_session() {
        // The test server closes a control connection idle for 1 s.
        let c = Ctx::new(FtpTestServer::start_idle(1));
        // Without a keepalive the idle session dies: the server's idle timeout is real.
        let quiet = c.fs();
        drop(quiet.keepalive.lock().unwrap().take());
        let fs = c.fs();
        fs.start_keepalive(Duration::from_millis(200), KEEPALIVE_TIMEOUT);
        std::thread::sleep(Duration::from_millis(2500));
        assert!(
            quiet.list("/").is_err(),
            "the server closed the idle session"
        );
        assert!(fs.list("/").is_ok(), "the keepalive kept the session");
        assert!(fs.noops.load(Ordering::SeqCst) >= 3);

        // A busy connection gets no NOOP: none while a command holds it.
        let before = fs.noops.load(Ordering::SeqCst);
        fs.with(|_| {
            std::thread::sleep(Duration::from_millis(700));
            Ok(())
        })
        .unwrap();
        assert_eq!(fs.noops.load(Ordering::SeqCst), before);

        // `close` stops it.
        fs.close();
        std::thread::sleep(Duration::from_millis(100));
        let n = fs.noops.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(fs.noops.load(Ordering::SeqCst), n);

        // So does dropping a connection (as a job drops its forked one).
        let fork = c.fs();
        fork.start_keepalive(Duration::from_millis(200), KEEPALIVE_TIMEOUT);
        let count = fork.noops.clone();
        std::thread::sleep(Duration::from_millis(700));
        assert!(count.load(Ordering::SeqCst) > 0);
        drop(fork);
        std::thread::sleep(Duration::from_millis(100));
        let n = count.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(count.load(Ordering::SeqCst), n);
    }

    #[test]
    fn ftp_failed_keepalive_marks_the_session_lost() {
        // The server goes away while the session is idle: the NOOP fails and the next
        // operation reports a lost connection, without waiting for a request timeout.
        let c = Ctx::new(FtpTestServer::start());
        let fs = c.fs();
        fs.with(|conn| {
            let _ = conn.ftp.get_ref().shutdown(Shutdown::Both);
            Ok(())
        })
        .unwrap();
        fs.start_keepalive(Duration::from_millis(100), KEEPALIVE_TIMEOUT);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            fs.noops.load(Ordering::SeqCst),
            1,
            "one NOOP, then the thread ended"
        );
        assert_eq!(fs.list("/"), Err(RemoteError::Disconnected));
    }

    #[test]
    fn ftp_stalled_server_times_out() {
        let data = tempfile::tempdir().unwrap();
        let known = KnownHosts::new(data.path().join(KNOWN_HOSTS_FILE));
        let opts = ConnectOptions {
            data_dir: data.path(),
            known: &known,
            accept_fingerprint: None,
        };
        let e = ServerEntry {
            host: "127.0.0.1".into(),
            port: stalling_server(),
            protocol: crate::servers::Protocol::Ftp,
            user: "u".into(),
            ..Default::default()
        };
        let fs = FtpFs::connect(&e, &pw(), &opts).unwrap();
        fs.set_request_timeout(Duration::from_millis(300));
        let t = Instant::now();
        assert_eq!(fs.home(), Err(RemoteError::Timeout));
        assert!(t.elapsed() < Duration::from_secs(5));
        // The reply may still come later: the session is not trusted any more.
        assert_eq!(fs.home(), Err(RemoteError::Disconnected));
    }
}
