//! Bounded, allocation-free AP beacon field ownership.
//!
//! The S31 vendor builder derives DTIM from a ROM TSF leaf which remains zero
//! in the strict runtime. This module owns only the stateless correction:
//! locate the fixed-size TIM element and derive its phase from the executor
//! timestamp and the beacon's advertised interval.

/// Locate the TIM element in one bounded AP beacon.
///
/// The strict profile qualifies management objects up to 1600 bytes. Expanding
/// the maximum element count keeps this parser finite without a data-dependent
/// control-flow cycle in either the submit or TX-done root.
#[allow(unused_assignments)]
pub(crate) fn dtim(bytes: &[u8]) -> Option<(usize, u8, u8)> {
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
pub(crate) fn stamp(bytes: &mut [u8], timestamp: u64, group_pending: bool) -> Option<(u8, u8)> {
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
    use super::{dtim, stamp};

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
}
