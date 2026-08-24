//! Generated-PAC ownership for the finite MAC interrupt transaction.

#![forbid(unsafe_code)]

use super::{
    MacInterruptMask, MacInterruptSnapshot, MacPowerInterruptSnapshot, WifiRadioRegisters,
    device_fence,
    svd::{self, interrupt_snapshot},
};

const STA_BEACON_FILTER_INTERRUPT: u32 = 1 << 15;
const RX_DELIVERY_INTERRUPTS: u32 = (1 << 5) | (1 << 14) | (1 << 24);

/// One of the four guarded generic TSF timers recovered from `hal_tsf.o`.
///
/// This type deliberately does not identify any timer as the station timer:
/// the reviewed low three control bits still have no semantic decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacTsfTimerIndex {
    Timer0,
    Timer1,
    Timer2,
    Timer3,
}

impl MacTsfTimerIndex {
    pub const fn get(self) -> u8 {
        match self {
            Self::Timer0 => 0,
            Self::Timer1 => 1,
            Self::Timer2 => 2,
            Self::Timer3 => 3,
        }
    }
}

/// WDEVPWR causes whose bit identity is proven by complete TSF timer leaves.
///
/// Beacon-miss, modem-limit and RF causes remain intentionally absent: the
/// reviewed bank exposes their opaque status bits but not their identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacPowerWakeCause {
    TsfTimer(MacTsfTimerIndex),
}

impl MacPowerWakeCause {
    pub const REVIEWED_MASK: u32 = 0x0000_00f0;

    /// `hal_enable_tsf_timer(index)` uses WDEVPWR bit `7 - index` for guarded
    /// indices zero through three.
    pub const fn event_mask(self) -> u32 {
        match self {
            Self::TsfTimer(timer) => 1 << (7 - timer.get()),
        }
    }
}

/// Proof that the connected-STA hardware policy was applied before IRQ activation.
///
/// Construction is private to the exact two-register transaction below. The
/// connected runtime consumes this value when it activates its interrupt
/// epoch, so removing the transaction from that lifecycle becomes a compile
/// error rather than an intermittent runtime regression.
#[must_use = "connected STA interrupt activation requires this preparation proof"]
pub struct ConnectedStaWithoutPowerSavePrepared {
    _private: (),
}

#[inline(always)]
fn disable_sta_beacon_filter(
    control: &svd::WifiMacStaBeaconFilter,
    interrupt: &svd::WifiMacInterrupt,
) {
    // Complete libpp.a[hal_mac.o]::hal_disable_sta_beacon_filter. Preserve
    // the two independent fresh-read RMW edges and their order: hardware
    // filtering is disabled before its matching interrupt source is masked.
    control
        .control()
        .modify(|_, writer| writer.enables_unknown().set(0));
    interrupt.enable().modify(|reader, writer| {
        writer
            .event_mask()
            .set(reader.event_mask().bits() & !STA_BEACON_FILTER_INTERRUPT)
    });
}

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
    pub(super) fn from_peripherals(
        peripherals: svd::peripheral_ownership::WifiInterruptPeripherals,
    ) -> Self {
        Self {
            peripheral: peripherals.wifi_mac_interrupt,
            power_peripheral: peripherals.wifi_mac_power_interrupt,
        }
    }

    /// Reassemble the generated Wi-Fi interrupt partition after a finite
    /// inactive epoch. This is ownership-only and performs no MMIO.
    pub(super) fn into_peripherals(self) -> svd::peripheral_ownership::WifiInterruptPeripherals {
        svd::peripheral_ownership::WifiInterruptPeripherals {
            wifi_mac_interrupt: self.peripheral,
            wifi_mac_power_interrupt: self.power_peripheral,
        }
    }

    /// Disable vendor hardware beacon filtering before a connected STA epoch.
    ///
    /// The operation requires both still-disjoint task owners and therefore
    /// cannot race an active ISR. It is the complete vendor disable leaf:
    /// CONTROL bits 2:0 are cleared before interrupt-enable bit 15.
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        registers: &mut WifiRadioRegisters,
    ) -> ConnectedStaWithoutPowerSavePrepared {
        disable_sta_beacon_filter(
            &registers.peripherals.wifi_mac.wifi_mac_sta_beacon_filter,
            &self.peripheral,
        );
        ConnectedStaWithoutPowerSavePrepared { _private: () }
    }

    /// Publish the runtime event mask and create the finite ISR capability.
    ///
    /// The CPU interrupt route must still be unbound while this transaction
    /// executes. The returned value should be installed in its final static
    /// storage before the platform route is enabled.
    pub fn activate(
        self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        // Preserve the HIL-qualified task order: publish the complete MAC
        // mask, explicitly keep the still-unqualified WDEVPWR causes masked,
        // acknowledge every stale event, then order all MMIO writes before
        // the caller exposes either ISR capability.
        super::generated::mac_interrupt_enable(&self.peripheral, event_mask);
        svd::fixed_register_write::mask_mac_power_interrupts(&self.power_peripheral);
        super::generated::mac_interrupt_clear(
            &self.peripheral,
            super::generated::MacInterruptClearImage::new(u32::MAX),
        );
        super::generated::mac_power_interrupt_clear(
            &self.power_peripheral,
            super::generated::MacPowerInterruptClearImage::new(u32::MAX),
        );
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
/// [`MacInterruptRegisters`]. Ordinary [`super::WifiRadioRegisters`] therefore
/// cannot race its STATUS/CLEAR transaction from task context.
pub struct MacPowerInterruptRegisters {
    peripheral: svd::WifiMacPowerInterrupt,
}

impl MacPowerInterruptRegisters {
    /// Mask the complete WDEVPWR bank and acknowledge one reviewed cause.
    ///
    /// The current reviewed SVD permits the complete MASKED image and exact
    /// CLEAR writes, but deliberately has no safe writer for a partial ENABLE
    /// image. Consequently this is a rollback/handoff operation, not a claim
    /// that the selected cause can yet wake production firmware.
    pub fn mask_and_acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        svd::fixed_register_write::mask_mac_power_interrupts(&self.peripheral);
        super::generated::mac_power_interrupt_clear(
            &self.peripheral,
            super::generated::MacPowerInterruptClearImage::new(cause.event_mask()),
        );
        device_fence();
    }

    /// Acknowledge one reviewed cause without changing the active mask.
    pub fn acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        super::generated::mac_power_interrupt_clear(
            &self.peripheral,
            super::generated::MacPowerInterruptClearImage::new(cause.event_mask()),
        );
        device_fence();
    }

    /// Sample the complete masked WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::
    /// hal_pwr_interrupt_get_event` reads `0x2010_d8bc`.
    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        MacPowerInterruptSnapshot(interrupt_snapshot::sample_mac_power_interrupt(
            &self.peripheral,
        ))
    }

    /// Acknowledge the complete sampled WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp.a[hal_tsf.o]::
    /// hal_pwr_interrupt_clr_event` stores its argument to `0x2010_d8c0`.
    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_power_interrupt(&self.peripheral, snapshot.0);
        device_fence();
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn from_peripheral_for_validation(peripheral: svd::WifiMacPowerInterrupt) -> Self {
        Self { peripheral }
    }
}

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued by [`MacInterruptSetup::activate`]; construction is
/// crate-private so application code cannot manufacture another ISR owner or
/// retain task-side interrupt enable/clear access during an active epoch.
pub struct MacInterruptRegisters {
    pub(crate) peripheral: svd::WifiMacInterrupt,
}

