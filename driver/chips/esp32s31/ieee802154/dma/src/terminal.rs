//! Affine proof of one accepted terminal hardware batch.

/// Non-replayable authority to reclaim the DMA resources retained by one MAC
/// operation.
///
/// Safe code cannot construct this value. The sealed ESP32-S31 runtime mints
/// it only after an ISR-sampled, acknowledged event value has been decoded and
/// accepted as terminal for the exact active operation. It remains private
/// inside that runtime completion until a type-specific reclaim consumes the
/// completion.
///
/// The evidence cannot be constructed or replayed by safe downstream code:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::DmaTerminalEvidence;
///
/// let _forged = DmaTerminalEvidence { _private: () };
/// ```
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::DmaTerminalEvidence;
///
/// fn replay(evidence: DmaTerminalEvidence) {
///     let consumed = evidence;
///     drop(evidence);
///     drop(consumed);
/// }
/// ```
#[must_use = "terminal DMA evidence must remain paired with its runtime completion"]
pub struct DmaTerminalEvidence {
    _private: (),
}

impl DmaTerminalEvidence {
    /// Mint evidence after accepting one non-replayable terminal IRQ batch.
    ///
    /// This hidden SPI exists only for the sealed ESP32-S31 runtime crate. It
    /// accepts no boolean, raw event image, or caller-selected completion kind.
    ///
    /// # Safety
    ///
    /// The caller must own the exact active command and DMA resource set and
    /// must have accepted an ISR-sampled and acknowledged terminal event batch
    /// for that operation. The returned evidence must not escape separately
    /// from those retained resources.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the unsafe constructor is the cross-crate terminal IRQ proof boundary"
    )]
    pub unsafe fn from_accepted_terminal_batch() -> Self {
        Self { _private: () }
    }

    /// Construct evidence for a native ownership model with no external DMA
    /// actor.
    #[cfg(not(target_arch = "riscv32"))]
    #[doc(hidden)]
    pub const fn for_native_model() -> Self {
        Self { _private: () }
    }
}
