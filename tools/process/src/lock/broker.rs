//! A lock's lifetime broker. It owns the file description, not its clients.
//!
//! Owner loss closes admission before draining admitted operation connections.
//! EOF on an operation means its caller has finished I/O and closed its ports.
//! The detached child uses only async-signal-safe syscalls after fork, like the
//! process guardian; it allocates nothing and never returns into Rust cleanup.

use std::{
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        linux::net::SocketAddrExt,
        unix::{
            fs::MetadataExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::Path,
    time::Duration,
};

use super::{FileLock, Mode};
use crate::Result;

const CAPACITY: usize = 256;
const TOKEN_BYTES: usize = 64;
const HANDSHAKE: Duration = Duration::from_secs(3);

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct LockBroker {
    owner: UnixStream,
    pid: libc::pid_t,
}

#[derive(Debug)]
pub struct BrokerOperation {
    // A separate close-on-exec connection for every admitted operation.
    lifetime: crate::IoLifetime,
}

impl BrokerOperation {
    pub fn lifetime(&self) -> &crate::IoLifetime {
        &self.lifetime
    }
}

impl LockBroker {
    /// Transfer an exclusive lock to a broker that drains admitted I/O after
    /// this owner drops or dies. No client's drop can unlock another's I/O.
    pub fn start(lock: FileLock, token: &str) -> Result<Self> {
        if lock.mode() != Mode::Exclusive {
            return Err("a lifetime broker needs an exclusive lock".into());
        }
        let token: [u8; TOKEN_BYTES] = token
            .as_bytes()
            .try_into()
            .map_err(|_| "a broker capability must be 64 bytes")?;
        let listener = UnixListener::bind_addr(&address(&lock.file().metadata()?)?)?;
        listener.set_nonblocking(true)?;
        let (mut owner, client) = UnixStream::pair()?;
        owner.set_read_timeout(Some(HANDSHAKE))?;
        // Suppress FileLock's explicit unlock: the child receives this open file
        // description, and the parent only closes its duplicate after startup.
        let file = lock.into_file();
        let descriptors = [file.as_raw_fd(), client.as_raw_fd(), listener.as_raw_fd()];
        // SAFETY: the child performs only async-signal-safe syscalls in serve
        // and exits with _exit. All arguments were allocated before the fork.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if pid == 0 {
            // SAFETY: descriptors and token are live in the child's copy
            // of this stack; serve never returns or runs Rust destructors.
            unsafe { serve(descriptors, &token) }
        }
        drop(client);
        drop(listener);
        drop(file);
        let mut ready = [0];
        if let Err(error) = owner.read_exact(&mut ready) {
            drop(owner);
            reap(pid, false);
            return Err(format!("start lock broker: {error}").into());
        }
        if ready != [1] {
            drop(owner);
            reap(pid, false);
            return Err("lock broker did not acknowledge ownership".into());
        }
        Ok(Self { owner, pid })
    }

    /// Admission and retention are one broker operation. Returning a guard
    /// means the broker already counts it; owner loss cannot release its lock.
    /// `path` identifies the lock file, regardless of its pathname's length.
    pub fn operation(path: &Path, token: &str) -> Result<BrokerOperation> {
        if token.len() != TOKEN_BYTES {
            return Err("invalid broker capability".into());
        }
        let mut connection = UnixStream::connect_addr(&address(&std::fs::metadata(path)?)?)
            .map_err(|error| format!("lock owner no longer admits operations: {error}"))?;
        connection.set_read_timeout(Some(HANDSHAKE))?;
        connection.set_write_timeout(Some(HANDSHAKE))?;
        connection.write_all(token.as_bytes())?;
        let mut admitted = [0];
        connection
            .read_exact(&mut admitted)
            .map_err(|error| format!("lock owner no longer admits operations: {error}"))?;
        if admitted != [1] {
            return Err("lock owner no longer admits this operation".into());
        }
        Ok(BrokerOperation {
            lifetime: crate::IoLifetime::new(connection),
        })
    }
}

// Linux abstract addresses are bounded by numeric file identity, rather than
// the XDG path. The capability is sent only in the handshake, never exposed in
// /proc/net/unix. Admission also checks peer credentials, replacing filesystem
// socket permissions. Closing the listener removes the address automatically.
fn address(metadata: &std::fs::Metadata) -> std::io::Result<SocketAddr> {
    // SAFETY: geteuid has no arguments or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    SocketAddr::from_abstract_name(format!(
        "oer-lock:{uid}:{}:{}",
        metadata.dev(),
        metadata.ino()
    ))
}

impl Drop for LockBroker {
    fn drop(&mut self) {
        // Normal release waits for the broker's admission closure. A draining
        // delegate never blocks the owner's drop; a reaper waits for its exit.
        let _ = self.owner.shutdown(std::net::Shutdown::Write);
        let mut draining = [1];
        let immediate = self.owner.read_exact(&mut draining).is_ok() && draining == [0];
        reap(self.pid, immediate);
    }
}

fn reap(pid: libc::pid_t, immediate: bool) {
    let wait = move || {
        // SAFETY: pid is the broker this process forked. No other owner waits
        // for it; waitpid touches only a valid local status pointer.
        unsafe {
            let mut status = 0;
            while libc::waitpid(pid, &mut status, 0) < 0 && *libc::__errno_location() == libc::EINTR
            {
            }
        }
    };
    if immediate {
        wait();
    } else {
        std::thread::spawn(wait);
    }
}

#[derive(Clone, Copy)]
struct Client {
    fd: i32,
    token: [u8; TOKEN_BYTES],
    filled: usize,
    admitted: bool,
    started: i64,
}

