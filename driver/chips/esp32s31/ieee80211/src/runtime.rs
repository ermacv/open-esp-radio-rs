//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use open_esp_radio_esp32s31_hal::{MacInterruptEnableState, MacInterruptSetup, RadioRuntimeOwner};
use open_esp_radio_esp32s31_phy::{PhyCalibrationCache, PhyState};
use open_esp_radio_esp32s31_wifi_mac::sta_ap_registers::disable_all_role_receive_registers;
use open_esp_radio_ieee80211::channel::WifiChannel;

use crate::mac_start::{Esp32s31WifiMacReady, Esp32s31WifiMacStartReport};

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: MacInterruptEnableState,
}

/// Common stopped Wi-Fi owner after cold MAC initialization.
///
/// No Wi-Fi role owns DMA or an installed CPU interrupt route in this state.
/// A station, AP or standalone monitor must consume this value to begin its
/// own finite runtime epoch and return the same ownership frontier only after
/// both DMA and interrupt routing have acknowledged their stopped edges.
pub struct Esp32s31WifiStopped<P> {
    platform: P,
    registers: RadioRuntimeOwner,
    interrupt_setup: MacInterruptSetup,
    phy: PhyState,
    start_report: Esp32s31WifiMacStartReport,
    transition_report: Esp32s31WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl<P> Esp32s31WifiStopped<P> {
    pub const fn start_report(&self) -> Esp32s31WifiMacStartReport {
        self.start_report
    }

    pub const fn transition_report(&self) -> Esp32s31WifiRuntimeTransitionReport {
        self.transition_report
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    /// Borrow the role-neutral radio state for stopped-only operations.
    pub fn radio_mut(
        &mut self,
    ) -> (
        open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal<'_>,
        &mut P,
    ) {
        (self.registers.wifi_mac_hal(), &mut self.platform)
    }

    /// Move the exact common ownership frontier into one role runtime.
    #[doc(hidden)]
    pub fn into_runtime_parts(self) -> Esp32s31WifiRuntimeParts<P> {
        Esp32s31WifiRuntimeParts {
            platform: self.platform,
            registers: self.registers,
            interrupt_setup: self.interrupt_setup,
            context: Esp32s31WifiRuntimeContext {
                phy: self.phy,
                start_report: self.start_report,
                transition_report: self.transition_report,
                current_channel: self.current_channel,
            },
        }
    }
}

/// Atomic transfer object between the common stopped owner and one role.
///
/// Keeping PHY, MMIO, interrupt setup and platform ownership together avoids
/// a public constructor which could combine pieces from unrelated epochs.
#[doc(hidden)]
pub struct Esp32s31WifiRuntimeParts<P> {
    pub platform: P,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub context: Esp32s31WifiRuntimeContext,
}

/// Common Wi-Fi state retained beside one materialized role.
///
/// Register ownership and the interrupt setup token deliberately do not live
/// in this value. A role can reconstruct [`Esp32s31WifiStopped`] only after
/// its DMA/task graph returns the exact runtime owner and its interrupt
/// route returns the exact [`MacInterruptSetup`].
#[doc(hidden)]
pub struct Esp32s31WifiRuntimeContext {
    phy: PhyState,
    start_report: Esp32s31WifiMacStartReport,
    transition_report: Esp32s31WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl Esp32s31WifiRuntimeContext {
    pub fn phy_mut(&mut self) -> &mut PhyState {
        &mut self.phy
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.current_channel = channel;
    }

    /// Reconstruct the role-neutral owner from independently proven DMA/task
    /// and interrupt-route return edges.
    pub fn into_stopped<P>(
        self,
        platform: P,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
    ) -> Esp32s31WifiStopped<P> {
        Esp32s31WifiStopped {
            platform,
            registers,
            interrupt_setup,
            phy: self.phy,
            start_report: self.start_report,
            transition_report: self.transition_report,
            current_channel: self.current_channel,
        }
    }
}

/// Unique PHY/platform owner while any finite Wi-Fi role graph is active.
///
/// RX DMA, register and interrupt capabilities move independently into the
/// concrete epoch. The owner is deliberately role-neutral: a same-channel
/// STA+AP graph still has exactly one physical radio and must not manufacture
/// separate station and access-point owners around it.
pub struct Esp32s31WifiRoleOwner<P> {
    platform: P,
    context: Esp32s31WifiRuntimeContext,
}

impl<P> Esp32s31WifiRoleOwner<P> {
    pub const fn start_report(&self) -> Esp32s31WifiMacStartReport {
        self.context.start_report
    }

    pub const fn transition_report(&self) -> Esp32s31WifiRuntimeTransitionReport {
        self.context.transition_report
    }

    pub fn radio_mut(&mut self) -> (&mut PhyState, &mut P) {
        (self.context.phy_mut(), &mut self.platform)
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
    pub fn into_runtime_parts(self) -> (P, Esp32s31WifiRuntimeContext) {
        (self.platform, self.context)
    }

    /// Rejoin the exact platform and logical context returned by a role.
    #[doc(hidden)]
    pub fn from_runtime_parts(platform: P, context: Esp32s31WifiRuntimeContext) -> Self {
        Self { platform, context }
    }

    /// Reassemble stopped Wi-Fi only after the exact register and interrupt
    /// setup owners have returned from the active epoch.
    pub fn into_stopped<L>(
        self,
        registers: RadioRuntimeOwner,
        interrupt_setup: MacInterruptSetup,
        resources: L,
    ) -> Esp32s31WifiRoleStopped<P, L> {
        Esp32s31WifiRoleStopped {
            wifi: self
                .context
                .into_stopped(self.platform, registers, interrupt_setup),
            resources,
        }
    }
}

/// Exact transfer from stopped Wi-Fi into one finite role composition.
pub struct Esp32s31WifiRoleMaterialized<P, L> {
    pub owner: Esp32s31WifiRoleOwner<P>,
    pub registers: RadioRuntimeOwner,
    pub interrupt_setup: MacInterruptSetup,
    pub resources: L,
}

/// Cleanly dematerialized finite role graph.
pub struct Esp32s31WifiRoleStopped<P, L> {
    pub wifi: Esp32s31WifiStopped<P>,
    pub resources: L,
}

/// Consume the common stopped owner before starting any finite role graph.
pub fn materialize_esp32s31_wifi_role<P, L>(
    wifi: Esp32s31WifiStopped<P>,
    resources: L,
) -> Esp32s31WifiRoleMaterialized<P, L> {
    let Esp32s31WifiRuntimeParts {
        platform,
        registers,
        interrupt_setup,
        context,
    } = wifi.into_runtime_parts();
    Esp32s31WifiRoleMaterialized {
        owner: Esp32s31WifiRoleOwner { platform, context },
        registers,
        interrupt_setup,
        resources,
    }
}

/// Close the cold polling phase and enter the reusable stopped-runtime state.
///
/// This is the only normal conversion from [`Esp32s31WifiMacReady`]. It masks
/// and acknowledges cold interrupt state before exposing the setup token used
/// by a finite task-owned interrupt epoch.
pub struct Esp32s31WifiRuntimeStart<P> {
    pub wifi: Esp32s31WifiStopped<P>,
    pub calibration_cache: Option<PhyCalibrationCache>,
}

pub fn enter_esp32s31_wifi_runtime<P>(
    mut mac: Esp32s31WifiMacReady<P>,
) -> Esp32s31WifiRuntimeStart<P> {
    let cold_interrupt_mask = { mac.radio_mut().close_cold_interrupt_phase() };
    let (radio, phy, calibration_cache, start_report) = mac.into_parts();
    let (platform, mut registers, interrupt_setup) = radio.into_running().into_runtime_parts();
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
    let current_channel = start_report.wifi.initial_channel;
    Esp32s31WifiRuntimeStart {
        wifi: Esp32s31WifiStopped {
            platform,
            registers,
            interrupt_setup,
            phy,
            start_report,
            transition_report: Esp32s31WifiRuntimeTransitionReport {
                cold_interrupt_mask,
            },
            current_channel,
        },
        calibration_cache,
    }
}
