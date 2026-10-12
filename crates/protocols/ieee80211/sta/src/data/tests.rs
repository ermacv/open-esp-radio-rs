//! The data machine's encapsulation, tokens and admission.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    Ieee80211Capabilities, LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter,
    RxEvidence, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use std::vec::Vec;

struct Caps;

impl Ieee80211Capabilities for Caps {
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: false,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        max_backoff_slots: 1023,
        tx_power_ceiling_min_dbm: None,
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        key_slots: 4,
        rx_block_ack_agreements: 0,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

struct Buffer(Vec<u8>);

impl TxBufferMemory for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

struct Data;

impl StaDataTokens for Data {
    type Ticket = u8;
    type Buffer = Buffer;
}

struct Station;

impl StaTokens for Station {
    type Ticket = u8;
    type SmallBuffer = Buffer;
    type PairwiseSlot = ();
    type GroupSlot = ();
    type PairwiseKey = ();
    type GroupKey = ();
}

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 3];

fn link<'a>(qos: bool, key: Option<&'a ()>, transmit: bool) -> LinkView<'a, Station> {
    LinkView {
        address: LOCAL,
        bssid: BSSID,
        association_id: 1,
        qos,
        pairwise: key,
        transmit,
    }
}

fn machine() -> StaData<Data> {
    StaData::new([1, 2], [Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600])])
}

fn msdu(priority: u8) -> Msdu<'static> {
    Msdu {
        destination: PEER,
        ether_type: 0x0800,
        priority,
        payload: &[0xab; 100],
    }
}

#[test]
fn a_protected_qos_msdu_leaves_room_for_the_ccmp_header_and_contends_by_its_priority() {
    let mut data = machine();
    let sent = data
        .encode::<Station, Caps>(&link(true, Some(&()), true), msdu(6), None)
        .ok()
        .unwrap();
    let frame = &sent.buffer.0;
    // QoS Data, To DS, Protected.
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x4188);
    assert_eq!(&frame[4..10], &BSSID);
    assert_eq!(&frame[16..22], &PEER);
    // The TID in the QoS Control field.
    assert_eq!(frame[24] & 0x0f, 6);
    // Header, the CCMP header's room, LLC/SNAP, payload; no MIC.
    assert_eq!(sent.frame.len().get(), 26 + 8 + 8 + 100);
    assert_eq!(&frame[34..42], &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    assert_eq!(sent.access_category, WmmAccessCategory::Voice);
    assert!(sent.protected);
}

#[test]
fn an_open_non_qos_msdu_takes_the_stations_sequence_number_and_no_ccmp_room() {
    let mut data = machine();
    let sequence = SequenceNumber::new(42).unwrap();
    let sent = data
        .encode::<Station, Caps>(&link(false, None, true), msdu(0), Some(sequence))
        .ok()
        .unwrap();
    let frame = &sent.buffer.0;
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x0108);
    assert_eq!(u16::from_le_bytes([frame[22], frame[23]]) >> 4, 42);
    assert_eq!(sent.frame.len().get(), 24 + 8 + 100);
    assert!(!sent.protected);
}

#[test]
fn msdus_wait_while_the_link_may_not_transmit_or_the_tokens_are_out() {
    let mut data = machine();
    assert!(matches!(
        data.encode::<Station, Caps>(&link(true, Some(&()), false), msdu(0), None),
        Err(DataRefusal::NotNow)
    ));
    let first = data
        .encode::<Station, Caps>(&link(true, Some(&()), true), msdu(0), None)
        .ok()
        .unwrap();
    let _second = data
        .encode::<Station, Caps>(&link(true, Some(&()), true), msdu(0), None)
        .ok()
        .unwrap();
    assert!(!data.can_send());
    assert!(matches!(
        data.encode::<Station, Caps>(&link(true, Some(&()), true), msdu(0), None),
        Err(DataRefusal::NoTokens)
    ));
    data.sent(first.ticket, first.buffer);
    assert!(data.can_send());
}

fn received(control: u16, ether_type: u16) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 26];
    frame[0..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    if control & 0x4000 != 0 {
        frame.extend_from_slice(&[1, 0, 0, 0x20, 0, 0, 0, 0]);
    }
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(&[0xcd; 20]);
    frame
}

fn meta(crypto: RxEvidence<RxCryptoStatus>) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
    meta.crypto = crypto;
    meta
}

#[test]
fn a_decrypted_data_mpdu_gives_its_msdu_and_an_unverified_or_eapol_one_none() {
    let key = ();
    let link = link(true, Some(&key), true);
    let decrypted = meta(RxEvidence::HardwareObserved(
        RxCryptoStatus::DecryptedAndIntegrityVerified,
    ));
    let frame = received(0x4288, 0x0800);
    let parts: Vec<_> = StaData::<Data>::received(&link, &frame, &decrypted)
        .unwrap()
        .collect();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].source, PEER);
    assert_eq!(parts[0].destination, LOCAL);
    assert_eq!(parts[0].payload, &[0xcd; 20]);
    // Protected but not verified: nothing for the network.
    assert!(StaData::<Data>::received(&link, &frame, &meta(RxEvidence::Unavailable)).is_none());
    // EAPOL is the station's.
    let eapol = received(0x4288, 0x888e);
    assert_eq!(
        StaData::<Data>::received(&link, &eapol, &decrypted)
            .unwrap()
            .count(),
        0
    );
}
