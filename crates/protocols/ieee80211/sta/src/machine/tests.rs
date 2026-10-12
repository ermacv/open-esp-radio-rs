//! The station's slice beside the real arbiter, on a host port the test
//! plays as the thin service would: scan, join of an open network,
//! connected, disconnect; alone and beside an access point.

use super::*;
use crate::join::test_support::{BSSID, LOCAL, association_response, authentication_response};
use oer_ieee80211_arbiter::{
    AccessPointCommand, AccessPointFact, Arbiter, ArbiterAction, ArbiterPolicy, GateTokens,
    NoReservation,
};
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use std::vec::Vec;

/// A 2.4 GHz backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// The test's tokens: numbered, neither `Clone` nor `Copy`; a buffer is
/// its memory.
#[derive(Debug, Eq, PartialEq)]
struct Ticket(u8);

struct Buffer([u8; 512]);

impl TxBufferMemory for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

/// A key slot and an installed key: neither `Clone` nor `Copy`.
#[derive(Debug, Eq, PartialEq)]
struct Slot(u8);
#[derive(Debug, Eq, PartialEq)]
struct Key(Slot);

struct Tokens;

impl StaTokens for Tokens {
    type Ticket = Ticket;
    type SmallBuffer = Buffer;
    type PairwiseSlot = Slot;
    type GroupSlot = Slot;
    type PairwiseKey = Key;
    type GroupKey = Key;
}

const SNONCE: [u8; 32] = [3; 32];

fn nonce() -> [u8; 32] {
    SNONCE
}

#[derive(Debug, Eq, PartialEq)]
struct Open;
#[derive(Debug, Eq, PartialEq)]
struct Closed;

struct Gate;

impl GateTokens for Gate {
    type Open = Open;
    type Closed = Closed;
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

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

/// The open network's Association Response: the join helper's, without
/// the Privacy capability.
fn open_association_response() -> [u8; 30] {
    let mut frame = association_response(0);
    frame[24..26].copy_from_slice(&0x0421_u16.to_le_bytes());
    frame
}

/// A beacon of the open network on channel 6.
fn beacon() -> [u8; 55] {
    let mut frame = [0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0001_u16.to_le_bytes());
    frame[36] = 0;
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = 6;
    frame
}

fn meta() -> RxMeta {
    RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap())
}

/// What the world saw the station and the arbiter do.
#[derive(Debug, Eq, PartialEq)]
enum Seen {
    Retune(u8),
    Configure(&'static str),
    Transmit { subtype: u16, on: u8 },
    Notify(StaNotice),
    AccessPoint(AccessPointCommand<Caps>),
    Upstream(UpstreamFact<Caps>),
    InstallPairwise([u8; 16]),
    InstallGroup(u8),
    RemoveKey,
}

/// The loop the thin service runs, with the test as the port.
struct World {
    station: Station<Tokens, Caps>,
    arbiter: Arbiter<'static, Gate, Caps, NoReservation>,
    seen: Vec<Seen>,
    /// The transmissions in flight, which the port ends in order.
    in_flight: Vec<(Ticket, Buffer)>,
    /// The receive configurations the port has yet to confirm.
    configuring: u32,
    /// The outcomes of the next transmissions; delivered once none is
    /// queued.
    outcomes: std::collections::VecDeque<bool>,
}

impl World {
    fn new(skipped_absences: u8) -> Self {
        Self::with_security(skipped_absences, StaSecurity::Open)
    }

    fn with_security(skipped_absences: u8, security: StaSecurity) -> Self {
        let mut ssid = [0; 32];
        ssid[..SSID.len()].copy_from_slice(SSID);
        let mut channels = [None; 16];
        channels[0] = Some(channel(1));
        channels[1] = Some(channel(6));
        let (arbiter, place, absence) = Arbiter::new(ArbiterPolicy::DEFAULT, channel(1), Open);
        let station = Station::new(
            StaConfig {
                address: LOCAL,
                ssid,
                ssid_len: SSID.len() as u8,
                channels,
                dwell: Duration::from_millis(10),
                skipped_absences,
                supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
                association: &CAPABILITIES,
                phy: PhyMode::Legacy,
                security,
                nonce,
            },
            [Ticket(0), Ticket(1)],
            [Buffer([0; 512]), Buffer([0; 512])],
            place,
            absence,
            (Slot(0), Slot(1)),
        );
        Self {
            station,
            arbiter,
            seen: Vec::new(),
            in_flight: Vec::new(),
            configuring: 0,
            outcomes: std::collections::VecDeque::new(),
        }
    }

