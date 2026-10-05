//! The lock a run holds on its boards, fixtures and the air: the
//! arbiter's grant, then the lock files of what it uses (the arbiter's
//! final exclusion layer), then the fixture software lease.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use oer_hil_arbiter::lock::{BoardLock, ResourceLock};
use oer_process::CommandExt as _;

use crate::Result;

/// The frequency ranges a lease uses and how.
pub use oer_hil_arbiter::spectrum::{BAND_2G4, Emits, Need, Spectrum};

/// Holds exclusive fixture ownership until the hardware command returns.
pub struct FixtureLock {
    /// The device under test's lock, when the lease includes it.
    device: Option<BoardLock>,
    /// The peer board's lock, when the run uses the peer.
    peer: Option<BoardLock>,
    _resources: Vec<ResourceLock>,
    /// Keeps the fixture software from being reinstalled during the lease.
    _software: Option<crate::software::SoftwareLease>,
    // Declared last so the fixture locks are released before the stand's
    // lease that ordered them.
    grant: Option<oer_hil_arbiter::Grant>,
    /// What to request again after yielding.
    request: Option<LeaseRequest>,
}

/// How a run leases the stand.
#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub required: oer_hil_scenario::requirements::Requirements,
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
    pub fn device(required: oer_hil_scenario::requirements::Requirements) -> Self {
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
        Self::acquire_for(lab, oer_hil_scenario::requirements::Requirements::default())
    }

    /// Wait for the stand's lease on the device and `required` fixtures,
    /// then take their locks.
    pub fn acquire_for(
        lab: &super::config::LabConfig,
        required: oer_hil_scenario::requirements::Requirements,
    ) -> Result<Self> {
        Self::lease(lab, LeaseRequest::device(required))
    }

    pub fn acquire_without_device(
        lab: &super::config::LabConfig,
        required: oer_hil_scenario::requirements::Requirements,
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
            &oer_hil_arbiter::Arbiter::open()?.with_stand_file(lab.path().to_owned()),
            claims(lab, &request, &keys),
            &request.scenarios,
            request.divisible,
        )?;
        let mut owner = oer_hil_arbiter::lock::wait_while_busy(|| {
            Self::lock_now(lab, &keys, request.device, request.required.peer)
        })?;
        // Taken only once granted: a queued run must not block an
        // installation queued before it.
        owner._software = Some(crate::software::SoftwareLease::acquire(
            crate::software::providers(lab, request.required),
        )?);
        owner.grant = Some(grant);
        owner.request = Some(request);
        Ok(owner)
    }

    /// The device under test's lock, when the lease includes it: what the
    /// flash operation writes the device under test under.
    pub fn device(&self) -> Option<&BoardLock> {
        self.device.as_ref()
    }

    /// The peer board's lock, when the run uses the peer.
    pub fn peer(&self) -> Option<&BoardLock> {
        self.peer.as_ref()
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

    /// Whether divisible work should renew this lease at its next boundary
    /// before the stand's hard limit ends it.
    pub fn renewal_due(&self) -> bool {
        self.grant
            .as_ref()
            .is_some_and(oer_hil_arbiter::Grant::renewal_due)
    }

    /// Release the lease, queue again behind the waiting requests and return
    /// the new lease. The boards' state is unknown afterwards.
    pub fn requeue(self, lab: &super::config::LabConfig) -> Result<Self> {
        let request = self
            .request
            .clone()
            .ok_or("only a queued lease can yield")?;
        if let Some(grant) = &self.grant {
            if grant.yield_requested() {
                grant.mark_yielded();
            } else {
                grant.mark_renewed();
            }
        }
        drop(self);
        Self::lease(lab, request)
    }

    /// Check that the device and fixture are free now, without queueing.
    pub fn probe_for(
        lab: &super::config::LabConfig,
        required: oer_hil_scenario::requirements::Requirements,
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
        Self::lock_now(lab, &keys, true, required.peer).map(drop)
    }

    fn lock_now(
        lab: &super::config::LabConfig,
        keys: &[String],
        device: bool,
        peer: bool,
    ) -> Result<Self> {
        let device = device
            .then(|| BoardLock::try_acquire(&lab.dut.mac))
            .transpose()?;
        // A peer that is not attached or not chosen is locked by no board;
        // the run's preflight reports it.
        let peer = peer
            .then(|| lab.peer().ok())
            .flatten()
            .map(|peer| BoardLock::try_acquire(&peer.mac))
            .transpose()?;
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        // Collect drops all acquired owners if any later resource is busy.
        let resources = keys
            .iter()
            .map(|key| ResourceLock::try_acquire(key))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            device,
            peer,
            _resources: resources,
            _software: None,
            grant: None,
            request: None,
        })
    }
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
        claims.push(Claim::board(&lab.dut.mac));
    }
    // A peer that is not chosen is claimed by no board; the run's preflight
    // reports it.
    if let Some(peer) = request.required.peer.then(|| lab.peer().ok()).flatten() {
        claims.push(Claim::board(&peer.mac));
    }
    claims.extend(keys.iter().map(Claim::exclusive));
    claims.extend(
        crate::software::providers(lab, request.required)
            .into_iter()
            .map(|provider| Claim::shared(crate::software::resource(provider))),
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
    arbiter: &oer_hil_arbiter::Arbiter,
    claims: Vec<oer_hil_arbiter::Claim>,
    scenarios: &[String],
    divisible: bool,
) -> Result<oer_hil_arbiter::Grant> {
    let work = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut request = oer_hil_arbiter::Request::from_environment(work)?;
    request.scenarios = scenarios.to_vec();
    request.claims = claims;
    let mut grant = arbiter.acquire(&request)?;
    grant.supervise_self(divisible);
    Ok(grant)
}

