//! SMB shares: macOS mounts them (NetFS framework) under /Volumes and fxfm browses the mount
//! point as local files. Unmounting runs `diskutil unmount`.
//!
//! The password goes to NetFS as its own argument; it is never part of the URL, a log line or
//! an error text.

use fxfm_core::remote::ConnectError;
use fxfm_core::servers::{matches_mount, ServerEntry};
use std::path::PathBuf;

/// A share without a user name and a server that wants one.
pub const SMB_NO_GUEST: &str = "The server does not allow guest access; enter a user name";
/// NetFS gave up (ETIMEDOUT); fxfm sets no time limit of its own for mounting.
pub const SMB_TIMEOUT: &str = "The SMB server did not answer.";
/// A Kerberos entry the server did not let in (no ticket, an expired one, or another realm).
pub const SMB_KERBEROS_FAILED: &str =
    "The Kerberos login was not accepted; get a ticket first (kinit or Ticket Viewer)";

/// NetFS's (errno) status for rejected credentials.
const EAUTH: i32 = 80;

/// One mounted smbfs volume, with its space as the mount table has it.
#[derive(Clone, Debug, PartialEq)]
pub struct SmbMount {
    /// `f_mntfromname`, e.g. `//user@host/share`.
    pub from: String,
    /// `f_mntonname`, the mount point.
    pub on: String,
    pub total: u64,
    /// Available to unprivileged users.
    pub free: u64,
}

/// The mount table, read with `MNT_NOWAIT` so a hung server never blocks this (the space is
/// what the kernel last knew).
#[cfg(target_os = "macos")]
fn fsstat() -> Vec<libc::statfs> {
    // getfsstat with our own buffer (getmntinfo's buffer is shared by the process).
    // SAFETY: a null buffer only asks for the count.
    let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if n <= 0 {
        return Vec::new();
    }
    // Room for a few mounts appearing in between.
    let cap = n as usize + 8;
    let mut buf: Vec<libc::statfs> = Vec::with_capacity(cap);
    let size = (cap * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: `buf` has room for `cap` entries and `size` says so.
    let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), size, libc::MNT_NOWAIT) };
    if got <= 0 {
        return Vec::new();
    }
    // SAFETY: getfsstat initialised the first `got` (<= cap) entries.
    unsafe { buf.set_len((got as usize).min(cap)) };
    buf
}

