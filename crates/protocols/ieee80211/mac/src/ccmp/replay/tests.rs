//! The replay check of an association's keys.

use super::*;
use crate::ccmp::CcmpPacketNumber;

fn header(packet_number: u64, key_id: u8) -> CcmpHeader {
    CcmpHeader::new(
        CcmpPacketNumber::new(packet_number).unwrap(),
        CcmpKeyId::new(key_id).unwrap(),
    )
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap()
}

#[test]
fn a_frame_sent_again_is_refused_in_its_lane_and_other_lanes_go_on() {
    let mut epoch = epoch();
    assert_eq!(epoch.check(CcmpRxProtection::Pairwise, Some(0), header(5, 0)), Ok(()));
    assert!(matches!(
        epoch.check(CcmpRxProtection::Pairwise, Some(0), header(5, 0)),
        Err(CcmpRxReplayError::Replay(CcmpReplayError::Replayed { .. }))
    ));
    // A lower number in its lane is refused too; another TID's and the
    // non-QoS lane's go on.
    assert!(epoch.check(CcmpRxProtection::Pairwise, Some(0), header(4, 0)).is_err());
    assert_eq!(epoch.check(CcmpRxProtection::Pairwise, Some(6), header(1, 0)), Ok(()));
    assert_eq!(epoch.check(CcmpRxProtection::Pairwise, None, header(1, 0)), Ok(()));
}

#[test]
fn a_new_group_key_starts_its_frontiers_from_its_receive_sequence() {
    let mut epoch = epoch();
    assert_eq!(epoch.check(CcmpRxProtection::Group, None, header(900, 1)), Ok(()));
    // The access point's new group key, index 2, sent from packet number 1.
    let replacement = epoch.prepare_group_rotation(2, [0; 8]).unwrap();
    epoch.commit_group_rotation(replacement).unwrap();
    assert_eq!(epoch.check(CcmpRxProtection::Group, None, header(1, 2)), Ok(()));
    // The old key's frames are refused.
    assert!(matches!(
        epoch.check(CcmpRxProtection::Group, None, header(901, 1)),
        Err(CcmpRxReplayError::UnexpectedKeyId { .. })
    ));
}

#[test]
fn the_same_group_key_installed_again_never_lowers_its_frontiers() {
    let mut epoch = epoch();
    assert_eq!(epoch.check(CcmpRxProtection::Group, None, header(900, 1)), Ok(()));
    let replacement = epoch.prepare_group_rotation(1, [0; 8]).unwrap();
    epoch.commit_group_rotation(replacement).unwrap();
    assert!(epoch.check(CcmpRxProtection::Group, None, header(900, 1)).is_err());
    assert_eq!(epoch.check(CcmpRxProtection::Group, None, header(901, 1)), Ok(()));
}
