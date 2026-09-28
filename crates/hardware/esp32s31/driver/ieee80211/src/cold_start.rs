//! Reusable role-neutral Wi-Fi bring-up as a client of the radio arbiter.
//!
//! This boundary owns the production ordering shared by standalone firmware
//! and HIL once the shared PHY domain is registered with RF open: common
//! radio power, the Wi-Fi module clocks and initialized status, the Wi-Fi PHY
//! client with its initial tracking, the Wi-Fi RX enable and the initial
//! channel. Registration, RF close and wake belong to the composition that
//! owns the arbiter. Board token construction and diagnostics remain caller
//! policy.

use crate::channel::lower_wifi_channel;

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiCold, WifiPowered},
    root::WifiPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
};

use oer_esp32s31_phy::{
    ConcurrentPhyTrackingError, ConcurrentWifiChannelError, PhyAsyncDelay, PhyTargetObserver,
    PhyTxTargetPowerProfile,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    maintain_concurrent_phy, select_concurrent_wifi_channel,
    state::client::PhyPllTrackClock,
    tracking::PhyParamTrackingOutcome,
    wifi_client::{WifiPhyMembership, join_wifi, set_wifi_rx},
};

use oer_ieee80211_mac::channel::WifiChannel;

/// Application-selected inputs for one Wi-Fi bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: wifi-frequency-tx-power-antenna-ftm-and-esp-now-adjustable-tx-power-ceiling
pub struct WifiColdStartConfig {
    pub initial_channel: WifiChannel,
    pub maximum_tx_power_quarter_dbm: i8,
}

impl WifiColdStartConfig {
    pub const fn new(initial_channel: WifiChannel) -> Self {
        Self {
            initial_channel,
            maximum_tx_power_quarter_dbm: i8::MAX,
        }
    }

    pub const fn with_maximum_tx_power_quarter_dbm(mut self, maximum: i8) -> Self {
        self.maximum_tx_power_quarter_dbm = maximum;
        self
    }
}

/// Observable result of the finite bring-up without HIL telemetry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiColdStartReport {
    pub initial_channel: WifiChannel,
    /// None when joining the domain did not require tracking.
    pub initial_tracking: Option<PhyParamTrackingOutcome>,
    /// The calibrated TX target-power profile under the configured ceiling;
    /// every Wi-Fi TX vector takes its power from it.
    pub tx_power: PhyTxTargetPowerProfile,
}

/// Complete owner set returned at the cold-MAC boundary.
pub struct WifiColdStart<W> {
    pub(crate) clocked: WifiClocked,
    pub(crate) membership: WifiPhyMembership,
    pub(crate) platform: W,
    pub(crate) report: WifiColdStartReport,
}

impl<W> WifiColdStart<W> {
    pub const fn report(&self) -> WifiColdStartReport {
        self.report
    }
}

/// Why the bring-up stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiColdStartError {
    /// Common radio power was rejected or failed a read-back checkpoint.
    Power(CommonRadioPowerError),
    /// The Wi-Fi module clocks or the initialized status could not change.
    Clocks(ModemClockError),
    /// The shared PHY domain rejected the Wi-Fi client.
    Join(ConcurrentPhyError),
    /// The initial tracking was rejected or failed.
    InitialTracking(ConcurrentPhyTrackingError),
    /// The initial channel was rejected or failed.
    InitialChannel(ConcurrentWifiChannelError),
}

/// The Wi-Fi owners a failed bring-up returns, at the stage it reached.
#[must_use = "a failed Wi-Fi bring-up still owns the Wi-Fi partition"]
pub enum WifiColdStartFrontier<W> {
    /// Nothing changed; the partition is unpowered.
    Cold { cold: WifiCold, platform: W },
    /// Common power is held; the Wi-Fi clocks are off.
    Powered { powered: WifiPowered, platform: W },
    /// The Wi-Fi clocks are on; Wi-Fi is not a PHY client.
    Clocked { clocked: WifiClocked, platform: W },
    /// Wi-Fi is a PHY client with its baseband receive path disabled, unless
    /// a started PHY transaction failed.
    Joined {
        clocked: WifiClocked,
        membership: WifiPhyMembership,
        platform: W,
    },
}

/// Failed bring-up retaining the Wi-Fi owners at their exact stage.
#[must_use = "a failed Wi-Fi bring-up still owns the Wi-Fi partition"]
pub struct WifiColdStartFailure<W> {
    error: WifiColdStartError,
    frontier: WifiColdStartFrontier<W>,
}

impl<W> WifiColdStartFailure<W> {
    pub const fn error(&self) -> WifiColdStartError {
        self.error
    }

    /// Whether a started PHY transaction failed: the shared domain is
    /// poisoned and the radio requires reset.
    pub const fn phy_hardware_ambiguous(&self) -> bool {
        matches!(
            self.error,
            WifiColdStartError::InitialTracking(ConcurrentPhyTrackingError::Failed(_))
                | WifiColdStartError::InitialChannel(ConcurrentWifiChannelError::Failed(_))
        )
    }

