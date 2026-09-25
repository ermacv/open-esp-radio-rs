//! Exercise the actual CLI under a controlling terminal, without sudo privileges.
#![cfg(target_os = "linux")]

use std::{
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Session(Child);

impl Drop for Session {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        // SAFETY: this child created its own session/process group. Kill only
        // that group while its leader is still owned and has not been reaped.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

#[test]
fn cancellation_during_unprivileged_build_never_reaches_sudo_apply() {
    let directory = tempfile::tempdir().unwrap();
    let cargo = directory.path().join("cargo");
    let started = directory.path().join("build-started");
    let sudo_called = directory.path().join("sudo-called");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\n: >'{}'\nwhile :; do /bin/sleep 1; done\n",
            started.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
    let sudo = directory.path().join("sudo");
    fs::write(
        &sudo,
        format!("#!/bin/sh\n: >'{}'\nexit 99\n", sudo_called.display()),
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_oer-hil-runner"));
    command
        .args(["fixture", "install", "--provider", "linux-net"])
        .env("CARGO", cargo)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the child performs only setsid before exec and becomes the sole
    // leader of a private process group owned by this test.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut session = Session(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.exists() {
        assert!(Instant::now() < deadline, "installer build did not start");
        thread::sleep(Duration::from_millis(10));
    }
    // SAFETY: session still owns this live child PID; signal only that process.
    assert_eq!(
        unsafe { libc::kill(session.0.id() as i32, libc::SIGTERM) },
        0
    );
    let status = loop {
        if let Some(status) = session.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled installer did not exit"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    assert!(!sudo_called.exists());
}

fn terminal() -> (File, File) {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: both output pointers are valid; null requests default PTY settings.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    // SAFETY: openpty returned two distinct, uniquely owned descriptors.
    let pair = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for fd in [master, slave] {
        // SAFETY: these are live descriptors owned by pair. The child must not
        // inherit extra PTY ends in addition to its standard streams.
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    pair
}

fn read_until(master: &mut File, transcript: &mut Vec<u8>, marker: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !transcript
        .windows(marker.len())
        .any(|window| window == marker)
    {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "terminal did not reach {:?}: {}",
            marker,
            String::from_utf8_lossy(transcript)
        );
        let mut descriptor = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one live descriptor and a valid output pointer.
        let ready = unsafe { libc::poll(&mut descriptor, 1, remaining.as_millis() as i32) };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        assert!(
            ready > 0,
            "terminal readiness failed: {}",
            String::from_utf8_lossy(transcript)
        );
        let mut bytes = [0; 1024];
        let count = master
            .read(&mut bytes)
            .expect("terminal closed before expected event");
        assert!(count > 0);
        transcript.extend_from_slice(&bytes[..count]);
    }
}

#[test]
fn installer_keeps_terminal_control_hides_input_and_preserves_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    let sudo = directory.path().join("sudo");
    fs::write(
        &sudo,
        r#"#!/bin/sh
set -eu
test "$#" -eq 5
case "$1" in */target/hil/fixture-build/debug/open-radio-fixture-install) ;; *) exit 98;; esac
test "$2" = --provider
test "$3" = linux-bluetooth
test "$4" = --bundle
test -f "$5/bundle.json"
stty -echo
printf 'fixture-password:'
IFS= read -r token
stty echo
test "$token" = fixture-test-token
printf '\nfixture-authenticated\n'
exit 23
"#,
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();
    let (mut master, slave) = terminal();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oer-hil-runner"));
    command
        .args(["fixture", "install", "--provider", "linux-bluetooth"])
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        )
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::from(slave));
    // SAFETY: the child performs only async-signal-safe syscalls before exec.
    // It becomes the session leader and foreground owner of its private PTY.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut session = Session(command.spawn().unwrap());
    drop(command);
    let mut transcript = Vec::new();
    read_until(&mut master, &mut transcript, b"fixture-password:");
    master.write_all(b"fixture-test-token\n").unwrap();
    read_until(&mut master, &mut transcript, b"fixture-authenticated");
    assert!(!String::from_utf8_lossy(&transcript).contains("fixture-test-token"));
    assert_eq!(session.0.wait().unwrap().code(), Some(23));
    let mut machine_output = Vec::new();
    session
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut machine_output)
        .unwrap();
    assert!(machine_output.is_empty());
}
