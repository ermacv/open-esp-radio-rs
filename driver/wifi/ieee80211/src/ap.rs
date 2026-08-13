//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms were extracted from the former migration
//! `wpa2_ap` and `ap_power_save` modules. Association ownership, deferred
//! queues and wakeups stay with the radio owner.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{DataInterfaceRole, ETHERNET_HEADER_LEN, plan_data_encapsulation},
};

pub const AP_ASSOCIATION_RESPONSE_BODY_LEN: usize = 51;
pub const AP_AUTHENTICATION_RESPONSE_LEN: usize = 30;
pub const AP_ASSOCIATION_RESPONSE_LEN: usize = 24 + AP_ASSOCIATION_RESPONSE_BODY_LEN;

const MANAGEMENT_HEADER_LEN: usize = 24;

const AP_BG_ASSOCIATION_RESPONSE: [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN] = [
    0x31, 0x04, 0x00, 0x00, 0x01, 0xc0, 0x01, 0x08, 0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60,
    0x32, 0x04, 0x6c, 0x12, 0x24, 0x48, 0x2a, 0x01, 0x00, 0xdd, 0x18, 0x00, 0x50, 0xf2, 0x02, 0x01,
    0x01, 0x04, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4, 0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62,
    0x32, 0x2f, 0x00,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationResponseError {
    MissingAssociationId,
    InvalidSequenceNumber,
    OutputTooSmall { required: usize },
}

/// Invalid AP data-frame construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApDataFrameError {
    InvalidAccessPoint,
    InvalidPeer,
    InvalidSequenceNumber,
    InvalidUserPriority,
    EthernetFrameTooShort,
    OutputTooSmall { required: usize },
}

/// One unprotected 802.11 data MPDU sent by an AP to its client.
///
/// The initial AP uses this for EAPOL before the controlled port opens. The
/// frame shape is portable; descriptor metadata and queue selection remain in
/// the chip TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApDataFrame<'payload> {
    pub access_point: [u8; 6],
    pub destination: [u8; 6],
    pub sequence_number: u16,
    pub ether_type: u16,
    pub payload: &'payload [u8],
}

impl ApDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        let mut ethernet = [0; ETHERNET_HEADER_LEN];
        ethernet[..6].copy_from_slice(&self.destination);
        ethernet[6..12].copy_from_slice(&self.access_point);
        ethernet[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        let plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet,
            7,
            false,
            false,
        )
        .expect("priority seven is a valid recovered queue class");
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(plan.llc_snap.len())
            .and_then(|length| length.checked_add(self.payload.len()))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// One protected Ethernet-II frame sent from an AP to its authorized peer.
///
/// The caller owns the pairwise packet number. This codec merely places the
/// supplied CCMP header in the DMA image; ESP32-S31 hardware encrypts the
/// plaintext LLC/SNAP payload and appends the MIC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: u16,
    pub user_priority: u8,
    pub peer_qos: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ethernet: &'frame [u8],
}

impl ApProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.sequence_number > 0x0fff {
            return Err(ApDataFrameError::InvalidSequenceNumber);
        }
        if self.user_priority > 7 {
            return Err(ApDataFrameError::InvalidUserPriority);
        }
        if self.ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        // The initial AP owns one pairwise slot. Group-addressed Ethernet
        // needs the distinct GTK packet-number owner and is rejected by the
        // chip engine before this codec is called.
        let mut ethernet_header = [0; ETHERNET_HEADER_LEN];
        ethernet_header.copy_from_slice(&self.ethernet[..ETHERNET_HEADER_LEN]);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            self.user_priority,
            self.peer_qos,
            false,
        )
        .ok_or(ApDataFrameError::InvalidUserPriority)?;
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        plan.header[1] |= 0x40;
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(CCMP_HEADER_LEN)
            .and_then(|length| length.checked_add(plan.llc_snap.len()))
            .and_then(|length| length.checked_add(self.ethernet.len() - ETHERNET_HEADER_LEN))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&(self.sequence_number << 4).to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        let llc_end = ccmp_end + plan.llc_snap.len();
        frame[ccmp_end..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(&self.ethernet[ETHERNET_HEADER_LEN..]);
        Ok(required)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRequest<'a> {
    OpenAuthentication {
        peer: [u8; 6],
    },
    Association {
        peer: [u8; 6],
        rsn_ie: Option<&'a [u8]>,
        /// Highest legacy rate shared with the B/G ERP rate set advertised
        /// by this AP, in 500-kbit/s units. Zero means no common rate.
        maximum_legacy_rate_500kbps: u8,
    },
    Disassociation {
        peer: [u8; 6],
        reason: u16,
    },
    Deauthentication {
        peer: [u8; 6],
        reason: u16,
    },
}

