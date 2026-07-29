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
const IEEE80211_MORE_FRAGMENTS: u8 = 0x04;
const IEEE80211_QOS_AMSDU_PRESENT: u8 = 0x80;
const QOS_NO_ACK_POLICY: u8 = 0x20;
const CALLBACK_STA_EAPOL: u32 = 1 << 3;
const CALLBACK_AP_POWER_SAVE: u32 = 1 << 12;
const RFC1042_LLC_SNAP_PREFIX: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

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

/// Bounded 802.11-to-Ethernet transform for one ordinary STA/AP MSDU.
///
/// The plan borrows no DMA storage. After [`decapsulate_data`] succeeds, the
/// caller owns a complete Ethernet frame in its output buffer and may return
/// the source RX descriptor to the radio. This is the copy-owned counterpart
/// of the slot/token boundary retained in
/// `migration/esp32s31-hybrid-runtime/src/data_rx.rs`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataDecapPlan {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub ethernet_length: usize,
}

/// One Ethernet MSDU borrowed from a validated 802.11 A-MSDU payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmsduSubframe<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: &'a [u8],
}

/// Allocation-free iterator over the subframes of one received A-MSDU.
///
/// SOURCE: IEEE 802.11 A-MSDU subframe format (DA, SA, big-endian MSDU
/// length, LLC/SNAP MSDU and four-byte padding). The need for this path was
/// confirmed by the promoted migration implementation at the parent of
/// `f233006`, `migration/esp32s31-hybrid-runtime/src/wdev.rs`, whose
/// `indicate_multi_received_frame` joins a received MPDU split across Wi-Fi
/// DMA descriptors before handing it to the upper data path.
#[derive(Clone, Debug)]
pub struct AmsduSubframes<'a> {
    remaining: &'a [u8],
    failed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataDecapError {
    Truncated,
    NotData,
    Fragmented,
    RoleMismatch,
    AmsduUnsupported,
    InvalidLlcSnap,
    OutputTooSmall { required: usize },
}

impl<'a> Iterator for AmsduSubframes<'a> {
    type Item = Result<AmsduSubframe<'a>, DataDecapError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < ETHERNET_HEADER_LEN {
            self.failed = true;
            return Some(Err(DataDecapError::Truncated));
        }
        let msdu_length = usize::from(u16::from_be_bytes([self.remaining[12], self.remaining[13]]));
        let subframe_length = match ETHERNET_HEADER_LEN.checked_add(msdu_length) {
            Some(length)
                if msdu_length >= LLC_SNAP_HEADER_LEN && length <= self.remaining.len() =>
            {
                length
            }
            _ => {
                self.failed = true;
                return Some(Err(DataDecapError::Truncated));
            }
        };
        let llc = &self.remaining[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN];
        if llc[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX {
            self.failed = true;
            return Some(Err(DataDecapError::InvalidLlcSnap));
        }
        let mut destination = [0; 6];
        destination.copy_from_slice(&self.remaining[..6]);
        let mut source = [0; 6];
        source.copy_from_slice(&self.remaining[6..12]);
        let subframe = AmsduSubframe {
            destination,
            source,
            ether_type: u16::from_be_bytes([llc[6], llc[7]]),
            payload: &self.remaining[ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN..subframe_length],
        };

        if subframe_length == self.remaining.len() {
            self.remaining = &[];
        } else {
            let padded_length = match subframe_length.checked_add(3) {
                Some(length) => length & !3,
                None => {
                    self.failed = true;
                    return Some(Err(DataDecapError::Truncated));
                }
            };
            if padded_length > self.remaining.len() {
                self.failed = true;
                return Some(Err(DataDecapError::Truncated));
            }
            self.remaining = &self.remaining[padded_length..];
        }
        Some(Ok(subframe))
    }
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

