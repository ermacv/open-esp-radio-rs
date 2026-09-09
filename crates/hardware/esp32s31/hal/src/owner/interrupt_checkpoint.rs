//! Retained interrupt authority across a same-epoch physical pause.

use super::{MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters};

/// Both disjoint ISR capabilities, with peripheral masks and pending events
/// unchanged. Obtaining these owners requires withdrawing them from ISR storage;
/// the platform adapter still owns CPU routing and binding-core checks.
///
/// This value alone proves neither MAC/DMA quiescence nor a shared RF grant.
/// PHY access also consumes the exclusive Wi-Fi register owner and checks MAC
/// and RX state. The caller retains paused descriptor and protocol owners.
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::MacInterruptCheckpoint;
/// fn duplicate(value: MacInterruptCheckpoint) -> (MacInterruptCheckpoint, MacInterruptCheckpoint) {
///     (value, value)
/// }
/// ```
/// The platform cannot transfer this checkpoint to a different executor core:
///
/// ```compile_fail
/// use oer_esp32s31_hal::owner::MacInterruptCheckpoint;
/// fn needs_send<T: Send>() {}
/// needs_send::<MacInterruptCheckpoint>();
/// ```
#[must_use = "resume the original IRQ epoch or explicitly terminate it"]
pub struct MacInterruptCheckpoint {
    mac: MacInterruptRegisters,
    power: MacPowerInterruptRegisters,
    _same_core: core::marker::PhantomData<*mut ()>,
}

impl MacInterruptRegisters {
    /// Retain the two returned ISR owners without masking or acknowledging
    /// any peripheral event. CPU route detachment belongs to the caller.
    pub fn checkpoint(self, power: MacPowerInterruptRegisters) -> MacInterruptCheckpoint {
        MacInterruptCheckpoint {
            mac: self,
            power,
            _same_core: core::marker::PhantomData,
        }
    }
}

impl MacInterruptCheckpoint {
    /// Return the exact owners for installation before re-enabling CPU routes.
    /// No peripheral write occurs at this ownership transition.
    pub fn into_registers(self) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        (self.mac, self.power)
    }

    /// Explicit terminal teardown, unlike a same-epoch resume. This masks and
    /// acknowledges peripheral state through the existing HAL transaction.
    pub fn deactivate(self) -> MacInterruptSetup {
        self.mac.deactivate(self.power)
    }
}
