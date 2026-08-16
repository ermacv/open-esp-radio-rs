//! Bounded, allocation-free AP beacon field ownership.
//!
//! The S31 vendor builder derives DTIM from a ROM TSF leaf which remains zero
//! in the strict runtime. This module owns only the stateless correction:
//! locate the fixed-size TIM element and derive its phase from the executor
//! timestamp and the beacon's advertised interval.

use crate::{
    channel::WifiChannel,
    ht::{ht_capability_ie, ht_operation_ie},
    ssid::WifiSsid,
};

pub const WPA2_BEACON_CAPACITY: usize = 256;

const MANAGEMENT_HEADER_LEN: usize = 24;
const BEACON_FIXED_BODY_LEN: usize = 12;
/// Exact RSN IE advertised by the initial WPA2-Personal AP profile.
///
/// The authenticator repeats this byte-for-byte in EAPOL Message 3. Keeping
/// one public value prevents the beacon and security transaction from
/// acquiring independent copies of the same protocol contract.
pub const WPA2_PERSONAL_CCMP_PSK_RSN_IE: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const SUPPORTED_RATES: [u8; 8] = [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60];
const EXTENDED_RATES: [u8; 4] = [0x6c, 0x12, 0x24, 0x48];
const WMM_PARAMETER_IE: [u8; 26] = [
    0xdd, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x04, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4,
    0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApBeaconBuildError {
    InvalidPrimaryChannel,
    InvalidDtimPeriod,
    InvalidSequenceNumber,
    OutputTooSmall { required: usize },
}

/// Build one visible WPA2-Personal HT/WMM beacon without allocating.
///
/// The caller owns timestamp/DTIM progression through [`stamp`]. This builder
/// publishes the fixed first-AP profile only: 100 TU, one-byte TIM bitmap,
/// CCMP, PSK, WMM and coherent one-stream HT capability records.
pub fn write_wpa2_ht_beacon(
    output: &mut [u8],
    access_point: [u8; 6],
    ssid: &WifiSsid,
    channel: WifiChannel,
    beacon_interval_tu: u16,
    dtim_period: u8,
    management_sequence: u16,
) -> Result<usize, ApBeaconBuildError> {
    if !(1..=13).contains(&channel.primary()) {
        return Err(ApBeaconBuildError::InvalidPrimaryChannel);
    }
    if dtim_period == 0 {
        return Err(ApBeaconBuildError::InvalidDtimPeriod);
    }
    if management_sequence > 0x0fff {
        return Err(ApBeaconBuildError::InvalidSequenceNumber);
    }

    let ht_capability = ht_capability_ie(channel);
    let ht_operation = ht_operation_ie(channel);
    let required = MANAGEMENT_HEADER_LEN
        + BEACON_FIXED_BODY_LEN
        + 2
        + ssid.as_bytes().len()
        + 2
        + SUPPORTED_RATES.len()
        + 3
        + 6
        + WPA2_PERSONAL_CCMP_PSK_RSN_IE.len()
        + 2
        + EXTENDED_RATES.len()
        + WMM_PARAMETER_IE.len()
        + ht_capability.len()
        + ht_operation.len();
    if output.len() < required {
        return Err(ApBeaconBuildError::OutputTooSmall { required });
    }

    let frame = &mut output[..required];
    frame.fill(0);
    frame[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&access_point);
    frame[16..22].copy_from_slice(&access_point);
    frame[22..24].copy_from_slice(&(management_sequence << 4).to_le_bytes());
    frame[32..34].copy_from_slice(&beacon_interval_tu.to_le_bytes());
    // ESS | Privacy | Short Preamble | Short Slot Time.
    frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());

    let mut offset = MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN;
    write_element(frame, &mut offset, 0, ssid.as_bytes());
    write_element(frame, &mut offset, 1, &SUPPORTED_RATES);
    write_element(frame, &mut offset, 3, &[channel.primary()]);
    write_element(frame, &mut offset, 5, &[dtim_period - 1, dtim_period, 0, 0]);
    copy_record(frame, &mut offset, &WPA2_PERSONAL_CCMP_PSK_RSN_IE);
    write_element(frame, &mut offset, 50, &EXTENDED_RATES);
    copy_record(frame, &mut offset, &WMM_PARAMETER_IE);
    copy_record(frame, &mut offset, &ht_capability);
    copy_record(frame, &mut offset, &ht_operation);
    debug_assert_eq!(offset, required);
    Ok(required)
}

fn write_element(frame: &mut [u8], offset: &mut usize, id: u8, value: &[u8]) {
    frame[*offset] = id;
    frame[*offset + 1] = value.len() as u8;
    *offset += 2;
    frame[*offset..*offset + value.len()].copy_from_slice(value);
    *offset += value.len();
}

fn copy_record(frame: &mut [u8], offset: &mut usize, record: &[u8]) {
    frame[*offset..*offset + record.len()].copy_from_slice(record);
    *offset += record.len();
}

/// Locate the TIM element in one bounded AP beacon.
///
/// The strict profile qualifies management objects up to 1600 bytes. Expanding
/// the maximum element count keeps this parser finite without a data-dependent
/// control-flow cycle in either the submit or TX-done root.
#[allow(unused_assignments)]
pub fn dtim(bytes: &[u8]) -> Option<(usize, u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;

    if bytes.len() < FIXED_BEACON_LENGTH
        || u16::from_le_bytes([bytes[0], bytes[1]]) & 0x00fc != 0x0080
    {
        return None;
    }
    let mut offset = FIXED_BEACON_LENGTH;
    macro_rules! inspect_element {
        () => {{
            if offset + 2 > bytes.len() {
                return None;
            }
            let id = bytes[offset];
            let length = usize::from(bytes[offset + 1]);
            let end = offset + 2 + length;
            if end > bytes.len() {
                return None;
            }
            if id == 5 {
                if length < 4 {
                    return None;
                }
                let count = bytes[offset + 2];
                let period = bytes[offset + 3];
                return (period != 0 && count < period).then_some((offset, count, period));
            }
            offset = end;
        }};
    }

    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    None
}

