//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use crate::mac_start::{WifiMacReady, WifiMacStartReport};

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiClocksOn, WifiPowered},
    owner::{MacInterruptSetup, RadioRuntimeOwner},
    root::WifiPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
    types::MacInterruptEnableState,
};

use oer_esp32s31_phy::{
    concurrent::{ConcurrentAcquire, ConcurrentPhy, ConcurrentPhyError},
    state::client::PhyPllTrackClock,
    wifi_client::{
        WifiPhyMembership, WifiPhySuspended, leave_suspended_wifi, leave_wifi, resume_wifi,
        set_wifi_rx, suspend_wifi,
    },
};

use oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers;

use oer_ieee80211_mac::channel::WifiChannel;

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: MacInterruptEnableState,
}

/// Wi-Fi's place in the shared PHY domain.
pub enum WifiPhyClient {
    /// Wi-Fi is a PHY client; RF is available to its MAC.
    Member(WifiPhyMembership),
    /// Wi-Fi's RF sleeps for modem sleep; it keeps its registration.
    Suspended(WifiPhySuspended),
}

/// Why Wi-Fi's RF did not change its sleep state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRfSleepError {
    /// The RF already sleeps, or is already awake.
    State,
    /// The shared PHY domain rejected the change.
    Phy(ConcurrentPhyError),
}

/// Common stopped Wi-Fi owner after cold MAC initialization.
///
/// No Wi-Fi role owns DMA or an installed CPU interrupt route in this state.
/// A station, AP or standalone monitor must consume this value to begin its
/// own finite runtime epoch and return the same ownership frontier only after
/// both DMA and interrupt routing have acknowledged their stopped edges.
///
/// `W` is Wi-Fi's own MAC platform. The shared PHY, its platform token and
/// the clock sources stay with the radio arbiter; operations that touch them
/// take the arbiter lease.
pub struct WifiStopped<W> {
    platform: W,
    registers: RadioRuntimeOwner,
    interrupt_setup: MacInterruptSetup,
    clocks: WifiClocksOn,
    phy: WifiPhyClient,
    start_report: WifiMacStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

/// Why a stopped Wi-Fi client could not leave the shared radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiReleaseError {
    /// The MAC or the RX walker is still active.
    Hardware(oer_esp32s31_hal::owner::maintenance::Error),
    /// The shared PHY domain rejected the Wi-Fi client's release.
    Phy(ConcurrentPhyError),
    /// The initialized status or the Wi-Fi clocks could not change.
    Clocks(ModemClockError),
    /// Leaving common radio power failed a read-back checkpoint.
    Power(CommonRadioPowerError),
}

/// The Wi-Fi owners a failed release returns, at the stage it reached.
#[must_use = "a failed Wi-Fi release still owns the Wi-Fi partition"]
pub enum WifiReleaseFrontier<W> {
    /// Nothing changed; the stopped client is unchanged.
    Stopped(WifiStopped<W>),
    /// Wi-Fi left the PHY domain; its clocks are still on.
    Clocked { clocked: WifiClocked, platform: W },
    /// The Wi-Fi clocks are off; common power is still held.
    Powered { powered: WifiPowered, platform: W },
}

/// Failed release retaining the Wi-Fi owners at their exact stage.
#[must_use = "a failed Wi-Fi release still owns the Wi-Fi partition"]
pub struct WifiReleaseFailure<W> {
    error: WifiReleaseError,
    frontier: WifiReleaseFrontier<W>,
}

impl<W> WifiReleaseFailure<W> {
    pub const fn error(&self) -> WifiReleaseError {
        self.error
    }

    pub fn into_frontier(self) -> WifiReleaseFrontier<W> {
        self.frontier
    }
}

/// A Wi-Fi client that left the shared radio.
pub struct WifiReleased<W> {
    pub partition: WifiPartition,
    pub platform: W,
    /// Whether Wi-Fi was the last PHY client; the radio system then closes
    /// RF.
    pub last_phy_client: bool,
}

