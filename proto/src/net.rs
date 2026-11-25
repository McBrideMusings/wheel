//! What both ends of a remote link set on its TCP socket.

use std::os::fd::AsRawFd;

/// How long a remote connection may be silent before the kernel asks whether its peer is still
/// there, how often it asks again, and how many unanswered asks end it: a peer gone without
/// closing its sockets (a laptop that slept, a phone off the network) is found within about a
/// minute.
pub const KEEPALIVE_IDLE: libc::c_int = 30;
pub const KEEPALIVE_EVERY: libc::c_int = 10;
pub const KEEPALIVE_MISSES: libc::c_int = 3;

/// Turn on TCP keepalive for `tcp` with the times above.
pub fn keep_alive(tcp: &impl AsRawFd) -> std::io::Result<()> {
    let fd = tcp.as_raw_fd();
    #[cfg(target_os = "macos")]
    let idle = libc::TCP_KEEPALIVE;
    #[cfg(not(target_os = "macos"))]
    let idle = libc::TCP_KEEPIDLE;
    for (level, name, value) in [
        (libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1),
        (libc::IPPROTO_TCP, idle, KEEPALIVE_IDLE),
        (libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, KEEPALIVE_EVERY),
        (libc::IPPROTO_TCP, libc::TCP_KEEPCNT, KEEPALIVE_MISSES),
    ] {
        // SAFETY: one c_int option on a socket descriptor `tcp` keeps open for the call.
        let set = unsafe {
            libc::setsockopt(fd, level, name, (&value as *const libc::c_int).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t)
        };
        if set != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}
