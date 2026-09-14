//! Shared software-generation ownership across one fixture operation or HIL run.

use std::fs::File;

#[cfg(target_os = "linux")]
use std::{
    fs::OpenOptions,
    os::unix::{fs::MetadataExt as _, fs::OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use open_esp_radio_hil_runner::fixture_install::Provider;

use crate::Result;

pub(crate) struct SoftwareLease {
    _files: Vec<File>,
}

impl SoftwareLease {
    pub(crate) fn acquire_for(
        lab: &crate::lab::config::LabConfig,
        required: crate::lab::requirements::Requirements,
    ) -> Result<Self> {
        let mut providers = Vec::new();
        if required.bluetooth_adapter {
            providers.push(Provider::LinuxBluetooth);
        }
        if required.local_radio()
            || (required.station_network
                && matches!(
                    lab.station_fixture,
                    crate::lab::config::StationFixtureConfig::LocalLinux(_)
                ))
        {
            providers.push(Provider::LinuxNet);
        }
        Self::acquire(providers)
    }

    pub(crate) fn acquire_one(provider: Provider) -> Result<Self> {
        Self::acquire([provider])
    }

    fn acquire(providers: impl IntoIterator<Item = Provider>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let mut providers = providers.into_iter().collect::<Vec<_>>();
            providers.sort_by_key(|provider| provider.as_str());
            providers.dedup();
            let mut files = Vec::new();
            for provider in providers {
                let path = PathBuf::from("/run/open-radio-fixture")
                    .join(format!("{}.session.lock", provider.as_str()));
                files.push(acquire_file(provider, &path)?);
            }
            Ok(Self { _files: files })
        }
        #[cfg(not(target_os = "linux"))]
        {
            if providers.into_iter().next().is_some() {
                return Err("Linux fixture software leases require Linux".into());
            }
            Ok(Self { _files: Vec::new() })
        }
    }
}

#[cfg(target_os = "linux")]
fn acquire_file(provider: Provider, path: &Path) -> Result<File> {
    acquire_file_for(provider, path, 0, 0)
}

#[cfg(target_os = "linux")]
fn acquire_file_for(provider: Provider, path: &Path, uid: u32, gid: u32) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            format!(
                "fixture provider {provider} has no installation lease at {} ({error}); reinstall it with cargo hil fixture install --provider {provider}",
                path.display()
            )
        })?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.gid() != gid
        || metadata.mode() & 0o777 != 0o644
    {
        return Err(format!(
            "fixture provider {provider} installation lease has unsafe ownership or mode"
        )
        .into());
    }
    fs2::FileExt::try_lock_shared(&file).map_err(
        |error| -> Box<dyn std::error::Error + Send + Sync> {
            format!("fixture provider {provider} is being updated: {error}").into()
        },
    )?;
    Ok(file)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn shared_lease_rejects_update_lock_symlink_and_writable_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.lock");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };

        let exclusive = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        fs2::FileExt::lock_exclusive(&exclusive).unwrap();
        assert!(acquire_file_for(Provider::LinuxNet, &path, uid, gid).is_err());
        fs2::FileExt::unlock(&exclusive).unwrap();
        let shared = acquire_file_for(Provider::LinuxNet, &path, uid, gid).unwrap();
        fs2::FileExt::unlock(&shared).unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(acquire_file_for(Provider::LinuxNet, &path, uid, gid).is_err());
        let link = directory.path().join("link.lock");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(acquire_file_for(Provider::LinuxNet, &link, uid, gid).is_err());
    }
}
