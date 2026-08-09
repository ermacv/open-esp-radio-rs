//! Role-neutral ownership after the one-way cold-MAC/runtime transition.

use open_esp_radio_esp32s31_hal::RadioRegisters;
use open_esp_radio_esp32s31_phy::phy_cold::{PhyCalibrationRecord, PhyColdState};
use open_esp_radio_esp32s31_registers::MacInterruptSetup;
use open_esp_radio_ieee80211::channel::WifiChannel;

use crate::mac_start::{Esp32s31WifiMacReady, Esp32s31WifiMacStartReport};

/// Evidence captured while closing the cold polling interrupt phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31WifiRuntimeTransitionReport {
    /// Mask published by the common MAC initializer before task-side routing.
    pub cold_interrupt_mask: u32,
}

/// Common stopped Wi-Fi owner after cold MAC initialization.
///
/// No Wi-Fi role owns DMA or an installed CPU interrupt route in this state.
/// A station, AP or standalone monitor must consume this value to begin its
/// own finite runtime epoch and return the same ownership frontier only after
/// both DMA and interrupt routing have acknowledged their stopped edges.
pub struct Esp32s31WifiStopped<P> {
    platform: P,
    registers: RadioRegisters,
    interrupt_setup: MacInterruptSetup,
    phy: PhyColdState,
    calibration_record: Option<PhyCalibrationRecord>,
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

    pub const fn calibration_record(&self) -> Option<&PhyCalibrationRecord> {
        self.calibration_record.as_ref()
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    /// Borrow the role-neutral radio state for stopped-only operations.
    pub fn radio_mut(&mut self) -> (&mut RadioRegisters, &mut P) {
        (&mut self.registers, &mut self.platform)
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
                calibration_record: self.calibration_record,
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
    pub registers: RadioRegisters,
    pub interrupt_setup: MacInterruptSetup,
    pub context: Esp32s31WifiRuntimeContext,
}

/// Common Wi-Fi state retained beside one materialized role.
///
/// Register ownership and the interrupt setup token deliberately do not live
/// in this value. A role can reconstruct [`Esp32s31WifiStopped`] only after
/// its DMA/task graph returns the exact [`RadioRegisters`] and its interrupt
/// route returns the exact [`MacInterruptSetup`].
#[doc(hidden)]
pub struct Esp32s31WifiRuntimeContext {
    phy: PhyColdState,
    calibration_record: Option<PhyCalibrationRecord>,
    start_report: Esp32s31WifiMacStartReport,
    transition_report: Esp32s31WifiRuntimeTransitionReport,
    current_channel: WifiChannel,
}

impl Esp32s31WifiRuntimeContext {
    pub fn phy_mut(&mut self) -> &mut PhyColdState {
        &mut self.phy
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.current_channel
    }

    pub const fn calibration_record(&self) -> Option<&PhyCalibrationRecord> {
        self.calibration_record.as_ref()
    }

    pub fn set_current_channel(&mut self, channel: WifiChannel) {
        self.current_channel = channel;
    }

    /// Reconstruct the role-neutral owner from independently proven DMA/task
    /// and interrupt-route return edges.
    pub fn into_stopped<P>(
        self,
        platform: P,
        registers: RadioRegisters,
        interrupt_setup: MacInterruptSetup,
    ) -> Esp32s31WifiStopped<P> {
        Esp32s31WifiStopped {
            platform,
            registers,
            interrupt_setup,
            phy: self.phy,
            calibration_record: self.calibration_record,
            start_report: self.start_report,
            transition_report: self.transition_report,
            current_channel: self.current_channel,
        }
    }
}

/// Close the cold polling phase and enter the reusable stopped-runtime state.
///
/// This is the only normal conversion from [`Esp32s31WifiMacReady`]. It masks
/// and acknowledges cold interrupt state before exposing the setup token used
/// by a finite task-owned interrupt epoch.
pub fn enter_esp32s31_wifi_runtime<P>(mut mac: Esp32s31WifiMacReady<P>) -> Esp32s31WifiStopped<P> {
    let cold_interrupt_mask = {
        let (_, registers) = mac.radio_mut().cold_parts_mut();
        let mask = registers.mac_interrupt_enable();
        registers.mask_and_clear_mac_interrupts(u32::MAX);
        mask
    };
    let (radio, phy, calibration_record, start_report) = mac.into_parts();
    let (platform, registers) = radio.into_parts();
    let (registers, interrupt_setup) = registers.into_running();
    let current_channel = start_report.wifi.initial_channel;
    Esp32s31WifiStopped {
        platform,
        registers,
        interrupt_setup,
        phy,
        calibration_record,
        start_report,
        transition_report: Esp32s31WifiRuntimeTransitionReport {
            cold_interrupt_mask,
        },
        current_channel,
    }
}
