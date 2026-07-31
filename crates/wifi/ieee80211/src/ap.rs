//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms were extracted from the former migration
//! `wpa2_ap` and `ap_power_save` modules. Association ownership, deferred
//! queues and wakeups stay with the radio owner.

pub const AP_ASSOCIATION_RESPONSE_BODY_LEN: usize = 103;

const AP_BGN_HT20_ASSOCIATION_RESPONSE: [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN] = [
    0x31, 0x04, 0x00, 0x00, 0x01, 0xc0, 0x01, 0x08, 0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60,
    0x32, 0x04, 0x6c, 0x12, 0x24, 0x48, 0x2a, 0x01, 0x00, 0x2d, 0x1a, 0x6e, 0x11, 0x00, 0xff, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x3d, 0x16, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xdd, 0x18, 0x00,
    0x50, 0xf2, 0x02, 0x01, 0x01, 0x04, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00, 0x00, 0x42,
    0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationResponseError {
    InvalidChannel,
    MissingAssociationId,
}

/// Build the measured ESP32-S31 B/G/N HT20 association response body.
///
/// Evidence: the captured hardware-oracle body in
/// `migration/esp32s31-hybrid-runtime/src/wpa2_ap.rs`. This is an ordinary
/// IEEE 802.11 byte transform and has no chip register dependency.
pub fn write_bgn_ht20_association_response(
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
    primary_channel: u8,
) -> Result<(), ApAssociationResponseError> {
    if !(1..=13).contains(&primary_channel) {
        return Err(ApAssociationResponseError::InvalidChannel);
    }
    if status == 0 && association_id & 0x3fff == 0 {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }
    body.copy_from_slice(&AP_BGN_HT20_ASSOCIATION_RESPONSE);
    body[2..4].copy_from_slice(&status.to_le_bytes());
    body[4..6].copy_from_slice(&if status == 0 { association_id } else { 0 }.to_le_bytes());
    body[55] = primary_channel;
    Ok(())
}

/// Update the TIM partial-virtual-bitmap byte containing an association ID.
pub const fn updated_tim_bitmap_byte(current: u8, association_id: u16, set: bool) -> u8 {
    let mask = 1_u8 << (association_id & 7);
    if set { current | mask } else { current & !mask }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPowerSaveObservation {
    Sleeping { peer: [u8; 6] },
    Active { peer: [u8; 6] },
    PsPoll { peer: [u8; 6], association_id: u16 },
}

/// Parse the RX-derived power-save edge used by the migrated AP owner.
///
/// Association validation is intentionally separate: the AP peer table must
/// confirm that `peer` currently owns the reported association ID.
pub fn observe_ap_power_save(frame: &[u8]) -> Option<ApPowerSaveObservation> {
    if frame.len() < 2 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let frame_type = (frame_control >> 2) & 3;
    let subtype = (frame_control >> 4) & 0x0f;

    if frame_type == 1 && subtype == 10 && frame.len() >= 16 {
        let raw_association_id = u16::from_le_bytes([frame[2], frame[3]]);
        let association_id = raw_association_id & 0x3fff;
        if raw_association_id & 0xc000 != 0xc000 || association_id == 0 {
            return None;
        }
        return Some(ApPowerSaveObservation::PsPoll {
            peer: frame[10..16].try_into().ok()?,
            association_id,
        });
    }

    if frame.len() < 24 {
        return None;
    }
    let to_ds = frame_control & 0x0100 != 0;
    let from_ds = frame_control & 0x0200 != 0;
    if frame_type != 2 || !to_ds || from_ds {
        return None;
    }
    let peer = frame[10..16].try_into().ok()?;
    if frame_control & 0x1000 != 0 {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn association_response_owns_status_aid_and_channel() {
        let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
        write_bgn_ht20_association_response(&mut body, 17, 0xc123, 11).unwrap();
        assert_eq!(&body[2..4], &17_u16.to_le_bytes());
        assert_eq!(&body[4..6], &[0, 0]);
        assert_eq!(body[55], 11);
        assert_eq!(
            write_bgn_ht20_association_response(&mut body, 0, 0, 1),
            Err(ApAssociationResponseError::MissingAssociationId)
        );
    }

    #[test]
    fn tim_bitmap_update_matches_aid_bit_selection() {
        assert_eq!(updated_tim_bitmap_byte(0, 0, true), 0x01);
        assert_eq!(updated_tim_bitmap_byte(0, 7, true), 0x80);
        assert_eq!(updated_tim_bitmap_byte(0xa5, 8, false), 0xa4);
        assert_eq!(updated_tim_bitmap_byte(0xa5, 15, false), 0x25);
    }

    #[test]
    fn observes_only_to_ds_data_power_state() {
        let peer = [1, 2, 3, 4, 5, 6];
        let mut frame = [0_u8; 24];
        frame[10..16].copy_from_slice(&peer);
        frame[..2].copy_from_slice(&0x1108_u16.to_le_bytes());
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::Sleeping { peer })
        );
        frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::Active { peer })
        );
        frame[..2].copy_from_slice(&0x0008_u16.to_le_bytes());
        assert_eq!(observe_ap_power_save(&frame), None);
    }

    #[test]
    fn ps_poll_owns_peer_and_association_id() {
        let peer = [1, 2, 3, 4, 5, 6];
        let mut frame = [0_u8; 16];
        frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
        frame[2..4].copy_from_slice(&0xc123_u16.to_le_bytes());
        frame[10..16].copy_from_slice(&peer);
        assert_eq!(
            observe_ap_power_save(&frame),
            Some(ApPowerSaveObservation::PsPoll {
                peer,
                association_id: 0x123
            })
        );
    }
}