impl<W> WifiStopped<W> {
    pub const fn start_report(&self) -> WifiMacStartReport {
        self.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.transition_report
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    /// Leave the shared radio from this fully stopped runtime frontier:
    /// `esp_phy_disable(PHY_MODEM_WIFI)`, then `esp_wifi_deinit`'s
    /// initialized status and module clock release, then common power.
    ///
    /// The stopped MAC and RX walker are checked before any change. Wi-Fi RX
    /// is disabled at the vendor-shaped per-client edge. RF close after the
    /// last client belongs to the radio system.
    #[allow(
        clippy::result_large_err,
        reason = "failure must return the complete Wi-Fi frontier"
    )]
    pub fn release(
        self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        clocks: &mut impl PlatformClockProvider,
    ) -> Result<WifiReleased<W>, WifiReleaseFailure<W>> {
        let Self {
            platform,
            registers,
            interrupt_setup,
            clocks: clocks_on,
            phy,
            start_report,
            transition_report,
            current_channel,
        } = self;
        let (registers, interrupt_setup) = match registers.try_confirm_phy_stopped(interrupt_setup)
        {
            Ok(frontier) => frontier,
            Err(failure) => {
                return Err(WifiReleaseFailure {
                    error: WifiReleaseError::Hardware(failure.error),
                    frontier: WifiReleaseFrontier::Stopped(Self {
                        platform,
                        registers: failure.registers,
                        interrupt_setup: failure.interrupts,
                        clocks: clocks_on,
                        phy,
                        start_report,
                        transition_report,
                        current_channel,
                    }),
                });
            }
        };
        let mut clocked = WifiClocked::from_running(registers, interrupt_setup, clocks_on);
        let last_phy_client = match phy {
            // Modem sleep already left the PHY client set and cleared the
            // Wi-Fi receive path; only its token ends.
            WifiPhyClient::Suspended(suspended) => {
                leave_suspended_wifi(suspended);
                false
            }
            WifiPhyClient::Member(membership) => {
                set_wifi_rx(lease, &membership, false);
                match leave_wifi(lease, &clocked, membership) {
                    Ok(last) => last,
                    Err(failure) => {
                        let error = WifiReleaseError::Phy(failure.error());
                        let membership = failure.into_membership();
                        set_wifi_rx(lease, &membership, true);
                        let (registers, interrupt_setup, clocks_on) = clocked.into_running();
                        return Err(WifiReleaseFailure {
                            error,
                            frontier: WifiReleaseFrontier::Stopped(Self {
                                platform,
                                registers,
                                interrupt_setup,
                                clocks: clocks_on,
                                phy: WifiPhyClient::Member(membership),
                                start_report,
                                transition_report,
                                current_channel,
                            }),
                        });
                    }
                }
            }
        };
        if let Err(error) = clocked.set_initialized(lease, false) {
            return Err(WifiReleaseFailure {
                error: WifiReleaseError::Clocks(error),
                frontier: WifiReleaseFrontier::Clocked { clocked, platform },
            });
        }
        let powered = match clocked.disable_clocks(lease, clocks) {
            Ok(powered) => powered,
            Err(failure) => {
                return Err(WifiReleaseFailure {
                    error: WifiReleaseError::Clocks(failure.error()),
                    frontier: WifiReleaseFrontier::Clocked {
                        clocked: failure.into_owner(),
                        platform,
                    },
                });
            }
        };
        match powered.power_down(lease) {
            Ok(cold) => Ok(WifiReleased {
                partition: cold.into_partition(),
                platform,
                last_phy_client,
            }),
            Err(failure) => Err(WifiReleaseFailure {
                error: WifiReleaseError::Power(failure.error()),
                frontier: WifiReleaseFrontier::Powered {
                    powered: failure.into_owner(),
                    platform,
                },
            }),
        }
    }

    /// Borrow the role-neutral radio state for stopped-only operations.
    pub fn radio_mut(&mut self) -> (oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_>, &mut W) {
        (self.registers.wifi_mac_hal(), &mut self.platform)
    }

    /// Move the exact common ownership frontier into one role runtime.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> WifiRuntimeParts<W> {
        WifiRuntimeParts {
            platform: self.platform,
            registers: self.registers,
            interrupt_setup: self.interrupt_setup,
            context: WifiRuntimeContext {
                clocks: self.clocks,
                phy: Some(self.phy),
                start_report: self.start_report,
                transition_report: self.transition_report,
                current_channel: self.current_channel,
            },
        }
    }
}