/// A fixed-size, NUL-terminated string field of `statfs`.
#[cfg(target_os = "macos")]
fn cstr(b: &[libc::c_char]) -> String {
    // SAFETY: the kernel fills these fixed arrays with NUL-terminated strings.
    unsafe { std::ffi::CStr::from_ptr(b.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// File system types of network volumes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const NETWORK_FS: [&str; 4] = ["smbfs", "afpfs", "nfs", "webdav"];

/// Mounted smbfs volumes, with their space as the mount table has it.
#[cfg(target_os = "macos")]
pub fn smb_mounts() -> Vec<SmbMount> {
    fsstat()
        .iter()
        .filter(|st| cstr(&st.f_fstypename) == "smbfs")
        .map(|st| {
            let bsize = st.f_bsize as u64;
            SmbMount {
                from: cstr(&st.f_mntfromname),
                on: cstr(&st.f_mntonname),
                total: bsize * st.f_blocks,
                free: bsize * st.f_bavail,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn smb_mounts() -> Vec<SmbMount> {
    let (mounts, gvfs_dir, gvfs) = linux_mount_sources();
    linux_smb_mounts(&mounts, &gvfs_dir, &gvfs)
        .into_iter()
        .map(|(from, on)| {
            let (total, free) =
                fxfm_core::volumes::space(std::path::Path::new(&on)).unwrap_or((0, 0));
            SmbMount {
                from,
                on,
                total,
                free,
            }
        })
        .collect()
}

/// `/proc/self/mounts`, the gvfs folder and the names in it.
#[cfg(target_os = "linux")]
fn linux_mount_sources() -> (String, String, Vec<String>) {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    // SAFETY: getuid cannot fail.
    let gvfs_dir = format!("/run/user/{}/gvfs", unsafe { libc::getuid() });
    let gvfs = std::fs::read_dir(&gvfs_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    (mounts, gvfs_dir, gvfs)
}

/// Linux SMB mounts as `(mntfrom, mount point)`: kernel CIFS mounts (`//host/share` in the
/// mount table) and shares mounted by gvfs (GNOME, `smb-share:server=…,share=…[,user=…]
/// [,domain=…]` folders), whose mntfrom is written as macOS would (`//[domain;user@]host/share`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_smb_mounts(mounts: &str, gvfs_dir: &str, gvfs: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 && matches!(f[2], "cifs" | "smb3") {
            let unesc = |s: &str| s.replace("\\040", " ").replace("\\134", "\\");
            out.push((unesc(f[0]).replace('\\', "/"), unesc(f[1])));
        }
    }
    for name in gvfs {
        let Some(rest) = name.strip_prefix("smb-share:") else {
            continue;
        };
        let field = |k: &str| {
            rest.split(',')
                .find_map(|p| p.strip_prefix(&format!("{k}=")).map(str::to_string))
        };
        let (Some(server), Some(share)) = (field("server"), field("share")) else {
            continue;
        };
        let user = match (field("domain"), field("user")) {
            (Some(d), Some(u)) => format!("{d};{u}@"),
            (None, Some(u)) => format!("{u}@"),
            _ => String::new(),
        };
        out.push((
            format!("//{user}{server}/{share}"),
            format!("{gvfs_dir}/{name}"),
        ));
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn smb_mounts() -> Vec<SmbMount> {
    Vec::new()
}

/// Mounted network volumes (smbfs, afpfs, nfs, webdav) as (mount point, fstype, mntfrom).
#[cfg(target_os = "macos")]
pub fn network_mounts() -> Vec<(String, String, String)> {
    fsstat()
        .iter()
        .filter_map(|st| {
            let fs = cstr(&st.f_fstypename);
            NETWORK_FS
                .contains(&fs.as_str())
                .then(|| (cstr(&st.f_mntonname), fs, cstr(&st.f_mntfromname)))
        })
        .collect()
}

/// Mounted network volumes (CIFS/SMB, NFS, gvfs shares) as (mount point, fstype, mntfrom);
/// the fstype is named as on macOS (`smbfs`, `nfs`).
#[cfg(target_os = "linux")]
pub fn network_mounts() -> Vec<(String, String, String)> {
    let (mounts, gvfs_dir, gvfs) = linux_mount_sources();
    let mut out: Vec<(String, String, String)> = linux_smb_mounts(&mounts, &gvfs_dir, &gvfs)
        .into_iter()
        .map(|(from, on)| (on, "smbfs".to_string(), from))
        .collect();
    for line in mounts.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 && matches!(f[2], "nfs" | "nfs4") {
            out.push((f[1].replace("\\040", " "), "nfs".into(), f[0].to_string()));
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn network_mounts() -> Vec<(String, String, String)> {
    Vec::new()
}

/// The mount of the entry's share now (also one made by Finder or by hand), from `mounts`.
pub fn mount_of<'a>(entry: &ServerEntry, mounts: &'a [SmbMount]) -> Option<&'a SmbMount> {
    mounts
        .iter()
        .find(|m| matches_mount(entry, &m.from))
        .or_else(|| {
            mounts.iter().find(|m| {
                fxfm_core::servers::matches_mount_resolving(entry, &m.from, &resolve_cached)
            })
        })
}

/// A host name's addresses, cached for five minutes (also when it does not resolve), so the
/// drive list does not wait on DNS each time.
fn resolve_cached(host: &str) -> Vec<std::net::IpAddr> {
    use std::collections::HashMap;
    use std::net::ToSocketAddrs;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};
    type Cache = HashMap<String, (Instant, Vec<std::net::IpAddr>)>;
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);
    let key = host.to_lowercase();
    if let Some((at, ips)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if at.elapsed() < Duration::from_secs(300) {
            return ips.clone();
        }
    }
    let ips: Vec<std::net::IpAddr> = (host, 445u16)
        .to_socket_addrs()
        .map(|a| a.map(|s| s.ip()).collect())
        .unwrap_or_default();
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, (Instant::now(), ips.clone()));
    ips
}

/// Where the entry's share is mounted now, from `mounts`.
pub fn find_mount_in(entry: &ServerEntry, mounts: &[SmbMount]) -> Option<PathBuf> {
    mount_of(entry, mounts).map(|m| PathBuf::from(&m.on))
}

pub fn find_mount(entry: &ServerEntry) -> Option<PathBuf> {
    find_mount_in(entry, &smb_mounts())
}

/// Text of a NetFS error (errno when positive, OSStatus when negative).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mount_error(code: i32) -> ConnectError {
    const ENETFSACCOUNTRESTRICTED: i32 = -5999;
    const ENETFSNOSHARESAVAIL: i32 = -5998;
    const ENETFSNOAUTHMECHSUPP: i32 = -5997;
    const ENETFSNOPROTOVERSSUPP: i32 = -5996;
    const ENETFSPWDNEEDSCHANGE: i32 = -5045;
    const GUEST_NOT_SUPPORTED: i32 = -6004;
    const USER_CANCELED: i32 = -128;
    let text = match code {
        EAUTH => return ConnectError::AuthFailed,
        GUEST_NOT_SUPPORTED => SMB_NO_GUEST.to_string(),
        ENETFSPWDNEEDSCHANGE => "The password must be changed on the server first".into(),
        ENETFSACCOUNTRESTRICTED => "The account is locked or restricted on the server".into(),
        ENETFSNOSHARESAVAIL => "The server has no shares for this user".into(),
        ENETFSNOAUTHMECHSUPP => "The server offers no login method macOS supports".into(),
        ENETFSNOPROTOVERSSUPP => "The server offers no SMB version macOS supports".into(),
        USER_CANCELED => "Mounting was cancelled".into(),
        60 => SMB_TIMEOUT.to_string(),
        2 => "The share was not found on the server".into(),
        c if c > 0 => format!(
            "Cannot mount the share: {}",
            std::io::Error::from_raw_os_error(c)
        ),
        c => format!("Cannot mount the share (error {c})"),
    };
    ConnectError::Failed(text)
}

/// NetFS's mount status, with one more try for a domain login: `call(true)` mounts with the
/// user argument; when the server rejects that and the user field has a domain, `call(false)`
/// mounts without it (the domain and the user stay in the URL, `DOMAIN;user@host`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn with_domain_retry<T>(entry: &ServerEntry, mut call: impl FnMut(bool) -> (i32, T)) -> (i32, T) {
    let first = call(true);
    let domain = fxfm_core::servers::smb_user(&entry.user).contains(';');
    if first.0 == EAUTH && domain && !entry.kerberos {
        call(false)
    } else {
        first
    }
}

/// Mounts the entry's share with NetFS (default mount point under /Volumes, no NetFS UI) and
/// returns the mount point. A share that is already mounted (also when NetFS says so with
/// EEXIST) returns its mount point. A Kerberos entry gets neither a user argument nor a
/// password, so NetFS logs in with the system's ticket (the user, if any, is in the URL).
/// Blocking.
#[cfg(target_os = "macos")]
pub fn mount(entry: &ServerEntry, password: Option<&str>) -> Result<PathBuf, ConnectError> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFMutableDictionary, CFMutableDictionaryRef};
    use core_foundation::string::{kCFStringEncodingUTF8, CFString, CFStringRef};
    use core_foundation::url::{CFURLCreateWithBytes, CFURLRef, CFURL};
    use fxfm_core::servers::{netfs_user, smb_url, smb_user};

    #[link(name = "NetFS", kind = "framework")]
    extern "C" {
        fn NetFSMountURLSync(
            url: CFURLRef,
            mountpath: CFURLRef,
            user: CFStringRef,
            passwd: CFStringRef,
            open_options: CFMutableDictionaryRef,
            mount_options: CFMutableDictionaryRef,
            mountpoints: *mut CFArrayRef,
        ) -> i32;
    }
    const EEXIST: i32 = 17;

    let url = smb_url(entry);
    // SAFETY: valid UTF-8 bytes with their length; a null base URL.
    let url_ref = unsafe {
        CFURLCreateWithBytes(
            kCFAllocatorDefault,
            url.as_ptr(),
            url.len() as _,
            kCFStringEncodingUTF8,
            std::ptr::null(),
        )
    };
    if url_ref.is_null() {
        return Err(ConnectError::Failed("Invalid host or share name".into()));
    }
    // SAFETY: a +1 reference from a Create function.
    let url_cf = unsafe { CFURL::wrap_under_create_rule(url_ref) };

    let mut open: CFMutableDictionary<CFString, CFType> = CFMutableDictionary::new();
    // fxfm asks for passwords itself; NetFS must never show its own dialog.
    open.set(
        CFString::from_static_string("UIOption"),
        CFString::from_static_string("NoUI").as_CFType(),
    );
    let user = smb_user(&entry.user);
    if user.is_empty() && !entry.kerberos {
        open.set(
            CFString::from_static_string("Guest"),
            CFBoolean::true_value().as_CFType(),
        );
    }
    let mount_opts: CFMutableDictionary<CFString, CFType> = CFMutableDictionary::new();
    let login = !user.is_empty() && !entry.kerberos;
    // The domain is in the URL; NetFS gets the bare name (see `netfs_user`).
    let user_cf = login.then(|| CFString::new(&netfs_user(&entry.user)));
    let pass_cf = password.filter(|_| login).map(CFString::new);
    let (status, first) = with_domain_retry(entry, |with_user| {
        let mut points: CFArrayRef = std::ptr::null();
        let user_ref = user_cf
            .as_ref()
            .filter(|_| with_user)
            .map_or(std::ptr::null(), |s| s.as_concrete_TypeRef());
        // SAFETY: every reference is valid for the call (or null where NetFS allows it);
        // `points` is an out-pointer NetFS fills with a +1 array on success.
        let status = unsafe {
            NetFSMountURLSync(
                url_cf.as_concrete_TypeRef(),
                std::ptr::null(),
                user_ref,
                pass_cf
                    .as_ref()
                    .map_or(std::ptr::null(), |s| s.as_concrete_TypeRef()),
                open.as_concrete_TypeRef(),
                mount_opts.as_concrete_TypeRef(),
                &mut points,
            )
        };
        let first = if points.is_null() {
            None
        } else {
            // SAFETY: a +1 array of CFStrings from NetFS.
            let arr: CFArray<CFString> = unsafe { CFArray::wrap_under_create_rule(points) };
            arr.get(0).map(|s| s.to_string())
        };
        (status, first)
    });
    drop(pass_cf);
    match status {
        0 => first
            .map(PathBuf::from)
            .or_else(|| find_mount(entry))
            .ok_or_else(|| ConnectError::Failed("The share was mounted, but not found".into())),
        EEXIST => find_mount(entry).ok_or_else(|| {
            ConnectError::Failed("The share is mounted, but its folder was not found".into())
        }),
        c => Err(mount_error(c)),
    }
}

