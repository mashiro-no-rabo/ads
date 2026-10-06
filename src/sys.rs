use std::{
    io,
    mem::{self, MaybeUninit},
    net::Ipv4Addr,
    os::unix::process::CommandExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub use libc::{SIGKILL, SIGTERM};

/// Probes without `SO_REUSEADDR`: with it (std's default), macOS lets `127.0.0.1:P` bind while
/// another socket holds `0.0.0.0:P`, which would report a busy port as free.
pub fn port_free(port: u16) -> bool {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return false;
        }
        let mut addr: libc::sockaddr_in = mem::zeroed();
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            addr.sin_len = mem::size_of::<libc::sockaddr_in>() as u8;
        }
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = port.to_be();
        addr.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
        let ok = libc::bind(
            fd,
            (&raw const addr).cast(),
            mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        ) == 0;
        libc::close(fd);
        ok
    }
}

pub struct SigSet(libc::sigset_t);

/// Must run before any thread is spawned so every thread inherits the mask.
pub fn block_shutdown_signals() -> io::Result<SigSet> {
    unsafe {
        let mut set = MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), libc::SIGINT);
        libc::sigaddset(set.as_mut_ptr(), libc::SIGTERM);
        let set = set.assume_init();
        match libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) {
            0 => Ok(SigSet(set)),
            e => Err(io::Error::from_raw_os_error(e)),
        }
    }
}

/// Children inherit the daemon's blocked mask (std doesn't reset it), which would make them
/// ignore SIGTERM.
pub fn unblock_signals_on_exec(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            let mut set = MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(set.as_mut_ptr());
            match libc::pthread_sigmask(libc::SIG_SETMASK, set.as_ptr(), std::ptr::null_mut()) {
                0 => Ok(()),
                e => Err(io::Error::from_raw_os_error(e)),
            }
        });
    }
}

/// Detached daemon gets its own session so closing the terminal (SIGHUP) can't reach it.
pub fn new_session_on_exec(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| match libc::setsid() {
            -1 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
}

pub fn wait_signal(set: &SigSet) -> io::Result<i32> {
    let mut sig = 0;
    match unsafe { libc::sigwait(&set.0, &mut sig) } {
        0 => Ok(sig),
        e => Err(io::Error::from_raw_os_error(e)),
    }
}

pub fn kill(pid: i32, sig: i32) -> io::Result<()> {
    if pid <= 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    match unsafe { libc::kill(pid, sig) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn killpg(pgid: i32, sig: i32) -> io::Result<()> {
    if pgid <= 1 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    match unsafe { libc::killpg(pgid, sig) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

pub fn group_alive(pgid: i32) -> bool {
    match killpg(pgid, 0) {
        Ok(()) => true,
        Err(e) => e.raw_os_error() == Some(libc::EPERM),
    }
}

pub fn terminate_groups(pgids: &[i32], grace: Duration) {
    let mut live: Vec<i32> = pgids
        .iter()
        .copied()
        .filter(|&g| killpg(g, SIGTERM).is_ok())
        .collect();
    wait_gone(&mut live, grace);
    for &g in &live {
        let _ = killpg(g, SIGKILL);
    }
    wait_gone(&mut live, Duration::from_secs(2));
}

fn wait_gone(live: &mut Vec<i32>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !live.is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
        live.retain(|&g| group_alive(g));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn port_held_on_wildcard_is_busy() {
        let l = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(!port_free(port));
        drop(l);
        assert!(port_free(port));
    }

    #[test]
    fn port_held_on_loopback_is_busy() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(!port_free(l.local_addr().unwrap().port()));
    }
}
