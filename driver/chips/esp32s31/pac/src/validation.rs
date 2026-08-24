//! Capability acquisition for isolated compiled probe images.
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only constructs the same finite PAC owners used by production
//! code; it does not implement semantic operations or comparison verdicts.

#![forbid(unsafe_code)]

use crate::{
    BluetoothInterruptRegisters, MacInterruptRegisters, MacInterruptSetup,
    MacPowerInterruptRegisters, RadioHardware, svd,
};

#[inline(always)]
fn wifi_interrupts() -> svd::peripheral_ownership::WifiInterruptPeripherals {
    let RadioHardware {
        wifi_interrupts, ..
    } = RadioHardware::for_validation();
    wifi_interrupts
}

/// Construct the ordinary task-owned register partition for one probe image.
#[inline(always)]
pub fn wifi_radio_registers() -> crate::WifiRadioRegisters {
    RadioHardware::for_validation().into_wifi().into_running().0
}

/// Construct the task-side interrupt setup owner used by the production
/// connected-STA preparation transaction.
#[inline(always)]
pub fn mac_interrupt_setup() -> MacInterruptSetup {
    RadioHardware::for_validation().into_wifi().into_running().1
}

/// Construct the disjoint hard-MAC interrupt capability for one probe image.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    let interrupts = wifi_interrupts();
    MacInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_interrupt)
}

/// Construct the disjoint power-interrupt capability for one probe image.
#[inline(always)]
pub fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    let interrupts = wifi_interrupts();
    MacPowerInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_power_interrupt)
}

/// Construct the active Bluetooth interrupt-bank capability for one isolated
/// compiled probe image.
///
/// Ordinary production code cannot call this constructor: the validation
/// module is absent unless `validation-probes` is enabled. The production
/// cold owner therefore still has no transition to active Bluetooth MMIO.
#[inline(always)]
pub fn bluetooth_interrupt_registers() -> BluetoothInterruptRegisters {
    let RadioHardware {
        bluetooth_interrupts,
        ..
    } = RadioHardware::for_validation();
    BluetoothInterruptRegisters {
        peripherals: bluetooth_interrupts,
    }
}

/// Execute the exact bounded MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` inside one isolated comparison image.
///
/// This is validation-only because production Bluetooth must first establish
/// the still-pending post-common-PHY typestate. It calls the crate-private PAC
/// transaction directly, so the comparison exercises the implementation
/// intended for that future production edge rather than a shadow model.
#[inline(always)]
pub fn initialize_bluetooth_baseband_v2(gain_parameter: u8) {
    let cold = RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    task.initialize_baseband_v2_printing(gain_parameter);
    let _cold = task.into_cold(interrupts);
}