/// Linux: mounts the share with gvfs (as the file managers do) and returns its folder under
/// `/run/user/<uid>/gvfs`. The login is answered by a mount operation in code (the password is
/// never an argument, never saved by gvfs); a guest gets an anonymous login; a Kerberos entry
/// answers no password, so gvfs uses the ticket. Runs its own GLib main loop on this thread.
/// Blocking, at most 60 s.
#[cfg(target_os = "linux")]
pub fn mount(entry: &ServerEntry, password: Option<&str>) -> Result<PathBuf, ConnectError> {
    use fxfm_core::servers::{smb_url, smb_user};
    use gtk::gio::{self, prelude::*};
    use gtk::glib;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    if let Some(p) = find_mount(entry) {
        return Ok(p);
    }
    let url = smb_url(entry);
    let user = smb_user(&entry.user);
    let login = !user.is_empty() && !entry.kerberos;
    if login && password.is_none() {
        return Err(ConnectError::NeedPassword);
    }
    let (domain, name) = match user.split_once(';') {
        Some((d, n)) => (Some(d.to_string()), n.to_string()),
        None => (None, user.clone()),
    };
    let password = password.map(str::to_string);
    let ctx = glib::MainContext::new();
    let outcome = ctx
        .with_thread_default(|| {
            let ml = glib::MainLoop::new(Some(&ctx), false);
            let op = gio::MountOperation::new();
            op.set_password_save(gio::PasswordSave::Never);
            if login {
                op.set_username(Some(&name));
                op.set_domain(domain.as_deref());
                op.set_password(password.as_deref());
            }
            let asked = Rc::new(Cell::new(0u32));
            let a = asked.clone();
            op.connect_ask_password(move |op, _msg, _user, _domain, flags| {
                a.set(a.get() + 1);
                // A second question means the first answer was rejected.
                if a.get() > 1 || entry_is_kerberos_without_answer(login, flags) {
                    op.reply(gio::MountOperationResult::Aborted);
                    return;
                }
                if !login && flags.contains(gio::AskPasswordFlags::ANONYMOUS_SUPPORTED) {
                    op.set_anonymous(true);
                }
                op.reply(gio::MountOperationResult::Handled);
            });
            let result: Rc<RefCell<Option<Result<(), glib::Error>>>> = Rc::default();
            let r = result.clone();
            let m = ml.clone();
            gio::File::for_uri(&url).mount_enclosing_volume(
                gio::MountMountFlags::NONE,
                Some(&op),
                gio::Cancellable::NONE,
                move |res| {
                    *r.borrow_mut() = Some(res);
                    m.quit();
                },
            );
            let m = ml.clone();
            let timer = glib::timeout_source_new(
                std::time::Duration::from_secs(60),
                None,
                glib::Priority::DEFAULT,
                move || {
                    m.quit();
                    glib::ControlFlow::Break
                },
            );
            timer.attach(Some(&ctx));
            ml.run();
            timer.destroy();
            let res = result.borrow_mut().take();
            (res, asked.get())
        })
        .map_err(|e| ConnectError::Failed(format!("Cannot mount the share: {e}")))?;
    match outcome {
        (None, _) => Err(ConnectError::Failed(SMB_TIMEOUT.to_string())),
        (Some(Ok(())), _) => find_mount(entry)
            .ok_or_else(|| ConnectError::Failed("The share was mounted, but not found".into())),
        (Some(Err(e)), _) if e.matches(gio::IOErrorEnum::AlreadyMounted) => find_mount(entry)
            .ok_or_else(|| {
                ConnectError::Failed("The share is mounted, but its folder was not found".into())
            }),
        (Some(Err(_)), asked) if asked > 1 => Err(ConnectError::AuthFailed),
        (Some(Err(e)), _) if e.matches(gio::IOErrorEnum::PermissionDenied) => Err(if login {
            ConnectError::AuthFailed
        } else {
            ConnectError::Failed(SMB_NO_GUEST.to_string())
        }),
        (Some(Err(e)), _) if e.matches(gio::IOErrorEnum::NotFound) => Err(ConnectError::Failed(
            "The share was not found on the server".into(),
        )),
        (Some(Err(e)), _) if e.matches(gio::IOErrorEnum::TimedOut) => {
            Err(ConnectError::Failed(SMB_TIMEOUT.to_string()))
        }
        (Some(Err(e)), _) => Err(ConnectError::Failed(format!(
            "Cannot mount the share: {}",
            e.message()
        ))),
    }
}

