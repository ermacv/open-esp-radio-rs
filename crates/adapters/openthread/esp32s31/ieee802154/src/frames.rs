//! The translations between OpenThread's radio values and the portable
//! IEEE 802.15.4 contract.

use oer_ieee802154::{
    AppliedSecurity, AutoPendingMode, FrameAddress, MAX_MAC_FRAME_LEN, MacKeys,
    SentAcknowledgement, TxSecurity, TxStatus,
};

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

/// The MAC keys and frame counter of ESP-IDF's OpenThread port before
/// OpenThread sets any: its zeroed statics.
pub const PORT_INITIAL_KEYS: MacKeys = MacKeys::new(0, [0; 16], [0; 16], [0; 16], 0);

/// Who secures a frame, from its OpenThread transmit information: nobody
/// when the stack secured it (`mIsSecurityProcessed`), the radio under the
/// frame's own counter and key index for a retransmission (`mIsARetx`),
/// otherwise the radio with new ones.
pub const fn tx_security(retransmission: bool, security_processed: bool) -> TxSecurity {
    if security_processed {
        TxSecurity::Processed
    } else if retransmission {
        TxSecurity::Retransmission
    } else {
        TxSecurity::Radio
    }
}

/// Write the security header fields the radio assigned into the PSDU
/// OpenThread gave it, as `otMacFrameSetFrameCounter` and
/// `otMacFrameSetKeyId` write them into the port's frame. Returns `false`
/// when the PSDU has no auxiliary security header to write.
pub fn write_applied_security(applied: AppliedSecurity, psdu: &mut [u8]) -> bool {
    let length = psdu.len();
    if length > MAX_MAC_FRAME_LEN + FCS_SIZE {
        return false;
    }
    // The portable writer reads the frame as a `[PHR, PSDU...]` image.
    let mut image = [0; 1 + MAX_MAC_FRAME_LEN + FCS_SIZE];
    image[0] = length as u8;
    image[1..=length].copy_from_slice(psdu);
    if !applied.write(&mut image[..=length]) {
        return false;
    }
    psdu.copy_from_slice(&image[1..=length]);
    true
}

/// Replace the keys (`otPlatRadioSetMacKey`), keeping the frame counter.
pub fn set_mac_keys(
    keys: &mut Option<MacKeys>,
    key_id: u8,
    previous: [u8; 16],
    current: [u8; 16],
    next: [u8; 16],
) {
    keys.get_or_insert(PORT_INITIAL_KEYS)
        .set_keys(key_id, previous, current, next);
}

/// Replace the frame counter (`otPlatRadioSetMacFrameCounter`), or raise it
/// (`otPlatRadioSetMacFrameCounterIfLarger`).
pub fn set_frame_counter(keys: &mut Option<MacKeys>, frame_counter: u32, if_larger: bool) {
    let keys = keys.get_or_insert(PORT_INITIAL_KEYS);
    if if_larger {
        keys.set_frame_counter_if_larger(frame_counter);
    } else {
        keys.set_frame_counter(frame_counter);
    }
}

/// The frame counter and key index of the secured enhanced ACK a received
/// frame was sent (`mAckFrameCounter`, `mAckKeyId`).
pub fn sent_ack_security(sent: SentAcknowledgement) -> Option<(u32, u8)> {
    let security = sent.security?;
    Some((security.frame_counter, security.key_id?))
}

/// A time OpenThread gives in the low 32 bits of the radio clock
/// (`otPlatRadioReceiveAt`, `mTxDelayBaseTime + mTxDelay`), in the full
/// clock: the instant nearest to `now` with those low bits, up to half the
/// 32-bit range before or after it.
pub const fn radio_time(now: u64, low: u32) -> u64 {
    let offset = low.wrapping_sub(now as u32) as i32;
    now.wrapping_add_signed(offset as i64)
}

/// The CSL drift of ESP-IDF's OpenThread port, in ± ppm
/// (`CONFIG_OPENTHREAD_CSL_ACCURACY` default).
pub const CSL_ACCURACY_PPM: u8 = 50;

/// The CSL uncertainty of ESP-IDF's OpenThread port, in units of 10
/// microseconds (`CONFIG_OPENTHREAD_CSL_UNCERTAIN` default).
pub const CSL_UNCERTAINTY: u8 = 50;

/// A CSL period as OpenThread passes it (`otPlatRadioEnableCsl`, units of
/// ten symbols) for the radio's 16-bit CSL IE field, as the port's
/// `otMacFrameSetCslIe` truncates it.
pub const fn csl_period(period: u32) -> u16 {
    period as u16
}

#[cfg(test)]
mod tests;