const EMPTY: Client = Client {
    fd: -1,
    token: [0; TOKEN_BYTES],
    filled: 0,
    admitted: false,
    started: 0,
};

/// SAFETY: runs only in the freshly forked child. Every descriptor, pointer and
/// fixed array stays valid until _exit; only async-signal-safe calls are used.
unsafe fn serve(descriptors: [i32; 3], token: &[u8; TOKEN_BYTES]) -> ! {
    // SAFETY: operations below use only descriptors inherited by this child and
    // preallocated stack storage. It never acquires a Rust lock or allocates.
    unsafe {
        libc::setsid();
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
            libc::signal(signal, libc::SIG_IGN);
        }
        // Duplicate all three first so dup2 cannot overwrite another input.
        let mut copies = [-1; 3];
        for index in 0..3 {
            copies[index] = libc::fcntl(descriptors[index], libc::F_DUPFD_CLOEXEC, 6);
            if copies[index] < 0 {
                libc::_exit(1);
            }
        }
        for (index, copy) in copies.iter().enumerate() {
            if libc::dup2(*copy, index as i32 + 3) < 0 {
                libc::_exit(1);
            }
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null < 0 {
            libc::_exit(1);
        }
        for fd in 0..3 {
            libc::dup2(null, fd);
        }
        if libc::syscall(libc::SYS_close_range, 6_u32, u32::MAX, 0_u32) != 0 {
            libc::_exit(1);
        }
        // fd 3: exclusive lock; fd 4: owner liveness; fd 5: admission listener.
        let ready = [1_u8];
        if libc::write(4, ready.as_ptr().cast(), 1) != 1 {
            libc::_exit(1);
        }
        let mut clients = [EMPTY; CAPACITY];
        let mut owner_alive = true;
        loop {
            let mut polls = [libc::pollfd {
                fd: -1,
                events: libc::POLLIN,
                revents: 0,
            }; CAPACITY + 2];
            polls[0].fd = if owner_alive { 4 } else { -1 };
            polls[1].fd = if owner_alive { 5 } else { -1 };
            for index in 0..CAPACITY {
                polls[index + 2].fd = clients[index].fd;
            }
            if libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 100) < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                // Fail closed: retain exclusion until every client goes away.
                continue;
            }
            let owner_lost = owner_alive && polls[0].revents != 0;
            if owner_lost {
                owner_alive = false;
                libc::close(5);
            }
            let mut clock: libc::timespec = std::mem::zeroed();
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock);
            let now = clock.tv_sec * 1000 + clock.tv_nsec / 1_000_000;
            for index in 0..CAPACITY {
                let client = &mut clients[index];
                if client.fd < 0 {
                    continue;
                }
                if !client.admitted && (!owner_alive || now - client.started > 3000) {
                    libc::close(client.fd);
                    *client = EMPTY;
                    continue;
                }
                if polls[index + 2].revents == 0 && !owner_lost {
                    continue;
                }
                if client.admitted {
                    let mut byte = 0_u8;
                    let read = libc::read(client.fd, (&mut byte as *mut u8).cast(), 1);
                    if read >= 0
                        || (*libc::__errno_location() != libc::EAGAIN
                            && *libc::__errno_location() != libc::EINTR)
                    {
                        libc::close(client.fd);
                        *client = EMPTY;
                    }
                } else {
                    let count = libc::read(
                        client.fd,
                        client.token.as_mut_ptr().add(client.filled).cast(),
                        TOKEN_BYTES - client.filled,
                    );
                    if count == 0
                        || (count < 0
                            && *libc::__errno_location() != libc::EAGAIN
                            && *libc::__errno_location() != libc::EINTR)
                    {
                        libc::close(client.fd);
                        *client = EMPTY;
                        continue;
                    }
                    if count > 0 {
                        client.filled += count as usize;
                    }
                    if client.filled == TOKEN_BYTES {
                        let admitted = [u8::from(client.token == *token)];
                        if admitted[0] == 1
                            && libc::write(client.fd, admitted.as_ptr().cast(), 1) == 1
                        {
                            client.admitted = true;
                        } else {
                            libc::close(client.fd);
                            *client = EMPTY;
                        }
                    }
                }
            }
            let active = clients
                .iter()
                .any(|client| client.fd >= 0 && client.admitted);
            if !owner_alive && !active {
                // A concurrent fork in the owner may have copied the file
                // description before handoff. Explicitly release its flock
                // after I/O drains, regardless of those unrelated copies.
                while libc::flock(3, libc::LOCK_UN) != 0 {
                    if *libc::__errno_location() != libc::EINTR {
                        libc::_exit(1);
                    }
                }
                libc::close(3);
            }
            if owner_lost {
                let draining = [u8::from(active)];
                libc::write(4, draining.as_ptr().cast(), 1);
                libc::close(4);
            }
            if !owner_alive && !active {
                libc::_exit(0);
            }
            if owner_alive && polls[1].revents != 0 {
                let fd = libc::accept4(
                    5,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                );
                if fd >= 0 {
                    let mut peer: libc::ucred = std::mem::zeroed();
                    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                    if libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        (&raw mut peer).cast(),
                        &raw mut length,
                    ) != 0
                        || peer.uid != libc::geteuid()
                    {
                        libc::close(fd);
                        continue;
                    }
                    if let Some(client) = clients.iter_mut().find(|client| client.fd < 0) {
                        *client = Client {
                            fd,
                            started: now,
                            ..EMPTY
                        };
                    } else {
                        libc::close(fd);
                    }
                }
            }
        }
    }
}
