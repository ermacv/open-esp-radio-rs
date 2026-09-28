//! Role-neutral ESP32-S31 Wi-Fi initialization on the shared radio.
//!
//! The Wi-Fi bring-up and common MAC startup happen once per Wi-Fi epoch.
//! Station, access-point, scan and monitor owners are materialized later by
//! the physical supervisor from the returned stopped frontier; startup
//! therefore cannot accidentally lock the radio into the first role an
//! application happens to use.

use crate::{
    cold_start::{
        WifiColdStartConfig as Esp32s31WifiStartConfig,
        WifiColdStartFailure as Esp32s31WifiStartFailure, start_esp32s31_wifi,
    },
    mac_start::{
        WifiMacPlatform, WifiMacStartConfig, WifiMacStartFailure, start_esp32s31_wifi_mac,
    },
    runtime::{WifiStopped, enter_esp32s31_wifi_runtime},
};

use oer_esp32s31_hal::{
    root::WifiPartition,
    shared_radio::{PlatformClockProvider, SharedRadioLease},
};

use oer_esp32s31_phy::{
    PhyAsyncDelay, PhyTargetObserver, concurrent::ConcurrentPhy, state::client::PhyPllTrackClock,
};

/// Inputs for the one common Wi-Fi PHY/MAC transition.
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

/// Failed common initialization retaining the exact Wi-Fi frontier.
#[allow(
    clippy::large_enum_variant,
    reason = "no-alloc start failure retains the exact recoverable Wi-Fi frontier"
)]
pub enum RadioStartFailure<W> {
    Wifi(Esp32s31WifiStartFailure<W>),
    Mac(WifiMacStartFailure<W>),
}

impl<W> RadioStartFailure<W> {
    /// Whether a started PHY transaction failed and poisoned the shared
    /// domain. This observation does not release ownership or choose a
    /// platform response.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        match self {
            Self::Wifi(failure) => failure.phy_hardware_ambiguous(),
            Self::Mac(_) => false,
        }
    }
}

/// Bring Wi-Fi up on the registered, RF-open shared PHY domain and perform
/// the common MAC initialization without choosing a Wi-Fi role. Role
/// topology is validated for each supervisor epoch immediately before that
/// epoch consumes this stopped owner.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result.
#[allow(
    clippy::too_many_arguments,
    reason = "the arbiter lease, both platforms and the clock sources are distinct owners"
)]
pub async fn start_esp32s31_radio<P, W, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &impl PlatformClockProvider,
    partition: WifiPartition,
    wifi_platform: W,
    config: RadioStartConfig,
    observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<WifiStopped<W>, RadioStartFailure<W>>
where
    W: WifiMacPlatform,
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let wifi = start_esp32s31_wifi::<P, W, D, O>(
        lease,
        platform,
        clocks,
        partition,
        wifi_platform,
        config.wifi,
        observer,
        clock,
    )
    .await
    .map_err(RadioStartFailure::Wifi)?;
    let mac = start_esp32s31_wifi_mac(wifi, lease, config.mac).map_err(RadioStartFailure::Mac)?;
    Ok(enter_esp32s31_wifi_runtime(mac))
}
