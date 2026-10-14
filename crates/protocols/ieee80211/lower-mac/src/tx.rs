//! The values of a transmission and of its completion.
//!
//! An attempt of the phase ([`Attempt`](crate::Attempt)) is one frame
//! exchange by its retry plan ([`RetryPlan`](crate::RetryPlan)), which the
//! backend carries out below the port; its one [`TxOutcome`] reports how
//! it ended, how many transmissions it made and at which rate the last
//! went. The header the caller encodes lives in a transmit buffer of the
//! phase; the body, such as a network frame's payload, travels by
//! ownership as a [`TxBody`]. Whether the backend sends the body from its
//! owner's memory or copies it into its own is the backend's choice, not
//! the contract's.

use core::num::NonZeroU16;
use oer_ieee80211_mac::sequence::SequenceNumber;

/// The response the attempt solicits, which the hardware waits for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxResponse {
    /// Nothing answers the frame: a group-addressed frame, or an
    /// individually addressed one sent without acknowledgement where the
    /// backend's
    /// [`individual_no_ack`](crate::LowerMacCapabilities::individual_no_ack)
    /// covers the rate.
    None,
    /// An individually addressed frame answered by an ACK.
    Ack,
    /// A BlockAckReq answered by a BlockAck, whose starting sequence and
    /// bitmap the completion reports.
    BlockAck,
}

/// The end of an MPDU that an attempt carries by ownership: a network
/// frame's payload, which the caller does not copy.
///
/// The backend holds the body from the attempt's admission until the
/// [`crate::LowerMacEvent::TxCompleted`] carries it back. A `Sent` MPDU
/// retains its body inside the returned buffer for retransmission;
/// [`crate::TxBufferPool::renew`] returns both memory and body. Fresh
/// returned MPDUs carry bodies separately by subframe index. Dropping a body is how
/// its owner learns it is no longer needed: a consumer that drops the
/// completion event drops its bodies, and a backend that discards an
/// attempt, as an uninstall does, drops them with it.
pub trait TxBody {
    /// The octets the MPDU ends with.
    fn bytes(&self) -> &[u8];
}

/// No body: the body type of a backend part that takes attempts whose
/// bodies its owner already placed, such as a driver behind a backend that
/// copies them.
impl TxBody for core::convert::Infallible {
    fn bytes(&self) -> &[u8] {
        match *self {}
    }
}

/// Medium protection sent ahead of the PPDU in the same attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Protection {
    /// No RTS/CTS or CTS-to-self exchange before the PPDU.
    None,
    /// RTS/CTS exchange with the receiver.
    RtsCts,
    /// CTS addressed to the transmitter itself.
    CtsToSelf,
}

/// Transmit power of one attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxPower {
    /// The backend's calibrated target power for the rate.
    Calibrated,
    /// At most this many dBm: the backend sends the calibrated target when
    /// that is lower. Accepted down to
    /// [`tx_power_ceiling_min_dbm`](crate::LowerMacCapabilities::tx_power_ceiling_min_dbm).
    MaxDbm(i8),
}

/// The CSMA/CA backoff of one attempt.
///
/// Drawing the backoff is contention policy above the port unless the
/// backend reports [`HardwareServices::BACKOFF_DRAW`](crate::HardwareServices::BACKOFF_DRAW);
/// `oer-ieee80211-softmac`'s `EdcaContention` draws it for a caller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Backoff {
    /// Count down exactly this many slots after the AIFS, at most
    /// [`max_backoff_slots`](crate::LowerMacCapabilities::max_backoff_slots).
    Slots(u16),
    /// Let the hardware draw from a contention window of
    /// `2^cw_exponent - 1` slots. Valid only when the backend reports
    /// [`HardwareServices::BACKOFF_DRAW`](crate::HardwareServices::BACKOFF_DRAW).
    HardwareDraw {
        /// Contention-window exponent: draw from zero through `2^cw_exponent - 1`.
        cw_exponent: u8,
    },
}

/// One hardware publication observed by a retry planner. This is not a
/// terminal of a port attempt: the backend carries its plan through to
/// [`TxOutcome`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxStatus {
    /// The PPDU went out and the solicited response, if any, arrived.
    Success,
    /// No ACK or BlockAck arrived.
    AckTimeout,
    /// The protection exchange drew no CTS.
    CtsTimeout,
    /// The attempt lost contention or collided before a response.
    Collision,
    /// Hardware timed out without a completion; the publication outcome
    /// is unknown. This ends the attempt as Indeterminate, never a retry.
    TimedOut,
}

/// The BlockAck the recipient answered an A-MPDU or BlockAckReq with.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockAckReport {
    /// The sequence number represented by bit zero of `bitmap`.
    pub start_sequence: SequenceNumber,
    /// Bit `n` acknowledges `start_sequence + n`.
    pub bitmap: u64,
}

impl BlockAckReport {
    /// Positions of the 64-bit bitmap.
    pub const BITMAP_BITS: u16 = 64;

