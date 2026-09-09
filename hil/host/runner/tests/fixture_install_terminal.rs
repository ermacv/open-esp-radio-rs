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
    let deadline = Instant::now() + Duration::from_secs(5);
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
    let cargo = directory.path().join("cargo");
    fs::write(&cargo, "#!/bin/sh\ncase \"$*\" in 'xtask build hostapd'|'build --locked -p open-esp-radio-hil-runner --bin open-radio-probe --target-dir target/hil/fixture-build') exit 0;; *) exit 98;; esac\n").unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &sudo,
        r#"#!/bin/sh
set -eu
test "$#" -eq 1
case "$1" in */hil/host/linux-net/install.sh) ;; *) exit 98;; esac
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_open-esp-radio-hil-runner"));
    command
        .args(["fixture", "install-host"])
        .env("CARGO", &cargo)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        )
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
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
}
