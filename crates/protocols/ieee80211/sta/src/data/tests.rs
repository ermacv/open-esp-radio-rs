//! The data machine's encapsulation, tokens and admission.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    Ieee80211Capabilities, LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter,
    RxEvidence, TxStatus, WidthSet,
};
use oer_ieee80211_mac::ccmp::{CcmpRxReplayEpoch, GroupKeyId};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::StaSequenceCounter;
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
    type RxMeta = RxMeta;
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
        // The station's QoS sequence spaces, which the link lends.
        mode: LinkMode::Qos(std::boxed::Box::leak(std::boxed::Box::new(
            [StaSequenceCounter::new(SequenceNumber::ZERO); 8],
        ))),
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
    StaData::new(
        [1, 2],
        [Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600])],
    )
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
    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap()
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
    data.input(
        NOW,
        DataInput::Msdu {
            msdu: msdu(6),
            link: &mut qos_link(&(), &mut replay),
        },
    )
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
    data.input(
        NOW,
        DataInput::Msdu {
            msdu: msdu(0),
            link: &mut open_link(&mut counter),
        },
    )
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
fn an_msdu_too_long_for_its_buffer_takes_no_sequence_number() {
    let mut data = StaData::<Data, Caps>::new([1], [Buffer(std::vec![0; 200])]);
    let mut counter = StaSequenceCounter::new(SequenceNumber::new(42).unwrap());
    let long = Msdu {
        payload: &[0; 200],
        ..msdu(0)
    };
    assert_eq!(
        data.input(
            NOW,
            DataInput::Msdu {
                msdu: long,
                link: &mut open_link(&mut counter)
            }
        ),
        Err(DataRefusal::TooLong)
    );
    assert_eq!(counter.peek(), SequenceNumber::new(42).unwrap());
    // The tokens stay, and a short MSDU takes the number.
    data.input(
        NOW,
        DataInput::Msdu {
            msdu: msdu(0),
            link: &mut open_link(&mut counter),
        },
    )
    .unwrap();
    let sent = transmit(&mut data);
    assert_eq!(
        u16::from_le_bytes([sent.buffer.0[22], sent.buffer.0[23]]) >> 4,
        42
    );
}

#[test]
fn msdus_wait_while_the_tokens_are_out() {
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        NOW,
        DataInput::Msdu {
            msdu: msdu(0),
            link: &mut link,
        },
    )
    .unwrap();
    data.input(
        NOW,
        DataInput::Msdu {
            msdu: msdu(0),
            link: &mut link,
        },
    )
    .unwrap();
    assert!(!data.accepts());
    assert_eq!(
        data.input(
            NOW,
            DataInput::Msdu {
                msdu: msdu(0),
                link: &mut link
            }
        ),
        Err(DataRefusal::NoTokens)
    );
    let first = transmit(&mut data);
    data.input::<Station>(
        NOW,
        DataInput::TxDone {
            ticket: first.ticket,
            buffer: first.buffer,
            completion: ended(TxStatus::Success, 1),
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
    assert_eq!(
        received(&mut link, &eapol, &decrypted()).unwrap().count(),
        0
    );
}

#[test]
fn a_frame_sent_again_is_dropped_by_the_replay_check() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 7, 0);
    assert_eq!(
        received(&mut link, &frame, &decrypted()).unwrap().count(),
        1
    );
    // The same packet number again: dropped before it reaches the network.
    assert!(received(&mut link, &frame, &decrypted()).is_none());
}

#[test]
fn the_old_and_the_new_group_key_are_both_taken_until_the_old_is_removed() {
    let mut replay = epoch();
    let group = |packet_number, key_id| {
        let mut frame = received_frame(0x4288, 0x0800, packet_number, key_id);
        frame[4..10].copy_from_slice(&[0xff; 6]);
        frame
    };
    {
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(200, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    // The access point's new group key, index 2, from packet number 1.
    replay
        .install_group(GroupKeyId::new(2).unwrap(), [0; 8])
        .unwrap();
    {
        // Both keys are taken while both are installed.
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(1, 2), &decrypted())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            received(&mut link, &group(201, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    replay.remove_group(GroupKeyId::new(1).unwrap());
    let mut link = qos_link(&(), &mut replay);
    assert!(received(&mut link, &group(202, 1), &decrypted()).is_none());
    assert_eq!(
        received(&mut link, &group(2, 2), &decrypted())
            .unwrap()
            .count(),
        1
    );
}

/// The terminal of an attempt that made `transmissions`.
fn ended(status: TxStatus, transmissions: u8) -> TxCompletion {
    TxCompletion {
        status,
        ack_rssi_dbm: None,
        ack_snr_db: None,
        block_ack: None,
        transmissions,
        rate: None,
    }
}

/// The TID and sequence number of a QoS Data MPDU in its buffer.
fn tid_and_sequence(frame: &[u8]) -> (u8, u16) {
    (frame[24] & 0x0f, u16::from_le_bytes([frame[22], frame[23]]) >> 4)
}

#[test]
fn an_attempt_aborted_before_the_air_goes_again_first_with_its_sequence_number() {
    let mut data = StaData::<Data, Caps>::new(
        [1, 2, 3],
        [Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600])],
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // Two MSDUs of TID 6: the first is submitted, then a newer one waits.
    data.input(NOW, DataInput::Msdu { msdu: msdu(6), link: &mut link }).unwrap();
    let first = transmit(&mut data);
    let sent = tid_and_sequence(&first.buffer.0);
    data.input(NOW, DataInput::Msdu { msdu: msdu(6), link: &mut link }).unwrap();
    // A closing gate aborts the first before any transmission.
    data.input::<Station>(
        NOW,
        DataInput::TxDone {
            ticket: first.ticket,
            buffer: first.buffer,
            completion: ended(TxStatus::Aborted, 0),
        },
    )
    .unwrap();
    // It goes again at the head of its TID's queue, before the newer MSDU,
    // with the same sequence number, in its buffer.
    let again = transmit(&mut data);
    assert_eq!(tid_and_sequence(&again.buffer.0), sent);
    let newer = transmit(&mut data);
    assert_eq!(tid_and_sequence(&newer.buffer.0), (6, sent.1 + 1));
    assert!(data.poll_action(NOW).is_none());
    // One that made a transmission ends: its tokens come back.
    data.input::<Station>(
        NOW,
        DataInput::TxDone {
            ticket: again.ticket,
            buffer: again.buffer,
            completion: ended(TxStatus::AckTimeout, 3),
        },
    )
    .unwrap();
    assert!(data.poll_action(NOW).is_none());
}
