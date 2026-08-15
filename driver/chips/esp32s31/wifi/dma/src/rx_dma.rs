//! Semantic MMIO contract for the ESP32-S31 RX descriptor walker.
//!
//! The contract exposes finite RX-DMA operations instead of register
//! identities. Descriptor-ring ownership is modeled separately, so production
//! and host models share the same state machine without exposing the generated
//! PAC above the register leaf.

use core::marker::PhantomData;

use open_esp_radio_dma::StableDmaRange;
use open_esp_radio_esp32s31_hal::RadioRuntimeOwner;
use open_esp_radio_esp32s31_hal::wifi_mac::WifiMacHal;

use crate::descriptor::{DESCRIPTOR_BYTES, Descriptor};

/// Unforgeable authority for mutating one validated RX descriptor walker.
///
/// Public trait methods accept this type so owning a runtime radio capability
/// alone is insufficient to publish an arbitrary DMA address. Constructors remain
/// inside the chip DMA leaf and ring typestates keep the value private.
pub struct RxDmaBinding<'storage> {
    descriptor_base: u32,
    descriptor_count: u8,
    range: StableDmaRange<'storage>,
}

/// Opaque proof that the RX walker disable request completed and read back
/// as stopped.
///
/// Production code can obtain this value only from the PAC-backed
/// [`RxDma`] implementation. Native models and explicit raw-DMA validation
/// may construct a model proof for deterministic state-machine tests.
pub struct RxDmaWalkerStopped<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
}

/// Opaque current proof that the RX walker enable edge was accepted.
pub struct RxDmaWalkerEnabled<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaWalkerEnabled<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
        Self::confirmed()
    }
}

/// Opaque current proof that the append/reload doorbell has self-cleared.
pub struct RxDmaReloadSettled<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaReloadSettled<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
        Self::confirmed()
    }
}

impl RxDmaWalkerStopped<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
        Self::confirmed()
    }
}

/// One ordered RX walker cursor observation.
///
/// The PAC implementation samples `LAST`, executes a device fence, samples
/// `NEXT`, then executes the second fence before invoking the callback. The
/// confirmation lifetime prevents safe forwarding layers from retaining and
/// replaying a stale observation as current ownership evidence.
pub struct RxDmaCursorObservation<'confirmation> {
    last_descriptor_low: u32,
    next_descriptor_low: u32,
    _confirmation: PhantomData<&'confirmation mut ()>,
}

/// Ordered observation used only by the vendor reload-repair suffix.
///
/// `wDev_AppendRxBlocks` samples `NEXT` first and reads `LAST` only when the
/// walker reports a zero successor. This is intentionally a different
/// contract from [`RxDmaCursorObservation`], whose `LAST -> NEXT` order proves
/// release of a completed descriptor link. Combining the two observations
/// can pair values from different hardware epochs and authorize a stale BASE
/// repair.
pub struct RxDmaReloadRepairObservation<'confirmation> {
    next_descriptor_word: u32,
    last_descriptor_low: Option<u32>,
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaReloadRepairObservation<'_> {
    const fn confirmed(next_descriptor_word: u32, last_descriptor_low: Option<u32>) -> Self {
        Self {
            next_descriptor_word,
            last_descriptor_low,
            _confirmation: PhantomData,
        }
    }

    /// Return the complete register word used by the vendor zero predicate.
    pub const fn next_descriptor_word(&self) -> u32 {
        self.next_descriptor_word
    }

    pub const fn next_descriptor_low(&self) -> u32 {
        self.next_descriptor_word & 0x000f_ffff
    }

    /// Return `LAST` only for the vendor branch where the preceding `NEXT`
    /// observation was zero.
    pub const fn exhausted_last_descriptor_low(&self) -> Option<u32> {
        self.last_descriptor_low
    }
}

