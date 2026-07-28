//! Generated-PAC ownership for the finite MAC interrupt transaction.

use super::{device_fence, svd};

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued once by `RadioRegisters::take_mac_interrupt`; construction is
/// crate-private so application code cannot manufacture another ISR owner.
pub struct MacInterruptRegisters {
    peripheral: svd::WifiMacInterrupt,
}

impl MacInterruptRegisters {
    pub(super) unsafe fn steal_from_radio_owner() -> Self {
        Self {
            // SAFETY: `RadioRegisters::take_mac_interrupt` enforces the
            // one-way, single-issue split from the complete radio owner.
            peripheral: unsafe { svd::WifiMacInterrupt::steal() },
        }
    }

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
