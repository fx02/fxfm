//! SCP backend: transfers through the remote `scp -t` / `scp -f` (the legacy SCP protocol),
//! everything else through exec commands. Listings, `stat`, `read_link` and `home` use SFTP
//! when the server offers it, otherwise `ls` and `pwd`.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use russh::client;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf};

use super::sftp::{ssh_connect, SftpFs, SshSession};
use super::{
    parent_path, rt, with_timeout, ConnectError, ConnectOptions, Credentials, RemoteError,
    RemoteFs, RemoteStat, REQUEST_TIMEOUT,
};
use crate::listing::Entry;
use crate::servers::ServerEntry;

/// Bytes per transfer block.
const BLOCK: usize = 32 * 1024;
/// Which `ls` the server has.
const LS_UNKNOWN: u8 = 0;
const LS_GNU: u8 = 1;
const LS_BSD: u8 = 2;

/// POSIX single quotes: `it's` -> `'it'\''s'`.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A command line with every word quoted.
fn sh(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The next space-separated field of an `ls` line; leaves `rest` at the space after it.
fn next_field<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let s = rest.trim_start_matches(' ');
    let end = s.find(' ')?;
    *rest = &s[end..];
    Some(&s[..end])
}

/// `drwxr-xr-x` (plus an optional `@`, `+` or `.`) as a mode with the file type bits.
fn parse_perms(p: &str) -> Option<u32> {
    let b = p.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let mut mode = match b[0] {
        b'-' => 0o100000,
        b'd' => 0o040000,
        b'l' => 0o120000,
        b'c' => 0o020000,
        b'b' => 0o060000,
        b'p' => 0o010000,
        b's' => 0o140000,
        _ => return None,
    };
    // Special bit of each triple: setuid, setgid, sticky.
    let special = [0o4000, 0o2000, 0o1000];
    for t in 0..3 {
        let shift = 3 * (2 - t);
        let (r, w, x) = (b[1 + 3 * t], b[2 + 3 * t], b[3 + 3 * t]);
        if r == b'r' {
            mode |= 4 << shift;
        }
        if w == b'w' {
            mode |= 2 << shift;
        }
        match x {
            b'x' => mode |= 1 << shift,
            b's' | b't' => mode |= (1 << shift) | special[t],
            b'S' | b'T' => mode |= special[t],
            _ => {}
        }
    }
    Some(mode)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `Mar`, `5`, `14:07:09`, `2024` (UTC) as Unix time.
fn bsd_time(mon: &str, day: &str, hms: &str, year: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = MONTHS.iter().position(|x| *x == mon)? as i64 + 1;
    let d: i64 = day.parse().ok()?;
    let y: i64 = year.parse().ok()?;
    let mut t = hms.split(':').map(|x| x.parse::<i64>().ok());
    let (h, mi, s) = (t.next()??, t.next()??, t.next()??);
    Some(days_from_civil(y, m, d) * 86400 + h * 3600 + mi * 60 + s)
}

/// One line of `ls -la` (GNU with `--time-style=+%s`, BSD with `-T` in UTC): name, stat and
/// the symlink target. `None` for lines that are not entries (`total ...`).
pub(crate) fn parse_ls(line: &str, gnu: bool) -> Option<(String, RemoteStat, Option<String>)> {
    let mut rest = line;
    let mode = parse_perms(next_field(&mut rest)?)?;
    next_field(&mut rest)?; // links
    next_field(&mut rest)?; // owner
    next_field(&mut rest)?; // group
    let mut size_field = next_field(&mut rest)?;
    let size = if size_field.ends_with(',') {
        // Device: `major, minor`.
        size_field = next_field(&mut rest)?;
        size_field.parse::<u64>().ok().map(|_| 0)?
    } else {
        size_field.parse().ok()?
    };
    let mtime = if gnu {
        next_field(&mut rest)?.parse().ok()?
    } else {
        let mon = next_field(&mut rest)?;
        let day = next_field(&mut rest)?;
        let hms = next_field(&mut rest)?;
        let year = next_field(&mut rest)?;
        bsd_time(mon, day, hms, year)?
    };
    // Exactly one space separates the time from the name.
    let name = rest.strip_prefix(' ').filter(|n| !n.is_empty())?;
    let kind = mode & 0o170000;
    let (name, target) = match name.split_once(" -> ") {
        Some((n, t)) if kind == 0o120000 => (n, Some(t.to_string())),
        _ => (name, None),
    };
    let stat = RemoteStat {
        is_dir: kind == 0o040000,
        is_symlink: kind == 0o120000,
        size,
        mtime,
        mode,
    };
    Some((name.to_string(), stat, target))
}

/// The error for a failed command or an SCP error line, from its message.
fn message_error(msg: &[u8], path: &str, status: Option<u32>) -> RemoteError {
    let text = String::from_utf8_lossy(msg).trim().to_string();
    if text.contains("No such file") {
        RemoteError::NotFound(path.to_string())
    } else if text.contains("Permission denied") {
        RemoteError::PermissionDenied(path.to_string())
    } else if text.contains("File exists") {
        RemoteError::Exists(path.to_string())
    } else if text.is_empty() {
        match status {
            Some(s) => RemoteError::Other(format!("{path}: command failed (exit code {s})")),
            None => RemoteError::Other(format!("{path}: the command ended unexpectedly")),
        }
    } else {
        RemoteError::Other(text)
    }
}

/// A remote command on its own channel. The channel is closed when this is dropped early.
struct Exec<'a> {
    ssh: &'a SshSession,
    /// What errors are about.
    path: &'a str,
    rd: ChannelReadHalf,
    wr: Arc<ChannelWriteHalf<client::Msg>>,
    /// Unread stdout.
    out: Vec<u8>,
    pos: usize,
    err: Vec<u8>,
    status: Option<u32>,
    ended: bool,
}