impl MacInterruptRegisters {
    /// Sample the complete MAC interrupt status image.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_get_event` proves the
    /// status address and complete `wDev_ProcessFiq` consumes exactly this
    /// status image. The runtime mask is configured before IRQ activation and
    /// is not sampled by the vendor FIQ transaction.
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        MacInterruptSnapshot(interrupt_snapshot::sample_mac_interrupt(&self.peripheral))
    }

    /// Acknowledge the complete sampled event image, then order the ISR edge.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_clr_event` is one
    /// full-width store to the generated write-to-clear register.
    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_interrupt(&self.peripheral, snapshot.0);
        device_fence();
    }

    /// Temporarily suppress the observed RX delivery group without changing
    /// any other member of the reviewed runtime mask.
    ///
    /// This is a source-moderation operation, not an acknowledgement. The
    /// hard ISR uses it before acknowledging the RX image. The group contains
    /// RX_SUCCESS and the two auxiliary sources observed on every saturated
    /// RX edge; TX and unrelated sources remain independently live.
    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.peripheral.enable().modify(|reader, writer| {
            writer
                .event_mask()
                .set(reader.event_mask().bits() & !RX_DELIVERY_INTERRUPTS)
        });
        device_fence();
    }

    /// Restore the RX delivery group while preserving the current mask.
    ///
    /// A latched RX status which arrived while masked becomes visible through
    /// the level CPU route after this ordered write, so the task does not need
    /// to fabricate a hardware completion edge.
    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.peripheral.enable().modify(|reader, writer| {
            writer
                .event_mask()
                .set(reader.event_mask().bits() | RX_DELIVERY_INTERRUPTS)
        });
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
        super::generated::mac_interrupt_enable(&self.peripheral, MacInterruptMask::NONE);
        svd::fixed_register_write::mask_mac_power_interrupts(&power.peripheral);
        super::generated::mac_interrupt_clear(
            &self.peripheral,
            super::generated::MacInterruptClearImage::new(u32::MAX),
        );
        super::generated::mac_power_interrupt_clear(
            &power.peripheral,
            super::generated::MacPowerInterruptClearImage::new(u32::MAX),
        );
        device_fence();
        MacInterruptSetup {
            peripheral: self.peripheral,
            power_peripheral: power.peripheral,
        }
    }

    #[cfg(feature = "validation-probes")]
    pub(crate) fn from_peripheral_for_validation(peripheral: svd::WifiMacInterrupt) -> Self {
        Self { peripheral }
    }
}
