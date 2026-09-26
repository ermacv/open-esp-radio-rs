//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use crate::mac_start::{WifiMacReady, WifiMacStartReport};

use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiPowered},
    owner::{MacInterruptSetup, RadioRuntimeOwner},
    root::WifiPartition,
    shared_radio::{
        CommonRadioPowerError, ModemClockError, PlatformClockProvider, SharedRadioLease,
    },
    types::MacInterruptEnableState,
};

use oer_esp32s31_phy::{
    concurrent::{ConcurrentPhy, ConcurrentPhyError},
    wifi_client::{WifiPhyMembership, leave_wifi, set_wifi_rx},
};

use oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers;

use oer_ieee80211_mac::channel::WifiChannel;

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: MacInterruptEnableState,
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
    membership: WifiPhyMembership,
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
            membership,
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
                        membership,
                        start_report,
                        transition_report,
                        current_channel,
                    }),
                });
            }
        };
        let mut clocked = WifiClocked::from_running(registers, interrupt_setup);
        set_wifi_rx(lease, &membership, false);
        let last_phy_client = match leave_wifi(lease, &clocked, membership) {
            Ok(last) => last,
            Err(failure) => {
                let error = WifiReleaseError::Phy(failure.error());
                let membership = failure.into_membership();
                set_wifi_rx(lease, &membership, true);
                let (registers, interrupt_setup) = clocked.into_running();
                return Err(WifiReleaseFailure {
                    error,
                    frontier: WifiReleaseFrontier::Stopped(Self {
                        platform,
                        registers,
                        interrupt_setup,
                        membership,
                        start_report,
                        transition_report,
                        current_channel,
                    }),
                });
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
                membership: self.membership,
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
    membership: WifiPhyMembership,
    start_report: WifiMacStartReport,
    transition_report: WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl WifiRuntimeContext {
    /// The Wi-Fi client's membership in the shared PHY domain.
    pub const fn membership(&self) -> &WifiPhyMembership {
        &self.membership
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
            membership: self.membership,
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
    let (mut registers, interrupt_setup) = clocked.into_running();
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
        membership,
        start_report,
        transition_report: WifiRuntimeTransitionReport {
            cold_interrupt_mask,
        },
        current_channel: start_report.wifi.initial_channel,
    }
}
