#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc cold-start failure retains the exact recoverable hardware frontier"
)]

//! Role-neutral ESP32-S31 radio initialization.
//!
//! Cold PHY and common MAC startup happen exactly once. Station, access-point,
//! scan and monitor owners are materialized later by the physical supervisor
//! from the returned stopped frontier; startup therefore cannot accidentally
//! lock the radio into the first role an application happens to use.

use crate::{
    cold_start::{
        WifiColdStartConfig as Esp32s31WifiStartConfig,
        WifiColdStartFailure as Esp32s31WifiStartFailure, resume_esp32s31_wifi,
        start_registered_esp32s31_wifi,
    },
    mac_start::{
        WifiMacPlatform, WifiMacStartConfig, WifiMacStartFailure, start_esp32s31_wifi_mac,
    },
    runtime::{WifiStopped, enter_esp32s31_wifi_runtime},
};

use oer_esp32s31_hal::owner::Radio;

use oer_esp32s31_phy::state::client::PhyPllTrackClock;
use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyCalibrationCache, PhyRegisterOutcome, PhyTargetObserver,
    RegisteredPhyColdReleased, RetainedPhy,
};

/// Inputs for the one common PHY/MAC transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioStartConfig {
    wifi: Esp32s31WifiStartConfig,
    mac: WifiMacStartConfig,
}

impl RadioStartConfig {
    pub const fn new(wifi: Esp32s31WifiStartConfig, mac: WifiMacStartConfig) -> Self {
        Self { wifi, mac }
    }
}

/// Role-neutral stopped Wi-Fi returned after cold common initialization.
pub struct RadioReady<P> {
    wifi: WifiStopped<P>,
    calibration_cache: Option<PhyCalibrationCache>,
    registration: PhyRegisterOutcome,
}

impl<P> RadioReady<P> {
    pub const fn wifi(&self) -> &WifiStopped<P> {
        &self.wifi
    }

    /// The cold registration this initialization performed.
    pub const fn registration(&self) -> PhyRegisterOutcome {
        self.registration
    }

    pub fn into_parts(self) -> (WifiStopped<P>, Option<PhyCalibrationCache>) {
        (self.wifi, self.calibration_cache)
    }
}

/// Role-neutral stopped Wi-Fi resumed from a retained PHY; no registration
/// ran.
pub struct RadioResumed<P> {
    wifi: WifiStopped<P>,
    calibration_cache: Option<PhyCalibrationCache>,
}

impl<P> RadioResumed<P> {
    pub const fn wifi(&self) -> &WifiStopped<P> {
        &self.wifi
    }

    pub fn into_parts(self) -> (WifiStopped<P>, Option<PhyCalibrationCache>) {
        (self.wifi, self.calibration_cache)
    }
}

/// Failed common initialization retaining the exact hardware frontier.
pub enum RadioStartFailure<P> {
    Wifi(Esp32s31WifiStartFailure<P>),
    Mac(WifiMacStartFailure<P>),
}

impl<P> RadioStartFailure<P> {
    /// Whether PHY execution failed without a completed safe cleanup. This
    /// observation does not release ownership or choose a platform response.
    pub fn phy_hardware_ambiguous(&self) -> bool {
        match self {
            Self::Wifi(Esp32s31WifiStartFailure::Registration(failure)) => {
                !failure.failure_cleanup_completed()
            }
            Self::Wifi(
                Esp32s31WifiStartFailure::RetainedWake(_)
                | Esp32s31WifiStartFailure::InitialTracking(_)
                | Esp32s31WifiStartFailure::InitialChannel { .. },
            ) => true,
            Self::Wifi(
                Esp32s31WifiStartFailure::Power(_) | Esp32s31WifiStartFailure::ClientAcquire(_),
            )
            | Self::Mac(_) => false,
        }
    }
}

/// Perform cold PHY and common MAC initialization without choosing a Wi-Fi
/// role. Role topology is validated for each supervisor epoch immediately
/// before that epoch consumes this stopped owner.
#[allow(
    large_assignments,
    reason = "the unique initialized radio owner graph crosses an explicit poll boundary; the linked-image stack-frame audit independently bounds this reviewed future"
)]
pub async fn start_esp32s31_radio<P, D, O>(
    radio: Radio<P>,
    config: RadioStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<RadioReady<P>, RadioStartFailure<P>>
where
    P: WifiMacPlatform,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let (wifi, registration) = start_registered_esp32s31_wifi::<P, D, O>(
        radio,
        config.wifi,
        calibration_cache,
        observer,
        clock,
    )
    .await
    .map_err(RadioStartFailure::Wifi)?;
    let mac = start_esp32s31_wifi_mac(wifi, config.mac).map_err(RadioStartFailure::Mac)?;
    let runtime = enter_esp32s31_wifi_runtime(mac);
    Ok(RadioReady {
        wifi: runtime.wifi,
        calibration_cache: runtime.calibration_cache,
        registration,
    })
}

/// Resume role-neutral Wi-Fi from a PHY another protocol route handed over.
///
/// The retained RF wake replaces power-up and registration; the common MAC
/// initialization then runs exactly as after a cold start, including its own
/// MAC clock enable and reset pulse.
///
/// # Cancellation
///
/// This has the same fail-stop cancellation contract as
/// [`start_esp32s31_radio`]. Once polled, it must reach a terminal result.
#[allow(
    large_assignments,
    reason = "the unique initialized radio owner graph crosses an explicit poll boundary; the linked-image stack-frame audit independently bounds this reviewed future"
)]
pub async fn resume_esp32s31_radio<P, D, O>(
    peripheral: P,
    retained: RetainedPhy,
    config: RadioStartConfig,
    calibration_cache: Option<PhyCalibrationCache>,
    observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<RadioResumed<P>, RadioStartFailure<P>>
where
    P: WifiMacPlatform,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let wifi = resume_esp32s31_wifi::<P, D, O>(
        peripheral,
        retained,
        config.wifi,
        calibration_cache,
        observer,
        clock,
    )
    .await
    .map_err(RadioStartFailure::Wifi)?;
    let mac = start_esp32s31_wifi_mac(wifi, config.mac).map_err(RadioStartFailure::Mac)?;
    let runtime = enter_esp32s31_wifi_runtime(mac);
    Ok(RadioResumed {
        wifi: runtime.wifi,
        calibration_cache: runtime.calibration_cache,
    })
}

/// Reconstruct a role-neutral Wi-Fi runtime after a completed cold release.
///
/// The released owner captures a cache from the final post-maintenance PHY
/// state using the same platform-derived identity selected for this cold
/// registration. Registration still validates the cache and republishes every
/// hardware-resident product; this is a cold restart, not retained sleep.
///
/// # Cancellation
///
/// This has the same fail-stop cancellation contract as
/// [`start_esp32s31_radio`]. Once polled, it must reach a terminal result.
#[must_use = "cold radio restart must be driven to a terminal result"]
pub fn restart_esp32s31_radio<'a, P, D, O>(
    released: RegisteredPhyColdReleased<P>,
    config: RadioStartConfig,
    observer: O,
    clock: &'a mut impl PhyPllTrackClock,
) -> impl core::future::Future<Output = Result<RadioReady<P>, RadioStartFailure<P>>> + 'a
where
    P: WifiMacPlatform + 'a,
    D: PhyAsyncDelay + 'a,
    O: PhyTargetObserver + Clone + 'a,
{
    let (radio, calibration_cache) = released.into_parts(config.wifi.calibration_identity);
    start_esp32s31_radio::<P, D, O>(radio, config, Some(calibration_cache), observer, clock)
}
