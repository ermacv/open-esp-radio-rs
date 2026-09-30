//! One hardware transmission attempt and its completion.
//!
//! A [`TxAttempt`] is exactly one publication to the hardware: the backend
//! contends for the medium with the backoff the caller chose, sends the
//! PPDU once, waits for the solicited response and reports the result as
//! one [`TxCompletion`]. It does not retry, fall back to another rate or
//! renumber the frame; the caller decides whether and how to submit the
//! next attempt from the completion. On the ESP32-S31 this is one
//! `prepare_bound_*`/`start_bound_*` pair of `TxHardware` and the one
//! completion `take_tx_completion` or `take_block_ack_completion` returns
//! (`hardware/esp32s31/driver/ieee80211/mac/src/tx.rs`).

use oer_ieee80211_mac::{phy::PhyRate, qos::WmmAccessCategory, sequence::SequenceNumber};

use crate::control::{KeyHandle, VifId};

/// Caller-chosen correlation identity of one attempt; its completion
/// carries it back.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxId(pub u32);

/// The response the attempt solicits, which the hardware waits for.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxResponse {
    /// A group-addressed or No-Ack frame: nothing answers it.
    None,
    /// An individually addressed frame answered by an ACK.
    Ack,
    /// A BlockAckReq answered by a BlockAck, whose starting sequence and
    /// bitmap the completion reports.
    BlockAck,
}

/// One aggregate: encoded MPDUs sent as one A-MPDU and answered by a
/// BlockAck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduSubmission<'a> {
    /// The encoded MPDUs in transmission order, each without FCS.
    pub subframes: &'a [&'a [u8]],
    /// The traffic identifier all subframes share.
    pub tid: u8,
    /// The recipient's Minimum MPDU Start Spacing, the IEEE encoding 0-7 of
    /// its HT Capabilities A-MPDU Parameters.
    pub min_mpdu_start_spacing: u8,
}

/// The frames of one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxPayload<'a> {
    /// One encoded MPDU, header to the end of its body, without the FCS the
    /// backend appends when its capabilities say so.
    Mpdu {
        frame: &'a [u8],
        response: TxResponse,
    },
    /// One A-MPDU.
    Ampdu(AmpduSubmission<'a>),
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
    /// At most this many dBm; the backend sends the calibrated target when
    /// that is lower.
    MaxDbm(i8),
}

/// One hardware transmission attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAttempt<'a> {
    pub id: TxId,
    /// The interface whose address and TSF the frame uses.
    pub vif: VifId,
    /// The EDCA queue whose parameters the attempt contends with.
    pub access_category: WmmAccessCategory,
    pub payload: TxPayload<'a>,
    pub rate: PhyRate,
    pub protection: Protection,
    pub key: KeySelector,
    pub power: TxPower,
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
    /// before it completed; whether it reached the air is unknown.
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
    /// Earlier attempts still hold the queue; submit again after a
    /// completion.
    Busy,
    /// The port is disabled or quiescing.
    Disabled,
    /// The attempt names an interface that is not configured.
    UnknownVif,
    /// The attempt names a key that is not installed.
    UnknownKey,
    /// The backend cannot send this rate on the current channel.
    UnsupportedRate,
    /// The frame or aggregate exceeds what the backend can send, or the
    /// aggregate is empty.
    InvalidLength,
    /// The aggregate has more subframes than
    /// [`LowerMacCapabilities::max_ampdu_subframes`](crate::LowerMacCapabilities::max_ampdu_subframes).
    TooManySubframes,
    /// An attempt with the same identity is still in flight.
    DuplicateId,
    /// The backend does not implement this kind of attempt.
    Unsupported,
}
