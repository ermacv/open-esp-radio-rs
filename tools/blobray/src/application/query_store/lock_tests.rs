//! Process-lifetime regressions for the database snapshot lock.

use super::{QueryStore, tests::manifest};

#[cfg(all(target_os = "linux", target_env = "gnu"))]
use std::{io::Write, os::fd::AsRawFd, os::unix::net::UnixStream};

/// Hold inherited descriptors without running Rust after a multithreaded fork.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
struct ForkedDescriptorHolder {
    release: UnixStream,
    pid: libc::pid_t,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
impl ForkedDescriptorHolder {
    fn new() -> Self {
        let (release, wait) = UnixStream::pair().unwrap();
        let release_fd = release.as_raw_fd();
        let wait_fd = wait.as_raw_fd();
        // SAFETY: fork duplicates the live descriptors. The child uses only
        // async-signal-safe close/read/_exit and runs no Rust destructors.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            let mut byte = 0_u8;
            // SAFETY: these descriptors and one-byte buffer are live in the
            // child. _exit avoids all inherited runtime/SQLite cleanup.
            unsafe {
                libc::close(release_fd);
                let received = loop {
                    let received = libc::read(wait_fd, (&mut byte as *mut u8).cast(), 1);
                    // On Linux/glibc errno is thread-local storage; reading it
                    // needs no allocation or inherited runtime lock.
                    if received >= 0 || *libc::__errno_location() != libc::EINTR {
                        break received;
                    }
                };
                libc::_exit(if received == 1 { 0 } else { 1 });
            }
        }
        drop(wait);
        Self { release, pid }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
impl Drop for ForkedDescriptorHolder {
    fn drop(&mut self) {
        let _ = self.release.write_all(&[1]);
        let mut status = 0;
        loop {
            // SAFETY: this parent reaps its own child, and status is writable.
            let waited = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if waited == self.pid {
                break;
            }
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::Interrupted
            );
        }
        assert_eq!(status, 0, "descriptor holder must exit cleanly");
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn writer_drop_releases_the_lock_while_a_fork_child_retains_its_descriptor() {
    let manifest = manifest("writer-fork-release");
    let writer = QueryStore::open(&manifest).unwrap();
    let child = ForkedDescriptorHolder::new();

    drop(writer);
    let successor = QueryStore::open(&manifest);
    // Reap before asserting so even the pre-fix failure cannot strand a child.
    drop(child);
    assert!(
        successor.is_ok(),
        "closed writer retained its lock: {:?}",
        successor.err()
    );
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn snapshot_drop_releases_the_lock_while_a_fork_child_retains_its_descriptor() {
    let manifest = manifest("reader-fork-release");
    drop(QueryStore::open(&manifest).unwrap());
    let snapshot = QueryStore::plan_read_guard(&manifest).unwrap().unwrap();
    let child = ForkedDescriptorHolder::new();

    drop(snapshot);
    let successor = QueryStore::open(&manifest);
    drop(child);
    assert!(
        successor.is_ok(),
        "closed snapshot retained its lock: {:?}",
        successor.err()
    );
}

#[test]
fn read_only_query_does_not_release_the_live_plan_snapshot_lock() {
    let manifest = manifest("reader-view-retains-lock");
    drop(QueryStore::open(&manifest).unwrap());
    let snapshot = QueryStore::plan_read_guard(&manifest).unwrap().unwrap();

    assert!(
        QueryStore::stage_output_digests_read_only(&snapshot, "missing", false)
            .unwrap()
            .is_none()
    );
    let blocked = QueryStore::open(&manifest)
        .err()
        .expect("snapshot still excludes a writer");
    assert!(blocked.to_string().contains("active writer or reader"));
    drop(snapshot);
    assert!(QueryStore::open(&manifest).is_ok());
}
