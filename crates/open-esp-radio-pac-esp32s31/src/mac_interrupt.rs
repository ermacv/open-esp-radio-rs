//! Generated-PAC ownership for the finite MAC interrupt transaction.

use super::{device_fence, svd};

/// One-shot task-side setup for the MAC interrupt handoff.
///
/// This token exists after the cold owner has been consumed but before the
/// interrupt is routed to a CPU. Activating it publishes the final mask,
/// clears stale events and consumes all task-side enable/clear access.
pub struct MacInterruptSetup {
    peripheral: svd::WifiMacInterrupt,
    power_peripheral: svd::WifiMacPowerInterrupt,
}

impl MacInterruptSetup {
    pub(super) unsafe fn steal_from_cold_radio_owner() -> Self {
        Self {
            // SAFETY: `ColdRadioRegisters::into_running` consumes the only
            // safe owner that can access interrupt enable/clear registers.
            peripheral: unsafe { svd::WifiMacInterrupt::steal() },
            // SAFETY: the same consumed cold owner uniquely owns the disjoint
            // WDEVPWR bank. It is transferred with the MAC bank so both ISR
            // capabilities exist before either CPU route is exposed.
            power_peripheral: unsafe { svd::WifiMacPowerInterrupt::steal() },
        }
    }

    /// Publish the runtime event mask and create the finite ISR capability.
    ///
    /// The CPU interrupt route must still be unbound while this transaction
    /// executes. The returned value should be installed in its final static
    /// storage before the platform route is enabled.
    pub fn activate(self, event_mask: u32) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        // Preserve the HIL-qualified task order: publish the complete mask,
        // acknowledge every stale event, then order both MMIO writes before
        // the caller exposes the ISR capability.
        unsafe {
            self.peripheral
                .enable()
                .write_with_zero(|w| w.event_mask().bits(event_mask));
            self.peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(u32::MAX));
        }
        device_fence();
        (
            MacInterruptRegisters {
                peripheral: self.peripheral,
            },
            MacPowerInterruptRegisters {
                peripheral: self.power_peripheral,
            },
        )
    }
}

/// Disjoint generated register capability intended for the hard power ISR.
///
/// This bank is split from the cold owner together with
/// [`MacInterruptRegisters`]. Ordinary [`super::RadioRegisters`] therefore
/// cannot race its STATUS/CLEAR transaction from task context.
pub struct MacPowerInterruptRegisters {
    peripheral: svd::WifiMacPowerInterrupt,
}

impl MacPowerInterruptRegisters {
    /// Sample the masked WDEVPWR event image and acknowledge that exact image.
    ///
    /// SOURCE: complete `_oracles/libpp.a[hal_pwr.o]::
    /// hal_pwr_interrupt_get_event` reads `0x2010_d8bc`; complete
    /// `hal_pwr_interrupt_clr_event` stores its argument to `0x2010_d8c0`.
    pub fn acknowledge_pending_power_interrupts(&mut self) -> u32 {
        let events = self.peripheral.status().read().events().bits();
        // SAFETY: CLEAR is the instruction-proven full-width W1C event image;
        // writing the status snapshot is the complete recovered transaction.
        unsafe {
            self.peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(events))
        };
        device_fence();
        events
    }
}

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued once by [`MacInterruptSetup::activate`]; construction is
/// crate-private so application code cannot manufacture another ISR owner or
/// retain task-side interrupt enable/clear access after activation.
pub struct MacInterruptRegisters {
    peripheral: svd::WifiMacInterrupt,
}

impl MacInterruptRegisters {
    /// Sample status and enable in the recovered common-ISR order.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_get_event` proves the
    /// status address; the recovered `wDev_ProcessFiq` transaction and cold
    /// initializer prove the paired enable snapshot.
    pub fn mac_interrupt_snapshot(&self) -> (u32, u32) {
        let block = &self.peripheral;
        let status = block.status().read().bits();
        let enabled = block.enable().read().event_mask().bits();
        (status, enabled)
    }

    /// Acknowledge the complete sampled event image, then order the ISR edge.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_clr_event` is one
    /// full-width store to the generated write-to-clear register.
    pub fn acknowledge_mac_interrupts(&mut self, events: u32) {
        // SAFETY: all 32 bits are the evidenced write-to-clear event bitmap;
        // writing back the sampled image is the complete recovered leaf.
        unsafe {
            self.peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(events))
        };
        device_fence();
    }
}