impl RxDmaCursorObservation<'_> {
    const fn confirmed(last_descriptor_low: u32, next_descriptor_low: u32) -> Self {
        Self {
            last_descriptor_low,
            next_descriptor_low,
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub const fn validation(last_descriptor_low: u32, next_descriptor_low: u32) -> Self {
        Self::confirmed(last_descriptor_low, next_descriptor_low)
    }

    pub const fn last_descriptor_low(&self) -> u32 {
        self.last_descriptor_low
    }

    pub const fn next_descriptor_low(&self) -> u32 {
        self.next_descriptor_low
    }
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

    /// Read the complete `RX_NEXT_DESCRIPTOR` word.
    ///
    /// Address consumers must use [`Self::next_descriptor_low`]. The vendor
    /// reload-repair branch instead compares the complete word with zero;
    /// projecting it to the low address field changes that branch predicate.
    fn next_descriptor_word(&mut self) -> u32;
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R;

    /// Preserve the exact reload-repair observation order recovered from
    /// `wDev_AppendRxBlocks`: `NEXT`, a device fence, and conditional `LAST`.
    fn with_reload_repair_observation<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaReloadRepairObservation<'confirmation>) -> R,
    ) -> R {
        let next_descriptor_word = self.next_descriptor_word();
        self.fence();
        let last_descriptor_low = if next_descriptor_word == 0 {
            let last_descriptor_low = self.last_descriptor_low();
            self.fence();
            Some(last_descriptor_low)
        } else {
            None
        };
        observed(RxDmaReloadRepairObservation::confirmed(
            next_descriptor_word,
            last_descriptor_low,
        ))
    }
    fn walker_enabled(&mut self) -> bool;
    fn reload_pending(&mut self) -> bool;
    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R>;
    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding<'_>, address_high: u16);
    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32);
    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>);
    fn request_reload(&mut self, binding: &RxDmaBinding<'_>);
    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R>;
    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R>;
    fn fence(&mut self);
}

impl RxDma for WifiMacHal<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.rx_buffer_full_count())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.rx_last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.rx_next_descriptor_low()
    }

    fn next_descriptor_word(&mut self) -> u32 {
        self.rx_next_descriptor_word()
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last_descriptor_low = self.rx_last_descriptor_low();
        self.order_device_accesses();
        let next_descriptor_low = self.rx_next_descriptor_low();
        self.order_device_accesses();
        observed(RxDmaCursorObservation::confirmed(
            last_descriptor_low,
            next_descriptor_low,
        ))
    }

    fn walker_enabled(&mut self) -> bool {
        self.rx_walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.rx_reload_pending()
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        (!self.rx_reload_pending()).then(|| settled(RxDmaReloadSettled::confirmed()))
    }

    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding<'_>, address_high: u16) {
        self.set_rx_descriptor_high_window(binding.range(), address_high);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        assert!(
            binding.admits(address),
            "RX descriptor base must belong to the bound static ring"
        );
        self.write_rx_descriptor_base(binding.range(), address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.publish_rx_walker_enable(binding.range());
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.request_rx_descriptor_reload(binding.range());
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.try_enable_rx_walker(binding.range())
            .then(|| enabled(RxDmaWalkerEnabled::confirmed()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.try_disable_rx_walker()
            .then(|| stopped(RxDmaWalkerStopped::confirmed()))
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

impl RxDma for RadioRuntimeOwner {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut self.wifi_mac_hal())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor_word(&mut self) -> u32 {
        RxDma::next_descriptor_word(&mut self.wifi_mac_hal())
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        RxDma::with_ordered_cursor(&mut self.wifi_mac_hal(), observed)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut self.wifi_mac_hal())
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut self.wifi_mac_hal())
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_reload_settled(&mut self.wifi_mac_hal(), settled)
    }

    fn set_descriptor_high_window(&mut self, binding: &RxDmaBinding<'_>, address_high: u16) {
        RxDma::set_descriptor_high_window(&mut self.wifi_mac_hal(), binding, address_high);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        RxDma::write_descriptor_base(&mut self.wifi_mac_hal(), binding, address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::publish_walker_enable(&mut self.wifi_mac_hal(), binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::request_reload(&mut self.wifi_mac_hal(), binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_enabled(&mut self.wifi_mac_hal(), binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_stopped(&mut self.wifi_mac_hal(), stopped)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut self.wifi_mac_hal());
    }
}
