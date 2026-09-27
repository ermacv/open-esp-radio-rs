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
pub struct FixtureLock {
    _cell: ResourceLease,
    _device: Option<oer_esp32s31_firmware::device::DeviceLease>,
    _resources: Vec<ResourceLease>,
    // Declared last so the fixture locks are released before the stand's
    // lease that ordered them.
    _grant: Option<oer_hil_arbiter::Grant>,
}

impl FixtureLock {
    pub fn acquire(lab: &super::config::LabConfig) -> Result<Self> {
        Self::acquire_for(lab, super::requirements::Requirements::default())
    }

    /// Wait for the stand's lease, then take the device and fixture locks.
    pub fn acquire_for(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<Self> {
        let grant = acquire_stand()?;
        let mut owner = wait_for_fixture(|| Self::lock_now(lab, required, true))?;
        owner._grant = Some(grant);
        Ok(owner)
    }

    pub fn acquire_without_device(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<Self> {
        let grant = acquire_stand()?;
        let mut owner = wait_for_fixture(|| Self::lock_now(lab, required, false))?;
        owner._grant = Some(grant);
        Ok(owner)
    }

    /// Check that the stand and fixture are free now, without queueing.
    pub fn probe_for(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<()> {
        if let Some(holder) = oer_hil_arbiter::Arbiter::open()?.status()?.holder
            && holder.pid != std::process::id()
        {
            return Err(format!(
                "HIL stand is leased by {} for `{}` (pid {})",
                holder.owner, holder.work, holder.pid
            )
            .into());
        }
        Self::lock_now(lab, required, true).map(drop)
    }

    fn lock_now(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
        device: bool,
    ) -> Result<Self> {
        let device = device
            .then(|| oer_esp32s31_firmware::device::DeviceLease::acquire(&lab.device.serial))
            .transpose()?;
        use sha2::{Digest, Sha256};
        let directory = oer_esp32s31_firmware::device::lease_directory()?.join(format!(
            "cell-{:x}",
            Sha256::digest(lab.cell_id().as_bytes())
        ));
        let cell = ResourceLease::acquire_directory(&directory)?;
        let root = oer_esp32s31_firmware::device::lease_directory()?;
        let resources = Self::acquire_resources(&root, resource_keys(lab, required)?)?;
        Ok(Self {
            _cell: cell,
            _device: device,
            _resources: resources,
            _grant: None,
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

/// Wait in the stand's queue. The lease is described by this runner's
/// arguments; the environment supplies owner, budget and short-lease choice.
/// A lease of its own terminates this process at twice its budget.
pub fn acquire_stand() -> Result<oer_hil_arbiter::Grant> {
    let work = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let request = oer_hil_arbiter::Request::from_environment(work)?;
    let mut grant = oer_hil_arbiter::Arbiter::open()?.acquire(&request)?;
    grant.terminate_self_on_overrun();
    Ok(grant)
}

/// Retry while a process outside the arbiter (for example a checkout without
/// it) still holds a device or fixture lock.
fn wait_for_fixture<T>(mut lock: impl FnMut() -> Result<T>) -> Result<T> {
    let mut reported = false;
    loop {
        match lock() {
            Err(error) if is_busy(&*error) => {
                if !reported {
                    eprintln!("hil-arbiter: waiting for a lock held outside the queue: {error}");
                    reported = true;
                }
                oer_process::sleep(std::time::Duration::from_secs(1))?;
            }
            result => return result,
        }
    }
}

fn is_busy(error: &(dyn std::error::Error + 'static)) -> bool {
    error.is::<FixtureBusy>() || error.is::<oer_esp32s31_firmware::device::DeviceBusy>()
}

/// Another process holds a fixture lock.
#[derive(Debug)]
pub struct FixtureBusy(String);

impl std::fmt::Display for FixtureBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FixtureBusy {}

/// Journal a change of the board the runner uses at `port`. The runner
/// builds ESP32-S31 firmware, so a board without a known chip is registered
/// as one. Failure is reported and never fails the run.
pub fn record_board(port: &Path, kind: oer_hil_arbiter::BoardEventKind) {
    let result = oer_hil_arbiter::Arbiter::open().and_then(|arbiter| {
        let device = oer_hil_arbiter::port_mac(port);
        if let Some(mac) = &device {
            arbiter.register_device(oer_hil_arbiter::Device {
                mac: mac.clone(),
                chip: Some(String::from("esp32s31")),
                name: None,
            })?;
        }
        arbiter.record_board(device, kind)
    });
    if let Err(error) = result {
        eprintln!("hil-arbiter: cannot record board change: {error}");
    }
}

/// Refuse a board whose newest journaled flash is not `expected`. A board
/// without a journaled flash passes; the consumer's own handshake decides.
pub fn require_board_image(port: &Path, expected: &str, reflash: &str) -> Result<()> {
    let Some(mac) = oer_hil_arbiter::port_mac(port) else {
        return Ok(());
    };
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let label = oer_hil_arbiter::device_label(&arbiter.devices()?, &mac);
    board_image_matches(
        arbiter.latest_flash(&mac)?.as_ref(),
        &label,
        expected,
        reflash,
    )
}

fn board_image_matches(
    latest: Option<&oer_hil_arbiter::BoardEvent>,
    label: &str,
    expected: &str,
    reflash: &str,
) -> Result<()> {
    match latest.map(|event| (event, &event.kind)) {
        Some((event, oer_hil_arbiter::BoardEventKind::Flashed { image, .. }))
            if image != expected =>
        {
            Err(format!(
                "board {label} carries `{image}` flashed by {}, not `{expected}`; {reflash}",
                event.owner
            )
            .into())
        }
        _ => Ok(()),
    }
}

/// Journal a successful flash of `application` to the DUT at `port`.
pub fn record_flash(
    port: &Path,
    image: &str,
    application: &Path,
    commit: Option<String>,
    dirty: Option<bool>,
    origin: String,
) {
    match crate::durable::sha256_file(application) {
        Ok(application_sha256) => record_board(
            port,
            oer_hil_arbiter::BoardEventKind::Flashed {
                image: image.to_owned(),
                application_sha256,
                commit,
                dirty,
                origin,
            },
        ),
        Err(error) => eprintln!("hil-arbiter: cannot identify flashed application: {error}"),
    }
}

pub struct ResourceLease {
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
            return Err(FixtureBusy(format!(
                "physical HIL fixture is already owned by {detail} ({}): {error}",
                path.display()
            ))
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
    if required.bluetooth_adapter {
        keys.push(bluetooth_key(
            lab.bluetooth_adapter
                .ok_or("missing Bluetooth fixture adapter")?,
        )?);
    }
    if required.ieee802154_peer {
        keys.push(ieee802154_peer_key(
            lab.ieee802154_peer
                .as_ref()
                .ok_or("missing IEEE 802.15.4 peer fixture")?,
        )?);
    }
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
    if required.station_network
        && let Some(observer) = &lab.air_observer
    {
        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
            .arg(&observer.ssh_target)
            .arg("cat /proc/sys/kernel/random/boot_id")
            .supervised_output()?;
        if !output.status.success() {
            return Err("cannot resolve independent OpenWrt host ownership".into());
        }
        keys.push(remote_host_key(std::str::from_utf8(&output.stdout)?)?);
    }
    Ok(keys)
}

fn local_radio_key(interface: &Path) -> Result<String> {
    if interface == Path::new("/sys/class/net/wlan0") && !interface.exists() {
        let output = Command::new("sudo")
            .args(["-n", super::NETWORK_HELPER, "identity"])
            .supervised_output()?;
        if !output.status.success() {
            return Err("cannot resolve saved HIL radio identity".into());
        }
        let phy = std::str::from_utf8(&output.stdout)?.trim();
        if !phy.strip_prefix("phy").is_some_and(|index| {
            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return Err("HIL helper returned an invalid physical radio identity".into());
        }
        let radio = Path::new("/sys/class/ieee80211").join(phy).canonicalize()?;
        return Ok(format!("local-radio:{}", radio.display()));
    }
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

fn bluetooth_key(adapter: oer_hil_fixture::bluetooth::model::Adapter) -> Result<String> {
    Ok(format!(
        "bluetooth:{}",
        Path::new("/sys/class/bluetooth")
            .join(adapter.to_string())
            .canonicalize()?
            .display()
    ))
}

/// The peer's serial device, resolved through symlinks such as
/// `/dev/serial/by-id`, identifies it across aliases.
fn ieee802154_peer_key(peer: &super::config::Ieee802154PeerConfig) -> Result<String> {
    Ok(format!(
        "ieee802154-peer:{}",
        peer.serial.canonicalize()?.display()
    ))
}

/// The Bluetooth adapter alone, under the stand's lease.
pub struct BluetoothLease {
    _resource: ResourceLease,
    _grant: oer_hil_arbiter::Grant,
}

pub fn acquire_bluetooth(
    adapter: oer_hil_fixture::bluetooth::model::Adapter,
) -> Result<BluetoothLease> {
    use sha2::{Digest, Sha256};
    let grant = acquire_stand()?;
    let directory = oer_esp32s31_firmware::device::lease_directory()?.join(format!(
        "resource-{:x}",
        Sha256::digest(bluetooth_key(adapter)?.as_bytes())
    ));
    Ok(BluetoothLease {
        _resource: wait_for_fixture(|| ResourceLease::acquire_directory(&directory))?,
        _grant: grant,
    })
}
