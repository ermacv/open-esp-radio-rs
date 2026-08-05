//! Generated-PAC ownership for the finite MAC interrupt transaction.

use super::{
    MacInterruptSnapshot, MacPowerInterruptSnapshot, device_fence,
    svd::{self, interrupt_snapshot},
};

/// Task-side setup token for one MAC interrupt handoff epoch.
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
        // Preserve the HIL-qualified task order: publish the complete MAC
        // mask, explicitly keep the still-unqualified WDEVPWR causes masked,
        // acknowledge every stale event, then order all MMIO writes before
        // the caller exposes either ISR capability.
        unsafe {
            self.peripheral
                .enable()
                .write_with_zero(|w| w.event_mask().bits(event_mask));
            self.power_peripheral
                .enable()
                .write_with_zero(|w| w.event_mask().bits(0));
            self.peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(u32::MAX));
            self.power_peripheral
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
    /// Sample the complete masked WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::
    /// hal_pwr_interrupt_get_event` reads `0x2010_d8bc`.
    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        interrupt_snapshot::sample_mac_power_interrupt(&self.peripheral)
    }

    /// Acknowledge the complete sampled WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::
    /// hal_pwr_interrupt_clr_event` stores its argument to `0x2010_d8c0`.
    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_power_interrupt(&self.peripheral, snapshot);
        device_fence();
    }
}

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued by [`MacInterruptSetup::activate`]; construction is
/// crate-private so application code cannot manufacture another ISR owner or
/// retain task-side interrupt enable/clear access during an active epoch.
pub struct MacInterruptRegisters {
    peripheral: svd::WifiMacInterrupt,
}

impl MacInterruptRegisters {
    /// Sample the complete MAC interrupt status image.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_get_event` proves the
    /// status address and complete `wDev_ProcessFiq` consumes exactly this
    /// status image. The runtime mask is configured before IRQ activation and
    /// is not sampled by the vendor FIQ transaction.
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        interrupt_snapshot::sample_mac_interrupt(&self.peripheral)
    }

    /// Acknowledge the complete sampled event image, then order the ISR edge.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_clr_event` is one
    /// full-width store to the generated write-to-clear register.
    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_interrupt(&self.peripheral, snapshot);
        device_fence();
    }

    /// Mask and acknowledge both interrupt banks, returning task-side setup.
    ///
    /// The caller must first disable both CPU interrupt routes and prove that
    /// neither hard handler retains a reference to `self` or `power`. Owning
    /// both values then closes the finite ISR epoch and makes a later
    /// [`MacInterruptSetup::activate`] transaction possible without stealing
    /// either PAC peripheral a second time.
    pub fn deactivate(self, power: MacPowerInterruptRegisters) -> MacInterruptSetup {
        // SAFETY: ENABLE and CLEAR are complete full-width event bitmaps. The
        // unique values consumed here prove that no safe task/ISR accessor can
        // overlap this transition; platform routing is the caller's separate
        // responsibility as documented above.
        unsafe {
            self.peripheral
                .enable()
                .write_with_zero(|w| w.event_mask().bits(0));
            power
                .peripheral
                .enable()
                .write_with_zero(|w| w.event_mask().bits(0));
            self.peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(u32::MAX));
            power
                .peripheral
                .clear()
                .write_with_zero(|w| w.events().bits(u32::MAX));
        }
        device_fence();
        MacInterruptSetup {
            peripheral: self.peripheral,
            power_peripheral: power.peripheral,
        }
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) unsafe fn steal_for_validation() -> Self {
        Self {
            // SAFETY: this constructor exists only in the isolated validation
            // image, whose exported probe is the sole peripheral owner.
            peripheral: unsafe { svd::WifiMacInterrupt::steal() },
        }
    }
}
