//! Synthetic peer management frames for STA state-machine tests.

use open_esp_radio_ieee80211::management::MANAGEMENT_HEADER_LEN;

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const ASSOCIATION_RESPONSE_FRAME_CONTROL: u16 = 0x0010;
const OPEN_SYSTEM_RESPONSE_SEQUENCE: u16 = 2;

pub(super) const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];

pub(super) const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

pub(super) fn authentication_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&OPEN_AUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&OPEN_SYSTEM_RESPONSE_SEQUENCE.to_le_bytes());
    frame[28..30].copy_from_slice(&status_code.to_le_bytes());
    frame
}

pub(super) fn association_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&ASSOCIATION_RESPONSE_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&status_code.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

pub(super) fn deauthentication(reason_code: u16) -> [u8; MANAGEMENT_HEADER_LEN + 2] {
    let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + 2];
    frame[0..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&reason_code.to_le_bytes());
    frame
}
