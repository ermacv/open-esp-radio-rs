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

/// The frequency ranges a lease uses and how.
pub use oer_hil_arbiter::spectrum::{BAND_2G4, Emits, Need, Spectrum};

/// Holds exclusive fixture ownership until the hardware command returns.
pub struct FixtureLock {
    _device: Option<oer_esp32s31_firmware::device::DeviceLease>,
    _resources: Vec<ResourceLease>,
    /// Keeps the fixture software from being reinstalled during the lease.
    _software: Option<crate::fixture::software::SoftwareLease>,
    // Declared last so the fixture locks are released before the stand's
    // lease that ordered them.
    grant: Option<oer_hil_arbiter::Grant>,
    /// What to request again after yielding.
    request: Option<LeaseRequest>,
}

/// How a run leases the stand.
#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub required: super::requirements::Requirements,
    /// Named in the lease so their durations estimate later budgets.
    pub scenarios: Vec<String>,
    /// The frequency ranges the work uses; empty when it never enables a
    /// radio.
    pub air: Vec<Spectrum>,
    /// Work with boundaries yields there once over budget while others wait;
    /// indivisible work is terminated at its budget instead.
    pub divisible: bool,
    /// Whether the lease includes the device under test.
    pub device: bool,
}

impl LeaseRequest {
    pub fn device(required: super::requirements::Requirements) -> Self {
        Self {
            required,
            scenarios: Vec::new(),
            air: vec![Spectrum::new(
                oer_hil_arbiter::spectrum::BAND_2G4,
                Need::Tolerant,
                Emits::Normal,
            )],
            divisible: false,
            device: true,
        }
    }
}

impl FixtureLock {
    pub fn acquire(lab: &super::config::LabConfig) -> Result<Self> {
        Self::acquire_for(lab, super::requirements::Requirements::default())
    }

