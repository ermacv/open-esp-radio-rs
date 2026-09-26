//! The translations between OpenThread's radio values and the portable
//! IEEE 802.15.4 contract.

use oer_ieee802154::{AutoPendingMode, FrameAddress, TxStatus};

/// Bytes of the FCS OpenThread counts in every PSDU; the MAC appends it on
/// transmission and replaces it with RSSI and LQI on reception.
pub const FCS_SIZE: usize = 2;

/// The MAC bytes of a PSDU to transmit: OpenThread counts the FCS the MAC
/// appends. `None` for a PSDU without MAC bytes.
pub fn psdu_mac(psdu: &[u8]) -> Option<&[u8]> {
    psdu.len()
        .checked_sub(FCS_SIZE)
        .filter(|&length| length > 0)
        .map(|length| &psdu[..length])
}

/// Copy MAC bytes into `psdu` as OpenThread reads a received PSDU: the MAC
/// bytes and a zero FCS it does not check. Returns the PSDU length, or
/// `None` when the buffer is too short.
pub fn write_psdu(mac: &[u8], psdu: &mut [u8]) -> Option<usize> {
    let length = mac.len() + FCS_SIZE;
    let target = psdu.get_mut(..length)?;
    target[..mac.len()].copy_from_slice(mac);
    target[mac.len()..].fill(0);
    Some(length)
}

/// An extended address as OpenThread's glue passes it
/// (`u64::from_be_bytes(otExtAddress.m8)`) in over-the-air byte order.
pub const fn extended_address(address: u64) -> [u8; 8] {
    address.to_le_bytes()
}

/// A short source-match entry in the pending table, in over-the-air byte
/// order.
pub const fn short_pending_address(address: u16) -> FrameAddress {
    FrameAddress::Short(address.to_le_bytes())
}

/// An extended source-match entry in the pending table.
pub const fn extended_pending_address(address: u64) -> FrameAddress {
    FrameAddress::Extended(extended_address(address))
}

/// The automatic frame-pending mode of a source-match configuration:
/// disabled matching answers every poll with frame pending, enabled
/// matching looks the source up, in the enhanced mode ESP-IDF's OpenThread
/// port selects for Thread 1.2 and later (`otPlatRadioEnableSrcMatch`).
pub const fn pending_mode(enabled: bool) -> AutoPendingMode {
    if enabled {
        AutoPendingMode::Enhanced
    } else {
        AutoPendingMode::Disable
    }
}

/// Why a transmission failed, in the classes OpenThread's glue turns into
/// `otError`: channel-access failure, missing or invalid acknowledgement,
/// an invalid frame, or an abort.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransmitFailure {
    /// The channel was not acquired (`OT_ERROR_CHANNEL_ACCESS_FAILURE`).
    ChannelAccess,
    /// No acknowledgement arrived (`OT_ERROR_NO_ACK`).
    NoAcknowledgement,
    /// The acknowledgement was invalid (`OT_ERROR_NO_ACK`).
    InvalidAcknowledgement,
    /// The frame could not be sent.
    InvalidFrame,
    /// Anything else (`OT_ERROR_ABORT`).
    Other,
}

/// Classify a transmission's status as ESP-IDF's OpenThread port does: a
/// busy channel, an abort and a coexistence rejection are channel-access
/// failures. `None` for success.
pub const fn transmit_failure(status: TxStatus) -> Option<TransmitFailure> {
    match status {
        TxStatus::Success => None,
        TxStatus::ChannelBusy | TxStatus::Aborted | TxStatus::CoexistenceRejected => {
            Some(TransmitFailure::ChannelAccess)
        }
        TxStatus::NoAcknowledgement => Some(TransmitFailure::NoAcknowledgement),
        TxStatus::InvalidAcknowledgement => Some(TransmitFailure::InvalidAcknowledgement),
        TxStatus::InvalidFrame => Some(TransmitFailure::InvalidFrame),
        TxStatus::HardwareFailure | TxStatus::SecurityFailure => Some(TransmitFailure::Other),
    }
}

/// An energy scan's duration in microseconds.
pub const fn scan_micros(duration_millis: u16) -> u32 {
    duration_millis as u32 * 1_000
}

#[cfg(test)]
mod tests;