/// Parse only management requests addressed to this AP.
///
/// Beacons, responses, foreign BSS traffic and unsupported authentication
/// algorithms are intentionally ignored rather than being promoted into AP
/// state transitions.
pub fn parse_ap_management_request<'a>(
    frame: &'a [u8],
    access_point: [u8; 6],
) -> Option<ApManagementRequest<'a>> {
    if frame.len() < MANAGEMENT_HEADER_LEN
        || frame[4..10] != access_point
        || frame[16..22] != access_point
    {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & 0x000c != 0 {
        return None;
    }
    let subtype = (frame_control >> 4) & 0x0f;
    let peer = frame[10..16].try_into().ok()?;
    match subtype {
        11 => {
            let body = frame.get(24..30)?;
            let algorithm = u16::from_le_bytes([body[0], body[1]]);
            let transaction = u16::from_le_bytes([body[2], body[3]]);
            let status = u16::from_le_bytes([body[4], body[5]]);
            (algorithm == 0 && transaction == 1 && status == 0)
                .then_some(ApManagementRequest::OpenAuthentication { peer })
        }
        0 => {
            let information_elements = frame.get(28..)?;
            Some(ApManagementRequest::Association {
                peer,
                rsn_ie: find_information_element(information_elements, 48),
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(information_elements),
            })
        }
        10 | 12 => {
            let body = frame.get(24..26)?;
            let reason = u16::from_le_bytes([body[0], body[1]]);
            if subtype == 10 {
                Some(ApManagementRequest::Disassociation { peer, reason })
            } else {
                Some(ApManagementRequest::Deauthentication { peer, reason })
            }
        }
        _ => None,
    }
}

const AP_BG_LEGACY_RATES_500KBPS: [u8; 12] = [2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108];

fn maximum_ap_legacy_rate(bytes: &[u8]) -> u8 {
    let mut maximum = 0;
    let mut remaining = bytes;
    while let Some((&id, tail)) = remaining.split_first() {
        let Some((&length, payload)) = tail.split_first() else {
            break;
        };
        let length = usize::from(length);
        let Some(value) = payload.get(..length) else {
            break;
        };
        if id == 1 || id == 50 {
            for encoded in value {
                let rate = encoded & 0x7f;
                if AP_BG_LEGACY_RATES_500KBPS.contains(&rate) {
                    maximum = maximum.max(rate);
                }
            }
        }
        remaining = &payload[length..];
    }
    maximum
}

fn find_information_element(bytes: &[u8], wanted: u8) -> Option<&[u8]> {
    let mut remaining = bytes;
    while let Some((&id, tail)) = remaining.split_first() {
        let (&length, payload) = tail.split_first()?;
        let record_len = usize::from(length).checked_add(2)?;
        let record = remaining.get(..record_len)?;
        if id == wanted {
            return Some(record);
        }
        remaining = payload.get(usize::from(length)..)?;
    }
    None
}