/// Interface aliases on one wiphy share one local owner. An OpenWrt client
/// changes firewall and radio state, so remote ownership covers the whole host
/// boot, including callers using different SSH aliases or radio interfaces.
fn resource_keys(
    lab: &super::config::LabConfig,
    required: oer_hil_scenario::requirements::Requirements,
) -> Result<Vec<String>> {
    use super::config::StationFixtureConfig;
    let mut keys = Vec::new();
    if required.bluetooth_adapter {
        keys.push(bluetooth_key(
            lab.bluetooth_adapter
                .ok_or("missing Bluetooth fixture adapter")?,
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
                let output = oer_hil_stand_host::ssh::command(
                    &config.ssh_target,
                    oer_hil_stand_host::ssh::BOOT_ID,
                )
                .supervised_output()?;
                if !output.status.success() {
                    return Err(
                        "cannot resolve OpenWrt host ownership over noninteractive SSH".into(),
                    );
                }
                keys.push(oer_hil_stand_host::ssh::host_key(std::str::from_utf8(
                    &output.stdout,
                )?)?);
            }
            StationFixtureConfig::External(_) => {}
        }
    }
    if required.station_network
        && let Some(observer) = &lab.air_observer
    {
        let output = oer_hil_stand_host::ssh::command(
            &observer.ssh_target,
            oer_hil_stand_host::ssh::BOOT_ID,
        )
        .supervised_output()?;
        if !output.status.success() {
            return Err("cannot resolve independent OpenWrt host ownership".into());
        }
        keys.push(oer_hil_stand_host::ssh::host_key(std::str::from_utf8(
            &output.stdout,
        )?)?);
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

/// The Bluetooth adapter alone, under the stand's lease.
pub struct BluetoothLease {
    _resource: ResourceLock,
    _grant: oer_hil_arbiter::Grant,
}

pub fn acquire_bluetooth(
    adapter: oer_hil_fixture::bluetooth::model::Adapter,
) -> Result<BluetoothLease> {
    let key = bluetooth_key(adapter)?;
    let grant = acquire_stand(
        &oer_hil_arbiter::Arbiter::open()?,
        vec![
            oer_hil_arbiter::Claim::exclusive(&key),
            oer_hil_arbiter::Claim::shared(oer_hil_arbiter::AIR),
        ],
        &[],
        false,
    )?;
    Ok(BluetoothLease {
        _resource: ResourceLock::acquire(&key)?,
        _grant: grant,
    })
}
