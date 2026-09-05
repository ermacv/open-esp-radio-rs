//! Exclusive host ownership of the physical HIL fixture.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use fs2::FileExt;
use oer_process::CommandExt as _;

use crate::Result;

/// Holds exclusive fixture ownership until the hardware command returns.
pub(crate) struct FixtureLock {
    _cell: ResourceLease,
    _device: oer_firmware::device::DeviceLease,
    _resources: Vec<ResourceLease>,
}

impl FixtureLock {
    pub(crate) fn acquire(lab: &super::config::LabConfig) -> Result<Self> {
        Self::acquire_for(lab, super::requirements::Requirements::default())
    }

    pub(crate) fn acquire_for(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<Self> {
        use sha2::{Digest, Sha256};
        let device = oer_firmware::device::DeviceLease::acquire(&lab.device.serial)?;
        let directory = oer_firmware::device::lease_directory()?.join(format!(
            "cell-{:x}",
            Sha256::digest(lab.cell_id().as_bytes())
        ));
        let cell = ResourceLease::acquire_directory(&directory)?;
        let root = oer_firmware::device::lease_directory()?;
        let resources = Self::acquire_resources(&root, resource_keys(lab, required)?)?;
        Ok(Self {
            _cell: cell,
            _device: device,
            _resources: resources,
        })
    }

    fn acquire_resources(root: &Path, mut keys: Vec<String>) -> Result<Vec<ResourceLease>> {
        use sha2::{Digest, Sha256};
        keys.sort();
        keys.dedup();
        // Collect drops all acquired owners if any later resource is busy.
        keys.iter()
            .map(|key| {
                ResourceLease::acquire_directory(
                    &root.join(format!("resource-{:x}", Sha256::digest(key.as_bytes()))),
                )
            })
            .collect()
    }
}

struct ResourceLease {
    file: File,
}

impl ResourceLease {
    fn acquire_directory(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join("fixture.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        if let Err(error) = file.try_lock_exclusive() {
            let mut owner = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut owner)?;
            let owner = owner.trim();
            let detail = if owner.is_empty() {
                "another HIL process".to_owned()
            } else {
                owner.to_owned()
            };
            return Err(format!(
                "physical HIL fixture is already owned by {detail} ({}): {error}",
                path.display()
            )
            .into());
        }

        // Establish the guard before fallible owner metadata writes so every
        // path after successful acquisition explicitly releases its authority.
        let mut owner = Self { file };
        let file = &mut owner.file;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(
            file,
            "pid={} command={}",
            std::process::id(),
            command_line()
        )?;
        file.flush()?;
        Ok(owner)
    }
}

/// Interface aliases on one wiphy share one local owner. An OpenWrt client
/// changes firewall and radio state, so remote ownership covers the whole host
/// boot, including callers using different SSH aliases or radio interfaces.
fn resource_keys(
    lab: &super::config::LabConfig,
    required: super::requirements::Requirements,
) -> Result<Vec<String>> {
    use super::config::StationFixtureConfig;
    let mut keys = Vec::new();
    if required.local_radio() {
        keys.push(local_radio_key(Path::new("/sys/class/net/wlan0"))?);
    }
    if required.station_network {
        match &lab.station_fixture {
            StationFixtureConfig::LocalLinux(config) => keys.push(local_radio_key(
                &PathBuf::from("/sys/class/net").join(&config.interface),
            )?),
            StationFixtureConfig::OpenWrt(config) => {
                let output = Command::new("ssh")
                    .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
                    .arg(&config.ssh_target)
                    .arg("cat /proc/sys/kernel/random/boot_id")
                    .supervised_output()?;
                if !output.status.success() {
                    return Err(
                        "cannot resolve OpenWrt host ownership over noninteractive SSH".into(),
                    );
                }
                keys.push(remote_host_key(std::str::from_utf8(&output.stdout)?)?);
            }
            StationFixtureConfig::External(_) => {}
        }
    }
    Ok(keys)
}

fn local_radio_key(interface: &Path) -> Result<String> {
    let radio = interface.join("phy80211").canonicalize().map_err(|error| {
        format!(
            "cannot resolve physical radio for {}: {error}",
            interface.display()
        )
    })?;
    Ok(format!("local-radio:{}", radio.display()))
}

fn remote_host_key(identity: &str) -> Result<String> {
    let identity = identity.trim();
    if identity.len() != 36
        || !identity.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err("OpenWrt host did not return a valid boot identity".into());
    }
    Ok(format!(
        "openwrt-host-boot:{}",
        identity.to_ascii_lowercase()
    ))
}

impl Drop for ResourceLease {
    fn drop(&mut self) {
        // A concurrent fork inherits this open file description until exec,
        // even with close-on-exec set. Closing only our descriptor can leave
        // flock held by that child after the hardware owner has returned.
        // Release at the logical owner boundary; File still closes afterward.
        let _ = FileExt::unlock(&self.file);
    }
}

fn command_line() -> String {
    std::env::args().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
