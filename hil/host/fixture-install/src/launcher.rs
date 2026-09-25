//! Fixed provider launchers bind a lease and committed generation before exec.

use std::process::ExitCode;

use crate::{ArtifactRole, Provider};

#[derive(Clone, Copy)]
pub enum LaunchTarget {
    Network,
    Probe,
    Bluetooth,
}

impl LaunchTarget {
    pub(crate) fn provider(self) -> Provider {
        match self {
            Self::Network | Self::Probe => Provider::LinuxNet,
            Self::Bluetooth => Provider::LinuxBluetooth,
        }
    }

    pub(crate) fn role(self) -> ArtifactRole {
        match self {
            Self::Network => ArtifactRole::NetworkHelper,
            Self::Probe => ArtifactRole::ProbeHelper,
            Self::Bluetooth => ArtifactRole::BluetoothHelper,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Network => "linux-net",
            Self::Probe => "linux-net-probe",
            Self::Bluetooth => "linux-bluetooth",
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_support {
    use std::{path::Path, process::Command};

    use super::LaunchTarget;

    pub(crate) fn run_at(
        root: &Path,
        target: LaunchTarget,
        arguments: &[&str],
    ) -> crate::Result<std::process::ExitStatus> {
        let lease = crate::admission::test_support::admit(root, target.provider())?;
        let helper = lease.artifact(target.role())?;
        Ok(Command::new(helper)
            .args(arguments)
            .env("OPEN_RADIO_GENERATION_BOUND", target.marker())
            .env("OPEN_RADIO_GENERATION_DIR", lease.generation())
            .status()?)
    }
}

pub fn main(target: LaunchTarget) -> ExitCode {
    match launch(target) {
        Ok(never) => match never {},
        Err(error) => {
            eprintln!("fixture operational admission: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
pub fn adopt_lease(expected_marker: &str) -> crate::Result<std::fs::File> {
    use std::{
        fs::OpenOptions,
        os::{
            fd::{FromRawFd as _, RawFd},
            unix::{fs::MetadataExt as _, fs::OpenOptionsExt as _},
        },
    };

    const LEASE_FD: RawFd = 9;
    let marker = std::env::var("OPEN_RADIO_GENERATION_BOUND").unwrap_or_default();
    if marker != expected_marker {
        return Err("generation helper was not entered through its fixed launcher".into());
    }
    unsafe { std::env::remove_var("OPEN_RADIO_GENERATION_BOUND") };
    unsafe { std::env::remove_var("OPEN_RADIO_GENERATION_DIR") };
    if unsafe { libc::fcntl(LEASE_FD, libc::F_GETFD) } < 0 {
        return Err("generation helper did not inherit its provider lease descriptor".into());
    }
    let owned_fd = unsafe { libc::fcntl(LEASE_FD, libc::F_DUPFD_CLOEXEC, 3) };
    if owned_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    unsafe { libc::close(LEASE_FD) };
    let file = unsafe { std::fs::File::from_raw_fd(owned_fd) };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != 0
        || metadata.gid() != 0
        || metadata.mode() & 0o777 != 0o644
    {
        return Err("inherited provider lease has unsafe ownership or mode".into());
    }
    let provider = match expected_marker {
        "linux-net" | "linux-net-probe" => "linux-net",
        "linux-bluetooth" => "linux-bluetooth",
        _ => return Err("unknown fixed launcher marker".into()),
    };
    let expected = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(format!(
            "/var/lib/open-radio/fixture/{provider}/session.lock"
        ))?;
    let expected_metadata = expected.metadata()?;
    if metadata.dev() != expected_metadata.dev() || metadata.ino() != expected_metadata.ino() {
        return Err("inherited descriptor is not the persistent provider lease".into());
    }
    fs2::FileExt::try_lock_shared(&file)?;
    Ok(file)
}

#[cfg(target_os = "linux")]
fn launch(target: LaunchTarget) -> crate::Result<std::convert::Infallible> {
    use std::{os::fd::AsRawFd as _, os::unix::process::CommandExt as _, process::Command};

    const LEASE_FD: libc::c_int = 9;
    let lease = super::admit_system(target.provider())?;
    let helper = lease.artifact(target.role())?;
    if unsafe { libc::dup2(lease.file().as_raw_fd(), LEASE_FD) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let flags = unsafe { libc::fcntl(LEASE_FD, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(LEASE_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let error = Command::new(&helper)
        .args(std::env::args_os().skip(1))
        .env("OPEN_RADIO_GENERATION_BOUND", target.marker())
        .env("OPEN_RADIO_GENERATION_DIR", lease.generation())
        .exec();
    Err(error.into())
}

#[cfg(not(target_os = "linux"))]
fn launch(_target: LaunchTarget) -> crate::Result<std::convert::Infallible> {
    Err("Linux fixture launcher requires Linux".into())
}