/// Atomic transfer object between the common stopped owner and one role.
///
/// Keeping the PHY membership, MMIO, interrupt setup and platform ownership
/// together avoids a public constructor which could combine pieces from
/// unrelated epochs.
#[doc(hidden)]
pub struct WifiRuntimeParts<W> {
    pub platform: W,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub context: WifiRuntimeContext,
}

/// Common Wi-Fi state retained beside one materialized role.
///
/// Register ownership and the interrupt setup token deliberately do not live
/// in this value. A role can reconstruct [`WifiStopped`] only after
/// its DMA/task graph returns the exact runtime owner and its interrupt
/// route returns the exact [`MacInterruptSetup`].
#[doc(hidden)]
pub struct WifiRuntimeContext {
    clocks: WifiClocksOn,
    /// Always present outside the PHY transitions below, which restore it
    /// before they return.
    phy: Option<WifiPhyClient>,
    start_report: WifiMacStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl WifiRuntimeContext {
    /// Whether Wi-Fi's RF sleeps for modem sleep.
    pub const fn rf_asleep(&self) -> bool {
        matches!(self.phy, Some(WifiPhyClient::Suspended(_)))
    }

    /// Put Wi-Fi's RF to sleep, as the vendor modem sleep's
    /// `wifi_rf_phy_disable` does: clear the Wi-Fi receive path, then leave
    /// the PHY client set, keeping the registration and calibration.
    /// Returns whether Wi-Fi was the last client; the radio system then
    /// closes RF.
    ///
    /// # Errors
    ///
    /// The RF sleeps already, or the domain rejected the release; the
    /// receive path is restored and nothing changed.
    pub fn suspend_rf(
        &mut self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    ) -> Result<bool, WifiRfSleepError> {
        let membership = match self.phy.take() {
            Some(WifiPhyClient::Member(membership)) => membership,
            other => {
                self.phy = other;
                return Err(WifiRfSleepError::State);
            }
        };
        set_wifi_rx(lease, &membership, false);
        match suspend_wifi(lease, &self.clocks, membership) {
            Ok((suspended, last)) => {
                self.phy = Some(WifiPhyClient::Suspended(suspended));
                Ok(last)
            }
            Err(failure) => {
                let error = failure.error();
                let membership = failure.into_membership();
                set_wifi_rx(lease, &membership, true);
                self.phy = Some(WifiPhyClient::Member(membership));
                Err(WifiRfSleepError::Phy(error))
            }
        }
    }

    /// Wake Wi-Fi's RF, as the vendor modem wake's `wifi_rf_phy_enable`
    /// does: re-enter the PHY client set, then enable the Wi-Fi receive
    /// path. The radio system must have woken closed RF first. A returned
    /// [`ConcurrentAcquire::TrackingDue`] means the domain must run its
    /// tracking before the MAC uses RF.
    ///
    /// # Errors
    ///
    /// The RF is awake already, or the domain rejected the client; nothing
    /// changed.
    pub fn resume_rf(
        &mut self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        clock: &mut impl PhyPllTrackClock,
    ) -> Result<ConcurrentAcquire, WifiRfSleepError> {
        let suspended = match self.phy.take() {
            Some(WifiPhyClient::Suspended(suspended)) => suspended,
            other => {
                self.phy = other;
                return Err(WifiRfSleepError::State);
            }
        };
        match resume_wifi(lease, &self.clocks, suspended, clock) {
            Ok((membership, acquired)) => {
                set_wifi_rx(lease, &membership, true);
                self.phy = Some(WifiPhyClient::Member(membership));
                Ok(acquired)
            }
            Err(failure) => {
                let error = failure.error();
                self.phy = Some(WifiPhyClient::Suspended(failure.into_suspended()));
                Err(WifiRfSleepError::Phy(error))
            }
        }
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.current_channel = channel;
    }

    /// Reconstruct the role-neutral owner from independently proven DMA/task
    /// and interrupt-route return edges.
    pub fn into_stopped<W>(
        self,
        platform: W,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
    ) -> WifiStopped<W> {
        WifiStopped {
            platform,
            registers,
            interrupt_setup,
            clocks: self.clocks,
            phy: self
                .phy
                .expect("the PHY transitions restore the client before they return"),
            start_report: self.start_report,
            transition_report: self.transition_report,
            current_channel: self.current_channel,
        }
    }
}

/// Unique Wi-Fi platform owner while any finite Wi-Fi role graph is active.
///
/// RX DMA, register and interrupt capabilities move independently into the
/// concrete epoch. The owner is deliberately role-neutral: a same-channel
/// STA+AP graph still has exactly one Wi-Fi client and must not manufacture
/// separate station and access-point owners around it.
pub struct WifiRoleOwner<W> {
    platform: W,
    context: WifiRuntimeContext,
}

impl<W> WifiRoleOwner<W> {
    pub const fn start_report(&self) -> WifiMacStartReport {
        self.context.start_report
    }