/// Encode the response to one successful or rejected Open System request.
pub fn write_open_authentication_response(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    management_sequence: u16,
) -> Result<usize, ApAssociationResponseError> {
    if management_sequence > 0x0fff {
        return Err(ApAssociationResponseError::InvalidSequenceNumber);
    }
    if output.len() < AP_AUTHENTICATION_RESPONSE_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_AUTHENTICATION_RESPONSE_LEN,
        });
    }
    let frame = &mut output[..AP_AUTHENTICATION_RESPONSE_LEN];
    frame.fill(0);
    write_management_header(frame, 0x00b0, access_point, peer, management_sequence);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    Ok(AP_AUTHENTICATION_RESPONSE_LEN)
}

/// Encode the AP-v1 B/G ERP association response as one complete MPDU.
pub fn write_bg_association_response_frame(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    association_id: u16,
    management_sequence: u16,
) -> Result<usize, ApAssociationResponseError> {
    if management_sequence > 0x0fff {
        return Err(ApAssociationResponseError::InvalidSequenceNumber);
    }
    if output.len() < AP_ASSOCIATION_RESPONSE_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        });
    }
    let frame = &mut output[..AP_ASSOCIATION_RESPONSE_LEN];
    frame.fill(0);
    write_management_header(frame, 0x0010, access_point, peer, management_sequence);
    let body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN] = (&mut frame[MANAGEMENT_HEADER_LEN..])
        .try_into()
        .expect("checked association response body length");
    write_bg_association_response(body, status, association_id)?;
    Ok(AP_ASSOCIATION_RESPONSE_LEN)
}

fn write_management_header(
    frame: &mut [u8],
    frame_control: u16,
    access_point: [u8; 6],
    peer: [u8; 6],
    management_sequence: u16,
) {
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&peer);
    frame[10..16].copy_from_slice(&access_point);
    frame[16..22].copy_from_slice(&access_point);
    frame[22..24].copy_from_slice(&(management_sequence << 4).to_le_bytes());
}

