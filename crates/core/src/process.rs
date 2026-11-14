//! Process helpers.

/// Whether process `pid` exists (a process of another user counts as existing).
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // 0 and negative values address process groups, not one process.
        return true;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Without a way to check, every process counts as alive (nothing of it is removed).
#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn unused_pid_is_dead() {
        // pid_t is i32; this is far above any pid_max.
        assert!(!pid_alive(i32::MAX as u32 - 7));
    }
}