    pub const fn transition_report(&self) -> WifiRuntimeTransitionReport {
        self.context.transition_report
    }

    pub fn platform_mut(&mut self) -> &mut W {
        &mut self.platform
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.context.set_current_channel(channel);
    }

    /// The role's common Wi-Fi state, for its RF sleep and wake.
    pub fn context_mut(&mut self) -> &mut WifiRuntimeContext {
        &mut self.context
    }

    /// Split the role-neutral logical owner from the platform value while a
    /// concrete role owns the physical register and interrupt epochs.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> (W, WifiRuntimeContext) {
        (self.platform, self.context)
    }

    /// Rejoin the exact platform and logical context returned by a role.
    #[doc(hidden)]
    pub fn from_runtime_parts(platform: W, context: WifiRuntimeContext) -> Self {
        Self { platform, context }
    }

    /// Reassemble stopped Wi-Fi only after the exact register and interrupt
    /// setup owners have returned from the active epoch.
    pub fn into_stopped<L>(
        self,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> WifiRoleStopped<W, L> {
        WifiRoleStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

/// Exact transfer from stopped Wi-Fi into one finite role composition.
pub struct WifiRoleMaterialized<W, L> {
    pub owner: WifiRoleOwner<W>,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub resources: L,
}

/// Cleanly dematerialized finite role graph.
pub struct WifiRoleStopped<W, L> {
    pub wifi: WifiStopped<W>,
    pub resources: L,
}

/// Consume the common stopped owner before starting any finite role graph.
pub fn materialize_esp32s31_wifi_role<W, L>(
    wifi: WifiStopped<W>,
    resources: L,
) -> WifiRoleMaterialized<W, L> {
    let WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    WifiRoleMaterialized {
        owner: WifiRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}

/// Close the cold polling phase and enter the reusable stopped-runtime state.
///
/// This is the only normal conversion from [`WifiMacReady`]. It masks
/// and acknowledges cold interrupt state before exposing the setup token used
/// by a finite task-owned interrupt epoch. It touches Wi-Fi MAC registers
/// only and needs no arbiter lease.
pub fn enter_esp32s31_wifi_runtime<W>(mac: WifiMacReady<W>) -> WifiStopped<W> {
    let start_report = mac.report();
    let WifiMacReady {
        mut clocked,
        membership,
        platform,
        ..
    } = mac;
    let cold_interrupt_mask = clocked.close_cold_interrupt_phase();
    let (mut registers, interrupt_setup, clocks) = clocked.into_running();
    // Cold `wifi_set_rx_policy(0)` first publishes both interface addresses,
    // then disables their receive policies. Our cold address transaction is
    // already complete; finish that exact role-neutral suffix before exposing
    // a stopped runtime. Scan/monitor subsequently own only queue three's
    // explicit promiscuous admission, while STA/AP reopen their own context.
    {
        let mut mac = registers.wifi_mac_hal();
        disable_all_role_receive_registers(&mut mac);
        // Complete the role-neutral half of the vendor no-power-save lifecycle.
        // The first role arms its replacement RX ring while this request remains
        // asserted and resumes the frontend only after those credits are live.
        mac.request_channel_stop();
    }
    WifiStopped {
        platform,
        registers,
        interrupt_setup,
        clocks,
        phy: WifiPhyClient::Member(membership),
        start_report,
        transition_report: WifiRuntimeTransitionReport {
            cold_interrupt_mask,
        },
        current_channel: start_report.wifi.initial_channel,
    }
}