    /// Wait for the stand's lease on the device and `required` fixtures,
    /// then take their locks.
    pub fn acquire_for(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<Self> {
        Self::lease(lab, LeaseRequest::device(required))
    }

    pub fn acquire_without_device(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<Self> {
        Self::lease(
            lab,
            LeaseRequest {
                device: false,
                ..LeaseRequest::device(required)
            },
        )
    }

    /// Wait for the stand's lease described by `request`, then take the
    /// device and fixture locks.
    pub fn lease(lab: &super::config::LabConfig, request: LeaseRequest) -> Result<Self> {
        let keys = resource_keys(lab, request.required)?;
        let grant = acquire_stand(
            claims(lab, &request, &keys),
            &request.scenarios,
            request.divisible,
        )?;
        let mut owner = wait_for_fixture(|| Self::lock_now(lab, &keys, request.device))?;
        // Taken only once granted: a queued run must not block an
        // installation queued before it.
        owner._software = Some(crate::fixture::software::SoftwareLease::acquire(
            crate::fixture::software::providers(lab, request.required),
        )?);
        owner.grant = Some(grant);
        owner.request = Some(request);
        Ok(owner)
    }

    /// Environment that lets a child command join this lease.
    pub fn environment(&self) -> Vec<(&'static str, String)> {
        self.grant
            .as_ref()
            .map_or_else(Vec::new, oer_hil_arbiter::Grant::environment)
    }

    /// Whether the stand asks this over-budget lease to yield to waiting
    /// requests at its next boundary.
    pub fn yield_requested(&self) -> bool {
        self.grant
            .as_ref()
            .is_some_and(oer_hil_arbiter::Grant::yield_requested)
    }

    /// Release the lease, queue again behind the waiting requests and return
    /// the new lease. The boards' state is unknown afterwards.
    pub fn requeue(self, lab: &super::config::LabConfig) -> Result<Self> {
        let request = self
            .request
            .clone()
            .ok_or("only a queued lease can yield")?;
        if let Some(grant) = &self.grant {
            grant.mark_yielded();
        }
        drop(self);
        Self::lease(lab, request)
    }

    /// Check that the device and fixture are free now, without queueing.
    pub fn probe_for(
        lab: &super::config::LabConfig,
        required: super::requirements::Requirements,
    ) -> Result<()> {
        let keys = resource_keys(lab, required)?;
        let claims = claims(lab, &LeaseRequest::device(required), &keys);
        if let Some(holder) = oer_hil_arbiter::Arbiter::open()?
            .conflicting_holders(&claims)?
            .into_iter()
            .find(|holder| holder.pid != std::process::id())
        {
            return Err(format!(
                "HIL stand is leased by {} for `{}` on {} (pid {})",
                holder.owner, holder.work, holder.claims, holder.pid
            )
            .into());
        }
        Self::lock_now(lab, &keys, true).map(drop)
    }

    fn lock_now(lab: &super::config::LabConfig, keys: &[String], device: bool) -> Result<Self> {
        let device = device
            .then(|| oer_esp32s31_firmware::device::DeviceLease::acquire(&lab.device.serial))
            .transpose()?;
        let root = oer_esp32s31_firmware::device::lease_directory()?;
        let resources = Self::acquire_resources(&root, keys.to_vec())?;
        Ok(Self {
            _device: device,
            _resources: resources,
            _software: None,
            grant: None,
            request: None,
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

/// A board's identity in claims: its MAC, or the canonical port without one.
pub fn board_identity(port: &Path) -> String {
    oer_hil_arbiter::port_mac(port).unwrap_or_else(|| {
        fs::canonicalize(port)
            .unwrap_or_else(|_| port.to_owned())
            .display()
            .to_string()
    })
}

/// The catalog image the reference peer board carried during a scenario,
/// from the board journal's newest flash of the board. Written into the
/// scenario's directory, so the scenario's seal binds the peer firmware.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PeerImageRecord {
    pub schema: u8,
    pub board: String,
    pub image: String,
    pub application_sha256: String,
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

/// The newest journaled flash of `board`, when it is `image`.
pub fn peer_flash(board: &str, image: &str) -> Result<Option<PeerImageRecord>> {
    let events = oer_hil_arbiter::Arbiter::open()?.board_events()?;
    Ok(newest_flash(&events, board, image))
}

fn newest_flash(
    events: &[oer_hil_arbiter::BoardEvent],
    board: &str,
    image: &str,
) -> Option<PeerImageRecord> {
    events
        .iter()
        .rev()
        .filter(|event| event.device.as_deref() == Some(board))
        .find_map(|event| match &event.kind {
            oer_hil_arbiter::BoardEventKind::Flashed {
                image: flashed,
                application_sha256,
                commit,
                dirty,
                ..
            } => Some((flashed, application_sha256, commit, dirty)),
            _ => None,
        })
        .filter(|(flashed, ..)| flashed.as_str() == image)
        .map(
            |(image, application_sha256, commit, dirty)| PeerImageRecord {
                schema: 1,
                board: board.to_owned(),
                image: image.clone(),
                application_sha256: application_sha256.clone(),
                commit: commit.clone(),
                dirty: *dirty,
            },
        )
}

/// The stand resources of a lease: its boards, fixtures and the air.
fn claims(
    lab: &super::config::LabConfig,
    request: &LeaseRequest,
    keys: &[String],
) -> Vec<oer_hil_arbiter::Claim> {
    use oer_hil_arbiter::Claim;
    let mut claims = Vec::new();
    if request.device {
        claims.push(Claim::board(&board_identity(&lab.device.serial)));
    }
    // A peer whose board is not attached is claimed by no port; the run's
    // preflight reports it.
    if let Some(serial) = lab
        .peer
        .as_ref()
        .filter(|_| request.required.peer)
        .and_then(|peer| peer.serial().ok())
    {
        claims.push(Claim::board(&board_identity(&serial)));
    }
    claims.extend(
        keys.iter()
            .filter(|key| !key.starts_with("ieee802154-peer:"))
            .map(Claim::exclusive),
    );
    claims.extend(
        crate::fixture::software::providers(lab, request.required)
            .into_iter()
            .map(|provider| Claim::shared(crate::fixture::software::resource(provider))),
    );
    claims.extend(request.air.iter().flat_map(Spectrum::claims));
    claims
}

/// What the run's evidence records about the air: the ranges `request` used
/// with their need and emission, and every other lease held at the grant
/// with its claims, so a measurement under a strict need is told apart from
/// one taken beside other radio work.
pub fn air_record(request: &LeaseRequest) -> Result<serde_json::Value> {
    use oer_hil_arbiter::spectrum::{Emits, Need};
    let ranges = request
        .air
        .iter()
        .map(|range| {
            serde_json::json!({
                "low_khz": range.low_khz,
                "high_khz": range.high_khz,
                "need": match range.need {
                    Need::None => "none",
                    Need::Tolerant => "tolerant",
                    Need::Strict => "strict",
                },
                "emits": match range.emits {
                    Emits::None => "none",
                    Emits::Normal => "normal",
                    Emits::Noisy => "noisy",
                },
            })
        })
        .collect::<Vec<_>>();
    let concurrent = oer_hil_arbiter::Arbiter::open()?
        .status()?
        .holders
        .into_iter()
        .filter(|holder| holder.pid != std::process::id())
        .map(|holder| {
            serde_json::json!({"owner": holder.owner, "work": holder.work, "claims": holder.claims})
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({"schema": 1, "ranges": ranges, "concurrent": concurrent}))
}

/// Wait in the stand's queue for `claims`. The lease is described by this
/// runner's arguments; the environment supplies owner, budget and
/// short-lease choice. The lease supervises this process.
pub fn acquire_stand(
    claims: Vec<oer_hil_arbiter::Claim>,
    scenarios: &[String],
    divisible: bool,
) -> Result<oer_hil_arbiter::Grant> {
    let work = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut request = oer_hil_arbiter::Request::from_environment(work)?;
    request.scenarios = scenarios.to_vec();
    request.claims = claims;
    let mut grant = oer_hil_arbiter::Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(divisible);
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
                ..oer_hil_arbiter::Device::default()
            })?;
        }
        arbiter.record_board(device, kind)
    });
    if let Err(error) = result {
        eprintln!("hil-arbiter: cannot record board change: {error}");
    }
}

/// The image journaled last on the board at `port` when it is not
/// `expected`. A board without a journaled flash or identity has none; the
/// consumer's own handshake decides.
pub fn other_board_image(port: &Path, expected: &str) -> Result<Option<String>> {
    let Some(mac) = oer_hil_arbiter::port_mac(port) else {
        return Ok(None);
    };
    Ok(other_image(
        oer_hil_arbiter::Arbiter::open()?
            .latest_flash(&mac)?
            .as_ref(),
        expected,
    ))
}

fn other_image(latest: Option<&oer_hil_arbiter::BoardEvent>, expected: &str) -> Option<String> {
    match latest.map(|event| &event.kind) {
        Some(oer_hil_arbiter::BoardEventKind::Flashed { image, .. }) if image != expected => {
            Some(image.clone())
        }
        _ => None,
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
    match (latest, other_image(latest, expected)) {
        (Some(event), Some(image)) => Err(format!(
            "board {label} carries `{image}` flashed by {}, not `{expected}`; {reflash}",
            event.owner
        )
        .into()),
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
    if required.peer {
        keys.push(peer_key(
            lab.peer
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
fn peer_key(peer: &super::config::PeerBoardConfig) -> Result<String> {
    Ok(format!(
        "ieee802154-peer:{}",
        peer.serial()?.canonicalize()?.display()
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
    let key = bluetooth_key(adapter)?;
    let grant = acquire_stand(
        vec![
            oer_hil_arbiter::Claim::exclusive(&key),
            oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR),
        ],
        &[],
        false,
    )?;
    let directory = oer_esp32s31_firmware::device::lease_directory()?
        .join(format!("resource-{:x}", Sha256::digest(key.as_bytes())));
    Ok(BluetoothLease {
        _resource: wait_for_fixture(|| ResourceLease::acquire_directory(&directory))?,
        _grant: grant,
    })
}
