//! One hardware transmission attempt and its completion.
//!
//! A [`TxAttempt`] is exactly one publication to the hardware: the backend
//! contends for the medium with the [`Backoff`] the caller chose, sends the
//! PPDU once, waits for the solicited response and reports the result as
//! one [`TxCompletion`]. It does not retry, fall back to another rate or
//! renumber the frame; the caller decides whether and how to submit the
//! next attempt from the completion. On the ESP32-S31 this is one
//! `prepare_bound_*`/`start_bound_*` pair of `TxHardware` and the one
//! completion `take_tx_completion` or `take_block_ack_completion` returns
//! (`hardware/esp32s31/driver/ieee80211/mac/src/tx.rs`).
//!
//! The frame lives in a [`TxBuffer`] the backend lends
//! ([`Ieee80211LowerMacPort::tx_buffer`](crate::Ieee80211LowerMacPort::tx_buffer)),
//! so a backend that publishes from its own DMA memory needs no copy.

use oer_ieee80211_mac::{phy::PhyRate, qos::WmmAccessCategory, sequence::SequenceNumber};
use oer_radio_coex::CoexPriority;

use crate::control::{KeyHandle, VifId};

/// Caller-chosen correlation identity of one attempt; its completion
/// carries it back.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxId(pub u32);

impl oer_radio_port::Correlation for TxId {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn raw(self) -> u32 {
        self.0
    }
}

/// Memory for one encoded MPDU, lent by the backend.
///
/// The buffer holds exactly the length it was requested with. The caller
/// writes the MPDU from its header to the end of its body, without the FCS
/// (and without the MIC when the backend's cipher transform appends it),
/// then submits the buffer inside a [`TxPayload`]. The backend releases a
/// submitted buffer when the attempt's completion is reported: the
/// completion does not return it, so an event lost to a queue overflow
/// cannot lose a buffer. A caller that retries re-encodes into a fresh
/// buffer. A buffer that is not submitted goes back through
/// [`Ieee80211LowerMacPort::release_tx_buffer`](crate::Ieee80211LowerMacPort::release_tx_buffer).
pub trait TxBuffer {
    /// The length the buffer was requested with.
    fn len(&self) -> usize;

    /// Whether the buffer holds no byte.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The frame bytes, for writing and reading.
    fn frame_mut(&mut self) -> &mut [u8];
}

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

/// One encoded MPDU and the response it solicits.
#[derive(Debug, Eq, PartialEq)]
pub struct TxPayload<B> {
    pub frame: B,
    pub response: TxResponse,
}

/// Medium protection sent ahead of the PPDU in the same attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Protection {
    None,
    /// RTS/CTS exchange with the receiver.
    RtsCts,
    /// CTS addressed to the transmitter itself.
    CtsToSelf,
}

/// Which installed key the hardware cipher applies.
///
/// The MPDU already carries its security header with the packet number the
/// caller allocated; the backend encrypts the body and appends the MIC
/// when its capabilities include the cipher transform.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeySelector {
    /// Send the frame as given.
    Plaintext,
    /// Protect the frame with an installed key.
    Key(KeyHandle),
}

/// Transmit power of one attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxPower {
    /// The backend's calibrated target power for the rate; the ESP32-S31
    /// selects the power table entry of the rate code.
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
    HardwareDraw { cw_exponent: u8 },
}

/// One hardware transmission attempt carrying payload `P`: a
/// [`TxPayload`] through the base port, an A-MPDU through
/// [`LowerMacAmpdu`](crate::LowerMacAmpdu).
#[derive(Debug, Eq, PartialEq)]
pub struct TxAttempt<P> {
    pub id: TxId,
    /// The interface whose address and TSF the frame uses.
    pub vif: VifId,
    /// The EDCA parameters the attempt contends with, and the queue it
    /// occupies ([`LowerMacCapabilities::tx_queue`](crate::LowerMacCapabilities::tx_queue)).
    pub access_category: WmmAccessCategory,
    pub payload: P,
    pub rate: PhyRate,
    pub protection: Protection,
    pub key: KeySelector,
    pub power: TxPower,
    pub backoff: Backoff,
    /// How urgently the attempt needs the shared antenna; the backend
    /// maps the levels its
    /// [`coex_priorities`](crate::LowerMacCapabilities::coex_priorities)
    /// state onto its coexistence arbitration.
    pub coex: CoexPriority,
}

/// Why an admitted attempt ended without a portable status of its own.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxFault {
    /// The protection exchange failed in a way the hardware does not
    /// retry (the ESP32-S31 RTS-error completion).
    ProtectionFailure,
    /// The hardware cipher found no usable key for the frame.
    KeyUnavailable,
    /// The hardware reported a completion the backend does not recognize;
    /// the backend keeps its raw code as its own diagnostic.
    Unrecognized,
}

/// How one attempt ended.
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
    /// The attempt was cancelled, or the backend quiesced or disabled
    /// before it completed, or the hardware timed it out; whether it
    /// reached the air is unknown.
    Aborted,
    Fault(TxFault),
}

/// The BlockAck the recipient answered an A-MPDU or BlockAckReq with.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockAckReport {
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

/// The terminal event of one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxCompletion {
    pub id: TxId,
    pub status: TxStatus,
    /// RSSI of the received ACK or BlockAck, when the hardware reports it.
    pub ack_rssi_dbm: Option<i8>,
    /// SNR of the received ACK or BlockAck, when the hardware reports it.
    pub ack_snr_db: Option<i8>,
    /// The BlockAck of a successful A-MPDU or BlockAckReq attempt.
    pub block_ack: Option<BlockAckReport>,
}

/// Why the backend refused an attempt; nothing was sent or changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubmitError {
    /// An earlier attempt still holds the attempt's queue; submit again
    /// after its completion.
    Busy,
    /// The port is disabled or quiescing.
    Disabled,
    /// The attempt names an interface that is not configured.
    UnknownVif,
    /// The attempt names a key that is not installed.
    UnknownKey,
    /// The backend cannot send this rate on the current channel.
    UnsupportedRate,
    /// The frame is too short to be an MPDU, or the aggregate is empty.
    InvalidLength,
    /// An attempt with the same identity is still in flight.
    DuplicateId,
    /// A value lies outside the limits the backend's capabilities declare:
    /// backoff, power ceiling, coexistence level, a response the rate
    /// cannot carry, or an aggregate larger than its limits.
    Unsupported,
}

/// A refused submission: the error and the attempt with its buffers, so
/// the caller can correct and resubmit it or release its buffers.
#[derive(Debug, Eq, PartialEq)]
pub struct Refused<A> {
    pub error: SubmitError,
    pub attempt: A,
}
