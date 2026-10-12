//! The data machine's encapsulation, tokens and admission.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    Ieee80211Capabilities, LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter,
    RxEvidence, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use oer_ieee80211_mac::ccmp::CcmpRxReplayEpoch;
use oer_ieee80211_mac::sequence::SequenceNumber;
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
const NOW: Instant = Instant::from_micros(0);

fn qos_link<'a>(key: &'a (), replay: &'a mut CcmpRxReplayEpoch) -> LinkView<'a, Station> {
    LinkView {
        address: LOCAL,
        bssid: BSSID,
        association_id: 1,
        mode: LinkMode::Qos,
        protection: LinkProtection::Ccmp { key, replay },
    }
}

fn open_link(counter: &mut StaSequenceCounter) -> LinkView<'_, Station> {
    LinkView {
        address: LOCAL,
        bssid: BSSID,
        association_id: 1,
        mode: LinkMode::NonQos(counter),
        protection: LinkProtection::Open,
    }
}

fn machine() -> StaData<Data, Caps> {
    StaData::new([1, 2], [Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600])])
}

fn msdu(priority: u8) -> Msdu<'static> {
    Msdu {
        destination: PEER,
        ether_type: 0x0800,
        priority: WmmUserPriority::new(priority).unwrap(),
        payload: &[0xab; 100],
    }
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap()
}

fn transmit(data: &mut StaData<Data, Caps>) -> DataTransmit<Data, Caps> {
    match data.poll_action(NOW) {
        Some(DataAction::Transmit(transmit)) => transmit,
        None => panic!("an attempt is owed"),
    }
}

#[test]
fn a_protected_qos_msdu_leaves_room_for_the_ccmp_header_and_contends_by_its_priority() {
    let mut data = machine();
    let mut replay = epoch();
    data.input(NOW, DataInput::Msdu { msdu: msdu(6), link: &mut qos_link(&(), &mut replay) })
        .unwrap();
    let sent = transmit(&mut data);
    assert!(data.poll_action(NOW).is_none());
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
    let mut counter = StaSequenceCounter::new(SequenceNumber::new(42).unwrap());
    data.input(NOW, DataInput::Msdu { msdu: msdu(0), link: &mut open_link(&mut counter) })
        .unwrap();
    let sent = transmit(&mut data);
    let frame = &sent.buffer.0;
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x0108);
    assert_eq!(u16::from_le_bytes([frame[22], frame[23]]) >> 4, 42);
    assert_eq!(counter.peek(), SequenceNumber::new(43).unwrap());
    assert_eq!(sent.frame.len().get(), 24 + 8 + 100);
    assert!(!sent.protected);
}

#[test]
fn msdus_wait_while_the_tokens_are_out() {
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(NOW, DataInput::Msdu { msdu: msdu(0), link: &mut link }).unwrap();
    data.input(NOW, DataInput::Msdu { msdu: msdu(0), link: &mut link }).unwrap();
    assert!(!data.accepts());
    assert_eq!(
        data.input(NOW, DataInput::Msdu { msdu: msdu(0), link: &mut link }),
        Err(DataRefusal::NoTokens)
    );
    let first = transmit(&mut data);
    data.input::<Station>(
        NOW,
        DataInput::TxDone {
            ticket: first.ticket,
            buffer: first.buffer,
        },
    )
    .unwrap();
    assert!(data.accepts());
    assert_eq!(data.poll_deadline(), None);
}

fn received_frame(control: u16, ether_type: u16, packet_number: u8, key_id: u8) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 26];
    frame[0..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    if control & 0x4000 != 0 {
        frame.extend_from_slice(&[packet_number, 0, 0, 0x20 | (key_id << 6), 0, 0, 0, 0]);
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

fn decrypted() -> RxMeta {
    meta(RxEvidence::HardwareObserved(
        RxCryptoStatus::DecryptedAndIntegrityVerified,
    ))
}

#[test]
fn a_decrypted_data_mpdu_gives_its_msdu_and_an_unverified_or_eapol_one_none() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 1, 0);
    let parts: Vec<_> = received(&mut link, &frame, &decrypted()).unwrap().collect();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].source, PEER);
    assert_eq!(parts[0].destination, LOCAL);
    assert_eq!(parts[0].payload, &[0xcd; 20]);
    // Protected but not verified: nothing for the network.
    let frame = received_frame(0x4288, 0x0800, 2, 0);
    assert!(received(&mut link, &frame, &meta(RxEvidence::Unavailable)).is_none());
    // EAPOL is the station's.
    let eapol = received_frame(0x4288, 0x888e, 3, 0);
    assert_eq!(received(&mut link, &eapol, &decrypted()).unwrap().count(), 0);
}

#[test]
fn a_frame_sent_again_is_dropped_by_the_replay_check() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 7, 0);
    assert_eq!(received(&mut link, &frame, &decrypted()).unwrap().count(), 1);
    // The same packet number again: dropped before it reaches the network.
    assert!(received(&mut link, &frame, &decrypted()).is_none());
}

#[test]
fn after_a_group_key_change_frames_under_the_new_key_are_taken() {
    let mut replay = epoch();
    let group = |packet_number, key_id| {
        let mut frame = received_frame(0x4288, 0x0800, packet_number, key_id);
        frame[4..10].copy_from_slice(&[0xff; 6]);
        frame
    };
    {
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(received(&mut link, &group(200, 1), &decrypted()).unwrap().count(), 1);
    }
    // The access point's new group key, index 2, from packet number 1.
    let rotation = replay.prepare_group_rotation(2, [0; 8]).unwrap();
    replay.commit_group_rotation(rotation).unwrap();
    let mut link = qos_link(&(), &mut replay);
    assert_eq!(received(&mut link, &group(1, 2), &decrypted()).unwrap().count(), 1);
    assert!(received(&mut link, &group(201, 1), &decrypted()).is_none());
}
