//! The station's slice on a host port it plays itself: scan, join of an
//! open network, connected, disconnect.

use super::*;
use crate::join::test_support::{BSSID, LOCAL, association_response, authentication_response};

/// The open network's Association Response: the join helper's, without
/// the Privacy capability.
fn open_association_response() -> [u8; 30] {
    let mut frame = association_response(0);
    frame[24..26].copy_from_slice(&0x0421_u16.to_le_bytes());
    frame
}
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};

/// A 2.4 GHz backend.
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
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        max_backoff_slots: 1023,
        tx_power_ceiling_min_dbm: Some(0),
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        key_slots: 4,
        rx_block_ack_agreements: 2,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

/// The test's tokens: numbered, neither `Clone` nor `Copy`.
#[derive(Debug, Eq, PartialEq)]
struct Ticket(u8);
#[derive(Debug, Eq, PartialEq)]
struct Buffer(u8);

struct Tokens;

impl StaTokens for Tokens {
    type Ticket = Ticket;
    type SmallBuffer = Buffer;
}

const SSID: &[u8] = b"open-net";

static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

fn channel(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn station() -> Station<Tokens, Caps> {
    let mut ssid = [0; 32];
    ssid[..SSID.len()].copy_from_slice(SSID);
    let mut channels = [None; 16];
    channels[0] = Some(channel(1));
    channels[1] = Some(channel(6));
    Station::new(
        StaConfig {
            address: LOCAL,
            ssid,
            ssid_len: SSID.len() as u8,
            channels,
            dwell: Duration::from_millis(100),
            supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
            association: &CAPABILITIES,
            phy: PhyMode::Legacy,
        },
        [Ticket(0), Ticket(1)],
        [Buffer(0), Buffer(1)],
    )
}

/// A beacon of the open network on channel 6.
fn beacon() -> [u8; 55] {
    let mut frame = [0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    // Timestamp, beacon interval 100 TU, capability ESS.
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0001_u16.to_le_bytes());
    // SSID.
    frame[36] = 0;
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    // Supported rates.
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    // DS parameter set: channel 6.
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = 6;
    frame
}

fn meta() -> RxMeta {
    RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap())
}

fn done(completion_status: TxStatus) -> TxCompletion {
    TxCompletion {
        status: completion_status,
        ack_rssi_dbm: None,
        ack_snr_db: None,
        block_ack: None,
        transmissions: 1,
        rate: None,
    }
}

/// What the station's next action was, with its tokens handed back as the
/// port would at its terminal.
#[derive(Debug, Eq, PartialEq)]
enum Seen {
    Channel(u8),
    Configure(&'static str),
    Transmit { subtype: u16, plan: StaRatePlan },
    Notify(StaNotice),
}

/// Take every action the station owes; a transmission's tokens are kept to
/// give back.
fn actions(
    station: &mut Station<Tokens, Caps>,
    now: Instant,
    sent: &mut std::vec::Vec<(Ticket, Buffer)>,
) -> std::vec::Vec<Seen> {
    let mut seen = std::vec::Vec::new();
    while let Some(action) = station.poll_action(now) {
        seen.push(match action {
            StaAction::RequestChannel(channel) => Seen::Channel(channel.channel().number()),
            StaAction::Configure(StationReceive::Off) => Seen::Configure("off"),
            StaAction::Configure(StationReceive::OtherBssManagement) => Seen::Configure("scan"),
            StaAction::Configure(StationReceive::Bss { .. }) => Seen::Configure("bss"),
            StaAction::Transmit(transmit) => {
                let subtype = u16::from_le_bytes([transmit.frame[0], transmit.frame[1]]);
                let plan = transmit.plan;
                sent.push((transmit.ticket, transmit.buffer));
                Seen::Transmit { subtype, plan }
            }
            StaAction::Notify(notice) => Seen::Notify(notice),
        });
    }
    seen
}

fn tx_done(station: &mut Station<Tokens, Caps>, now: Instant, sent: &mut std::vec::Vec<(Ticket, Buffer)>) {
    let (ticket, buffer) = sent.remove(0);
    station.input(
        now,
        StaInput::TxDone {
            ticket,
            buffer,
            completion: done(TxStatus::Success),
        },
    );
}

#[test]
fn a_station_scans_joins_an_open_network_and_leaves_it() {
    let mut station = station();
    let mut sent = std::vec::Vec::new();
    let mut now = Instant::from_micros(0);
    station.input(now, StaInput::Command(StaCommand::Connect));

    // Channel 1: tune, listen, probe, dwell; nothing answers.
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Channel(1)]);
    station.input(now, StaInput::ChannelGranted(channel(1)));
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Configure("scan")]);
    station.input(now, StaInput::Configured);
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Transmit {
            subtype: 0x0040,
            plan: StaRatePlan::Group
        }]
    );
    tx_done(&mut station, now, &mut sent);
    assert_eq!(station.poll_deadline(), now.checked_add(Duration::from_millis(100)));
    now = station.poll_deadline().unwrap();
    station.input(now, StaInput::Tick);

    // Channel 6: the access point's beacon arrives while the station dwells.
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Channel(6)]);
    station.input(now, StaInput::ChannelGranted(channel(6)));
    actions(&mut station, now, &mut sent);
    station.input(now, StaInput::Configured);
    actions(&mut station, now, &mut sent);
    tx_done(&mut station, now, &mut sent);
    station.input(
        now,
        StaInput::Received {
            frame: &beacon(),
            meta: meta(),
        },
    );
    now = station.poll_deadline().unwrap();
    station.input(now, StaInput::Tick);

    // The join: the access point's channel, its BSS, authentication.
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Channel(6)]);
    station.input(now, StaInput::ChannelGranted(channel(6)));
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Configure("bss")]);
    station.input(now, StaInput::Configured);
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Transmit {
            subtype: 0x00b0,
            plan: StaRatePlan::Management
        }]
    );
    tx_done(&mut station, now, &mut sent);
    station.input(
        now,
        StaInput::Received {
            frame: &authentication_response(0),
            meta: meta(),
        },
    );

    // Association, then connected.
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Transmit {
            subtype: 0x0000,
            plan: StaRatePlan::Management
        }]
    );
    tx_done(&mut station, now, &mut sent);
    station.input(
        now,
        StaInput::Received {
            frame: &open_association_response(),
            meta: meta(),
        },
    );
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Notify(StaNotice::Connected {
            bssid: BSSID,
            channel: 6
        })]
    );
    assert!(station.is_connected());

    // The application leaves: Deauthentication, then receive off.
    station.input(now, StaInput::Command(StaCommand::Disconnect));
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Transmit {
            subtype: 0x00c0,
            plan: StaRatePlan::Management
        }]
    );
    tx_done(&mut station, now, &mut sent);
    assert_eq!(actions(&mut station, now, &mut sent), [Seen::Configure("off")]);
    station.input(now, StaInput::Configured);
    assert_eq!(
        actions(&mut station, now, &mut sent),
        [Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Requested
        ))]
    );
    // Every token came back.
    assert!(sent.is_empty());
}
