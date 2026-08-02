//! Bounded infrastructure-beacon observation for a connected station.
//!
//! This parser owns IEEE 802.11 fields only. It has no clock, executor,
//! hardware or sleep policy; the connected runtime decides how a received
//! observation changes its beacon-loss deadline and power state.

const BEACON_FRAME_CONTROL: u16 = 0x0080;
const FRAME_TYPE_AND_SUBTYPE_MASK: u16 = 0x00fc;
const FIXED_BEACON_LENGTH: usize = 36;
const TIM_ELEMENT_ID: u8 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconError {
    Truncated,
    NotBeacon,
    ForeignBssid,
    ZeroInterval,
    MalformedInformationElement,
    MalformedTim,
}

/// Traffic indication decoded for the connected station's Association ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTimObservation {
    pub dtim_count: u8,
    pub dtim_period: u8,
    pub unicast_buffered: bool,
    pub group_buffered: bool,
}

/// Fixed connected-beacon fields that may outlive the borrowed MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconObservation {
    pub timestamp_tsf: u64,
    pub interval_tu: u16,
    pub capability_information: u16,
    pub tim: Option<StaTimObservation>,
}

/// Parse one beacon from the associated BSS and locate the station's TIM bit.
pub fn parse_sta_beacon(
    mpdu: &[u8],
    expected_bssid: [u8; 6],
    association_id: u16,
) -> Result<StaBeaconObservation, StaBeaconError> {
    if mpdu.len() < FIXED_BEACON_LENGTH {
        return Err(StaBeaconError::Truncated);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if frame_control & FRAME_TYPE_AND_SUBTYPE_MASK != BEACON_FRAME_CONTROL {
        return Err(StaBeaconError::NotBeacon);
    }
    if mpdu[10..16] != expected_bssid || mpdu[16..22] != expected_bssid {
        return Err(StaBeaconError::ForeignBssid);
    }
    let interval_tu = u16::from_le_bytes([mpdu[32], mpdu[33]]);
    if interval_tu == 0 {
        return Err(StaBeaconError::ZeroInterval);
    }

    let mut timestamp = [0_u8; 8];
    timestamp.copy_from_slice(&mpdu[24..32]);
    let mut tim = None;
    let mut offset = FIXED_BEACON_LENGTH;
    while offset < mpdu.len() {
        let header = mpdu
            .get(offset..offset + 2)
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        let length = usize::from(header[1]);
        let body_start = offset + 2;
        let body_end = body_start
            .checked_add(length)
            .filter(|end| *end <= mpdu.len())
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        if header[0] == TIM_ELEMENT_ID {
            let body = &mpdu[body_start..body_end];
            if body.len() < 4 || body[1] == 0 || tim.is_some() {
                return Err(StaBeaconError::MalformedTim);
            }
            let bitmap_control = body[2];
            let aid = usize::from(association_id & 0x3fff);
            let aid_byte = aid / 8;
            // Bitmap Offset is encoded in units of two octets in bits 7:1;
            // masking bit zero therefore already yields N1 in octets.
            let bitmap_offset = usize::from(bitmap_control & 0xfe);
            let unicast_buffered = aid_byte
                .checked_sub(bitmap_offset)
                .and_then(|index| body.get(3 + index))
                .is_some_and(|byte| byte & (1 << (aid & 7)) != 0);
            tim = Some(StaTimObservation {
                dtim_count: body[0],
                dtim_period: body[1],
                unicast_buffered,
                group_buffered: body[0] == 0 && bitmap_control & 1 != 0,
            });
        }
        offset = body_end;
    }

    Ok(StaBeaconObservation {
        timestamp_tsf: u64::from_le_bytes(timestamp),
        interval_tu,
        capability_information: u16::from_le_bytes([mpdu[34], mpdu[35]]),
        tim,
    })
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];

    fn beacon(tim: &[u8]) -> std::vec::Vec<u8> {
        let mut frame = std::vec![0_u8; FIXED_BEACON_LENGTH];
        frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..32].copy_from_slice(&0x0102_0304_0506_0708_u64.to_le_bytes());
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(tim);
        frame
    }

    #[test]
    fn decodes_dtim_group_and_partial_virtual_bitmap_for_local_aid() {
        // AID 17 lives in byte two, bit one. Bitmap offset one is encoded as
        // two in bits 7:1 and therefore body bitmap byte zero names AID16..23.
        let frame = beacon(&[TIM_ELEMENT_ID, 4, 0, 3, 0x03, 0x02]);
        assert_eq!(
            parse_sta_beacon(&frame, BSSID, 17),
            Ok(StaBeaconObservation {
                timestamp_tsf: 0x0102_0304_0506_0708,
                interval_tu: 100,
                capability_information: 0x0431,
                tim: Some(StaTimObservation {
                    dtim_count: 0,
                    dtim_period: 3,
                    unicast_buffered: true,
                    group_buffered: true,
                }),
            })
        );
    }

    #[test]
    fn rejects_foreign_or_structurally_incomplete_beacons() {
        let mut foreign = beacon(&[TIM_ELEMENT_ID, 4, 1, 3, 0, 0]);
        foreign[10] ^= 1;
        assert_eq!(
            parse_sta_beacon(&foreign, BSSID, 1),
            Err(StaBeaconError::ForeignBssid)
        );

        let malformed = beacon(&[TIM_ELEMENT_ID, 5, 1, 3, 0, 0]);
        assert_eq!(
            parse_sta_beacon(&malformed, BSSID, 1),
            Err(StaBeaconError::MalformedInformationElement)
        );
    }
}
