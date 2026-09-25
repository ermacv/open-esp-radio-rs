//! Cancel the actual installer CLI during its unprivileged build, before sudo.
#![cfg(target_os = "linux")]

use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
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