impl Drop for Exec<'_> {
    fn drop(&mut self) {
        if !self.ended {
            let wr = self.wr.clone();
            rt().spawn(async move {
                let _ = wr.close().await;
            });
        }
    }
}

impl<'a> Exec<'a> {
    async fn start(ssh: &'a SshSession, cmd: &str, path: &'a str) -> Result<Exec<'a>, RemoteError> {
        let ch = ssh
            .handle
            .channel_open_session()
            .await
            .map_err(|e| ssh.ssh_err(e))?;
        let (rd, wr) = ch.split();
        let mut ex = Exec {
            ssh,
            path,
            rd,
            wr: Arc::new(wr),
            out: Vec::new(),
            pos: 0,
            err: Vec::new(),
            status: None,
            ended: false,
        };
        ex.wr.exec(true, cmd).await.map_err(|e| ssh.ssh_err(e))?;
        loop {
            match ex.rd.wait().await {
                Some(ChannelMsg::Success) => return Ok(ex),
                Some(ChannelMsg::Failure) => {
                    return Err(RemoteError::Other(
                        "The server refused to run a command".into(),
                    ))
                }
                Some(m) => ex.take(m),
                None => {
                    ex.ended = true;
                    return Err(ex.broken());
                }
            }
        }
    }

    fn take(&mut self, m: ChannelMsg) {
        match m {
            ChannelMsg::Data { data } => self.out.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => self.err.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => self.status = Some(exit_status),
            _ => {}
        }
    }

    /// Waits for one message; false once the channel is gone.
    async fn pump(&mut self) -> Result<bool, RemoteError> {
        match self.rd.wait().await {
            Some(m) => {
                self.take(m);
                Ok(true)
            }
            None => {
                self.ended = true;
                Ok(false)
            }
        }
    }

    /// The error when the command stopped before the protocol was done.
    fn broken(&self) -> RemoteError {
        if self.ssh.is_closed() {
            RemoteError::Disconnected
        } else {
            message_error(&self.err, self.path, self.status)
        }
    }

    /// Waits until unread stdout is there.
    async fn fill(&mut self) -> Result<(), RemoteError> {
        while self.pos == self.out.len() {
            self.out.clear();
            self.pos = 0;
            if !self.pump().await? {
                return Err(self.broken());
            }
        }
        Ok(())
    }

    /// Up to `max` bytes of stdout.
    async fn read_some(&mut self, max: usize) -> Result<Vec<u8>, RemoteError> {
        self.fill().await?;
        let end = self.out.len().min(self.pos + max);
        let v = self.out[self.pos..end].to_vec();
        self.pos = end;
        Ok(v)
    }

    async fn read_byte(&mut self) -> Result<u8, RemoteError> {
        self.fill().await?;
        self.pos += 1;
        Ok(self.out[self.pos - 1])
    }

    /// A line of stdout without its `\n`.
    async fn read_line(&mut self) -> Result<Vec<u8>, RemoteError> {
        let mut line = Vec::new();
        loop {
            match self.read_byte().await? {
                b'\n' => return Ok(line),
                b => line.push(b),
            }
        }
    }

    /// An SCP reply: `\0` is OK, `\1`/`\2` are followed by an error line.
    async fn ack(&mut self) -> Result<(), RemoteError> {
        match self.read_byte().await? {
            0 => Ok(()),
            1 | 2 => {
                let msg = self.read_line().await?;
                Err(message_error(&msg, self.path, None))
            }
            b => Err(RemoteError::Other(format!(
                "{}: unexpected SCP reply 0x{b:02x}",
                self.path
            ))),
        }
    }

    /// The header of the file `scp -f` sends: its size.
    async fn header(&mut self) -> Result<u64, RemoteError> {
        let first = self.read_byte().await?;
        let line = self.read_line().await?;
        match first {
            b'C' => {
                // `C<mode> <size> <name>`
                let text = String::from_utf8_lossy(&line);
                text.split(' ')
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| RemoteError::Other(format!("{}: bad SCP header", self.path)))
            }
            1 | 2 => Err(message_error(&line, self.path, None)),
            _ => Err(RemoteError::Other(format!(
                "{}: unexpected SCP header",
                self.path
            ))),
        }
    }

    async fn send(&self, data: Vec<u8>) -> Result<(), RemoteError> {
        self.wr
            .data_bytes(data)
            .await
            .map_err(|e| self.ssh.ssh_err(e))
    }

    async fn eof(&self) -> Result<(), RemoteError> {
        self.wr.eof().await.map_err(|e| self.ssh.ssh_err(e))
    }
}