    pub fn into_frontier(self) -> WifiColdStartFrontier<W> {
        self.frontier
    }
}

/// Bring Wi-Fi up as a client of the registered, RF-open shared PHY domain.
///
/// `platform` and `clocks` are the radio system's PHY platform token and
/// clock sources; `wifi_platform` is Wi-Fi's own MAC platform, kept with the
/// returned owner. Initial tracking follows the domain's admission policy
/// without quiescence proofs.
///
/// # Cancellation
///
/// Once polled, drive this future to a terminal result: cancelling during
/// tracking or the channel transaction leaves the shared PHY ambiguous.
#[must_use = "Wi-Fi bring-up must be driven to a terminal result"]
#[allow(
    clippy::too_many_arguments,
    reason = "the arbiter lease, both platforms and the clock sources are distinct owners"
)]
pub async fn start_esp32s31_wifi<P, W, D, O>(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    clocks: &mut impl PlatformClockProvider,
    partition: WifiPartition,
    wifi_platform: W,
    config: WifiColdStartConfig,
    mut observer: O,
    clock: &mut impl PhyPllTrackClock,
) -> Result<WifiColdStart<W>, WifiColdStartFailure<W>>
where
    D: PhyAsyncDelay,
    O: PhyTargetObserver + Clone,
{
    let powered = match WifiCold::from_partition(partition).power_up(lease, clocks) {
        Ok(powered) => powered,
        Err(failure) => {
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::Power(failure.error()),
                frontier: WifiColdStartFrontier::Cold {
                    cold: failure.into_owner(),
                    platform: wifi_platform,
                },
            });
        }
    };
    let mut clocked = match powered.enable_clocks(lease, clocks) {
        Ok(clocked) => clocked,
        Err(failure) => {
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::Clocks(failure.error()),
                frontier: WifiColdStartFrontier::Powered {
                    powered: failure.into_owner(),
                    platform: wifi_platform,
                },
            });
        }
    };
    if let Err(error) = clocked.set_initialized(lease, true) {
        return Err(WifiColdStartFailure {
            error: WifiColdStartError::Clocks(error),
            frontier: WifiColdStartFrontier::Clocked {
                clocked,
                platform: wifi_platform,
            },
        });
    }
    let (membership, acquired) = match join_wifi(lease, &clocked, clock) {
        Ok(joined) => joined,
        Err(error) => {
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::Join(error),
                frontier: WifiColdStartFrontier::Clocked {
                    clocked,
                    platform: wifi_platform,
                },
            });
        }
    };
    let initial_tracking = if acquired == ConcurrentAcquire::TrackingDue {
        match maintain_concurrent_phy::<P, D, _>(lease, platform, &[], observer.clone()).await {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                return Err(WifiColdStartFailure {
                    error: WifiColdStartError::InitialTracking(error),
                    frontier: WifiColdStartFrontier::Joined {
                        clocked,
                        membership,
                        platform: wifi_platform,
                    },
                });
            }
        }
    } else {
        None
    };

    set_wifi_rx(lease, &membership, true);
    let initial_channel = lower_wifi_channel(config.initial_channel);
    let selected = {
        let (mut channel, phy) = clocked.channel_hal_with_attachment(platform, lease);
        select_concurrent_wifi_channel::<D, _, _>(
            phy,
            initial_channel.channel_or_frequency,
            initial_channel.cbw,
            &mut channel,
            &mut observer,
        )
        .await
    };
    if let Err(error) = selected {
        if matches!(error, ConcurrentWifiChannelError::Rejected(_)) {
            // Nothing started; restore the receive path the joined frontier
            // reports.
            set_wifi_rx(lease, &membership, false);
        }
        return Err(WifiColdStartFailure {
            error: WifiColdStartError::InitialChannel(error),
            frontier: WifiColdStartFrontier::Joined {
                clocked,
                membership,
                platform: wifi_platform,
            },
        });
    }

    let tx_power = match lease.attachment().phy_state() {
        Ok(state) => state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(config.maximum_tx_power_quarter_dbm),
        Err(error) => {
            set_wifi_rx(lease, &membership, false);
            return Err(WifiColdStartFailure {
                error: WifiColdStartError::InitialChannel(ConcurrentWifiChannelError::Rejected(
                    error,
                )),
                frontier: WifiColdStartFrontier::Joined {
                    clocked,
                    membership,
                    platform: wifi_platform,
                },
            });
        }
    };
    Ok(WifiColdStart {
        clocked,
        membership,
        platform: wifi_platform,
        report: WifiColdStartReport {
            initial_channel: config.initial_channel,
            initial_tracking,
            tx_power,
        },
    })
}
