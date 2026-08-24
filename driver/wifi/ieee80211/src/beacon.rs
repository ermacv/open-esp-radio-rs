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
pub const TIM_MAX_ASSOCIATION_ID: u16 = 2_007;
pub const TIM_MAX_VIRTUAL_BITMAP_OCTETS: usize = 251;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimBitmapError {
    InvalidCapacity {
        bitmap_octets: usize,
    },
    InvalidAssociationId(u16),
    AssociationIdOutsideCapacity {
        association_id: u16,
        bitmap_octets: usize,
    },
}

/// Valid unicast association identifier for an IEEE 802.11 TIM bitmap.
///
/// AID zero is represented by the multicast indication in Bitmap Control and
/// is therefore intentionally not constructible through this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TimAssociationId(u16);

impl TimAssociationId {
    pub const fn new(value: u16) -> Result<Self, TimBitmapError> {
        if value == 0 || value > TIM_MAX_ASSOCIATION_ID {
            return Err(TimBitmapError::InvalidAssociationId(value));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Fixed-capacity owner for the complete virtual bitmap in one AP profile.
///
/// `OCTETS` is validated when the value is constructed. Setting an AID beyond
/// that profile fails explicitly instead of aliasing it through `aid & 7`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimVirtualBitmap<const OCTETS: usize> {
    octets: [u8; OCTETS],
}

impl<const OCTETS: usize> TimVirtualBitmap<OCTETS> {
    pub const fn try_new() -> Result<Self, TimBitmapError> {
        if OCTETS == 0 || OCTETS > TIM_MAX_VIRTUAL_BITMAP_OCTETS {
            return Err(TimBitmapError::InvalidCapacity {
                bitmap_octets: OCTETS,
            });
        }
        Ok(Self {
            octets: [0; OCTETS],
        })
    }

    pub fn set(
        &mut self,
        association_id: TimAssociationId,
        buffered: bool,
    ) -> Result<(), TimBitmapError> {
        let association_id = association_id.get();
        let octet = usize::from(association_id / 8);
        let Some(value) = self.octets.get_mut(octet) else {
            return Err(TimBitmapError::AssociationIdOutsideCapacity {
                association_id,
                bitmap_octets: OCTETS,
            });
        };
        let mask = 1_u8 << (association_id % 8);
        if buffered {
            *value |= mask;
        } else {
            *value &= !mask;
        }
        Ok(())
    }

    /// Derive canonical N1/N2 bounds for the Partial Virtual Bitmap field.
    /// N1 is even, so a set bit in odd octet 1 still retains octet 0.
    pub fn partial(&self) -> TimPartialVirtualBitmap<'_> {
        let Some(first) = self.octets.iter().position(|octet| *octet != 0) else {
            return TimPartialVirtualBitmap {
                bitmap_offset: 0,
                octets: &self.octets[..1],
            };
        };
        let last = self
            .octets
            .iter()
            .rposition(|octet| *octet != 0)
            .expect("a first nonzero TIM octet has a last nonzero octet");
        let first = first & !1;
        TimPartialVirtualBitmap {
            bitmap_offset: first as u8,
            octets: &self.octets[first..=last],
        }
    }
}

/// Borrowed canonical Partial Virtual Bitmap and its absolute even N1 offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimPartialVirtualBitmap<'bitmap> {
    bitmap_offset: u8,
    octets: &'bitmap [u8],
}

impl<'bitmap> TimPartialVirtualBitmap<'bitmap> {
    pub const fn bitmap_offset(self) -> u8 {
        self.bitmap_offset
    }

    pub const fn octets(self) -> &'bitmap [u8] {
        self.octets
    }
}

/// Build one visible WPA2-Personal HT/WMM beacon without allocating.
///
/// The caller owns timestamp/DTIM progression through [`stamp`]. This builder
/// publishes the fixed first-AP profile only: 100 TU, a two-byte TIM bitmap
/// covering the complete public AID 1..=15 range, CCMP, PSK, WMM and coherent
/// one-stream HT capability records.
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
        + 7
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
    // Bitmap offset zero plus two octets covers AID 1..=15 without aliasing
    // AID 8..=15 onto the first byte. Bit zero of bitmap control remains the
    // independent DTIM multicast indication maintained by `stamp`.
    write_element(
        frame,
        &mut offset,
        5,
        &[dtim_period - 1, dtim_period, 0, 0, 0],
    );
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

/// Replace the TIM Partial Virtual Bitmap while preserving every following
/// information element in the bounded beacon owner.
///
/// The returned length reflects canonical one- through 251-octet N1/N2
/// compression. Capacity and all existing TIM bounds are checked before the
/// frame is moved, so `None` leaves `storage` unchanged.
pub fn write_tim_partial_virtual_bitmap(
    storage: &mut [u8],
    frame_len: usize,
    partial: TimPartialVirtualBitmap<'_>,
) -> Option<usize> {
    let frame = storage.get(..frame_len)?;
    let (tim_offset, _, _) = dtim(frame)?;
    let current_body_len = usize::from(*frame.get(tim_offset + 1)?);
    if current_body_len < 4 || partial.octets.is_empty() {
        return None;
    }
    let current_end = tim_offset.checked_add(2 + current_body_len)?;
    if current_end > frame_len {
        return None;
    }
    let desired_body_len = 3_usize.checked_add(partial.octets.len())?;
    let desired_body_len_u8 = u8::try_from(desired_body_len).ok()?;
    let new_len = if desired_body_len >= current_body_len {
        frame_len.checked_add(desired_body_len - current_body_len)?
    } else {
        frame_len.checked_sub(current_body_len - desired_body_len)?
    };
    if new_len > storage.len() {
        return None;
    }

    let desired_end = tim_offset + 2 + desired_body_len;
    if desired_end > current_end {
        storage.copy_within(current_end..frame_len, desired_end);
    } else if desired_end < current_end {
        storage.copy_within(current_end..frame_len, desired_end);
    }

    storage[tim_offset + 1] = desired_body_len_u8;
    let group_indication = storage[tim_offset + 4] & 1;
    storage[tim_offset + 4] = group_indication | partial.bitmap_offset;
    storage[tim_offset + 5..desired_end].copy_from_slice(partial.octets);
    if new_len < frame_len {
        storage[new_len..frame_len].fill(0);
    }
    Some(new_len)
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
                .any(|window| window == [45, 26, 0x6e, 0x10])
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