    /// Run the loop at `now` until neither machine owes an action; the
    /// port confirms configurations and ends transmissions at once.
    fn run(&mut self, now: Instant) {
        loop {
            let mut acted = false;
            while let Some(action) = self.station.poll_action(now) {
                acted = true;
                match action {
                    StaAction::RequestChannel(request, place) => {
                        let answer = self.arbiter.request(now, request, place);
                        self.station.input(now, StaInput::Answer(answer));
                    }
                    StaAction::ReleaseChannel(lease) => self.arbiter.release(lease),
                    StaAction::RequestAbsence {
                        request,
                        home,
                        place,
                    } => {
                        let answer = self.arbiter.request_absence(now, request, home, place);
                        self.station.input(now, StaInput::AbsenceAnswer(answer));
                    }
                    StaAction::Ready(id) => self.arbiter.ready(id),
                    StaAction::InstallPairwiseKey { slot, key, .. } => {
                        self.seen.push(Seen::InstallPairwise(key));
                        self.station
                            .input(now, StaInput::PairwiseKeyInstalled(Key(slot)));
                    }
                    StaAction::InstallGroupKey { slot, key_id, .. } => {
                        self.seen.push(Seen::InstallGroup(key_id));
                        self.station.input(now, StaInput::GroupKeyInstalled(Key(slot)));
                    }
                    StaAction::RemovePairwiseKey(Key(slot)) => {
                        self.seen.push(Seen::RemoveKey);
                        self.station.input(now, StaInput::PairwiseKeyRemoved(slot));
                    }
                    StaAction::RemoveGroupKey(Key(slot)) => {
                        self.seen.push(Seen::RemoveKey);
                        self.station.input(now, StaInput::GroupKeyRemoved(slot));
                    }
                    StaAction::ExchangeLease(old) => {
                        let new = self.arbiter.exchange(old);
                        self.station.input(now, StaInput::Exchanged(new));
                    }
                    StaAction::Upstream(fact) => {
                        self.seen.push(Seen::Upstream(fact));
                        self.arbiter.upstream(now, fact);
                    }
                    StaAction::Configure(receive) => {
                        self.seen.push(Seen::Configure(match receive {
                            StationReceive::Off => "off",
                            StationReceive::OtherBssManagement => "scan",
                            StationReceive::Bss { .. } => "bss",
                        }));
                        self.configuring += 1;
                    }
                    StaAction::Transmit(mut transmit) => {
                        let frame = transmit.buffer.bytes_mut();
                        // A Null's PM bit tells its power state apart.
                        let subtype = u16::from_le_bytes([frame[0], frame[1]]) & 0x10fc;
                        let on = self.arbiter.channel().map_or(0, |c| c.channel().number());
                        self.seen.push(Seen::Transmit { subtype, on });
                        self.in_flight.push((transmit.ticket, transmit.buffer));
                    }
                    StaAction::Notify(notice) => self.seen.push(Seen::Notify(notice)),
                }
            }
            while let Some(action) = self.arbiter.poll_action(now) {
                acted = true;
                match action {
                    ArbiterAction::Station(notice) => {
                        self.station.input(now, StaInput::Arbiter(notice));
                    }
                    ArbiterAction::AccessPoint(command) => {
                        self.seen.push(Seen::AccessPoint(command));
                    }
                    // The gate closes once every transmission ended.
                    ArbiterAction::CloseGate { gate: Open, .. } => {
                        self.end_transmissions(now);
                        self.arbiter.gate_closed(Closed);
                    }
                    ArbiterAction::Retune(to) => {
                        self.seen.push(Seen::Retune(to.channel().number()));
                        self.arbiter.retuned(to);
                    }
                    ArbiterAction::OpenGate(Closed) => self.arbiter.gate_opened(Open),
                }
            }
            if self.configuring > 0 {
                self.configuring -= 1;
                self.station.input(now, StaInput::Configured);
                acted = true;
            }
            if !self.in_flight.is_empty() {
                self.end_transmissions(now);
                acted = true;
            }
            if !acted {
                return;
            }
        }
    }