enum Conn {
    /// SFTP for listings; its SSH connection for commands.
    Sftp(SftpFs),
    Ssh(SshSession),
}

impl Conn {
    fn ssh(&self) -> &SshSession {
        match self {
            Conn::Sftp(s) => s.ssh(),
            Conn::Ssh(s) => s,
        }
    }
}

struct Inner {
    conn: Conn,
    display: String,
    ls: AtomicU8,
    closed: AtomicBool,
}

/// SCP over one SSH connection. Clones from `fork()` share the connection (every command
/// has its own channel); only the original closes it.
pub struct ScpFs {
    inner: Arc<Inner>,
    owner: bool,
}

/// `input` copied to an unlinked temporary file (SCP needs the size up front), rewound.
/// `progress(0)` is asked between blocks so Cancel works while copying.
fn spool(
    input: &mut dyn Read,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<(std::fs::File, u64), RemoteError> {
    static N: AtomicU64 = AtomicU64::new(0);
    let io_err = |e: io::Error| RemoteError::Other(format!("temporary file: {e}"));
    let path = std::env::temp_dir().join(format!(
        "fxfm-scp-{}-{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(io_err)?;
    let _ = std::fs::remove_file(&path);
    let mut buf = vec![0u8; BLOCK];
    let mut size = 0u64;
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(RemoteError::Other(format!("read: {e}"))),
        };
        file.write_all(&buf[..n]).map_err(io_err)?;
        size += n as u64;
        if !progress(0) {
            return Err(RemoteError::Cancelled);
        }
    }
    file.seek(SeekFrom::Start(0)).map_err(io_err)?;
    Ok((file, size))
}

impl ScpFs {
    /// Connects over SSH; uses the SFTP subsystem for listings when the server has one.
    pub fn connect(
        server: &ServerEntry,
        creds: &Credentials,
        opts: &ConnectOptions,
    ) -> Result<ScpFs, ConnectError> {
        let ssh = ssh_connect(server, creds, opts)?;
        let display = ssh.display("scp");
        let conn = match SftpFs::try_from_session(ssh, "scp") {
            Ok(sftp) => Conn::Sftp(sftp),
            // No (working) SFTP: commands only.
            Err((ssh, RemoteError::Unsupported(_) | RemoteError::Other(_))) if !ssh.is_closed() => {
                Conn::Ssh(*ssh)
            }
            Err((ssh, e)) => {
                rt().block_on(ssh.disconnect());
                return Err(match e {
                    RemoteError::Timeout => ConnectError::Timeout,
                    e => ConnectError::Failed(e.to_string()),
                });
            }
        };
        Ok(ScpFs {
            inner: Arc::new(Inner {
                conn,
                display,
                ls: AtomicU8::new(LS_UNKNOWN),
                closed: AtomicBool::new(false),
            }),
            owner: true,
        })
    }

    #[cfg(test)]
    fn uses_sftp(&self) -> bool {
        matches!(self.inner.conn, Conn::Sftp(_))
    }

    fn check_open(&self) -> Result<(), RemoteError> {
        if self.inner.closed.load(Ordering::SeqCst) {
            Err(RemoteError::Disconnected)
        } else {
            Ok(())
        }
    }

    /// The SFTP session, when listings go through SFTP.
    fn sftp(&self) -> Result<Option<&SftpFs>, RemoteError> {
        self.check_open()?;
        Ok(match &self.inner.conn {
            Conn::Sftp(s) => Some(s),
            Conn::Ssh(_) => None,
        })
    }

    /// One step of a command, bounded by the request timeout.
    fn io<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, RemoteError>>,
    ) -> Result<T, RemoteError> {
        with_timeout(fut, REQUEST_TIMEOUT)
    }

    fn start<'a>(&'a self, cmd: &str, path: &'a str) -> Result<Exec<'a>, RemoteError> {
        self.check_open()?;
        self.io(Exec::start(self.inner.conn.ssh(), cmd, path))
    }

    /// Reads to the end of the command; an exit code other than 0 is an error.
    fn finish(&self, ex: &mut Exec) -> Result<(), RemoteError> {
        while self.io(ex.pump())? {}
        match ex.status {
            Some(0) => Ok(()),
            None if ex.ssh.is_closed() => Err(RemoteError::Disconnected),
            status => Err(message_error(&ex.err, ex.path, status)),
        }
    }

    /// Runs `cmd` (about `path`) and returns its stdout.
    fn run(&self, cmd: &str, path: &str) -> Result<Vec<u8>, RemoteError> {
        let mut ex = self.start(cmd, path)?;
        self.finish(&mut ex)?;
        Ok(std::mem::take(&mut ex.out))
    }

    /// `ls <opt>` of `path`, parsed; GNU `ls` first, then BSD (remembered per connection).
    fn ls(
        &self,
        opt: &str,
        path: &str,
    ) -> Result<Vec<(String, RemoteStat, Option<String>)>, RemoteError> {
        let parse = |out: Vec<u8>, gnu: bool| {
            String::from_utf8_lossy(&out)
                .split('\n')
                .filter_map(|l| parse_ls(l, gnu))
                .collect()
        };
        let known = self.inner.ls.load(Ordering::SeqCst);
        if known != LS_BSD {
            let cmd = format!(
                "env LC_ALL=C {}",
                sh(&["ls", opt, "--time-style=+%s", "--", path])
            );
            match self.run(&cmd, path) {
                Ok(out) => {
                    self.inner.ls.store(LS_GNU, Ordering::SeqCst);
                    return Ok(parse(out, true));
                }
                // An `ls` without `--time-style` says so as `Other`.
                Err(RemoteError::Other(_)) if known == LS_UNKNOWN => {}
                Err(e) => return Err(e),
            }
        }
        // BSD `ls -T` prints local time; UTC makes it parseable.
        let cmd = format!("env LC_ALL=C TZ=UTC {}", sh(&["ls", opt, "-T", "--", path]));
        let out = self.run(&cmd, path)?;
        self.inner.ls.store(LS_BSD, Ordering::SeqCst);
        Ok(parse(out, false))
    }

    /// The `ls -lad` line of `path`; `None` when it does not exist.
    fn ls_one(&self, path: &str) -> Result<Option<(RemoteStat, Option<String>)>, RemoteError> {
        match self.ls("-lad", path) {
            Ok(v) => Ok(v.into_iter().next().map(|(_, st, t)| (st, t))),
            Err(RemoteError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn command(&self, words: &[&str], path: &str) -> Result<(), RemoteError> {
        self.run(&sh(words), path).map(|_| ())
    }

    fn upload(
        &self,
        ex: &mut Exec,
        name: &str,
        file: &mut std::fs::File,
        size: u64,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.io(ex.ack())?;
        self.io(ex.send(format!("C0644 {size} {name}\n").into_bytes()))?;
        self.io(ex.ack())?;
        let mut buf = vec![0u8; BLOCK];
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| RemoteError::Other(format!("temporary file: {e}")))?;
            if n == 0 {
                break;
            }
            self.io(ex.send(buf[..n].to_vec()))?;
            if !progress(n as u64) {
                return Err(RemoteError::Cancelled);
            }
        }
        self.io(ex.send(vec![0]))?;
        self.io(ex.ack())?;
        self.io(ex.eof())?;
        self.finish(ex)
    }

    fn download(
        &self,
        ex: &mut Exec,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        self.io(ex.send(vec![0]))?;
        let mut left = self.io(ex.header())?;
        self.io(ex.send(vec![0]))?;
        while left > 0 {
            let data = self.io(ex.read_some(left.min(BLOCK as u64) as usize))?;
            out.write_all(&data)
                .map_err(|e| RemoteError::Other(format!("write: {e}")))?;
            left -= data.len() as u64;
            if !progress(data.len() as u64) {
                return Err(RemoteError::Cancelled);
            }
        }
        self.io(ex.ack())?;
        self.io(ex.send(vec![0]))?;
        self.io(ex.eof())?;
        self.finish(ex)
    }
}

impl RemoteFs for ScpFs {
    fn display(&self) -> String {
        self.inner.display.clone()
    }

    fn home(&self) -> Result<String, RemoteError> {
        if let Some(sftp) = self.sftp()? {
            return sftp.home();
        }
        let out = self.run(&sh(&["pwd"]), ".")?;
        let home = String::from_utf8_lossy(&out)
            .trim_end_matches('\n')
            .to_string();
        if home.is_empty() {
            return Err(RemoteError::Other("empty pwd reply".into()));
        }
        Ok(home)
    }

    fn list(&self, path: &str) -> Result<Vec<Entry>, RemoteError> {
        if let Some(sftp) = self.sftp()? {
            return sftp.list(path);
        }
        // The trailing slash lists the folder a symlink points to.
        let dir = if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        };
        let entries = self.ls("-la", &dir).map_err(|e| match e {
            RemoteError::NotFound(_) => RemoteError::NotFound(path.to_string()),
            RemoteError::PermissionDenied(_) => RemoteError::PermissionDenied(path.to_string()),
            e => e,
        })?;
        Ok(entries
            .into_iter()
            .filter(|(n, _, _)| n != "." && n != "..")
            .map(|(name, st, _)| Entry {
                hidden: name.starts_with('.'),
                name,
                is_dir: st.is_dir,
                is_symlink: st.is_symlink,
                size: st.size,
                mtime: st.mtime,
                mode: st.mode,
            })
            .collect())
    }

    fn stat(&self, path: &str) -> Result<Option<RemoteStat>, RemoteError> {
        if let Some(sftp) = self.sftp()? {
            return sftp.stat(path);
        }
        Ok(self.ls_one(path)?.map(|(st, _)| st))
    }

    fn read_link(&self, path: &str) -> Result<String, RemoteError> {
        if let Some(sftp) = self.sftp()? {
            return sftp.read_link(path);
        }
        match self.ls_one(path)? {
            Some((_, Some(target))) => Ok(target),
            Some(_) => Err(RemoteError::Other(format!("{path}: not a symlink"))),
            None => Err(RemoteError::NotFound(path.to_string())),
        }
    }

    fn read_to(
        &self,
        path: &str,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let mut ex = self.start(&sh(&["scp", "-f", "--", path]), path)?;
        self.download(&mut ex, out, progress)
    }

    fn write_from(
        &self,
        path: &str,
        input: &mut dyn Read,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), RemoteError> {
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        if name.is_empty() || name.contains('\n') {
            return Err(RemoteError::Other(format!(
                "{path}: SCP cannot write a file with this name"
            )));
        }
        self.check_open()?;
        let (mut file, size) = spool(input, progress)?;
        // Into the folder, named in the `C` line: a folder at `path` is then an error
        // instead of a target to write into.
        let dir = parent_path(path.trim_end_matches('/'));
        let mut ex = self.start(&sh(&["scp", "-d", "-t", "--", dir]), path)?;
        self.upload(&mut ex, name, &mut file, size, progress)
    }

    fn mkdir(&self, path: &str) -> Result<(), RemoteError> {
        self.command(&["mkdir", "--", path], path)
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        self.command(&["rm", "--", path], path)
    }

    fn remove_dir(&self, path: &str) -> Result<(), RemoteError> {
        self.command(&["rmdir", "--", path], path)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        if self.stat(to)?.is_some() {
            return Err(RemoteError::Exists(to.to_string()));
        }
        let r = self.command(&["mv", "-n", "--", from, to], from);
        // `mv -n` leaves `from` in place when `to` appeared in the meantime (some
        // versions also fail then).
        if self.stat(from)?.is_some() && self.stat(to)?.is_some() {
            return Err(RemoteError::Exists(to.to_string()));
        }
        r
    }

    fn replace(&self, from: &str, to: &str) -> Result<(), RemoteError> {
        if let Some(sftp) = self.sftp()? {
            return sftp.replace(from, to);
        }
        // `mv` moves into a folder (also one behind a symlink) instead of replacing it.
        match self.ls_one(to)? {
            Some((st, _)) if st.is_symlink => {
                self.command(&["rm", "-f", "--", to], to)?;
                self.rename(from, to)
            }
            Some((st, _)) if st.is_dir => Err(RemoteError::Other(format!("{to}: is a folder"))),
            _ => self.command(&["mv", "-f", "--", from, to], from),
        }
    }

    fn symlink(&self, target: &str, link: &str) -> Result<(), RemoteError> {
        self.command(&["ln", "-s", "--", target, link], link)
    }

    fn set_mode(&self, path: &str, mode: u32) -> Result<(), RemoteError> {
        let mode = format!("{:o}", mode & 0o7777);
        self.command(&["chmod", "--", &mode, path], path)
    }

    fn fork(&self) -> Result<Arc<dyn RemoteFs>, RemoteError> {
        self.check_open()?;
        if self.inner.conn.ssh().is_closed() {
            return Err(RemoteError::Disconnected);
        }
        Ok(Arc::new(ScpFs {
            inner: self.inner.clone(),
            owner: false,
        }))
    }

    fn close(&self) {
        if !self.owner || self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        match &self.inner.conn {
            Conn::Sftp(sftp) => sftp.close(),
            Conn::Ssh(_) => {
                let inner = self.inner.clone();
                rt().spawn(async move {
                    inner.conn.ssh().disconnect().await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::known_hosts::{KnownHosts, KNOWN_HOSTS_FILE};
    use crate::remote::testutil::{have_sftp_server, SshTestServer};
    use crate::remote::{connect, join_path};
    use crate::servers::Protocol;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn shell_quote_cases() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(shell_quote(""), "''");
        // What the shell makes of it is the original string.
        for s in [
            "a b",
            "it's",
            "$(rm -rf ~)",
            "$x;y*.sh",
            "''",
            "a\nb",
            "čćž",
        ] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", shell_quote(s)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), s);
        }
    }

    fn st(is_dir: bool, is_symlink: bool, size: u64, mtime: i64, mode: u32) -> RemoteStat {
        RemoteStat {
            is_dir,
            is_symlink,
            size,
            mtime,
            mode,
        }
    }

    #[test]
    fn parse_ls_gnu_and_bsd() {
        // 2024-03-05 14:07:09 UTC
        let t = 1_709_647_629;
        let gnu = [
            "-rw-r--r-- 1 root root 1234 1709647629 file.txt",
            "drwxr-xr-x 2 root root 4096 1709647629 dir",
            "lrwxrwxrwx 1 root root 1 1709647629 a -> b",
            "-rw-r--r-- 1 root root 5 1709647629 name  with   spaces ",
            "-rw-r--r-- 1 root root 0 1709647629 x -> y",
            "-rwsr-xr-t. 1 www-data www-data 7 1709647629 suid",
        ];
        let bsd = [
            "-rw-r--r--  1 root  wheel  1234 Mar  5 14:07:09 2024 file.txt",
            "drwxr-xr-x  2 root  wheel  4096 Mar  5 14:07:09 2024 dir",
            "lrwxr-xr-x@ 1 root  wheel     1 Mar  5 14:07:09 2024 a -> b",
            "-rw-r--r--  1 root  wheel     5 Mar  5 14:07:09 2024 name  with   spaces ",
            "-rw-r--r--+ 1 root  wheel     0 Mar  5 14:07:09 2024 x -> y",
            "-rwsr-xr-t  1 www   www       7 Mar  5 14:07:09 2024 suid",
        ];
        let want = [
            ("file.txt", st(false, false, 1234, t, 0o100644), None),
            ("dir", st(true, false, 4096, t, 0o040755), None),
            ("a", st(false, true, 1, t, 0o120777), Some("b")),
            (
                "name  with   spaces ",
                st(false, false, 5, t, 0o100644),
                None,
            ),
            ("x -> y", st(false, false, 0, t, 0o100644), None),
            ("suid", st(false, false, 7, t, 0o104755 | 0o1000), None),
        ];
        for (lines, gnu_flag) in [(&gnu, true), (&bsd, false)] {
            for (line, (name, stat, target)) in lines.iter().zip(want.iter()) {
                let mut stat = stat.clone();
                if !gnu_flag && *name == "a" {
                    stat.mode = 0o120755;
                }
                assert_eq!(
                    parse_ls(line, gnu_flag),
                    Some((name.to_string(), stat, target.map(str::to_string))),
                    "{line}"
                );
            }
            assert_eq!(parse_ls("total 12", gnu_flag), None);
            assert_eq!(parse_ls("", gnu_flag), None);
        }
        // Device files show `major, minor` in place of the size.
        let (name, s, _) = parse_ls("crw-rw-rw- 1 root root 1, 3 1709647629 null", true).unwrap();
        assert_eq!((name.as_str(), s.size, s.mode), ("null", 0, 0o020666));
    }

    fn have_scp() -> bool {
        let ok = std::process::Command::new("sh")
            .args(["-c", "command -v scp"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipped: no scp");
        }
        ok
    }

    struct Ctx {
        srv: SshTestServer,
        data: tempfile::TempDir,
        known: KnownHosts,
    }

    impl Ctx {
        fn root(&self) -> String {
            self.srv.root.to_str().unwrap().to_string()
        }

        fn p(&self, name: &str) -> String {
            join_path(&self.root(), name)
        }

        fn opts(&self) -> ConnectOptions<'_> {
            self.known
                .trust(
                    "127.0.0.1",
                    self.srv.port,
                    &self.srv.host_key_type(),
                    &self.srv.host_fingerprint(),
                )
                .unwrap();
            ConnectOptions {
                data_dir: self.data.path(),
                known: &self.known,
                accept_fingerprint: None,
            }
        }

        fn entry(&self) -> ServerEntry {
            let mut e = self.srv.entry();
            e.protocol = Protocol::Scp;
            e
        }

        /// Connected through `connect()` (trusted host key, password `pw`).
        fn fs(&self) -> Arc<dyn RemoteFs> {
            connect(&self.entry(), &pw(), &self.opts()).unwrap()
        }

        fn scp(&self) -> ScpFs {
            ScpFs::connect(&self.entry(), &pw(), &self.opts()).unwrap()
        }
    }

    fn setup() -> Option<Ctx> {
        if !have_sftp_server() || !have_scp() {
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

    fn names(fs: &dyn RemoteFs, dir: &str) -> Vec<String> {
        let mut v: Vec<String> = fs.list(dir).unwrap().into_iter().map(|e| e.name).collect();
        v.sort();
        v
    }

    #[test]
    fn scp_roundtrip_against_test_server() {
        let Some(c) = setup() else { return };
        let fs = c.fs();
        assert_eq!(
            fs.display(),
            format!("scp://tester@127.0.0.1:{}", c.srv.port)
        );
        assert_eq!(fs.home().unwrap(), c.root());

        // 1 MB both ways, with progress adding up.
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut sent = 0u64;
        fs.write_from(&c.p("big.bin"), &mut &data[..], &mut |n| {
            sent += n;
            true
        })
        .unwrap();
        assert_eq!(sent, data.len() as u64);
        assert_eq!(fs::read(c.srv.root.join("big.bin")).unwrap(), data);
        let mut got = Vec::new();
        let mut recv = 0u64;
        fs.read_to(&c.p("big.bin"), &mut got, &mut |n| {
            recv += n;
            true
        })
        .unwrap();
        assert_eq!(got, data);
        assert_eq!(recv, data.len() as u64);

        // Overwrite truncates; empty files work.
        put(&*fs, &c.p("big.bin"), b"short");
        assert_eq!(get(&*fs, &c.p("big.bin")), b"short");
        put(&*fs, &c.p("empty"), b"");
        assert_eq!(get(&*fs, &c.p("empty")), b"");
        assert_eq!(
            fs.read_to(&c.p("missing"), &mut Vec::new(), &mut noop),
            Err(RemoteError::NotFound(c.p("missing")))
        );
        assert_eq!(
            fs.write_from(&c.p("nodir/x"), &mut &b"x"[..], &mut noop),
            Err(RemoteError::NotFound(c.p("nodir/x")))
        );
        let mut cancel = |_: u64| false;
        assert_eq!(
            fs.read_to(&c.p("big.bin"), &mut Vec::new(), &mut cancel),
            Err(RemoteError::Cancelled)
        );
        assert_eq!(
            fs.write_from(&c.p("c.bin"), &mut &data[..], &mut cancel),
            Err(RemoteError::Cancelled)
        );
        // Cancel while sending (copying to the temporary file asks with 0 bytes).
        let mut cancel_sending = |n: u64| n == 0;
        assert_eq!(
            fs.write_from(&c.p("c.bin"), &mut &data[..], &mut cancel_sending),
            Err(RemoteError::Cancelled)
        );
        // The connection is still usable after a cancel.
        assert_eq!(get(&*fs, &c.p("big.bin")), b"short");

        let s = fs.stat(&c.p("big.bin")).unwrap().unwrap();
        assert!(!s.is_dir && !s.is_symlink);
        assert_eq!(s.size, 5);
        assert_eq!(fs.stat(&c.p("missing")).unwrap(), None);

        fs.mkdir(&c.p("d")).unwrap();
        assert_eq!(fs.mkdir(&c.p("d")), Err(RemoteError::Exists(c.p("d"))));
        assert!(c.srv.root.join("d").is_dir());
        assert!(fs.stat(&c.p("d")).unwrap().unwrap().is_dir);
        assert!(matches!(
            fs.mkdir(&c.p("x/y")),
            Err(RemoteError::NotFound(_))
        ));

        // rename refuses a taken target and changes nothing.
        put(&*fs, &c.p("g.txt"), b"g");
        assert_eq!(
            fs.rename(&c.p("big.bin"), &c.p("g.txt")),
            Err(RemoteError::Exists(c.p("g.txt")))
        );
        assert_eq!(get(&*fs, &c.p("g.txt")), b"g");
        fs.rename(&c.p("big.bin"), &c.p("d/f2.bin")).unwrap();
        assert!(!c.srv.root.join("big.bin").exists());
        assert_eq!(fs::read(c.srv.root.join("d/f2.bin")).unwrap(), b"short");
        assert!(matches!(
            fs.rename(&c.p("missing"), &c.p("m2")),
            Err(RemoteError::NotFound(_))
        ));

        // replace overwrites.
        fs.replace(&c.p("d/f2.bin"), &c.p("g.txt")).unwrap();
        assert_eq!(fs::read(c.srv.root.join("g.txt")).unwrap(), b"short");
        assert!(!c.srv.root.join("d/f2.bin").exists());

        fs.set_mode(&c.p("g.txt"), 0o640).unwrap();
        let m = fs::metadata(c.srv.root.join("g.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o7777, 0o640);
        assert_eq!(
            fs.stat(&c.p("g.txt")).unwrap().unwrap().mode & 0o7777,
            0o640
        );

        fs.symlink("g.txt", &c.p("link")).unwrap();
        assert_eq!(
            fs::read_link(c.srv.root.join("link")).unwrap().to_str(),
            Some("g.txt")
        );
        assert_eq!(fs.read_link(&c.p("link")).unwrap(), "g.txt");
        assert!(fs.stat(&c.p("link")).unwrap().unwrap().is_symlink);

        fs.remove_file(&c.p("link")).unwrap();
        assert!(fs::symlink_metadata(c.srv.root.join("link")).is_err());
        assert!(c.srv.root.join("g.txt").exists());
        fs.remove_file(&c.p("g.txt")).unwrap();
        assert!(matches!(
            fs.remove_file(&c.p("g.txt")),
            Err(RemoteError::NotFound(_))
        ));
        fs.remove_dir(&c.p("d")).unwrap();
        assert!(!c.srv.root.join("d").exists());

        // fork shares the connection; closing the fork leaves the original usable.
        let f = fs.fork().unwrap();
        assert!(f.list(&c.root()).is_ok());
        f.close();
        assert!(fs.home().is_ok());
        fs.close();
        assert_eq!(fs.home(), Err(RemoteError::Disconnected));
        assert_eq!(fs.mkdir(&c.p("z")), Err(RemoteError::Disconnected));
        fs.close();
    }

    /// Upload, list, rename, download and delete of hostile names, with or without SFTP.
    fn shell_chars_case(c: &Ctx, fs: &dyn RemoteFs) {
        fs::write(c.srv.root.join("keep.txt"), b"keep").unwrap();
        fs::create_dir(c.srv.root.join("keepdir")).unwrap();
        fs::write(c.srv.root.join("keepdir/inner"), b"in").unwrap();
        let names = ["a b.txt", "it's.php", "$x;y*.sh", "čćž.txt", "-n", "--"];
        for n in names {
            let body = format!("content of {n}").into_bytes();
            put(fs, &c.p(n), &body);
            assert_eq!(fs::read(c.srv.root.join(n)).unwrap(), body, "{n}");
            assert!(names_of(fs, c).contains(&n.to_string()), "{n}");
            let s = fs.stat(&c.p(n)).unwrap().unwrap();
            assert_eq!(s.size, body.len() as u64, "{n}");
            let renamed = format!("{n} renamed");
            fs.rename(&c.p(n), &c.p(&renamed)).unwrap();
            assert!(!c.srv.root.join(n).exists(), "{n}");
            assert_eq!(get(fs, &c.p(&renamed)), body, "{n}");
            fs.set_mode(&c.p(&renamed), 0o600).unwrap();
            fs.remove_file(&c.p(&renamed)).unwrap();
            assert!(!c.srv.root.join(&renamed).exists(), "{n}");
        }
        // Folders with such names too.
        fs.mkdir(&c.p("$x;y*")).unwrap();
        fs.remove_dir(&c.p("$x;y*")).unwrap();
        assert_eq!(names_of(fs, c), vec!["keep.txt", "keepdir"]);
        assert_eq!(fs::read(c.srv.root.join("keepdir/inner")).unwrap(), b"in");
        assert!(!c.srv.root.join("y").exists());
    }

    fn names_of(fs: &dyn RemoteFs, c: &Ctx) -> Vec<String> {
        names(fs, &c.root())
    }

    #[test]
    fn scp_names_with_shell_chars() {
        let Some(c) = setup() else { return };
        let fs = c.scp();
        assert!(fs.uses_sftp());
        shell_chars_case(&c, &fs);

        let Some(c) = setup() else { return };
        c.srv.disable_sftp();
        let fs = c.scp();
        assert!(!fs.uses_sftp());
        shell_chars_case(&c, &fs);
    }

    #[test]
    fn scp_replace_onto_symlinked_folder() {
        for sftp in [true, false] {
            let Some(c) = setup() else { return };
            let r = &c.srv.root;
            fs::create_dir(r.join("dir")).unwrap();
            fs::write(r.join("dir/inside"), b"in").unwrap();
            std::os::unix::fs::symlink("dir", r.join("link")).unwrap();
            fs::write(r.join("src.txt"), b"new").unwrap();
            fs::create_dir(r.join("real")).unwrap();
            fs::write(r.join("src2.txt"), b"two").unwrap();
            if !sftp {
                c.srv.disable_sftp();
            }
            let fs = c.scp();
            assert_eq!(fs.uses_sftp(), sftp);

            fs.replace(&c.p("src.txt"), &c.p("link")).unwrap();
            assert!(!fs::symlink_metadata(r.join("link"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read(r.join("link")).unwrap(), b"new", "sftp={sftp}");
            assert!(!r.join("src.txt").exists());
            let mut inside: Vec<String> = fs::read_dir(r.join("dir"))
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            inside.sort();
            assert_eq!(inside, vec!["inside"], "sftp={sftp}");

            // A real folder is never replaced (nor written into).
            assert!(fs.replace(&c.p("src2.txt"), &c.p("real")).is_err());
            assert!(r.join("src2.txt").exists());
            assert_eq!(fs::read_dir(r.join("real")).unwrap().count(), 0);
        }
    }

    #[test]
    fn scp_list_without_sftp_uses_ls() {
        let Some(c) = setup() else { return };
        let r = &c.srv.root;
        fs::write(r.join("a.txt"), b"hello").unwrap();
        fs::write(r.join(".hidden"), b"x").unwrap();
        fs::write(r.join("name  with   spaces "), vec![1u8; 3000]).unwrap();
        fs::write(r.join("x -> y"), b"arrow").unwrap();
        fs::write(r.join("čćž.txt"), b"utf").unwrap();
        fs::create_dir(r.join("sub")).unwrap();
        fs::write(r.join("sub/inner"), b"inner").unwrap();
        std::os::unix::fs::symlink("a.txt", r.join("ln")).unwrap();
        std::os::unix::fs::symlink("sub", r.join("dirlink")).unwrap();
        fs::set_permissions(r.join("a.txt"), fs::Permissions::from_mode(0o4751)).unwrap();

        let with_sftp = c.scp();
        assert!(with_sftp.uses_sftp());
        c.srv.disable_sftp();
        let plain = c.scp();
        assert!(!plain.uses_sftp());

        type Row = (String, bool, bool, bool, u64, i64, u32);
        let rows = |fs: &ScpFs, dir: &str| -> Vec<Row> {
            let mut v: Vec<Row> = fs
                .list(dir)
                .unwrap()
                .into_iter()
                .map(|e| {
                    (
                        e.name,
                        e.is_dir,
                        e.is_symlink,
                        e.hidden,
                        e.size,
                        e.mtime,
                        e.mode,
                    )
                })
                .collect();
            v.sort();
            v
        };
        let want = rows(&with_sftp, &c.root());
        assert_eq!(want.len(), 8, "{want:?}");
        assert_eq!(rows(&plain, &c.root()), want);
        // A symlink to a folder lists the folder.
        assert_eq!(
            rows(&plain, &c.p("dirlink")),
            rows(&with_sftp, &c.p("dirlink"))
        );
        assert_eq!(rows(&plain, &c.p("dirlink")).len(), 1);

        for n in ["a.txt", "sub", "ln", "x -> y", "čćž.txt"] {
            assert_eq!(plain.stat(&c.p(n)), with_sftp.stat(&c.p(n)), "{n}");
        }
        assert_eq!(plain.stat(&c.p("nope")), Ok(None));
        assert_eq!(plain.read_link(&c.p("ln")).unwrap(), "a.txt");
        assert_eq!(plain.home().unwrap(), with_sftp.home().unwrap());
        assert!(matches!(
            plain.list(&c.p("nope")),
            Err(RemoteError::NotFound(_))
        ));
    }
}