/// Validate one ordinary STA/AP data MPDU and locate its Ethernet payload.
///
/// `payload_offset` and `payload_length` describe the decrypted LLC/SNAP view
/// produced by the MAC crate. For protected frames the offset is after the
/// retained CCMP header; a hardware-consumed MIC is therefore naturally
/// excluded by `payload_length`.
pub fn plan_data_decapsulation(
    role: DataInterfaceRole,
    mpdu: &[u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<DataDecapPlan, DataDecapError> {
    if mpdu.len() < IEEE80211_LEGACY_DATA_HEADER_LEN {
        return Err(DataDecapError::Truncated);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if frame_control & 0x0003 != 0 || frame_control & 0x000c != 0x0008 {
        return Err(DataDecapError::NotData);
    }
    if mpdu[1] & IEEE80211_MORE_FRAGMENTS != 0 || mpdu[22] & 0x0f != 0 {
        return Err(DataDecapError::Fragmented);
    }

    let to_ds = mpdu[1] & IEEE80211_TO_DS != 0;
    let from_ds = mpdu[1] & IEEE80211_FROM_DS != 0;
    let (destination_offset, source_offset) = match (role, to_ds, from_ds) {
        (DataInterfaceRole::Station, false, true) => (4, 16),
        (DataInterfaceRole::AccessPoint, true, false) => (16, 10),
        _ => return Err(DataDecapError::RoleMismatch),
    };

    let qos = mpdu[0] & 0x80 != 0;
    let header_length = if qos {
        IEEE80211_QOS_DATA_HEADER_LEN
    } else {
        IEEE80211_LEGACY_DATA_HEADER_LEN
    };
    if mpdu.len() < header_length {
        return Err(DataDecapError::Truncated);
    }
    if qos && mpdu[24] & IEEE80211_QOS_AMSDU_PRESENT != 0 {
        return Err(DataDecapError::AmsduUnsupported);
    }

    let payload_end = payload_offset
        .checked_add(payload_length)
        .ok_or(DataDecapError::Truncated)?;
    if payload_offset < header_length
        || payload_length < LLC_SNAP_HEADER_LEN
        || payload_end > mpdu.len()
    {
        return Err(DataDecapError::Truncated);
    }
    let llc = &mpdu[payload_offset..payload_offset + LLC_SNAP_HEADER_LEN];
    if llc[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX {
        return Err(DataDecapError::InvalidLlcSnap);
    }

    let mut destination = [0_u8; 6];
    destination.copy_from_slice(&mpdu[destination_offset..destination_offset + 6]);
    let mut source = [0_u8; 6];
    source.copy_from_slice(&mpdu[source_offset..source_offset + 6]);
    let payload_length = payload_length - LLC_SNAP_HEADER_LEN;
    let ethernet_length = ETHERNET_HEADER_LEN
        .checked_add(payload_length)
        .ok_or(DataDecapError::Truncated)?;
    Ok(DataDecapPlan {
        destination,
        source,
        ether_type: u16::from_be_bytes([llc[6], llc[7]]),
        payload_offset: payload_offset + LLC_SNAP_HEADER_LEN,
        payload_length,
        ethernet_length,
    })
}

/// Validate the outer QoS data frame and borrow its complete A-MSDU payload.
pub fn amsdu_subframes<'a>(
    role: DataInterfaceRole,
    mpdu: &'a [u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<AmsduSubframes<'a>, DataDecapError> {
    if mpdu.len() < IEEE80211_QOS_DATA_HEADER_LEN {
        return Err(DataDecapError::Truncated);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if frame_control & 0x0003 != 0 || frame_control & 0x000c != 0x0008 {
        return Err(DataDecapError::NotData);
    }
    if mpdu[1] & IEEE80211_MORE_FRAGMENTS != 0 || mpdu[22] & 0x0f != 0 {
        return Err(DataDecapError::Fragmented);
    }
    let to_ds = mpdu[1] & IEEE80211_TO_DS != 0;
    let from_ds = mpdu[1] & IEEE80211_FROM_DS != 0;
    if !matches!(
        (role, to_ds, from_ds),
        (DataInterfaceRole::Station, false, true) | (DataInterfaceRole::AccessPoint, true, false)
    ) {
        return Err(DataDecapError::RoleMismatch);
    }
    if mpdu[0] & 0x80 == 0 || mpdu[24] & IEEE80211_QOS_AMSDU_PRESENT == 0 {
        return Err(DataDecapError::AmsduUnsupported);
    }
    let payload_end = payload_offset
        .checked_add(payload_length)
        .ok_or(DataDecapError::Truncated)?;
    if payload_offset < IEEE80211_QOS_DATA_HEADER_LEN
        || payload_length < ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN
        || payload_end > mpdu.len()
    {
        return Err(DataDecapError::Truncated);
    }
    Ok(AmsduSubframes {
        remaining: &mpdu[payload_offset..payload_end],
        failed: false,
    })
}

/// Copy one validated A-MSDU subframe into ordinary Ethernet-II storage.
pub fn decapsulate_amsdu_subframe(
    subframe: AmsduSubframe<'_>,
    ethernet: &mut [u8],
) -> Result<usize, DataDecapError> {
    let required = ETHERNET_HEADER_LEN
        .checked_add(subframe.payload.len())
        .ok_or(DataDecapError::Truncated)?;
    if ethernet.len() < required {
        return Err(DataDecapError::OutputTooSmall { required });
    }
    ethernet[..6].copy_from_slice(&subframe.destination);
    ethernet[6..12].copy_from_slice(&subframe.source);
    ethernet[12..14].copy_from_slice(&subframe.ether_type.to_be_bytes());
    ethernet[14..required].copy_from_slice(subframe.payload);
    Ok(required)
}

/// Copy one validated MSDU into caller-owned Ethernet storage.
///
/// This finite copy/validation leaf is SRAM-resident on S31 PSRAM-code
/// builds. SOURCE[HIL_OPEN_HE20_RX_RING_STARVATION_2026_07_29]: the complete
/// post-CCMP receive path executed from PSRAM plateaued at 63.1..65.3 Mbit/s
/// while the MAC reported an additional raw interrupt bit under load.
#[inline(never)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_rx_hot")
)]
pub fn decapsulate_data(
    role: DataInterfaceRole,
    mpdu: &[u8],
    payload_offset: usize,
    payload_length: usize,
    ethernet: &mut [u8],
) -> Result<DataDecapPlan, DataDecapError> {
    let plan = plan_data_decapsulation(role, mpdu, payload_offset, payload_length)?;
    if ethernet.len() < plan.ethernet_length {
        return Err(DataDecapError::OutputTooSmall {
            required: plan.ethernet_length,
        });
    }
    ethernet[..6].copy_from_slice(&plan.destination);
    ethernet[6..12].copy_from_slice(&plan.source);
    ethernet[12..14].copy_from_slice(&plan.ether_type.to_be_bytes());
    ethernet[14..plan.ethernet_length]
        .copy_from_slice(&mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length]);
    Ok(plan)
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

    #[test]
    fn station_decapsulation_reverses_from_ds_rfc1042_data() {
        let ethernet = ethernet(DESTINATION);
        let plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            BSSID,
            BSSID,
            ethernet,
            0,
            false,
            false,
        )
        .unwrap();
        let header_length = usize::from(plan.header_len);
        let payload = [1, 2, 3, 4];
        let mut mpdu = [0_u8; 64];
        mpdu[..header_length].copy_from_slice(&plan.header[..header_length]);
        mpdu[header_length..header_length + LLC_SNAP_HEADER_LEN].copy_from_slice(&plan.llc_snap);
        mpdu[header_length + LLC_SNAP_HEADER_LEN
            ..header_length + LLC_SNAP_HEADER_LEN + payload.len()]
            .copy_from_slice(&payload);
        let mpdu_length = header_length + LLC_SNAP_HEADER_LEN + payload.len();
        let mut output = [0_u8; 64];
        let decoded = decapsulate_data(
            DataInterfaceRole::Station,
            &mpdu[..mpdu_length],
            header_length,
            LLC_SNAP_HEADER_LEN + payload.len(),
            &mut output,
        )
        .unwrap();

        assert_eq!(decoded.destination, DESTINATION);
        assert_eq!(decoded.source, SOURCE);
        assert_eq!(decoded.ether_type, 0x0800);
        assert_eq!(
            &output[..decoded.ethernet_length],
            &[&ethernet[..], &payload].concat()
        );
    }

    #[test]
    fn protected_station_decapsulation_accepts_a_separate_ccmp_header() {
        let payload = [1, 2, 3, 4];
        let mut mpdu = [0_u8; 64];
        mpdu[0] = IEEE80211_DATA;
        mpdu[1] = IEEE80211_FROM_DS | 0x40;
        mpdu[4..10].copy_from_slice(&DESTINATION);
        mpdu[10..16].copy_from_slice(&BSSID);
        mpdu[16..22].copy_from_slice(&SOURCE);
        let ccmp_offset = IEEE80211_LEGACY_DATA_HEADER_LEN;
        let llc_offset = ccmp_offset + 8;
        mpdu[ccmp_offset + 3] = 0x20;
        mpdu[llc_offset..llc_offset + LLC_SNAP_HEADER_LEN]
            .copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
        mpdu[llc_offset + LLC_SNAP_HEADER_LEN..llc_offset + LLC_SNAP_HEADER_LEN + payload.len()]
            .copy_from_slice(&payload);
        let mpdu_length = llc_offset + LLC_SNAP_HEADER_LEN + payload.len();
        let mut output = [0_u8; 64];
        let decoded = decapsulate_data(
            DataInterfaceRole::Station,
            &mpdu[..mpdu_length],
            llc_offset,
            LLC_SNAP_HEADER_LEN + payload.len(),
            &mut output,
        )
        .unwrap();

        assert_eq!(decoded.ether_type, 0x0806);
        assert_eq!(&output[..6], &DESTINATION);
        assert_eq!(&output[6..12], &SOURCE);
        assert_eq!(&output[14..decoded.ethernet_length], &payload);
    }

    #[test]
    fn station_amsdu_iterator_removes_subframe_length_llc_and_padding() {
        let mut mpdu = [0_u8; 96];
        mpdu[0] = IEEE80211_QOS_DATA;
        mpdu[1] = IEEE80211_FROM_DS;
        mpdu[24] = IEEE80211_QOS_AMSDU_PRESENT;
        let mut offset = IEEE80211_QOS_DATA_HEADER_LEN;

        mpdu[offset..offset + 6].copy_from_slice(&DESTINATION);
        mpdu[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        mpdu[offset + 12..offset + 14].copy_from_slice(&10_u16.to_be_bytes());
        mpdu[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
        mpdu[offset + 22..offset + 24].copy_from_slice(&[1, 2]);
        offset += 24;

        mpdu[offset..offset + 6].copy_from_slice(&[0xff; 6]);
        mpdu[offset + 6..offset + 12].copy_from_slice(&SOURCE);
        mpdu[offset + 12..offset + 14].copy_from_slice(&11_u16.to_be_bytes());
        mpdu[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
        mpdu[offset + 22..offset + 25].copy_from_slice(&[3, 4, 5]);
        offset += 25;

        let mut subframes = amsdu_subframes(
            DataInterfaceRole::Station,
            &mpdu[..offset],
            IEEE80211_QOS_DATA_HEADER_LEN,
            offset - IEEE80211_QOS_DATA_HEADER_LEN,
        )
        .unwrap();
        let first = subframes.next().unwrap().unwrap();
        assert_eq!(first.destination, DESTINATION);
        assert_eq!(first.source, SOURCE);
        assert_eq!(first.ether_type, 0x0800);
        assert_eq!(first.payload, &[1, 2]);
        let second = subframes.next().unwrap().unwrap();
        assert_eq!(second.destination, [0xff; 6]);
        assert_eq!(second.ether_type, 0x0806);
        assert_eq!(second.payload, &[3, 4, 5]);
        assert!(subframes.next().is_none());

        let mut output = [0; 32];
        let length = decapsulate_amsdu_subframe(second, &mut output).unwrap();
        assert_eq!(length, 17);
        assert_eq!(&output[..6], &[0xff; 6]);
        assert_eq!(&output[12..17], &[0x08, 0x06, 3, 4, 5]);
    }

    #[test]
    fn decapsulation_rejects_role_mismatch_amsdu_and_non_snap_payload() {
        let mut mpdu = [0_u8; 40];
        mpdu[0] = IEEE80211_QOS_DATA;
        mpdu[1] = IEEE80211_FROM_DS;
        mpdu[24] = IEEE80211_QOS_AMSDU_PRESENT;
        mpdu[26..34].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);

        assert_eq!(
            plan_data_decapsulation(DataInterfaceRole::AccessPoint, &mpdu, 26, 8),
            Err(DataDecapError::RoleMismatch)
        );
        assert_eq!(
            plan_data_decapsulation(DataInterfaceRole::Station, &mpdu, 26, 8),
            Err(DataDecapError::AmsduUnsupported)
        );
        mpdu[24] = 0;
        mpdu[26] = 0;
        assert_eq!(
            plan_data_decapsulation(DataInterfaceRole::Station, &mpdu, 26, 8),
            Err(DataDecapError::InvalidLlcSnap)
        );
    }
}