    /// Whether the transmitter may consider `sequence` delivered.
    ///
    /// Recipients use both standard-compliant starting-sequence conventions:
    /// some keep the oldest possible sequence and describe it with the
    /// bitmap, others advance the start to the first sequence not yet
    /// received. In the latter form an MPDU just before the window is
    /// already delivered although it has no bitmap bit. Only a bounded
    /// predecessor, at most one bitmap width behind the start, counts; a
    /// sequence beyond either side of the window stays unacknowledged, so a
    /// stale report cannot release newer traffic.
    pub const fn acknowledges(self, sequence: SequenceNumber) -> bool {
        let distance = self.start_sequence.forward_distance(sequence);
        if distance < Self::BITMAP_BITS {
            self.bitmap & (1_u64 << distance) != 0
        } else {
            sequence.forward_distance(self.start_sequence) <= Self::BITMAP_BITS
        }
    }
}

/// A known result after the backend has finished the attempt's plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxEndStatus {
    /// The solicited response arrived, or the frame solicited none.
    Delivered,
    /// The plan exhausted its ACK/BlockAck response budget.
    AckTimeout,
    /// The plan exhausted its protection response budget.
    CtsTimeout,
    /// Internal contention exhausted the backend's budget. No rate sample.
    ContentionLimit,
}

/// Why an admitted attempt was returned without publishing it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WithdrawCause {
    /// The transmit gate closed.
    GateClosed,
    /// The attempt's key is being removed.
    KeyRemoved,
    /// The port is being disabled.
    Disabled,
}

/// Why a published MPDU's outcome cannot be established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndeterminateCause {
    /// Hardware timed out and no completion arrived before detachment.
    HardwareTimeout,
}

/// Evidence carried by the solicited ACK or BlockAck.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxAck {
    /// The response's RSSI, when measured.
    pub rssi_dbm: Option<i8>,
    /// The response's SNR, when measured.
    pub snr_db: Option<i8>,
    /// A BlockAck's sequence window.
    pub block_ack: Option<BlockAckReport>,
}

/// The sole terminal of an admitted operation, beside its owned resources.
///
/// A hardware timeout is final: the backend does not republish the MPDU.
/// A completion already observed takes priority over the timeout. Only
/// `Ended` claims an exact transmission count; rate selection ignores
/// withdrawals, indeterminate outcomes and contention exhaustion.
/// The next operation in the queue may proceed after every terminal.
///
/// A terminal cannot claim a rate with zero transmissions:
///
/// ```compile_fail,E0559
/// use oer_ieee80211_lower_mac::{PhyRate, TxEndStatus, TxOutcome};
/// fn zero(rate: PhyRate) -> TxOutcome {
///     TxOutcome::Ended {
///         status: TxEndStatus::ContentionLimit, transmissions: 0, rate: Some(rate), ack: None,
///     }
/// }
/// ```
///
/// Nor can it report transmissions while omitting their last rate:
///
/// ```compile_fail,E0559
/// use oer_ieee80211_lower_mac::{TxEndStatus, TxOutcome};
/// fn missing_rate() -> TxOutcome {
///     TxOutcome::Ended {
///         status: TxEndStatus::AckTimeout, transmissions: 2, rate: None, ack: None,
///     }
/// }
/// ```
///
/// The complete observation is either absent or carries both values:
///
/// ```
/// use core::num::NonZeroU16;
/// use oer_ieee80211_lower_mac::{PhyRate, TxEndStatus, TxOutcome};
/// fn observed(rate: PhyRate) -> [TxOutcome; 2] {
///     [
///         TxOutcome::Ended { status: TxEndStatus::ContentionLimit, last: None, ack: None },
///         TxOutcome::Ended {
///             status: TxEndStatus::Delivered, last: Some((NonZeroU16::MIN, rate)), ack: None,
///         },
///     ]
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome {
    /// Lifecycle stopped the attempt before its plan ended; no transmission
    /// delivered it. ReturnedMpdu is Fresh if never published, otherwise Sent,
    /// retaining the original SN, PN and body. No rate sample.
    Withdrawn(WithdrawCause),
    /// The plan has a known result. Returned MPDUs are `Sent`.
    Ended {
        /// The known result, including known failure without an MPDU on air.
        status: TxEndStatus,
        /// Exact MPDU transmission count, including the first, paired with
        /// the last transmitted MPDU's rate. `None` means no MPDU reached air
        /// after protection or contention failure; this is not publication count.
        last: Option<(NonZeroU16, crate::PhyRate)>,
        /// Evidence from the solicited response.
        ack: Option<TxAck>,
    },
    /// Final unknown outcome. Returned MPDUs are `Sent`; Data retires them.
    Indeterminate(IndeterminateCause),
}

impl TxOutcome {
    /// Whether this terminal supplies a usable radio-rate observation.
    pub const fn is_rate_sample(self) -> bool {
        matches!(
            self,
            Self::Ended {
                status: TxEndStatus::Delivered | TxEndStatus::AckTimeout | TxEndStatus::CtsTimeout,
                last: Some(_),
                ..
            }
        )
    }
}