    fn end_transmissions(&mut self, now: Instant) {
        for (ticket, buffer) in self.in_flight.drain(..).collect::<Vec<_>>() {
            self.station.input(
                now,
                StaInput::TxDone {
                    ticket,
                    buffer,
                    completion: TxCompletion {
                        status: if self.outcomes.pop_front().unwrap_or(true) {
                            TxStatus::Success
                        } else {
                            TxStatus::AckTimeout
                        },
                        ack_rssi_dbm: None,
                        ack_snr_db: None,
                        block_ack: None,
                        transmissions: 1,
                        rate: None,
                    },
                },
            );
        }
    }

    fn receive(&mut self, now: Instant, frame: &[u8]) {
        self.station.input(now, StaInput::Received { frame, meta: meta() });
        self.run(now);
    }

    fn take(&mut self) -> Vec<Seen> {
        core::mem::take(&mut self.seen)
    }

    /// Run until the next deadline of either machine.
    fn advance(&mut self) -> Instant {
        let next = [self.station.poll_deadline(), self.arbiter.poll_deadline()]
            .into_iter()
            .flatten()
            .min()
            .expect("a machine waits for a deadline");
        self.station.input(next, StaInput::Tick);
        self.run(next);
        next
    }
}

#[test]
fn a_station_alone_scans_joins_an_open_network_and_leaves_it() {
    let mut world = World::new(3);
    world.station.input(at(0), StaInput::Command(StaCommand::Connect));
    world.run(at(0));
    // Channel 1, where the port is: the station listens and probes.
    assert_eq!(
        world.take(),
        [
            Seen::Configure("scan"),
            Seen::Transmit {
                subtype: 0x0040,
                on: 1
            },
        ]
    );
    // The lease ends after the dwell; channel 6 is next, where the beacon
    // arrives.
    world.advance();
    assert_eq!(
        world.take(),
        [
            Seen::Retune(6),
            Seen::Configure("scan"),
            Seen::Transmit {
                subtype: 0x0040,
                on: 6
            },
        ]
    );
    let now = at(13);
    world.receive(now, &beacon());
    // The scan ends; the join leases channel 6 and authenticates.
    world.advance();
    assert_eq!(
        world.take(),
        [
            Seen::Configure("bss"),
            Seen::Transmit {
                subtype: 0x00b0,
                on: 6
            },
        ]
    );
    world.receive(at(30), &authentication_response(0));
    assert_eq!(
        world.take(),
        [Seen::Transmit {
            subtype: 0x0000,
            on: 6
        }]
    );
    world.receive(at(31), &open_association_response());
    assert_eq!(
        world.take(),
        [
            Seen::Notify(StaNotice::Connected {
                bssid: BSSID,
                channel: 6
            }),
            Seen::Upstream(UpstreamFact::Connected(channel(6))),
        ]
    );
    assert!(world.station.is_connected());

    // The application leaves: Deauthentication, receive off, the lease
    // back, then the report.
    world
        .station
        .input(at(40), StaInput::Command(StaCommand::Disconnect));
    world.run(at(40));
    assert_eq!(
        world.take(),
        [
            Seen::Transmit {
                subtype: 0x00c0,
                on: 6
            },
            Seen::Configure("off"),
            Seen::Notify(StaNotice::Disconnected(StaDisconnectReason::Requested)),
        ]
    );
    // Every token came back.
    assert!(world.in_flight.is_empty());
    assert!(world.station.place.is_some());
}

#[test]
fn beside_an_access_point_the_scan_goes_in_absences_after_its_tbtts() {
    let mut world = World::new(3);
    world.arbiter.access_point(
        at(0),
        AccessPointFact::Running {
            channel: channel(1),
            next_tbtt: at(50),
            beacon_interval: Duration::from_millis(100),
        },
    );
    world.station.input(at(0), StaInput::Command(StaCommand::Connect));
    world.run(at(0));
    // Channel 1 is the access point's: the station scans it at once.
    assert_eq!(
        world.take(),
        [
            Seen::Configure("scan"),
            Seen::Transmit {
                subtype: 0x0040,
                on: 1
            },
        ]
    );
    // Channel 6 only in the absence after the TBTT at 50 ms: the port
    // moves at 52 ms, and the station probes there.
    let ended = world.advance();
    assert_eq!(ended, at(12));
    assert_eq!(world.take(), []);
    let started = world.advance();
    assert_eq!(started, at(52));
    assert_eq!(
        world.take(),
        [
            Seen::Retune(6),
            Seen::Configure("scan"),
            Seen::Transmit {
                subtype: 0x0040,
                on: 6
            },
        ]
    );
    // The absence ends before the next beacon: the port is back home.
    let back = world.advance();
    assert_eq!(back, at(64));
    assert_eq!(world.take().first(), Some(&Seen::Retune(1)));
}

#[test]
fn a_scan_ends_with_a_partial_result_after_skipped_absences() {
    let mut world = World::new(2);
    world.arbiter.access_point(
        at(0),
        AccessPointFact::Running {
            channel: channel(1),
            next_tbtt: at(50),
            beacon_interval: Duration::from_millis(100),
        },
    );
    world.station.input(at(0), StaInput::Command(StaCommand::Connect));
    world.run(at(0));
    world.advance();
    world.take();
    // The access point stops before the absence, which cancels it, and
    // runs again before the station asks anew: twice in a row.
    for _ in 0..2 {
        world.arbiter.access_point(at(20), AccessPointFact::Stopped);
        world.arbiter.access_point(
            at(20),
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: at(50),
                beacon_interval: Duration::from_millis(100),
            },
        );
        world.run(at(20));
    }
    assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
        StaDisconnectReason::NotFound { partial: true }
    ))));
}

