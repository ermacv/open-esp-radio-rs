//! Stateless geometry for the ordinary ESP32-S31 STA/AP data encapsulator.
//!
//! The field layout was reconstructed from the pinned
//! `libnet80211.a[ieee80211_output.o]::ieee80211_encap_esfbuf` body. Raw ESF,
//! descriptor, node, key, and interface accesses deliberately remain outside
//! this module. Keeping address selection, LLC/SNAP construction, QoS policy,
//! and sequence arithmetic pure makes the eventual target adapter small and
//! independently reviewable.

use crate::net80211_state::Net80211InterfaceRole;

pub(crate) const ETHERNET_HEADER_LEN: usize = 14;
pub(crate) const IEEE80211_LEGACY_DATA_HEADER_LEN: usize = 24;
pub(crate) const IEEE80211_QOS_DATA_HEADER_LEN: usize = 26;
pub(crate) const LLC_SNAP_HEADER_LEN: usize = 8;

const ETHER_TYPE_EAPOL: u16 = 0x888e;
const IEEE80211_DATA: u8 = 0x08;
const IEEE80211_QOS_DATA: u8 = 0x88;
const IEEE80211_TO_DS: u8 = 0x01;
const IEEE80211_FROM_DS: u8 = 0x02;
const QOS_NO_ACK_POLICY: u8 = 0x20;
const CALLBACK_STA_EAPOL: u32 = 1 << 3;
const CALLBACK_AP_POWER_SAVE: u32 = 1 << 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DataEncapPlan {
    /// Largest supported header storage; `header_len` selects the live prefix.
    pub header: [u8; IEEE80211_QOS_DATA_HEADER_LEN],
    pub header_len: u8,
    pub llc_snap: [u8; LLC_SNAP_HEADER_LEN],
    /// PP descriptor bit 1. An Ethernet multicast destination remains an
    /// 802.11 unicast transmission on a station, but is multicast for SoftAP.
    pub descriptor_multicast: bool,
    /// Recovered four-way WMM queue class encoded in descriptor byte `+4`.
    pub queue_class: u8,
    /// Recovered coexistence packet type selected after encapsulation.
    pub packet_type: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SequencePlan {
    pub next_counter: u16,
    pub sequence_number: u16,
    pub sequence_control: u16,
}

/// Map an 802.1D user priority to the four queue classes used by the pinned
/// descriptor builder.
///
/// The unusual numeric order is the vendor ABI: VO=0, VI=1, BE=2, BK=3.
pub(crate) const fn queue_class(priority: u8) -> Option<u8> {
    match priority {
        0 | 3 => Some(2),
        1 | 2 => Some(3),
        4 | 5 => Some(1),
        6 | 7 => Some(0),
        _ => None,
    }
}

pub(crate) const fn descriptor_priority_byte(priority: u8) -> Option<u8> {
    match queue_class(priority) {
        Some(class) => Some((class << 4) | priority),
        None => None,
    }
}

/// Select the exact completion callback mask for an ordinary STA/AP frame.
///
/// The pinned `ieee80211_output_process` writes callback bit 3 for STA EAPOL
/// before encapsulation. The ordinary AP branch of
/// `ieee80211_encap_esfbuf` adds bit 12 to every transmitted frame. Keeping
/// both decisions here prevents a recycled descriptor from retaining an
/// implicit callback owner.
pub(crate) const fn completion_callback_mask(role: Net80211InterfaceRole, ether_type: u16) -> u32 {
    match role {
        Net80211InterfaceRole::Station if ether_type == ETHER_TYPE_EAPOL => CALLBACK_STA_EAPOL,
        Net80211InterfaceRole::Station => 0,
        Net80211InterfaceRole::AccessPoint => CALLBACK_AP_POWER_SAVE,
    }
}

pub(crate) const fn advance_sequence(counter: u16) -> SequencePlan {
    let sequence_number = counter & 0x0fff;
    SequencePlan {
        next_counter: counter.wrapping_add(1),
        sequence_number,
        sequence_control: sequence_number << 4,
    }
}

/// Build the ordinary non-mesh 802.11 header and RFC 1042 LLC/SNAP prefix.
///
/// `ethernet` is the exact fourteen-byte Ethernet header. `bssid` is used by
/// a station as Address 1; `interface_mac` is used by an AP as Address 2.
/// The source address from the Ethernet header remains Address 2 for STA and
/// Address 3 for AP, matching the pinned bridge semantics.
pub(crate) const fn plan_data_encapsulation(
    role: Net80211InterfaceRole,
    bssid: [u8; 6],
    interface_mac: [u8; 6],
    ethernet: [u8; ETHERNET_HEADER_LEN],
    priority: u8,
    peer_qos: bool,
    no_ack_policy: bool,
) -> Option<DataEncapPlan> {
    let class = match queue_class(priority) {
        Some(value) => value,
        None => return None,
    };

    let mut destination = [0_u8; 6];
    let mut source = [0_u8; 6];
    let mut index = 0;
    while index != 6 {
        destination[index] = ethernet[index];
        source[index] = ethernet[index + 6];
        index += 1;
    }

    let descriptor_multicast =
        matches!(role, Net80211InterfaceRole::AccessPoint) && destination[0] & 1 != 0;
    let qos = peer_qos && !descriptor_multicast;
    let mut header = [0_u8; IEEE80211_QOS_DATA_HEADER_LEN];
    header[0] = if qos {
        IEEE80211_QOS_DATA
    } else {
        IEEE80211_DATA
    };

    match role {
        Net80211InterfaceRole::Station => {
            header[1] = IEEE80211_TO_DS;
            copy_six(&mut header, 4, bssid);
            copy_six(&mut header, 10, source);
            copy_six(&mut header, 16, destination);
        }
        Net80211InterfaceRole::AccessPoint => {
            header[1] = IEEE80211_FROM_DS;
            copy_six(&mut header, 4, destination);
            copy_six(&mut header, 10, interface_mac);
            copy_six(&mut header, 16, source);
        }
    }

    if qos {
        header[24] = priority | if no_ack_policy { QOS_NO_ACK_POLICY } else { 0 };
    }

    let packet_class = if qos { class } else { 0 };
    Some(DataEncapPlan {
        header,
        header_len: if qos {
            IEEE80211_QOS_DATA_HEADER_LEN as u8
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN as u8
        },
        llc_snap: [0xaa, 0xaa, 0x03, 0, 0, 0, ethernet[12], ethernet[13]],
        descriptor_multicast,
        queue_class: class,
        packet_type: 10 + packet_class,
    })
}

const fn copy_six(output: &mut [u8; IEEE80211_QOS_DATA_HEADER_LEN], at: usize, value: [u8; 6]) {
    let mut index = 0;
    while index != value.len() {
        output[at + index] = value[index];
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESTINATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
    const SOURCE: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
    const BSSID: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
    const AP_MAC: [u8; 6] = [0x40, 0x41, 0x42, 0x43, 0x44, 0x45];

    const fn ethernet(destination: [u8; 6]) -> [u8; ETHERNET_HEADER_LEN] {
        [
            destination[0],
            destination[1],
            destination[2],
            destination[3],
            destination[4],
            destination[5],
            SOURCE[0],
            SOURCE[1],
            SOURCE[2],
            SOURCE[3],
            SOURCE[4],
            SOURCE[5],
            0x08,
            0x00,
        ]
    }

    #[test]
    fn queue_class_and_descriptor_byte_match_every_priority() {
        let expected = [
            (2, 0x20),
            (3, 0x31),
            (3, 0x32),
            (2, 0x23),
            (1, 0x14),
            (1, 0x15),
            (0, 0x06),
            (0, 0x07),
        ];
        for (priority, (class, descriptor)) in expected.into_iter().enumerate() {
            assert_eq!(queue_class(priority as u8), Some(class));
            assert_eq!(descriptor_priority_byte(priority as u8), Some(descriptor));
        }
        assert_eq!(queue_class(8), None);
        assert_eq!(descriptor_priority_byte(0xff), None);
    }

    #[test]
    fn completion_callbacks_have_one_explicit_owner() {
        assert_eq!(
            completion_callback_mask(Net80211InterfaceRole::Station, ETHER_TYPE_EAPOL),
            1 << 3
        );
        assert_eq!(
            completion_callback_mask(Net80211InterfaceRole::Station, 0x0800),
            0
        );
        assert_eq!(
            completion_callback_mask(Net80211InterfaceRole::AccessPoint, ETHER_TYPE_EAPOL),
            1 << 12
        );
        assert_eq!(
            completion_callback_mask(Net80211InterfaceRole::AccessPoint, 0x0800),
            1 << 12
        );
    }

    #[test]
    fn station_qos_header_uses_bssid_source_and_ethernet_destination() {
        let plan = plan_data_encapsulation(
            Net80211InterfaceRole::Station,
            BSSID,
            AP_MAC,
            ethernet(DESTINATION),
            5,
            true,
            true,
        )
        .unwrap();
        assert_eq!(plan.header_len, 26);
        assert_eq!(&plan.header[..2], &[0x88, 0x01]);
        assert_eq!(&plan.header[4..10], &BSSID);
        assert_eq!(&plan.header[10..16], &SOURCE);
        assert_eq!(&plan.header[16..22], &DESTINATION);
        assert_eq!(&plan.header[24..26], &[0x25, 0]);
        assert_eq!(plan.llc_snap, [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
        assert!(!plan.descriptor_multicast);
        assert_eq!(plan.queue_class, 1);
        assert_eq!(plan.packet_type, 11);
    }

    #[test]
    fn access_point_qos_header_uses_destination_bssid_and_source() {
        let plan = plan_data_encapsulation(
            Net80211InterfaceRole::AccessPoint,
            BSSID,
            AP_MAC,
            ethernet(DESTINATION),
            6,
            true,
            false,
        )
        .unwrap();
        assert_eq!(plan.header_len, 26);
        assert_eq!(&plan.header[..2], &[0x88, 0x02]);
        assert_eq!(&plan.header[4..10], &DESTINATION);
        assert_eq!(&plan.header[10..16], &AP_MAC);
        assert_eq!(&plan.header[16..22], &SOURCE);
        assert_eq!(&plan.header[24..26], &[6, 0]);
        assert!(!plan.descriptor_multicast);
        assert_eq!(plan.packet_type, 10);
    }

    #[test]
    fn ap_multicast_disables_qos_but_sta_multicast_remains_wifi_unicast() {
        let mut multicast = DESTINATION;
        multicast[0] |= 1;
        let ap = plan_data_encapsulation(
            Net80211InterfaceRole::AccessPoint,
            BSSID,
            AP_MAC,
            ethernet(multicast),
            5,
            true,
            true,
        )
        .unwrap();
        assert_eq!(ap.header_len, 24);
        assert_eq!(ap.header[0], 0x08);
        assert!(ap.descriptor_multicast);
        assert_eq!(ap.packet_type, 10);

        let sta = plan_data_encapsulation(
            Net80211InterfaceRole::Station,
            BSSID,
            AP_MAC,
            ethernet(multicast),
            5,
            true,
            true,
        )
        .unwrap();
        assert_eq!(sta.header_len, 26);
        assert_eq!(sta.header[0], 0x88);
        assert!(!sta.descriptor_multicast);
    }

    #[test]
    fn sequence_counter_wraps_but_air_sequence_is_twelve_bits() {
        assert_eq!(
            advance_sequence(0x1abc),
            SequencePlan {
                next_counter: 0x1abd,
                sequence_number: 0x0abc,
                sequence_control: 0xabc0,
            }
        );
        assert_eq!(advance_sequence(u16::MAX).next_counter, 0);
    }
}