/// Build the finite AP-v1 B/G ERP association response body.
///
/// HT records are intentionally absent until AP RX can atomically hand every
/// A-MSDU subframe to the network owner. This is an ordinary IEEE 802.11 byte
/// transform and has no chip register dependency.
pub fn write_bg_association_response(
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
) -> Result<(), ApAssociationResponseError> {
    if status == 0 && association_id & 0x3fff == 0 {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }
    body.copy_from_slice(&AP_BG_ASSOCIATION_RESPONSE);
    body[2..4].copy_from_slice(&status.to_le_bytes());
    let encoded_association_id = if status == 0 {
        0xc000 | (association_id & 0x3fff)
    } else {
        0
    };
    body[4..6].copy_from_slice(&encoded_association_id.to_le_bytes());
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
    fn ap_eapol_data_frame_uses_from_ds_address_mapping() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut output = [0; 64];
        let len = ApDataFrame {
            access_point,
            destination: peer,
            sequence_number: 9,
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(len, 24 + 8 + 3);
        assert_eq!(&output[..2], &0x0208_u16.to_le_bytes());
        assert_eq!(&output[4..10], &peer);
        assert_eq!(&output[10..16], &access_point);
        assert_eq!(&output[16..22], &access_point);
        assert_eq!(&output[22..24], &0x0090_u16.to_le_bytes());
        assert_eq!(&output[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        assert_eq!(&output[32..35], &[1, 2, 3]);
    }

    #[test]
    fn protected_ap_frame_owns_from_ds_ccmp_and_plaintext_boundary() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let source = [2, 0, 0, 0, 0, 3];
        let mut ethernet = [0_u8; 18];
        ethernet[..6].copy_from_slice(&peer);
        ethernet[6..12].copy_from_slice(&source);
        ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
        let ccmp = [3, 0, 0, 0x20, 0, 0, 0, 0];
        let mut output = [0; 96];
        let len = ApProtectedDataFrame {
            access_point,
            peer,
            sequence_number: 7,
            user_priority: 0,
            peer_qos: true,
            ccmp_header: ccmp,
            ethernet: &ethernet,
        }
        .encode(&mut output)
        .unwrap();

        assert_eq!(len, 26 + 8 + 8 + 4);
        assert_eq!(&output[..2], &0x4288_u16.to_le_bytes());
        assert_eq!(&output[4..10], &peer);
        assert_eq!(&output[10..16], &access_point);
        assert_eq!(&output[16..22], &source);
        assert_eq!(&output[22..24], &0x0070_u16.to_le_bytes());
        assert_eq!(&output[26..34], &ccmp);
        assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
        assert_eq!(&output[42..46], &[1, 2, 3, 4]);
    }

    #[test]
    fn protected_ap_frame_rejects_a_destination_outside_pairwise_owner() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut ethernet = [0_u8; 14];
        ethernet[..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
        let mut output = [0; 64];
        assert_eq!(
            ApProtectedDataFrame {
                access_point,
                peer,
                sequence_number: 0,
                user_priority: 0,
                peer_qos: false,
                ccmp_header: [0; 8],
                ethernet: &ethernet,
            }
            .encode(&mut output),
            Err(ApDataFrameError::InvalidPeer)
        );
    }

    #[test]
    fn association_response_owns_status_and_aid_without_false_ht_capability() {
        let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
        write_bg_association_response(&mut body, 17, 0x0123).unwrap();
        assert_eq!(&body[2..4], &17_u16.to_le_bytes());
        assert_eq!(&body[4..6], &[0, 0]);
        write_bg_association_response(&mut body, 0, 1).unwrap();
        assert_eq!(&body[4..6], &0xc001_u16.to_le_bytes());
        assert!(!body.windows(2).any(|window| window == [45, 26]));
        assert!(!body.windows(2).any(|window| window == [61, 22]));
        assert_eq!(
            write_bg_association_response(&mut body, 0, 0),
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

    #[test]
    fn parses_only_requests_for_the_owned_bssid() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut authentication = [0_u8; 30];
        authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
        authentication[4..10].copy_from_slice(&access_point);
        authentication[10..16].copy_from_slice(&peer);
        authentication[16..22].copy_from_slice(&access_point);
        authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            parse_ap_management_request(&authentication, access_point),
            Some(ApManagementRequest::OpenAuthentication { peer })
        );
        authentication[4] ^= 1;
        assert_eq!(
            parse_ap_management_request(&authentication, access_point),
            None
        );
    }

    #[test]
    fn association_retains_the_highest_common_advertised_legacy_rate() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut association = [0_u8; 42];
        association[4..10].copy_from_slice(&access_point);
        association[10..16].copy_from_slice(&peer);
        association[16..22].copy_from_slice(&access_point);
        association[28..34].copy_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x30]);
        association[34..39].copy_from_slice(&[50, 3, 0x48, 0x6c, 0x7f]);
        association[39..42].copy_from_slice(&[48, 1, 0]);
        assert_eq!(
            parse_ap_management_request(&association, access_point),
            Some(ApManagementRequest::Association {
                peer,
                rsn_ie: Some(&association[39..42]),
                maximum_legacy_rate_500kbps: 108,
            })
        );
    }

    #[test]
    fn complete_response_encoders_own_addresses_sequence_and_status() {
        let access_point = [2, 0, 0, 0, 0, 1];
        let peer = [2, 0, 0, 0, 0, 2];
        let mut authentication = [0xaa; AP_AUTHENTICATION_RESPONSE_LEN];
        write_open_authentication_response(&mut authentication, access_point, peer, 17, 7).unwrap();
        assert_eq!(&authentication[4..10], &peer);
        assert_eq!(&authentication[10..16], &access_point);
        assert_eq!(&authentication[26..28], &2_u16.to_le_bytes());
        assert_eq!(&authentication[28..30], &17_u16.to_le_bytes());

        let mut association = [0; AP_ASSOCIATION_RESPONSE_LEN];
        write_bg_association_response_frame(&mut association, access_point, peer, 0, 0xc001, 8)
            .unwrap();
        assert_eq!(&association[..2], &0x0010_u16.to_le_bytes());
        assert_eq!(&association[22..24], &0x0080_u16.to_le_bytes());
        assert_eq!(&association[28..30], &0xc001_u16.to_le_bytes());
    }
}
