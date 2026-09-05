use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

#[test]
fn fixture_has_exactly_one_live_host_owner() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "open-radio-fixture-lock-{}-{nonce}",
        std::process::id()
    ));

    let owner = FixtureLock::acquire(&root).unwrap();
    assert!(FixtureLock::acquire(&root).is_err());
    drop(owner);
    FixtureLock::acquire(&root).unwrap();

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn dropping_owner_releases_lock_while_a_forked_child_retains_the_descriptor() {
    use std::{io::Write, os::fd::AsRawFd, os::unix::net::UnixStream};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "open-radio-fixture-fork-lock-{}-{nonce}",
        std::process::id()
    ));
    let owner = FixtureLock::acquire(&root).unwrap();
    let (mut release_child, child_signal) = UnixStream::pair().unwrap();
    let parent_fd = release_child.as_raw_fd();
    let child_fd = child_signal.as_raw_fd();

    // SAFETY: the child executes only async-signal-safe read/close/_exit calls,
    // without Rust allocation, unwinding or destruction after this fork of a
    // multithreaded test process. The parent retains ordinary Rust execution.
    let child = unsafe { libc::fork() };
    assert!(
        child >= 0,
        "fork failed: {}",
        std::io::Error::last_os_error()
    );
    if child == 0 {
        // SAFETY: both socket descriptors were valid at fork. A byte or EOF
        // ends the child; interrupted reads retry without touching Rust state.
        // _exit prevents inherited Rust owners from running their destructors.
        unsafe {
            libc::close(parent_fd);
            let mut byte = 0_u8;
            while libc::read(child_fd, (&mut byte as *mut u8).cast(), 1) < 0 {}
            libc::_exit(0);
        }
    }
    drop(child_signal);

    // The child's fork-inherited descriptor keeps the same open file
    // description alive even though File::open sets close-on-exec.
    drop(owner);
    let successor = FixtureLock::acquire(&root);

    // Reap before asserting so the regression's intentional failure on the
    // old implementation cannot leave a waiting child or temporary files.
    release_child.write_all(&[1]).unwrap();
    let mut status = 0;
    let waited = loop {
        // SAFETY: child is the live PID returned above; status is valid writable
        // storage and this thread is the only waiter for that child.
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        if waited >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
        {
            break waited;
        }
    };
    fs::remove_dir_all(root).unwrap();
    assert_eq!(waited, child);
    assert_eq!(status, 0);
    assert!(
        successor.is_ok(),
        "owner drop retained the fixture lock: {:?}",
        successor.as_ref().err()
    );
}
