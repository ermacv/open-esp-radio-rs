//! A guardian process that kills every owned child process group once the
//! owner is gone, however it ends.
//!
//! Each owned child leads a process group of its own, so a terminal's
//! signals reach the owner alone and [`crate::owned::Child`] can stop the
//! whole group. When the owner itself dies without that cleanup (SIGKILL,
//! an abort, a panic in another thread), its groups would keep running:
//! a `cargo test` started by a killed `cargo xtask` held its build lock for
//! as long as the tests ran. The guardian closes that gap. It is forked on
//! the first spawn and reads the process groups the owner registers from a
//! pipe. Only the owner holds the pipe's write end (it is close-on-exec, so
//! no child inherits it), and the kernel closes it when the owner exits by
//! any means; the guardian then sees end of file and terminates every group
//! still registered, first with SIGTERM, a second later with SIGKILL.
//!
//! The guardian runs only async-signal-safe system calls after the fork:
//! no allocation, no locks, no Rust standard I/O. It therefore tracks a
//! fixed number of groups. The owner mirrors the registered set, so a group
//! the guardian could not track is refused before its record is sent, and
//! a guardian that could not start or no longer reads refuses every group:
//! no group runs unguarded.

use std::{
    collections::HashSet,
    fs::File,
    io::Write,
    os::fd::FromRawFd,
    sync::{Mutex, OnceLock},
};

/// Process groups one guardian tracks at once.
pub(crate) const CAPACITY: usize = 256;

/// One registration record: an operation and a process group id.
const ADD: i32 = 1;
const REMOVE: i32 = 2;

/// The owner's end: the registration pipe and the groups the guardian
/// tracks, in the order its records were sent.
struct Registry {
    pipe: File,
    groups: HashSet<i32>,
}

fn registry() -> crate::Result<&'static Mutex<Registry>> {
    static REGISTRY: OnceLock<Result<Mutex<Registry>, String>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            start()
                .map(|pipe| {
                    Mutex::new(Registry {
                        pipe,
                        groups: HashSet::new(),
                    })
                })
                .map_err(|error| format!("the process guardian did not start: {error}"))
        })
        .as_ref()
        .map_err(|error| error.clone().into())
}

/// Registers `group` with the guardian; an error when the guardian cannot
/// take it, and the caller must not leave the group running.
pub(crate) fn register(group: i32) -> crate::Result<()> {
    let mut registry = registry()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry.groups.len() >= CAPACITY {
        return Err(format!(
            "the process guardian already tracks {CAPACITY} process groups, so group \
             {group} would outlive a lost owner"
        )
        .into());
    }
    send(&mut registry.pipe, ADD, group)
        .map_err(|error| format!("the process guardian does not read: {error}"))?;
    registry.groups.insert(group);
    Ok(())
}

/// Unregisters `group` once the owner itself has terminated it.
pub(crate) fn unregister(group: i32) {
    let Ok(registry) = registry() else { return };
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if registry.groups.remove(&group) {
        // A guardian that no longer reads has nothing to forget.
        let _ = send(&mut registry.pipe, REMOVE, group);
    }
}

fn send(pipe: &mut File, operation: i32, group: i32) -> std::io::Result<()> {
    let mut record = [0_u8; 8];
    record[..4].copy_from_slice(&operation.to_ne_bytes());
    record[4..].copy_from_slice(&group.to_ne_bytes());
    // Eight bytes are below PIPE_BUF: the write is atomic.
    pipe.write_all(&record)
}

/// Forks the guardian and returns the write end of its registration pipe.
fn start() -> std::io::Result<File> {
    let mut fds = [0; 2];
    // SAFETY: `fds` is a valid two-element array for pipe2 to fill.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let [read, write] = fds;
    // SAFETY: the child runs only async-signal-safe calls in `guard`, which
    // never returns; the parent continues normally.
    match unsafe { libc::fork() } {
        -1 => {
            let error = std::io::Error::last_os_error();
            // SAFETY: both descriptors were just created by pipe2.
            unsafe {
                libc::close(read);
                libc::close(write);
            }
            Err(error)
        }
        0 => guard(read),
        _ => {
            // SAFETY: `read` belongs to this process and is not used again.
            unsafe { libc::close(read) };
            // SAFETY: `write` is an open descriptor this process owns alone.
            Ok(unsafe { File::from_raw_fd(write) })
        }
    }
}

/// The guardian: collect registrations until the owner is gone, then
/// terminate every registered group and exit.
fn guard(read: i32) -> ! {
    // SAFETY: every call below is async-signal-safe and touches only this
    // process's descriptors, signal dispositions and the registered groups.
    unsafe {
        // Detach from the owner's terminal signals: a Ctrl-C reaches the
        // owner, which cleans up itself; the guardian must outlive it.
        libc::setsid();
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
            libc::signal(signal, libc::SIG_IGN);
        }
        // Keep only the registration pipe: an inherited pipe write end would
        // keep the owner's readers waiting for end of file.
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        for standard in 0..3 {
            libc::dup2(null, standard);
        }
        libc::dup2(read, 3);
        if libc::syscall(libc::SYS_close_range, 4_u32, u32::MAX, 0_u32) != 0 {
            for descriptor in 4..4096 {
                libc::close(descriptor);
            }
        }
        let mut groups = [0_i32; CAPACITY];
        let mut record = [0_u8; 8];
        let mut filled = 0;
        loop {
            let count = libc::read(3, record.as_mut_ptr().add(filled).cast(), 8 - filled);
            if count == 0 {
                break;
            }
            if count < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                break;
            }
            filled += count as usize;
            if filled < 8 {
                continue;
            }
            filled = 0;
            let operation = i32::from_ne_bytes([record[0], record[1], record[2], record[3]]);
            let group = i32::from_ne_bytes([record[4], record[5], record[6], record[7]]);
            if group <= 1 {
                continue;
            }
            // The owner refuses a group beyond CAPACITY before sending it,
            // so an ADD always finds a free slot.
            if operation == ADD {
                if let Some(slot) = groups.iter_mut().find(|slot| **slot == 0) {
                    *slot = group;
                }
            } else if operation == REMOVE
                && let Some(slot) = groups.iter_mut().find(|slot| **slot == group)
            {
                *slot = 0;
            }
        }
        let mut any = false;
        for &group in groups.iter().filter(|group| **group > 1) {
            any |= libc::kill(-group, libc::SIGTERM) == 0;
        }
        if any {
            let grace = libc::timespec {
                tv_sec: 1,
                tv_nsec: 0,
            };
            libc::nanosleep(&grace, std::ptr::null_mut());
            for &group in groups.iter().filter(|group| **group > 1) {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        libc::_exit(0)
    }
}
