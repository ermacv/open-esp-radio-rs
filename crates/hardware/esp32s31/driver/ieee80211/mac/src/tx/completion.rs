//! The meaning of a completion event, independent of its published owner.

use oer_esp32s31_hal::types::MacTxCompletionObservation;
use oer_ieee80211_lower_mac::TxStatus;

/// Whether the vendor handler observes a publication result at this event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionObservation {
    /// A known observation; the driver may consume its published attempt.
    Observed(TxStatus),
    /// The handler does not end the frame exchange. Retain the publication
    /// until a later result or hardware timeout; no terminal or rate sample.
    NoEffect,
}

/// Hardware contradicts the checked publication or reports no defined status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionFault {
    /// The backend published with an installation the cipher cannot use.
    SecurityKeyError,
    /// A status outside all recovered completion-dispatch branches.
    InvalidStatus { status: u8 },
}

/// Value-only input to the completion decoder. No attempt, buffer or driver
/// is borrowed or consumed. The selector is a handler argument, not a bit
/// in hardware's Trigger-flow metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionRaw {
    status: u8,
    detail: u8,
    handler_argument: u32,
}

impl CompletionRaw {
    /// Ordinary completion dispatch passes zero to both error handlers.
    ///
    /// SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxComplete +0x19a
    /// clears RTS a3; +0x1cc clears TxError a2. rom[]::lmacProcessTxComplete
    /// +0x152 clears RTS a3 too. libpp[wdev.o]::wdev_funcs_init +0x900 puts
    /// the library RTS handler into the ROM table at offset 892.
    pub const fn ordinary(raw: MacTxCompletionObservation) -> Self {
        Self {
            status: raw.status(),
            detail: raw.detail(),
            handler_argument: 0,
        }
    }

    /// An explicit invocation of the vendor RTS handler, including its a3.
    pub const fn rts_error(detail: u8, fourth_argument: u32) -> Self {
        Self {
            status: 1,
            detail,
            handler_argument: fourth_argument,
        }
    }

    /// An explicit invocation of TxError, including its a2 selector.
    pub const fn tx_error(detail: u8, selector: u32) -> Self {
        Self {
            status: 4,
            detail,
            handler_argument: selector,
        }
    }
}

/// Decode an event without owning the publication. NoEffect deliberately
/// carries no status with which a driver could finish that attempt.
/// Faults belong to the backend's existing Poisoned path, never TxOutcome.
pub const fn decode(raw: CompletionRaw) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::Observed;
    // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxComplete +0x188,
    // +0x1a2, +0x1b0, +0x1d0 and +0x1be dispatch success, RTS, CTS,
    // TxError and ACK respectively. Status 3 reaches wifi_log, then
    // unconditional wifi_assert(0) at +0x252 (lmac.c:1575); statuses beyond
    // the dispatch table also stop. Model that vendor stop as Poisoned.
    // SOURCE(esp32s31): libpp[pp.o]::ppTask +0x19c calls the library
    // libpp[lmac.o]::lmacProcessTxComplete directly. libpp[wdev.o]::wdev_funcs_init
    // +0x42c/+0x430 loads that same address and +0x434 installs it in the
    // dispatch table. Thus ROM table calls also execute the library copy;
    // the divergent rom[]::lmacProcessTxComplete status-3 branch is not
    // on this path. `cargo verification copies --chip esp32s31` audits
    // that divergence; these citations register its executable provenance.
    match raw.status {
        0 => Ok(Observed(TxStatus::Success)),
        1 => rts_error(raw.detail, raw.handler_argument),
        2 => Ok(Observed(TxStatus::CtsTimeout)),
        4 => tx_error(raw.detail, raw.handler_argument),
        5 => Ok(Observed(TxStatus::AckTimeout)),
        status => Err(CompletionFault::InvalidStatus { status }),
    }
}