/// A Kerberos entry answers nothing: asked for a password, it gives up (no ticket, or rejected).
#[cfg(target_os = "linux")]
fn entry_is_kerberos_without_answer(login: bool, flags: gtk::gio::AskPasswordFlags) -> bool {
    !login && !flags.contains(gtk::gio::AskPasswordFlags::ANONYMOUS_SUPPORTED)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn mount(_entry: &ServerEntry, _password: Option<&str>) -> Result<PathBuf, ConnectError> {
    Err(ConnectError::Failed(
        "SMB shares cannot be mounted here".into(),
    ))
}

/// Unmounts a share (never forced): `diskutil unmount` on macOS; on Linux `gio mount -u` for a
/// gvfs share, else `umount` (a kernel CIFS mount the user may unmount). The error is the
/// tool's own text, e.g. when a file on the share is still open.
pub fn unmount(mount_point: &std::path::Path) -> Result<(), String> {
    let (tool, args): (&str, Vec<&std::ffi::OsStr>) = if cfg!(target_os = "macos") {
        (
            "/usr/sbin/diskutil",
            vec!["unmount".as_ref(), mount_point.as_os_str()],
        )
    } else if mount_point.to_string_lossy().contains("/gvfs/") {
        (
            "gio",
            vec!["mount".as_ref(), "-u".as_ref(), mount_point.as_os_str()],
        )
    } else {
        ("umount", vec![mount_point.as_os_str()])
    };
    let out = std::process::Command::new(tool)
        .args(&args)
        .output()
        .map_err(|e| format!("Cannot run {tool}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let err = text(&out.stderr);
    Err(if err.is_empty() {
        let o = text(&out.stdout);
        if o.is_empty() {
            "Unmount failed".to_string()
        } else {
            o
        }
    } else {
        err
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxfm_core::servers::Protocol;

    fn entry(host: &str, share: &str) -> ServerEntry {
        ServerEntry {
            name: "nas".into(),
            protocol: Protocol::Smb,
            host: host.into(),
            port: 445,
            share: share.into(),
            ..ServerEntry::default()
        }
    }

    #[test]
    fn find_mount_in_picks_the_matching_share() {
        let m = |from: &str, on: &str| SmbMount {
            from: from.into(),
            on: on.into(),
            total: 10,
            free: 4,
        };
        let mounts = vec![
            m("//zb@nas/other", "/Volumes/other"),
            m("//zb@NAS/Data", "/Volumes/Data-1"),
        ];
        assert_eq!(
            find_mount_in(&entry("nas", "data"), &mounts),
            Some(PathBuf::from("/Volumes/Data-1"))
        );
        assert_eq!(find_mount_in(&entry("nas", "x"), &mounts), None);
        assert_eq!(find_mount_in(&entry("nas2", "data"), &[]), None);
    }

    #[test]
    fn mount_errors_never_ask_for_ui() {
        assert!(matches!(mount_error(80), ConnectError::AuthFailed));
        // Not fxfm's 15 s connect timeout text: NetFS has its own limits.
        match mount_error(60) {
            ConnectError::Failed(m) => assert_eq!(m, SMB_TIMEOUT),
            _ => panic!(),
        }
        match mount_error(2) {
            ConnectError::Failed(m) => assert_eq!(m, "The share was not found on the server"),
            _ => panic!(),
        }
        match mount_error(-6004) {
            ConnectError::Failed(m) => assert_eq!(m, SMB_NO_GUEST),
            _ => panic!(),
        }
        match mount_error(-1234) {
            ConnectError::Failed(m) => assert_eq!(m, "Cannot mount the share (error -1234)"),
            _ => panic!(),
        }
    }

    #[test]
    fn domain_login_is_tried_once_more_without_the_user_argument() {
        let run = |user: &str, kerberos: bool, statuses: &[i32]| {
            let mut e = entry("nas", "data");
            e.user = user.into();
            e.kerberos = kerberos;
            let mut calls = Vec::new();
            let mut next = statuses.iter();
            let (status, ()) = with_domain_retry(&e, |with_user| {
                calls.push(with_user);
                (*next.next().unwrap(), ())
            });
            (status, calls)
        };
        // A domain login the server rejects: once more without the user argument.
        assert_eq!(run("CORP\\zb", false, &[80, 0]), (0, vec![true, false]));
        assert_eq!(run("CORP;zb", false, &[80, 80]), (80, vec![true, false]));
        // Success, another error, no domain or Kerberos: a single try.
        assert_eq!(run("CORP\\zb", false, &[0]), (0, vec![true]));
        assert_eq!(run("CORP\\zb", false, &[60]), (60, vec![true]));
        assert_eq!(run("zb", false, &[80]), (80, vec![true]));
        assert_eq!(run("CORP\\zb", true, &[80]), (80, vec![true]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn smb_mounts_reads_without_error() {
        // Only reads the mount table; whatever is mounted, each row is an smbfs one.
        for m in smb_mounts() {
            assert!(m.from.starts_with("//"), "{}", m.from);
            assert!(m.free <= m.total);
        }
    }
}

// ---------------------------------------------------------------- Bonjour browsing

/// A computer that offers SMB on the local network (Bonjour or NetBIOS).
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct SmbHost {
    /// The name it announces (as Finder's sidebar shows it), or its NetBIOS name.
    pub name: String,
    /// Its host name to connect to (`nas.local`), the Bonjour name when not resolved, or the
    /// NetBIOS name (macOS resolves it when mounting).
    pub host: String,
    /// The address it answered from (NetBIOS only), shown next to the name.
    pub address: Option<String>,
}

/// Instance names of `dns-sd -B _smb._tcp` output ("Add" lines), each once, in order.
pub fn parse_browse(out: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for line in out.lines() {
        let Some(i) = line.find("_smb._tcp.") else {
            continue;
        };
        if !line.contains(" Add ") {
            continue;
        }
        let name = line[i + "_smb._tcp.".len()..].trim().to_string();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The host of `dns-sd -L` output ("… can be reached at nas.local.:445 …"), without the dot.
pub fn parse_resolve(out: &str) -> Option<String> {
    let at = out.find("can be reached at ")? + "can be reached at ".len();
    let rest = &out[at..];
    let hostport = rest.split_whitespace().next()?;
    let host = hostport.rsplit_once(':').map_or(hostport, |(h, _)| h);
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_string())
}

/// Linux: resolved SMB services of `avahi-browse -rpt _smb._tcp` (`=;iface;proto;name;type;
/// domain;host;address;port;txt` lines; `\\DDD` decimal escapes in the name), each host once.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_avahi(out: &str) -> Vec<SmbHost> {
    let unescape = |s: &str| {
        let b = s.as_bytes();
        let mut v = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(u8::is_ascii_digit) {
                let n = (b[i + 1] - b'0') as u32 * 100
                    + (b[i + 2] - b'0') as u32 * 10
                    + (b[i + 3] - b'0') as u32;
                if n < 256 {
                    v.push(n as u8);
                    i += 4;
                    continue;
                }
            }
            v.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&v).into_owned()
    };
    let mut hosts: Vec<SmbHost> = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split(';').collect();
        if f.len() < 9 || f[0] != "=" {
            continue;
        }
        let host = f[6].trim_end_matches('.').to_string();
        if host.is_empty() || hosts.iter().any(|h| h.host == host) {
            continue;
        }
        hosts.push(SmbHost {
            name: unescape(f[3]),
            host,
            address: None,
        });
    }
    hosts
}

/// `avahi-browse -rpt _smb._tcp` output, stopped after `wait` at the latest (empty without Avahi).
#[cfg(target_os = "linux")]
fn avahi_browse(wait: std::time::Duration) -> String {
    use std::io::Read;
    let Ok(mut child) = std::process::Command::new("avahi-browse")
        .args(["-rpt", "_smb._tcp"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let _ = child.wait();
    out
}

/// Output of `dns-sd args…` after `wait` (dns-sd never ends on its own).
fn dns_sd(args: &[&str], wait: std::time::Duration) -> String {
    use std::io::Read;
    let Ok(mut child) = std::process::Command::new("/usr/bin/dns-sd")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    std::thread::sleep(wait);
    let _ = child.kill();
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let _ = child.wait();
    out
}

/// Computers sharing files on the local network: those announcing SMB over Bonjour (about
/// 2 s, plus 1 s per resolve) and, asked at the same time, Windows file servers over NetBIOS.
#[tauri::command]
pub async fn smb_browse() -> Vec<SmbHost> {
    tauri::async_runtime::spawn_blocking(|| {
        let netbios = std::thread::spawn(netbios_hosts);
        #[cfg(target_os = "linux")]
        {
            let bonjour = parse_avahi(&avahi_browse(std::time::Duration::from_secs(4)));
            return merge_hosts(bonjour, netbios.join().unwrap_or_default(), &resolve_cached);
        }
        #[allow(unreachable_code)]
        let names = parse_browse(&dns_sd(
            &["-B", "_smb._tcp", "local."],
            std::time::Duration::from_millis(1800),
        ));
        let bonjour: Vec<SmbHost> = names
            .into_iter()
            .take(30)
            .map(|name| {
                let out = dns_sd(
                    &["-L", &name, "_smb._tcp", "local."],
                    std::time::Duration::from_millis(700),
                );
                let host = parse_resolve(&out).unwrap_or_else(|| format!("{name}._smb._tcp.local"));
                SmbHost {
                    name,
                    host,
                    address: None,
                }
            })
            .collect();
        merge_hosts(bonjour, netbios.join().unwrap_or_default(), &resolve_cached)
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod browse_tests {
    use super::*;

    #[test]
    fn browse_and_resolve_output() {
        let out = "Browsing for _smb._tcp.local.\nDATE: ---Sat 21 Nov 2026---\n 0:57:21.880  ...STARTING...\nTimestamp     A/R    Flags  if Domain               Service Type         Instance Name\n 0:57:21.882  Add        3   1 local.               _smb._tcp.           My NAS\n 0:57:21.882  Add        3  14 local.               _smb._tcp.           My NAS\n 0:57:22.000  Rmv        0  14 local.               _smb._tcp.           Old\n 0:57:22.100  Add        2  14 local.               _smb._tcp.           studio\n";
        assert_eq!(parse_browse(out), ["My NAS", "studio"]);
        let r = " 0:57:30.341  My\\032NAS._smb._tcp.local. can be reached at my-nas.local.:445 (interface 26) Flags: 1\n";
        assert_eq!(parse_resolve(r).as_deref(), Some("my-nas.local"));
        assert_eq!(parse_resolve("nothing"), None);
    }
}

// ---------------------------------------------------------------- Listing a server's shares
//
// `smbutil view` lists the shares. Its password must never be on its command line (any
// process can read another's arguments with `ps`), and smbutil has no option to read it from
// stdin: SMBClient asks with `readpassphrase(…, RPP_REQUIRE_TTY)`, which reads only the
// controlling terminal. So smbutil runs in its own session with a pseudo-terminal as that
// terminal, and fxfm types the password there when the prompt appears (echo is off by then).
// The SMBClient/NetFS libraries have no public call that lists shares (NetFS only mounts), so
// this is the safe way that does not depend on private interfaces.

/// One share a server lists (a disk share; pipes and printers are left out).
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct ShareName {
    pub name: String,
    /// The server's description; may be empty.
    pub comment: String,
}

/// Share types `smbutil view` prints in its second column (`%-48s%-8s%s`).
const VIEW_TYPES: [&str; 5] = ["Disk", "Printer", "Pipe", "Comm", "Unknown"];

/// The disk shares of `smbutil view` output, in its order with hidden (`…$`) shares last.
/// A share name longer than the 48-byte column pushes the type to the right, so the type is
/// looked for from byte 48 on.
pub fn parse_view(out: &str) -> Vec<ShareName> {
    let mut shares = Vec::new();
    let mut table = false;
    for line in out.lines() {
        if !table {
            table = line.starts_with("----");
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_end().ends_with("shares listed") {
            break;
        }
        let found = (48.min(line.len())..line.len())
            .filter(|&p| line.is_char_boundary(p))
            .find_map(|p| {
                let rest = &line[p..];
                VIEW_TYPES
                    .iter()
                    .find(|t| {
                        rest.starts_with(*t)
                            && rest.as_bytes().get(t.len()).is_none_or(|b| *b == b' ')
                    })
                    .map(|t| (p, *t))
            });
        let Some((p, kind)) = found else { continue };
        let name = line[..p].trim_end();
        if kind != "Disk" || name.is_empty() {
            continue;
        }
        shares.push(ShareName {
            name: name.to_string(),
            comment: line[p + kind.len()..].trim().to_string(),
        });
    }
    shares.sort_by_key(|s| s.name.ends_with('$'));
    shares
}

/// `smbutil view` arguments for the entry: a guest (no user) and Kerberos never get a prompt
/// (`-N`), a password login does (answered through the terminal).
pub fn view_args(entry: &ServerEntry) -> Vec<String> {
    let user = fxfm_core::servers::smb_user(&entry.user);
    let mut args = vec!["view".to_string()];
    if entry.kerberos {
        args.push("-N".into());
    } else if user.is_empty() {
        args.extend(["-N".into(), "-g".into()]);
    }
    args.push(fxfm_core::servers::smb_server_url(entry));
    args
}

/// `smbclient -L` arguments (Linux): machine-readable output (`-g`); a guest or Kerberos never
/// gets a prompt (`-N`, `--use-kerberos=required`), a user is asked for the password on the
/// terminal. Never holds a password.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn smbclient_args(entry: &ServerEntry) -> Vec<String> {
    let user = fxfm_core::servers::smb_user(&entry.user);
    let host = entry.host.trim();
    let mut args = vec!["-g".to_string(), "-L".to_string(), format!("//{host}")];
    if entry.port != 0 && entry.port != 445 {
        args.extend(["-p".into(), entry.port.to_string()]);
    }
    if entry.kerberos {
        args.extend(["--use-kerberos=required".into(), "-N".into()]);
    } else if user.is_empty() {
        args.push("-N".into());
    } else {
        // DOMAIN;user → DOMAIN\user, as smbclient writes it.
        args.extend(["-U".into(), user.replace(';', "\\")]);
    }
    args
}

/// Disk shares of `smbclient -g -L` output (`Disk|name|comment` lines), hidden ones last.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn parse_smbclient(out: &str) -> Vec<ShareName> {
    let mut shares: Vec<ShareName> = out
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '|');
            (f.next()? == "Disk").then(|| ShareName {
                name: f.next().unwrap_or("").to_string(),
                comment: f.next().unwrap_or("").trim().to_string(),
            })
        })
        .filter(|s| !s.name.is_empty())
        .collect();
    shares.sort_by_key(|s| s.name.ends_with('$'));
    shares
}

/// smbutil's error text for the UI ("smbutil: " and the call it failed in left out).
pub fn view_error(stderr: &str) -> String {
    let text = stderr.trim();
    if text.contains("Authentication error") {
        return "The user name or password was not accepted".to_string();
    }
    let text = text.strip_prefix("smbutil: ").unwrap_or(text);
    // "server connection failed: No route to host" → the reason.
    let text = text.rsplit_once(": ").map_or(text, |(_, why)| why);
    if text.is_empty() {
        "The server did not list its shares".to_string()
    } else {
        format!("The server did not list its shares: {text}")
    }
}

/// What `run_with_tty_password` saw.
#[derive(Debug)]
pub struct TtyRun {
    /// The exit code; None when it was stopped (time limit, or a second password prompt).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Password prompts on the terminal (only the first is answered).
    pub prompts: usize,
    pub timed_out: bool,
}

/// Runs `program args` in a new session whose controlling terminal is a pseudo-terminal
/// (stdin is that terminal too, stdout and stderr are pipes). When the program asks for a
/// password on the terminal, `password` is typed there once; a second prompt (a rejected
/// password), a prompt without a password, or `timeout` stops the program. The password is
/// never an argument or in the environment.
pub fn run_with_tty_password(
    program: &str,
    args: &[String],
    password: Option<&str>,
    timeout: std::time::Duration,
) -> Result<TtyRun, String> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let (mut m, mut s): (libc::c_int, libc::c_int) = (-1, -1);
    // SAFETY: out-pointers for two descriptors; no name buffer, default terminal settings.
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(format!(
            "Cannot open a terminal: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: openpty returned two new descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        // SAFETY: a valid descriptor; other children must not inherit the terminal.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: as above; reads below must not block.
    unsafe {
        let fl = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe calls between fork and exec. stdin (fd 0) is the
    // terminal's slave side by now; it becomes the new session's controlling terminal.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot run {program}: {e}"))?;
    // The parent's copy of the slave side goes with `cmd`.
    drop(cmd);
    fn drain(r: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            String::from_utf8_lossy(&b).into_owned()
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let start = Instant::now();
    let mut seen: Vec<u8> = Vec::new();
    let (mut prompts, mut typed, mut timed_out, mut stopped) = (0, false, false, false);
    let code = loop {
        let mut pfd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        unsafe { libc::poll(&mut pfd, 1, 50) };
        let mut buf = [0u8; 512];
        // SAFETY: reading into a buffer of its size; non-blocking.
        let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            seen.extend_from_slice(&buf[..n as usize]);
            if seen.len() > 4096 {
                seen.drain(..seen.len() - 4096);
            }
            let text = String::from_utf8_lossy(&seen).to_lowercase();
            if text.contains("password") && text.trim_end().ends_with([':', '?']) {
                seen.clear();
                prompts += 1;
                match password.filter(|_| !typed) {
                    Some(pw) => {
                        typed = true;
                        let mut line = Vec::with_capacity(pw.len() + 1);
                        line.extend_from_slice(pw.as_bytes());
                        line.push(b'\n');
                        // SAFETY: writing a buffer of its length.
                        unsafe {
                            libc::write(master.as_raw_fd(), line.as_ptr().cast(), line.len())
                        };
                        line.fill(0);
                    }
                    None => {
                        let _ = child.kill();
                        stopped = true;
                    }
                }
            }
        }
        match child.try_wait() {
            Ok(Some(st)) => break if stopped { None } else { st.code() },
            Ok(None) => {}
            Err(e) => return Err(format!("Waiting for {program}: {e}")),
        }
        if !stopped && start.elapsed() > timeout {
            let _ = child.kill();
            timed_out = true;
            stopped = true;
        }
    };
    drop(master);
    Ok(TtyRun {
        code,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        prompts,
        timed_out,
    })
}

/// The disk shares of a server (`smbutil view`), as the user would see them: a guest or a
/// Kerberos login, or a user with `password` (typed through a terminal, never an argument).
/// Blocking, at most 20 s.
pub fn list_shares(entry: &ServerEntry, password: Option<&str>) -> Result<Vec<ShareName>, String> {
    let login = !entry.user.trim().is_empty() && !entry.kerberos;
    if login && password.is_none() {
        return Err("Enter the password".into());
    }
    let (program, args) = if cfg!(target_os = "macos") {
        ("/usr/bin/smbutil", view_args(entry))
    } else {
        ("smbclient", smbclient_args(entry))
    };
    let run = match run_with_tty_password(
        program,
        &args,
        password.filter(|_| login),
        std::time::Duration::from_secs(20),
    ) {
        Err(e) if !cfg!(target_os = "macos") && e.contains("No such file") => {
            return Err("Shares… needs smbclient (install the smbclient package)".into())
        }
        r => r?,
    };
    if !cfg!(target_os = "macos") {
        if run.timed_out {
            return Err(SMB_TIMEOUT.to_string());
        }
        if run.prompts > 1 || run.stdout.contains("NT_STATUS_LOGON_FAILURE") {
            return Err("The user name or password was not accepted".into());
        }
        let shares = parse_smbclient(&run.stdout);
        if run.code != Some(0) && shares.is_empty() {
            return Err(view_error(&format!("{}{}", run.stderr, run.stdout)));
        }
        return Ok(shares);
    }
    if run.timed_out {
        return Err(SMB_TIMEOUT.to_string());
    }
    if run.prompts > 1 || (run.prompts == 1 && !login) {
        return Err("The user name or password was not accepted".into());
    }
    if run.code != Some(0) {
        return Err(view_error(&run.stderr));
    }
    Ok(parse_view(&run.stdout))
}

/// `list_shares` for the server edit dialog's Shares… button (the entry may be unsaved).
#[tauri::command]
pub async fn smb_list_shares(
    host: String,
    port: u16,
    user: String,
    kerberos: bool,
    password: Option<fxfm_core::archive::pack::Secret>,
) -> Result<Vec<ShareName>, String> {
    if host.trim().is_empty() {
        return Err("Host is required".into());
    }
    let entry = ServerEntry {
        protocol: fxfm_core::servers::Protocol::Smb,
        host,
        port,
        user,
        kerberos,
        ..ServerEntry::default()
    };
    tauri::async_runtime::spawn_blocking(move || {
        list_shares(&entry, password.as_ref().map(|p| p.0.as_str()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod view_tests {
    use super::*;

    fn row(name: &str, kind: &str, comment: &str) -> String {
        format!("{name:<48}{kind:<8}{comment}\n")
    }

    #[test]
    fn parse_view_keeps_disk_shares() {
        let long = "a share name that is longer than the forty-eight byte column";
        let out = format!(
            "{}-------------------------------\n{}{}{}{}{}{}{}\n7 shares listed\n",
            row("Share", "Type", "Comments"),
            row("IPC$", "Pipe", "Remote IPC"),
            row("C$", "Disk", "Default share"),
            row("Public", "Disk", ""),
            row("Žana's Files", "Disk", "Disk images and more"),
            row("HP LaserJet", "Printer", "HP LaserJet 400"),
            row(long, "Disk", "long one"),
            row("Disk", "Disk", "a share named like the type"),
        );
        let got = parse_view(&out);
        let names: Vec<_> = got.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Public", "Žana's Files", long, "Disk", "C$"]);
        assert_eq!(got[0].comment, "");
        assert_eq!(got[1].comment, "Disk images and more");
        assert_eq!(got[2].comment, "long one");
        assert!(parse_view("0 shares listed\n").is_empty());
        assert!(parse_view("").is_empty());
    }

    #[test]
    fn view_arguments_never_hold_a_password() {
        let e = |user: &str, kerberos: bool| ServerEntry {
            protocol: fxfm_core::servers::Protocol::Smb,
            host: "nas.example.com".into(),
            port: 445,
            user: user.into(),
            kerberos,
            ..ServerEntry::default()
        };
        assert_eq!(
            view_args(&e("", false)),
            ["view", "-N", "-g", "//nas.example.com"]
        );
        assert_eq!(
            view_args(&e("CORP\\zb", false)),
            ["view", "//CORP;zb@nas.example.com"]
        );
        assert_eq!(
            view_args(&e("zb", true)),
            ["view", "-N", "//zb@nas.example.com"]
        );
        assert_eq!(
            list_shares(&e("zb", false), None),
            Err("Enter the password".into())
        );
        assert_eq!(
            view_error("smbutil: server rejected the connection: Authentication error\n"),
            "The user name or password was not accepted"
        );
        assert_eq!(
            view_error("smbutil: server connection failed: No route to host\n"),
            "The server did not list its shares: No route to host"
        );
        assert_eq!(view_error(""), "The server did not list its shares");
    }

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".into(), script.into()]
    }

    #[test]
    fn the_password_is_typed_on_the_terminal_only() {
        // A stand-in for smbutil: asks on /dev/tty with echo off, like
        // readpassphrase(RPP_REQUIRE_TTY), and prints what it read and its arguments.
        let script = r#"stty -echo </dev/tty; printf 'Password for nas: ' >/dev/tty; IFS= read -r pw </dev/tty; echo "got:$pw"; echo "args:$0 $*""#;
        let run = run_with_tty_password(
            "/bin/sh",
            &sh(script),
            Some("s3cret pass"),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(run.code, Some(0), "{run:?}");
        assert_eq!(run.prompts, 1);
        assert!(run.stdout.contains("got:s3cret pass\n"), "{}", run.stdout);
        assert!(!run
            .stdout
            .lines()
            .any(|l| l.starts_with("args:") && l.contains("s3cret")));
    }

    #[test]
    fn a_second_prompt_or_no_password_stops_it() {
        let twice = r#"for i in 1 2; do printf 'Password: ' >/dev/tty; read -r pw </dev/tty; done; echo done"#;
        let run = run_with_tty_password(
            "/bin/sh",
            &sh(twice),
            Some("bad"),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!((run.code, run.prompts), (None, 2), "{run:?}");
        assert!(!run.stdout.contains("done"));
        let run = run_with_tty_password(
            "/bin/sh",
            &sh(twice),
            None,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!((run.code, run.prompts), (None, 1), "{run:?}");
    }

    #[test]
    fn no_prompt_and_time_limit() {
        let run = run_with_tty_password(
            "/bin/sh",
            &sh("echo out; echo err >&2; exit 3"),
            Some("unused"),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!((run.code, run.prompts), (Some(3), 0));
        assert_eq!(
            (run.stdout.as_str(), run.stderr.as_str()),
            ("out\n", "err\n")
        );
        let t = std::time::Instant::now();
        let run = run_with_tty_password(
            "/bin/sh",
            &sh("sleep 20"),
            None,
            std::time::Duration::from_millis(300),
        )
        .unwrap();
        assert!(run.timed_out && run.code.is_none());
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }
}

// ---------------------------------------------------------------- NetBIOS discovery
//
// Windows computers announce themselves over NetBIOS, not Bonjour. Browse… asks every
// address of the local IPv4 networks for its NetBIOS names (a node status request, UDP 137,
// as `nbtscan` does). That needs no root: the requests go out from an ordinary port and the
// answers come back to it. Only the local /24 (or the smaller network) of each interface is
// asked, never a VPN (point-to-point) or a link-local network.

/// A NetBIOS node status request for the name `*` (RFC 1002 4.2.17) with id `id`.
pub fn node_status_query(id: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(50);
    q.extend_from_slice(&id.to_be_bytes());
    // Flags 0, one question, no other records.
    q.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    q.push(32);
    // "*" and 15 NULs, each byte as two letters 'A' + nibble.
    let mut name = [0u8; 16];
    name[0] = b'*';
    for b in name {
        q.push(b'A' + (b >> 4));
        q.push(b'A' + (b & 0x0f));
    }
    q.push(0);
    // NBSTAT, IN.
    q.extend_from_slice(&[0, 0x21, 0, 1]);
    q
}

/// What a computer said about itself in a node status answer.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeStatus {
    /// Its computer name (the unique name of type 0x00 or 0x20).
    pub name: String,
    /// Whether it runs the file server service (a unique name of type 0x20).
    pub file_server: bool,
}

/// The node status answer to request `id`, or None for anything else.
pub fn parse_node_status(buf: &[u8], id: u16) -> Option<NodeStatus> {
    let u16_at = |i: usize| Some(u16::from_be_bytes([*buf.get(i)?, *buf.get(i + 1)?]));
    if u16_at(0)? != id || u16_at(2)? & 0x8000 == 0 || u16_at(6)? == 0 {
        return None;
    }
    // The answer's name: a pointer or labels up to a zero byte.
    let mut i = 12;
    loop {
        let len = *buf.get(i)? as usize;
        if len & 0xc0 == 0xc0 {
            i += 2;
            break;
        }
        i += 1 + len;
        if len == 0 {
            break;
        }
    }
    if u16_at(i)? != 0x21 {
        return None;
    }
    // Type, class, TTL, data length.
    let rdlen = u16_at(i + 8)? as usize;
    let data = buf.get(i + 10..i + 10 + rdlen)?;
    let count = *data.first()? as usize;
    let mut name = None;
    let mut file_server = false;
    for e in data.get(1..1 + count * 18)?.as_chunks::<18>().0 {
        let group = e[16] & 0x80 != 0;
        if group {
            continue;
        }
        let suffix = e[15];
        let text: String = e[..15].iter().map(|&b| b as char).collect();
        let text = text.trim_end_matches([' ', '\0']).to_string();
        if text.is_empty() || text.chars().any(|c| c.is_control()) {
            continue;
        }
        if suffix == 0x20 {
            file_server = true;
        }
        if matches!(suffix, 0x00 | 0x20) && name.is_none() {
            name = Some(text);
        }
    }
    Some(NodeStatus {
        name: name?,
        file_server,
    })
}

/// The addresses to ask: for each (address, netmask) of the local interfaces, the other hosts
/// of its network, at most of its /24. At most 1024 in all.
pub fn scan_targets(nets: &[(std::net::Ipv4Addr, std::net::Ipv4Addr)]) -> Vec<std::net::Ipv4Addr> {
    let mut out: Vec<std::net::Ipv4Addr> = Vec::new();
    for (ip, mask) in nets {
        let (ip, mask) = (u32::from(*ip), u32::from(*mask) | 0xffff_ff00);
        let net = ip & mask;
        let bcast = net | !mask;
        for a in net + 1..bcast {
            let a = std::net::Ipv4Addr::from(a);
            if u32::from(a) != ip && !out.contains(&a) && out.len() < 1024 {
                out.push(a);
            }
        }
    }
    out
}

/// IPv4 address and netmask of each interface that is up, not loopback, not point-to-point
/// (VPN) and not link-local.
fn local_nets() -> Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we free below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut p = head;
    while !p.is_null() {
        // SAFETY: a node of the list getifaddrs gave.
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        let flags = ifa.ifa_flags as libc::c_int;
        let wanted = libc::IFF_UP | libc::IFF_RUNNING;
        if flags & wanted != wanted
            || flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) != 0
            || ifa.ifa_addr.is_null()
            || ifa.ifa_netmask.is_null()
        {
            continue;
        }
        // SAFETY: non-null socket addresses; read as sockaddr_in only for AF_INET.
        unsafe {
            if (*ifa.ifa_addr).sa_family as libc::c_int != libc::AF_INET {
                continue;
            }
            let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
            let m = &*(ifa.ifa_netmask as *const libc::sockaddr_in);
            let ip = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            let mask = std::net::Ipv4Addr::from(u32::from_be(m.sin_addr.s_addr));
            if !ip.is_link_local() {
                out.push((ip, mask));
            }
        }
    }
    // SAFETY: the list from getifaddrs, freed once.
    unsafe { libc::freeifaddrs(head) };
    out
}

/// Asks each of `targets` (on `port`, 137 outside tests) for its NetBIOS names and collects
/// the answers for `wait`: the file servers, each address once, in the order they answered.
pub fn netbios_scan(
    targets: &[std::net::Ipv4Addr],
    port: u16,
    wait: std::time::Duration,
) -> Vec<(std::net::Ipv4Addr, String)> {
    use std::time::Instant;
    let Ok(sock) = std::net::UdpSocket::bind(("0.0.0.0", 0)) else {
        return Vec::new();
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let id = (std::process::id() as u16) ^ (nanos as u16);
    let query = node_status_query(id);
    for t in targets {
        // A host or network that cannot be reached is simply not listed.
        let _ = sock.send_to(&query, (*t, port));
    }
    let mut found: Vec<(std::net::Ipv4Addr, String)> = Vec::new();
    let end = Instant::now() + wait;
    let mut buf = [0u8; 1024];
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        if left.is_zero() || sock.set_read_timeout(Some(left)).is_err() {
            break;
        }
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            break;
        };
        let std::net::IpAddr::V4(ip) = from.ip() else {
            continue;
        };
        if !targets.contains(&ip) || found.iter().any(|(a, _)| *a == ip) {
            continue;
        }
        if let Some(st) = parse_node_status(&buf[..n], id).filter(|s| s.file_server) {
            found.push((ip, st.name));
        }
    }
    found
}

/// Windows (and Samba) file servers on the local networks found over NetBIOS (about 1.5 s).
fn netbios_hosts() -> Vec<SmbHost> {
    let targets = scan_targets(&local_nets());
    if targets.is_empty() {
        return Vec::new();
    }
    let mut hosts: Vec<SmbHost> =
        netbios_scan(&targets, 137, std::time::Duration::from_millis(1500))
            .into_iter()
            .map(|(ip, name)| SmbHost {
                host: name.clone(),
                name,
                address: Some(ip.to_string()),
            })
            .collect();
    hosts.sort_by_key(|h| h.name.to_lowercase());
    hosts
}

/// Bonjour hosts first, then the NetBIOS ones that are not the same computer (same name, or
/// a Bonjour host name that resolves to its address).
pub fn merge_hosts(
    mut bonjour: Vec<SmbHost>,
    netbios: Vec<SmbHost>,
    resolve: &dyn Fn(&str) -> Vec<std::net::IpAddr>,
) -> Vec<SmbHost> {
    let key = |s: &str| {
        let s = s.trim_end_matches('.').to_lowercase();
        s.strip_suffix(".local").map(str::to_string).unwrap_or(s)
    };
    let addrs: Vec<std::net::IpAddr> = bonjour.iter().flat_map(|h| resolve(&h.host)).collect();
    for n in netbios {
        let same_name = bonjour
            .iter()
            .any(|b| key(&b.name) == key(&n.name) || key(&b.host) == key(&n.name));
        let same_addr = n
            .address
            .as_deref()
            .and_then(|a| a.parse::<std::net::IpAddr>().ok())
            .is_some_and(|a| addrs.contains(&a));
        if !same_name && !same_addr {
            bonjour.push(n);
        }
    }
    bonjour
}

#[cfg(test)]
mod netbios_tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// A node status answer to `id` with the (name, suffix, group) entries.
    fn answer(id: u16, names: &[(&str, u8, bool)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(&[0x84, 0x00, 0, 0, 0, 1, 0, 0, 0, 0]);
        // The question's name again, in full.
        b.extend_from_slice(&node_status_query(id)[12..46]);
        b.extend_from_slice(&[0, 0x21, 0, 1, 0, 0, 0, 0]);
        let rdlen = 1 + names.len() * 18 + 46;
        b.extend_from_slice(&(rdlen as u16).to_be_bytes());
        b.push(names.len() as u8);
        for (n, suffix, group) in names {
            b.extend_from_slice(format!("{n:<15}").as_bytes());
            b.push(*suffix);
            b.extend_from_slice(&[if *group { 0x84 } else { 0x04 }, 0]);
        }
        b.extend_from_slice(&[0u8; 46]);
        b
    }

    #[test]
    fn query_and_answer_bytes() {
        let q = node_status_query(0x1234);
        assert_eq!(q.len(), 50);
        assert_eq!(&q[..2], &[0x12, 0x34]);
        assert_eq!(&q[13..15], b"CK");
        assert!(q[15..45].iter().all(|&b| b == b'A'));
        assert_eq!(&q[46..], &[0, 0x21, 0, 1]);

        let a = answer(
            7,
            &[
                ("WORKGROUP", 0x00, true),
                ("PC-ANA", 0x00, false),
                ("PC-ANA", 0x20, false),
            ],
        );
        assert_eq!(
            parse_node_status(&a, 7),
            Some(NodeStatus {
                name: "PC-ANA".into(),
                file_server: true
            })
        );
        // Another request's answer, a query, a cut-off answer.
        assert_eq!(parse_node_status(&a, 8), None);
        assert_eq!(parse_node_status(&node_status_query(7), 7), None);
        assert_eq!(parse_node_status(&a[..60], 7), None);
        // A printer that only has a workstation name: not a file server.
        let p = parse_node_status(&answer(7, &[("PRN", 0x00, false)]), 7).unwrap();
        assert!(!p.file_server);
        // A compressed answer name (a pointer to the question).
        let mut c = answer(7, &[("NAS", 0x20, false)]);
        c.splice(12..46, [0xc0, 0x0c]);
        assert_eq!(parse_node_status(&c, 7).unwrap().name, "NAS");
    }

    #[test]
    fn targets_stay_in_the_local_slash_24() {
        let ip = Ipv4Addr::new(192, 168, 1, 20);
        let t = scan_targets(&[(ip, Ipv4Addr::new(255, 255, 255, 0))]);
        assert_eq!(t.len(), 253);
        assert!(!t.contains(&ip));
        assert_eq!(t[0], Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(*t.last().unwrap(), Ipv4Addr::new(192, 168, 1, 254));
        // A /16 is narrowed to the /24; a /30 keeps its one other host.
        let t = scan_targets(&[(Ipv4Addr::new(10, 1, 7, 9), Ipv4Addr::new(255, 255, 0, 0))]);
        assert_eq!(t.len(), 253);
        assert!(t.iter().all(|a| a.octets()[..3] == [10, 1, 7]));
        let t = scan_targets(&[(
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(255, 255, 255, 252),
        )]);
        assert_eq!(t, [Ipv4Addr::new(10, 0, 0, 2)]);
        // The same network on two interfaces is asked once.
        assert_eq!(
            scan_targets(&[
                (ip, Ipv4Addr::new(255, 255, 255, 0)),
                (
                    Ipv4Addr::new(192, 168, 1, 21),
                    Ipv4Addr::new(255, 255, 255, 0)
                )
            ])
            .len(),
            254
        );
    }

    #[test]
    fn scan_collects_file_servers_from_a_local_responder() {
        // A NetBIOS responder of our own on 127.0.0.1 (never a real host).
        let srv = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = srv.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (n, from) = srv.recv_from(&mut buf).unwrap();
            assert_eq!(n, 50);
            let id = u16::from_be_bytes([buf[0], buf[1]]);
            // A wrong id first (ignored), then the answer, then the same again (once only).
            srv.send_to(&answer(id ^ 1, &[("WRONG", 0x20, false)]), from)
                .unwrap();
            srv.send_to(&answer(id, &[("FILESRV", 0x20, false)]), from)
                .unwrap();
            srv.send_to(&answer(id, &[("FILESRV", 0x20, false)]), from)
                .unwrap();
        });
        let found = netbios_scan(
            &[Ipv4Addr::LOCALHOST],
            port,
            std::time::Duration::from_millis(800),
        );
        t.join().unwrap();
        assert_eq!(found, [(Ipv4Addr::LOCALHOST, "FILESRV".to_string())]);
    }

    #[test]
    fn netbios_hosts_that_bonjour_found_are_listed_once() {
        let h = |name: &str, host: &str, address: Option<&str>| SmbHost {
            name: name.into(),
            host: host.into(),
            address: address.map(str::to_string),
        };
        let bonjour = vec![
            h("Studio", "studio.local", None),
            h("My NAS", "my-nas.local", None),
        ];
        let netbios = vec![
            h("STUDIO", "STUDIO", Some("192.168.1.10")),
            h("NAS", "NAS", Some("192.168.1.5")),
            h("PC-ANA", "PC-ANA", Some("192.168.1.20")),
        ];
        let resolve = |host: &str| match host {
            "my-nas.local" => vec!["192.168.1.5".parse().unwrap()],
            _ => Vec::new(),
        };
        let names: Vec<_> = merge_hosts(bonjour, netbios, &resolve)
            .into_iter()
            .map(|h| h.name)
            .collect();
        assert_eq!(names, ["Studio", "My NAS", "PC-ANA"]);
    }
}

/// One item on a mounted SMB share: its `smb://` link and its Windows path.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct ShareLink {
    pub smb: String,
    pub windows: String,
}

/// The `smb://` link and Windows path of each local path (null when it is not on an SMB
/// share), for sharing with Windows users. The mount whose point is the longest prefix wins.
#[tauri::command]
pub async fn share_links(paths: Vec<String>) -> Vec<Option<ShareLink>> {
    tauri::async_runtime::spawn_blocking(move || links_for(&smb_mounts(), &paths))
        .await
        .unwrap_or_default()
}

fn links_for(mounts: &[SmbMount], paths: &[String]) -> Vec<Option<ShareLink>> {
    paths
        .iter()
        .map(|p| {
            mounts
                .iter()
                .filter_map(|m| {
                    fxfm_core::servers::share_links(&m.from, &m.on, p).map(|l| (m.on.len(), l))
                })
                .max_by_key(|(n, _)| *n)
                .map(|(_, (smb, windows))| ShareLink { smb, windows })
        })
        .collect()
}

#[cfg(test)]
mod linux_tests {
    use super::*;

    #[test]
    fn linux_mounts_and_gvfs_shares_read_as_mntfrom() {
        let mounts = "//nas/music /mnt/music cifs rw 0 0\n//pc/My\\040Docs /home/me/docs smb3 rw 0 0\n/dev/sda1 / ext4 rw 0 0\n";
        let gvfs = vec![
            "smb-share:server=nas.local,share=media,user=me".to_string(),
            "smb-share:domain=CORP,server=fs1,share=Team,user=ann".to_string(),
            "sftp:host=web".to_string(),
        ];
        let got = linux_smb_mounts(mounts, "/run/user/1000/gvfs", &gvfs);
        assert_eq!(
            got,
            [
                ("//nas/music".to_string(), "/mnt/music".to_string()),
                ("//pc/My Docs".to_string(), "/home/me/docs".to_string()),
                (
                    "//me@nas.local/media".to_string(),
                    "/run/user/1000/gvfs/smb-share:server=nas.local,share=media,user=me"
                        .to_string()
                ),
                (
                    "//CORP;ann@fs1/Team".to_string(),
                    "/run/user/1000/gvfs/smb-share:domain=CORP,server=fs1,share=Team,user=ann"
                        .to_string()
                ),
            ]
        );
        // They match the saved entries like macOS mounts do.
        assert_eq!(
            fxfm_core::servers::parse_mntfrom(&got[3].0),
            Some(("fs1".to_string(), "Team".to_string()))
        );
    }

    #[test]
    fn smbclient_arguments_and_output() {
        let mut e = ServerEntry {
            host: "nas".into(),
            port: 445,
            user: "CORP\\ann".into(),
            ..ServerEntry::default()
        };
        assert_eq!(smbclient_args(&e), ["-g", "-L", "//nas", "-U", "CORP\\ann"]);
        e.user.clear();
        e.port = 1445;
        assert_eq!(
            smbclient_args(&e),
            ["-g", "-L", "//nas", "-p", "1445", "-N"]
        );
        e.kerberos = true;
        assert!(smbclient_args(&e).contains(&"--use-kerberos=required".to_string()));
        let out = "Disk|media|Music and films\nIPC|IPC$|IPC Service\nDisk|admin$|\nPrinter|hp|Office\nDisk|backup|\n";
        let names: Vec<_> = parse_smbclient(out)
            .into_iter()
            .map(|s| (s.name, s.comment))
            .collect();
        assert_eq!(
            names,
            [
                ("media".to_string(), "Music and films".to_string()),
                ("backup".to_string(), String::new()),
                ("admin$".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn avahi_lines_give_hosts_once() {
        let out = "+;eth0;IPv4;My\\032NAS;_smb._tcp;local\n=;eth0;IPv4;My\\032NAS;_smb._tcp;local;nas.local;192.168.1.5;445;\n=;eth0;IPv6;My\\032NAS;_smb._tcp;local;nas.local;fe80::1;445;\n=;eth0;IPv4;pc;_smb._tcp;local;pc.local.;192.168.1.6;445;\n";
        let hosts = parse_avahi(out);
        let got: Vec<_> = hosts
            .iter()
            .map(|h| (h.name.as_str(), h.host.as_str()))
            .collect();
        assert_eq!(got, [("My NAS", "nas.local"), ("pc", "pc.local")]);
    }
}

#[cfg(test)]
mod share_link_tests {
    use super::*;

    #[test]
    fn the_deepest_mount_wins_and_other_paths_get_none() {
        let m = |from: &str, on: &str| SmbMount {
            from: from.into(),
            on: on.into(),
            total: 0,
            free: 0,
        };
        let mounts = [
            m("//nas/data", "/Volumes/data"),
            m("//other/pub", "/Volumes/data/x"),
        ];
        let out = links_for(
            &mounts,
            &[
                "/Volumes/data/a b".into(),
                "/Volumes/data/x/f".into(),
                "/Users/me".into(),
            ],
        );
        assert_eq!(
            out,
            vec![
                Some(ShareLink {
                    smb: "smb://nas/data/a%20b".into(),
                    windows: r"\\nas\data\a b".into()
                }),
                Some(ShareLink {
                    smb: "smb://other/pub/f".into(),
                    windows: r"\\other\pub\f".into()
                }),
                None
            ]
        );
    }
}
