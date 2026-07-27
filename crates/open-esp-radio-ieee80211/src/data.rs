//! Stateless ordinary STA/AP data encapsulation.
//!
//! This is the live, chip-independent extraction of the finite policy
//! formerly held in `migration/.../net80211_encap.rs`. Raw ESF, descriptor,
//! node, key, and interface accesses deliberately remain outside this module.

pub const ETHERNET_HEADER_LEN: usize = 14;
pub const IEEE80211_LEGACY_DATA_HEADER_LEN: usize = 24;
pub const IEEE80211_QOS_DATA_HEADER_LEN: usize = 26;
pub const LLC_SNAP_HEADER_LEN: usize = 8;

const ETHER_TYPE_EAPOL: u16 = 0x888e;
const IEEE80211_DATA: u8 = 0x08;
const IEEE80211_QOS_DATA: u8 = 0x88;
const IEEE80211_TO_DS: u8 = 0x01;
const IEEE80211_FROM_DS: u8 = 0x02;
const QOS_NO_ACK_POLICY: u8 = 0x20;
const CALLBACK_STA_EAPOL: u32 = 1 << 3;
const CALLBACK_AP_POWER_SAVE: u32 = 1 << 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataInterfaceRole {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataEncapPlan {
    pub header: [u8; IEEE80211_QOS_DATA_HEADER_LEN],
    pub header_len: u8,
    pub llc_snap: [u8; LLC_SNAP_HEADER_LEN],
    pub descriptor_multicast: bool,
    pub queue_class: u8,
    pub packet_type: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequencePlan {
    pub next_counter: u16,
    pub sequence_number: u16,
    pub sequence_control: u16,
}

/// Map 802.1D user priority to the recovered vendor queue numbering:
/// VO=0, VI=1, BE=2, BK=3.
pub const fn queue_class(priority: u8) -> Option<u8> {
    match priority {
        0 | 3 => Some(2),
        1 | 2 => Some(3),
        4 | 5 => Some(1),
        6 | 7 => Some(0),
        _ => None,
    }
}

pub const fn descriptor_priority_byte(priority: u8) -> Option<u8> {
    match queue_class(priority) {
        Some(class) => Some((class << 4) | priority),
        None => None,
    }
}

pub const fn completion_callback_mask(role: DataInterfaceRole, ether_type: u16) -> u32 {
    match role {
        DataInterfaceRole::Station if ether_type == ETHER_TYPE_EAPOL => CALLBACK_STA_EAPOL,
        DataInterfaceRole::Station => 0,
        DataInterfaceRole::AccessPoint => CALLBACK_AP_POWER_SAVE,
    }
}

pub const fn advance_sequence(counter: u16) -> SequencePlan {
    let sequence_number = counter & 0x0fff;
    SequencePlan {
        next_counter: counter.wrapping_add(1),
        sequence_number,
        sequence_control: sequence_number << 4,
    }
}

pub const fn plan_data_encapsulation(
    role: DataInterfaceRole,
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
        matches!(role, DataInterfaceRole::AccessPoint) && destination[0] & 1 != 0;
    let qos = peer_qos && !descriptor_multicast;
    let mut header = [0_u8; IEEE80211_QOS_DATA_HEADER_LEN];
    header[0] = if qos {
        IEEE80211_QOS_DATA
    } else {
        IEEE80211_DATA
    };

    match role {
        DataInterfaceRole::Station => {
            header[1] = IEEE80211_TO_DS;
            copy_six(&mut header, 4, bssid);
            copy_six(&mut header, 10, source);
            copy_six(&mut header, 16, destination);
        }
        DataInterfaceRole::AccessPoint => {
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
    }

    #[test]
    fn station_qos_header_matches_the_recovered_plan() {
        let plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            BSSID,
            AP_MAC,
            ethernet(DESTINATION),
            7,
            true,
            false,
        )
        .unwrap();
        assert_eq!(plan.header_len, 26);
        assert_eq!(&plan.header[..2], &[0x88, 0x01]);
        assert_eq!(&plan.header[4..10], &BSSID);
        assert_eq!(&plan.header[10..16], &SOURCE);
        assert_eq!(&plan.header[16..22], &DESTINATION);
        assert_eq!(&plan.header[24..26], &[7, 0]);
        assert_eq!(plan.queue_class, 0);
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
    }
}