/// The Null data frame's subtype in the seen frames, with and without PM.
const NULL_ACTIVE: u16 = 0x0048;
const NULL_POWER_SAVE: u16 = 0x1048;

/// A world whose station is connected to the open network on channel 6,
/// alone on the port.
fn connected_world() -> World {
    let mut world = World::new(3);
    world.station.input(at(0), StaInput::Command(StaCommand::Connect));
    world.run(at(0));
    world.advance();
    world.receive(at(13), &beacon());
    world.advance();
    world.receive(at(30), &authentication_response(0));
    world.receive(at(31), &open_association_response());
    assert!(world.station.is_connected());
    world.take();
    world
}

#[test]
fn a_connected_station_scans_in_an_absence_announced_with_nulls() {
    let mut world = connected_world();
    world
        .station
        .input(at(40), StaInput::Command(StaCommand::BackgroundScan));
    world.run(at(40));
    // PM=1 first; the absence waits for its readiness and the window.
    assert_eq!(
        world.take(),
        [Seen::Transmit {
            subtype: NULL_POWER_SAVE,
            on: 6
        }]
    );
    // Its start: the port on channel 1, a scan there; then home, the
    // network's receive rules and PM=0.
    world.advance();
    assert_eq!(
        world.take(),
        [
            Seen::Retune(1),
            Seen::Configure("scan"),
            Seen::Transmit {
                subtype: 0x0040,
                on: 1
            },
        ]
    );
    world.advance();
    assert_eq!(
        world.take(),
        [
            Seen::Retune(6),
            Seen::Configure("bss"),
            Seen::Transmit {
                subtype: NULL_ACTIVE,
                on: 6
            },
            // Channel 6 is the network's own: the scan is done.
            Seen::Notify(StaNotice::BackgroundScanDone { partial: false }),
        ]
    );
}

#[test]
fn an_undelivered_sleep_null_cancels_the_absence_and_the_station_stays() {
    let mut world = connected_world();
    world.outcomes.push_back(false);
    world
        .station
        .input(at(40), StaInput::Command(StaCommand::BackgroundScan));
    world.run(at(40));
    world.take();
    // No readiness: the arbiter cancels the absence at its start; the port
    // stays on channel 6, and the station wakes its access point again.
    world.advance();
    let seen = world.take();
    assert!(!seen.iter().any(|seen| matches!(seen, Seen::Retune(_))));
    assert!(seen.contains(&Seen::Transmit {
        subtype: NULL_ACTIVE,
        on: 6
    }));
}

