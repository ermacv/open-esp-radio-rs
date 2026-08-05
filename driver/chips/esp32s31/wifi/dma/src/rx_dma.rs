//! Semantic MMIO contract for the ESP32-S31 RX descriptor walker.
//!
//! The contract exposes finite RX-DMA operations instead of register
//! identities. Descriptor-ring ownership is modeled separately, so production
//! and host models share the same state machine without exposing the generated
//! PAC above the register leaf.

use open_esp_radio_dma::StableDmaRange;
use open_esp_radio_esp32s31_registers::{ColdRadioRegisters, RadioRegisters};

use crate::descriptor::{DESCRIPTOR_BYTES, Descriptor};

/// Unforgeable authority for mutating one validated RX descriptor walker.
///
/// Public trait methods accept this type so owning `RadioRegisters` alone is
/// insufficient to publish an arbitrary DMA address. Constructors remain
/// inside the chip DMA leaf and ring typestates keep the value private.
pub struct RxDmaBinding<'storage> {
    descriptor_base: u32,
    descriptor_count: u8,
    range: StableDmaRange<'storage>,
}

impl<'storage> RxDmaBinding<'storage> {
    #[allow(
        unsafe_code,
        reason = "ring construction retains the descriptor owner for the DMA epoch"
    )]
    pub(crate) fn new<const COUNT: usize>(
        descriptors: &'storage [Descriptor; COUNT],
        descriptor_base: u32,
    ) -> Option<Self> {
        let descriptor_count = u8::try_from(COUNT).ok()?;
        let range_len = u32::try_from(core::mem::size_of_val(descriptors)).ok()?;
        // SAFETY: the binding retains the descriptor borrow. The safe target
        // entry requires static `RxDmaStorage`; the raw target entry states
        // the equivalent allocation/lifetime proof in its unsafe contract.
        // Native entry points have no asynchronous DMA actor.
        let range = unsafe { StableDmaRange::from_owner(descriptors, descriptor_base, range_len) }?;
        (descriptor_count != 0).then_some(Self {
            descriptor_base,
            descriptor_count,
            range,
        })
    }

    #[allow(
        unsafe_code,
        reason = "raw target publication is already an unsafe validation boundary"
    )]
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub(crate) fn raw_validation(descriptor_base: u32) -> RxDmaBinding<'static> {
        // SAFETY: target callers cross an unsafe public API and retain the
        // addressed ring; native callers use a model with no DMA actor.
        let range = unsafe {
            StableDmaRange::from_raw_parts(descriptor_base, DESCRIPTOR_BYTES)
                .expect("one descriptor always forms a non-empty DMA range")
        };
        RxDmaBinding {
            descriptor_base,
            descriptor_count: 1,
            range,
        }
    }

    fn admits(&self, address: u32) -> bool {
        let Some(offset) = address.checked_sub(self.descriptor_base) else {
            return false;
        };
        offset % DESCRIPTOR_BYTES == 0
            && offset / DESCRIPTOR_BYTES < u32::from(self.descriptor_count)
    }

    fn range(&self) -> &StableDmaRange<'storage> {
        &self.range
    }
}

/// Semantic ownership boundary for the S31 RX descriptor walker.
///
/// Production uses the generated PAC implementation below. Host tests model
/// these finite operations without receiving arbitrary register identities.
pub trait RxDma {
    /// Optional monotonic hardware starvation counter for boundary telemetry.
    ///
    /// This observation never participates in descriptor ownership. Host
    /// models and platforms without such a counter keep the default `None`.
    fn buffer_full_count(&mut self) -> Option<u16> {
        None
    }

    fn last_descriptor_low(&mut self) -> u32;
    fn next_descriptor_low(&mut self) -> u32;
    fn walker_enabled(&mut self) -> bool;
    fn reload_pending(&mut self) -> bool;
    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding<'_>, address_high: u16);
    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32);
    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>);
    fn request_reload(&mut self, binding: &RxDmaBinding<'_>);
    fn try_enable_walker(&mut self, binding: &RxDmaBinding<'_>) -> bool;
    fn try_disable_walker(&mut self) -> bool;
    fn fence(&mut self);
}

impl RxDma for RadioRegisters {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.mac_rx_buffer_full_count())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.mac_rx_last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.mac_rx_next_descriptor_low()
    }

    fn walker_enabled(&mut self) -> bool {
        self.mac_rx_walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.mac_rx_reload_pending()
    }

    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding<'_>, address_high: u16) {
        self.set_mac_rx_descriptor_high_window(binding.range(), address_high);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        assert!(
            binding.admits(address),
            "RX descriptor base must belong to the bound static ring"
        );
        self.write_mac_rx_descriptor_base(binding.range(), address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.publish_mac_rx_walker_enable(binding.range());
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.request_mac_rx_descriptor_reload(binding.range());
    }

    fn try_enable_walker(&mut self, binding: &RxDmaBinding<'_>) -> bool {
        self.try_enable_mac_rx_walker(binding.range())
    }

    fn try_disable_walker(&mut self) -> bool {
        self.try_disable_mac_rx_walker()
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

impl RxDma for ColdRadioRegisters {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut **self)
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut **self)
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut **self)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut **self)
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut **self)
    }

    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding, address_high: u16) {
        RxDma::set_descriptor_high_window(&mut **self, binding, address_high);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding, address: u32) {
        RxDma::write_descriptor_base(&mut **self, binding, address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding) {
        RxDma::publish_walker_enable(&mut **self, binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding) {
        RxDma::request_reload(&mut **self, binding);
    }

    fn try_enable_walker(&mut self, binding: &RxDmaBinding) -> bool {
        RxDma::try_enable_walker(&mut **self, binding)
    }

    fn try_disable_walker(&mut self) -> bool {
        RxDma::try_disable_walker(&mut **self)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut **self);
    }
}
