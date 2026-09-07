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
