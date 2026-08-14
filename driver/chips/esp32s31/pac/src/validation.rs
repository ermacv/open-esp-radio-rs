//! Capability acquisition for isolated compiled probe images.
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only constructs the same finite PAC owners used by production
//! code; it does not implement semantic operations or comparison verdicts.

#![forbid(unsafe_code)]

use crate::{MacInterruptRegisters, MacPowerInterruptRegisters, svd};

#[inline(always)]
fn partitions() -> (
    svd::peripheral_ownership::RadioPeripherals,
    svd::peripheral_ownership::InterruptPeripherals,
) {
    svd::peripheral_ownership::split(svd::peripheral_ownership::peripherals_for_validation())
}

/// Construct the ordinary task-owned register partition for one probe image.
#[inline(always)]
pub fn radio_registers() -> crate::RadioRegisters {
    let (radio, _) = partitions();
    crate::RadioRegisters::from_peripherals(radio)
}

/// Construct the disjoint hard-MAC interrupt capability for one probe image.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    let (_, interrupts) = partitions();
    MacInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_interrupt)
}

/// Construct the disjoint power-interrupt capability for one probe image.
#[inline(always)]
pub fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    let (_, interrupts) = partitions();
    MacPowerInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_power_interrupt)
}
