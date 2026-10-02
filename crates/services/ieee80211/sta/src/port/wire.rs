//! Frame Control fields the station reads to route a received MPDU.

use oer_ieee80211_mac::ccmp::CCMP_HEADER_LEN;

const TYPE_MANAGEMENT: u8 = 0;
const TYPE_CONTROL: u8 = 1;
const TYPE_DATA: u8 = 2;
const PROTECTED: u8 = 0x40;
const RETRY: u8 = 0x08;
const MORE_DATA: u8 = 0x20;
const FROM_DS: u8 = 0x02;
const TO_DS: u8 = 0x01;
const QOS_SUBTYPE: u8 = 0x08;
const NULL_SUBTYPE: u8 = 0x04;
const MANAGEMENT_HEADER_LEN: usize = 24;
const LEGACY_DATA_HEADER_LEN: usize = 24;
const QOS_DATA_HEADER_LEN: usize = 26;

pub(crate) const fn frame_type(frame: &[u8]) -> u8 {
    (frame[0] >> 2) & 0b11
}

pub(crate) fn is_management(frame: &[u8]) -> bool {
    frame.len() >= MANAGEMENT_HEADER_LEN && frame_type(frame) == TYPE_MANAGEMENT
}

pub(crate) fn is_control(frame: &[u8]) -> bool {
    frame.len() >= 10 && frame_type(frame) == TYPE_CONTROL
}

pub(crate) fn is_data(frame: &[u8]) -> bool {
    frame.len() >= LEGACY_DATA_HEADER_LEN && frame_type(frame) == TYPE_DATA
}

/// Frame Control subtype.
pub(crate) const fn subtype(frame: &[u8]) -> u8 {
    frame[0] >> 4
}

pub(crate) const fn is_protected(frame: &[u8]) -> bool {
    frame[1] & PROTECTED != 0
}

pub(crate) const fn is_retry(frame: &[u8]) -> bool {
    frame[1] & RETRY != 0
}

pub(crate) const fn more_data(frame: &[u8]) -> bool {
    frame[1] & MORE_DATA != 0
}

/// A data frame the access point sent to its BSS: From DS only.
pub(crate) const fn from_access_point(frame: &[u8]) -> bool {
    frame[1] & (FROM_DS | TO_DS) == FROM_DS
}

pub(crate) const fn is_qos_data(frame: &[u8]) -> bool {
    subtype(frame) & QOS_SUBTYPE != 0
}

/// A Null or QoS Null frame, which carries no MSDU.
pub(crate) const fn is_null_data(frame: &[u8]) -> bool {
    subtype(frame) & NULL_SUBTYPE != 0
}

pub(crate) fn address(frame: &[u8], offset: usize) -> Option<[u8; 6]> {
    frame.get(offset..offset + 6)?.try_into().ok()
}

pub(crate) fn address1(frame: &[u8]) -> Option<[u8; 6]> {
    address(frame, 4)
}

pub(crate) fn address2(frame: &[u8]) -> Option<[u8; 6]> {
    address(frame, 10)
}

pub(crate) const fn is_group(address: [u8; 6]) -> bool {
    address[0] & 1 != 0
}

pub(crate) fn sequence_control(frame: &[u8]) -> u16 {
    u16::from_le_bytes([frame[22], frame[23]])
}

/// The data frame's MAC header length.
pub(crate) const fn data_header_len(frame: &[u8]) -> usize {
    if is_qos_data(frame) {
        QOS_DATA_HEADER_LEN
    } else {
        LEGACY_DATA_HEADER_LEN
    }
}

/// The TID of a QoS data frame.
pub(crate) fn tid(frame: &[u8]) -> Option<u8> {
    (is_qos_data(frame) && frame.len() >= QOS_DATA_HEADER_LEN).then(|| frame[24] & 0x0f)
}

/// The CCMP header of a protected frame whose MAC header is `header_len`
/// long.
pub(crate) fn ccmp_header(frame: &[u8], header_len: usize) -> Option<[u8; CCMP_HEADER_LEN]> {
    frame
        .get(header_len..header_len + CCMP_HEADER_LEN)?
        .try_into()
        .ok()
}

/// The body of a management frame, after its CCMP header when protected.
pub(crate) fn management_body(frame: &[u8]) -> Option<&[u8]> {
    let start = MANAGEMENT_HEADER_LEN
        + if is_protected(frame) {
            CCMP_HEADER_LEN
        } else {
            0
        };
    frame.get(start..)
}
