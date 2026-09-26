//! Closed HAL capability for the radio-owned coexistence timer bank.
//!
//! [`CoexTimerBank`] borrows the shared radio owner and exposes
//! only named timer transactions and the shared low-power clock sample.
//! Coexistence policy, tick conversion and cleanup bookkeeping belong to the
//! coexistence driver, which consumes this capability through its own ports.
//! The bank is reachable only from validation images; no live protocol
//! runtime composes an operational coexistence service.

#![cfg(feature = "validation-probes")]

use oer_esp32s31_pac::{
    CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister,
    CoexistenceLowPowerClockObservation, SharedRadioRegisters,
};

/// Borrowed authority over the five coexistence hardware timers.
///
/// Every method is one finite register transaction. A write does not report
/// RF admission or grant revocation.
pub struct CoexTimerBank<'registers> {
    registers: &'registers mut SharedRadioRegisters,
}

impl<'registers> CoexTimerBank<'registers> {
    pub(crate) fn from_owned(registers: &'registers mut SharedRadioRegisters) -> Self {
        Self { registers }
    }

    /// Publish the client and PTI request fields of one timer.
    pub fn configure(
        &mut self,
        timer: CoexTimerRegister,
        client: CoexTimerClientValue,
        pti: CoexTimerPtiValue,
    ) {
        self.registers.configure_coex_timer(timer, client, pti);
    }

    /// Publish one already converted primary target tick image.
    pub fn set_primary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.registers
            .set_coex_timer_primary_target(timer, tick_image);
    }

    /// Publish one already converted secondary target tick image.
    pub fn set_secondary_target(&mut self, timer: CoexTimerRegister, tick_image: u32) {
        self.registers
            .set_coex_timer_secondary_target(timer, tick_image);
    }

    pub fn enable(&mut self, timer: CoexTimerRegister) {
        self.registers.enable_coex_timer(timer);
    }

    pub fn disable(&mut self, timer: CoexTimerRegister) {
        self.registers.disable_coex_timer(timer);
    }

    pub fn force(&mut self, timer: CoexTimerRegister) {
        self.registers.force_coex_timer(timer);
    }

    pub fn unforce(&mut self, timer: CoexTimerRegister) {
        self.registers.unforce_coex_timer(timer);
    }

    /// Sample the shared coexistence low-power clock selection once.
    ///
    /// Each call performs fresh reads; `None` reports an unreviewed encoding.
    pub fn sample_low_power_clock(&mut self) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers.sample_coexistence_low_power_clock()
    }
}