#[test]
fn an_undelivered_wake_null_is_sent_again_when_the_tim_shows_buffered_frames() {
    let mut world = connected_world();
    world
        .station
        .input(at(40), StaInput::Command(StaCommand::BackgroundScan));
    world.run(at(40));
    world.advance();
    // Back home, the PM=0 Null is not delivered.
    world.outcomes.push_back(false);
    world.advance();
    world.take();
    // The access point's beacon shows the station's association identifier
    // (0xc02a & 0x3fff = 42: octet 5, bit 2) in its TIM, a partial virtual
    // bitmap from octet 4: PM=0 again.
    let mut tim_beacon = [0_u8; 62];
    tim_beacon[..55].copy_from_slice(&beacon());
    tim_beacon[55..62].copy_from_slice(&[5, 5, 0, 1, 4, 0, 0b100]);
    world.receive(at(200), &tim_beacon);
    assert_eq!(
        world.take(),
        [Seen::Transmit {
            subtype: NULL_ACTIVE,
            on: 6
        }]
    );
}

/// The WPA2 network's RSN element: CCMP-128 with PSK.
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const PMK: [u8; 32] = [7; 32];
const ANONCE: [u8; 32] = [4; 32];

/// A beacon of the WPA2 network on channel 6.
fn wpa2_beacon() -> [u8; 77] {
    let mut frame = [0_u8; 77];
    frame[..55].copy_from_slice(&beacon());
    // ESS and Privacy.
    frame[34..36].copy_from_slice(&0x0011_u16.to_le_bytes());
    frame[55..77].copy_from_slice(&RSN);
    frame
}

/// An EAPOL frame from the access point to the station, in a data frame
/// from the DS.
fn eapol_frame(eapol: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame.extend_from_slice(eapol);
    frame
}

#[test]
fn a_station_joins_a_wpa2_network_through_the_four_way_handshake() {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::{PtkContext, frames::RsnTxFrame};

    let mut world = World::with_security(
        3,
        StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(PMK),
        },
    );
    world.station.input(at(0), StaInput::Command(StaCommand::Connect));
    world.run(at(0));
    world.advance();
    world.receive(at(13), &wpa2_beacon());
    world.advance();
    world.receive(at(30), &authentication_response(0));
    // The association response with the Privacy capability.
    world.receive(at(31), &association_response(0));
    world.take();

    // Message 1: the station answers with Message 2 in a data frame.
    let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    world.receive(at(32), &eapol_frame(message1.as_bytes()));
    assert_eq!(
        world.take(),
        [Seen::Transmit {
            subtype: 0x0008,
            on: 6
        }]
    );

    // Message 3, with the access point's RSN element and the GTK in its
    // key data: the station installs both keys, then sends Message 4.
    let ptk = Pmk::from_bytes(PMK).derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: BSSID,
            supplicant_address: LOCAL,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let mut plain = std::vec::Vec::new();
    plain.extend_from_slice(&RSN);
    // The GTK KDE of key 1.
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(&ptk);
    world.receive(at(33), &eapol_frame(message3.as_bytes()));
    let seen = world.take();
    assert!(matches!(seen[0], Seen::InstallPairwise(_)));
    assert_eq!(
        seen[1..],
        [
            Seen::InstallGroup(1),
            Seen::Transmit {
                subtype: 0x0008,
                on: 6
            },
            Seen::Notify(StaNotice::Connected {
                bssid: BSSID,
                channel: 6
            }),
            Seen::Upstream(UpstreamFact::Connected(channel(6))),
        ]
    );
    assert!(world.station.is_connected());

    // Leaving removes both keys before the Deauthentication.
    world
        .station
        .input(at(40), StaInput::Command(StaCommand::Disconnect));
    world.run(at(40));
    assert_eq!(
        world.take()[..3],
        [
            Seen::RemoveKey,
            Seen::RemoveKey,
            Seen::Transmit {
                subtype: 0x00c0,
                on: 6
            },
        ]
    );
}

