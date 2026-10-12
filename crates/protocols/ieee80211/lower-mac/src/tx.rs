//! The values of a transmission and of its completion.
//!
//! An attempt of the phase ([`Attempt`](crate::Attempt)) is one frame
//! exchange by its retry plan ([`RetryPlan`](crate::RetryPlan)), which the
//! backend carries out below the port; its one [`TxCompletion`] reports how
//! it ended, how many transmissions it made and at which rate the last
//! went. The header the caller encodes lives in a transmit buffer of the
//! phase; the body, such as a network frame's payload, travels by
//! ownership as a [`TxBody`]. Whether the backend sends the body from its
//! owner's memory or copies it into its own is the backend's choice, not
//! the contract's.

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
/// attempt's completion event carries it back
/// (`Ieee80211LowerMacPort::into_completed`),
/// so a retransmission sends the same owner again. Dropping a body is how
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
    HardwareDraw { cw_exponent: u8 },
}

/// Why an admitted attempt ended without a portable status of its own.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxFault {
    /// The protection exchange failed in a way the hardware does not
    /// retry.
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
    pub status: TxStatus,
    /// RSSI of the received ACK or BlockAck, when the hardware reports it.
    pub ack_rssi_dbm: Option<i8>,
    /// SNR of the received ACK or BlockAck, when the hardware reports it.
    pub ack_snr_db: Option<i8>,
    /// The BlockAck of a successful A-MPDU or BlockAckReq attempt.
    pub block_ack: Option<BlockAckReport>,
    /// The transmissions the attempt made by its retry plan, the first
    /// included; 0 when it ended before its first.
    pub transmissions: u8,
    /// The rate of its last transmission, for the rate control above the
    /// port; `None` when it made none.
    pub rate: Option<crate::PhyRate>,
}