const fn rts_error(
    detail: u8,
    fourth_argument: u32,
) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::{NoEffect, Observed};
    match detail {
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0x4e..+0x6e accepts
        // 1, 3..5 and 0xa0..0xad and tail-calls libpp[lmac.o]::lmacProcessCollision +0x124.
        1 | 3..=5 | 0xa0..=0xad => Ok(Observed(TxStatus::Collision)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0xd2 returns without
        // ending FES when a3 != 0 for detail 0xc0.
        0xc0 if fourth_argument != 0 => Ok(NoEffect),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0xe6 calls
        // libpp[lmac.o]::lmacEndFrameExchangeSequence(inst, 1, 0), the same
        // arguments as libpp[lmac.o]::lmacProcessTxseckiderr +0x26. This is a key fault.
        0xc0 => Err(CompletionFault::SecurityKeyError),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0x72..+0xaa only logs
        // every remaining detail. It does not end the publication.
        _ => Ok(NoEffect),
    }
}

const fn tx_error(detail: u8, selector: u32) -> Result<CompletionObservation, CompletionFault> {
    use CompletionObservation::{NoEffect, Observed};
    match detail {
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xe2 dispatches detail 0
        // to libpp[lmac.o]::lmacProcessCtsTimeout, independently of a2.
        0 => Ok(Observed(TxStatus::CtsTimeout)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xb2 dispatches 1 and
        // 3..5 to libpp[lmac.o]::lmacProcessCollision only when a2 == 0.
        1 | 3..=5 if selector == 0 => Ok(Observed(TxStatus::Collision)),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xea returns without an
        // end for detail 0xc0 when a2 != 0.
        0xc0 if selector != 0 => Ok(NoEffect),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0x98 calls
        // libpp[lmac.o]::lmacProcessTxseckiderr; that function's +0x26 ends FES(inst, 1, 0).
        0xc0 => Err(CompletionFault::SecurityKeyError),
        // SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xca dispatches the other
        // details, including 1/3..5 with nonzero a2, to libpp[lmac.o]::lmacProcessAckTimeout.
        _ => Ok(Observed(TxStatus::AckTimeout)),
    }
}

/// A resolved LMAC instance queue, bounded to the five vendor instances.
/// The virtual handler argument 10 is never a hardware queue index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionQueue(u8);

/// A raw handler argument or descriptor does not name a vendor instance.
/// This is a route-construction refusal, not a publication observation or a
/// CompletionFault. The direct driver retains a typed ordinary queue and
/// therefore does not need to parse such untrusted ABI values at completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionQueueError {
    pub argument: u8,
    pub resolved: u8,
}

impl CompletionQueue {
    /// The directly retained EDCA queue needs no dynamic route check.
    pub const fn ordinary(queue: super::LegacyTxQueue) -> Self {
        Self(queue.hardware_index())
    }

    /// Resolve the vendor handler argument through its owning TX entry.
    /// `our_tx_eb_queue_word` is descriptor word +4, reached through
    /// our_tx_eb's descriptor pointer at +52; it is descriptor data, not MMIO.
    ///
    /// SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0x1a..+0x2c resolves
    /// argument 10 through our_tx_eb, descriptor +52, and word +4 bits 7:4.
    /// +0x42 requires the resolved instance to be below five.
    pub const fn from_vendor_argument(
        argument: u8,
        our_tx_eb_queue_word: u32,
    ) -> Result<Self, CompletionQueueError> {
        let resolved = if argument == 10 {
            ((our_tx_eb_queue_word >> 4) & 0x0f) as u8
        } else {
            argument
        };
        if resolved < 5 {
            Ok(Self(resolved))
        } else {
            Err(CompletionQueueError { argument, resolved })
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// A read event is separate from the typed completed publication's metadata.
/// NoEffect cannot supply a completion to a detach/finish path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionEvent<T> {
    Observed(T),
    NoEffect,
}

impl<T> CompletionEvent<T> {
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> CompletionEvent<U> {
        match self {
            Self::Observed(value) => CompletionEvent::Observed(map(value)),
            Self::NoEffect => CompletionEvent::NoEffect,
        }
    }
}