/// Replace the TSF, DTIM phase and group-traffic indication before HW submit.
pub fn stamp(bytes: &mut [u8], timestamp: u64, group_pending: bool) -> Option<(u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;
    if bytes.len() < FIXED_BEACON_LENGTH {
        return None;
    }
    bytes[24..32].copy_from_slice(&timestamp.to_le_bytes());

    let (tim_offset, _, period) = dtim(bytes)?;
    let interval_tu = u16::from_le_bytes([bytes[32], bytes[33]]);
    if interval_tu == 0 {
        return None;
    }
    let interval_us = u64::from(interval_tu) * 1_024;
    let phase = ((timestamp / interval_us) % u64::from(period)) as u8;
    let count = period - 1 - phase;
    bytes[tim_offset + 2] = count;

    // IEEE 802.11 TIM bitmap-control bit 0 announces buffered group traffic
    // only in a DTIM beacon. The queue itself remains Rust-owned and is
    // released by the corresponding TX-done edge.
    let bitmap_control = &mut bytes[tim_offset + 4];
    if count == 0 && group_pending {
        *bitmap_control |= 1;
    } else {
        *bitmap_control &= !1;
    }
    Some((count, period))
}

#[cfg(test)]
mod tests {
    use super::{
        ApBeaconBuildError, WPA2_BEACON_CAPACITY, WPA2_PERSONAL_CCMP_PSK_RSN_IE, dtim, stamp,
        write_wpa2_ht_beacon,
    };
    use crate::{
        channel::{WifiChannel, WifiChannelWidth},
        ssid::WifiSsid,
    };

    fn beacon() -> [u8; 44] {
        let mut bytes = [0_u8; 44];
        bytes[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
        bytes[32..34].copy_from_slice(&100_u16.to_le_bytes());
        // SSID with zero-length payload, followed by a complete TIM.
        bytes[36] = 0;
        bytes[37] = 0;
        bytes[38] = 5;
        bytes[39] = 4;
        bytes[40] = 1;
        bytes[41] = 2;
        bytes
    }

    #[test]
    fn executor_tsf_drives_dtim_and_group_indication() {
        let mut bytes = beacon();

        assert_eq!(stamp(&mut bytes, 0, true), Some((1, 2)));
        assert_eq!(dtim(&bytes), Some((38, 1, 2)));
        assert_eq!(bytes[42] & 1, 0);

        assert_eq!(stamp(&mut bytes, 100 * 1_024, true), Some((0, 2)));
        assert_eq!(dtim(&bytes), Some((38, 0, 2)));
        assert_eq!(bytes[42] & 1, 1);

        assert_eq!(stamp(&mut bytes, 3 * 100 * 1_024, false), Some((0, 2)));
        assert_eq!(bytes[42] & 1, 0);
    }

    #[test]
    fn builds_the_bounded_wpa2_ht20_beacon() {
        let ap = [0x02, 0, 0, 0, 0, 1];
        let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
        let mut bytes = [0; WPA2_BEACON_CAPACITY];
        let len = write_wpa2_ht_beacon(
            &mut bytes,
            ap,
            &ssid,
            WifiChannel::mhz20(6).unwrap(),
            100,
            2,
            0x0abc,
        )
        .unwrap();

        assert_eq!(&bytes[..2], &0x0080_u16.to_le_bytes());
        assert_eq!(&bytes[4..10], &[0xff; 6]);
        assert_eq!(&bytes[10..16], &ap);
        assert_eq!(&bytes[16..22], &ap);
        assert_eq!(&bytes[22..24], &0xabc0_u16.to_le_bytes());
        assert_eq!(dtim(&bytes[..len]), Some((64, 1, 2)));
        assert!(
            bytes[..len]
                .windows(22)
                .any(|window| window == WPA2_PERSONAL_CCMP_PSK_RSN_IE)
        );
        assert!(bytes[..len].windows(3).any(|window| window == [3, 1, 6]));
        assert!(bytes[..len].windows(2).any(|window| window == [45, 26]));
        assert!(bytes[..len].windows(3).any(|window| window == [61, 22, 6]));
    }

    #[test]
    fn ht40_beacon_advertises_the_validated_secondary_channel() {
        let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
        let mut bytes = [0; WPA2_BEACON_CAPACITY];
        let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
        let len = write_wpa2_ht_beacon(&mut bytes, [2; 6], &ssid, channel, 100, 2, 0).unwrap();
        assert!(
            bytes[..len]
                .windows(4)
                .any(|window| window == [45, 26, 0x62, 0])
        );
        assert!(
            bytes[..len]
                .windows(4)
                .any(|window| window == [61, 22, 6, 0x05])
        );
    }

    #[test]
    fn rejects_unrepresentable_beacon_policy_before_mutation() {
        let ssid = WifiSsid::new(b"ap").unwrap();
        let mut bytes = [0xaa; WPA2_BEACON_CAPACITY];
        assert_eq!(
            write_wpa2_ht_beacon(
                &mut bytes,
                [0; 6],
                &ssid,
                WifiChannel::mhz20(14).unwrap(),
                100,
                2,
                0,
            ),
            Err(ApBeaconBuildError::InvalidPrimaryChannel)
        );
        assert!(bytes.iter().all(|byte| *byte == 0xaa));
    }
}
