//! The station over the lower-MAC port against `oer-ieee80211-lower-mac`'s
//! host model and a scripted access point: scan, Open System and SAE joins,
//! the WPA2-PSK handshake with keys installed through the port, the data
//! plane both ways with a Block Ack window, replay and duplicate checks,
//! power save around a TBTT, and the end of the association.

mod scripted_ap;

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
    vec::Vec,
};

use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, CoexPriority, Ieee80211LowerMacPort, KeySelector, LifecycleCommand,
    LowerMacSetting, ReceiveFilter, TxPower, VifId,
    model::{LowerMacModel, ModelOutcome},
};
use oer_ieee80211_mac::{
    ccmp::CcmpPacketNumberStep,
    phy::{LegacyRate, PhyRate},
    scan::ScanTable,
    sequence::SequenceNumber,
    station::{AssociationCapabilities, StaTxSequenceCounters, association::PhyMode},
};
use oer_ieee80211_rsn::{
    Pmk,
    aes::RsnSoftwareAes,
    sae::{SaeCommit, SaePassword, SaePasswordElement},
};
use oer_ieee80211_softmac::{BackoffEntropy, EdcaContention};
use oer_ieee80211_sta::{
    attempt::{
        AssociationAttemptOutcome, StaAttemptSecurity, StaPersonalCredentials,
        Wpa2Message4Protection,
    },
    modem_sleep::{PmState, SleepType},
    pmksa::StaSharedPmksa,
    scan::{StaCandidateScanExit, StaScanConfig},
    station::{StaLifecycleExit, StaReconnectPolicy},
};
use oer_ieee80211_sta_service::{
    port::{
        EventRouter, PortDisconnect, PortLink, PortLinkError, PortProbe, PortRouter, PortScan,
        PortScanTarget, PortSend, PortStation, PortStationApplication, PortStationConfig,
        PortStationEnv, PortStationLifecycle, PortStationProfile,
    },
    scan::{StaCandidateScanService, StaScanBackend},
    station::StaLifecycleService,
};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, FixedRate, ProtectEveryHeTxop, ProtectionPolicy, RetryLimits, TxPlanner,
};
use oer_ieee80211_upper_mac_service::UpperMacTxError;
use oer_time::{Clock, Duration, Instant, RadioInstant, Timer};

use scripted_ap::{AP, AP_CHANNEL, ApSecurity, PASSPHRASE, RATES, SNONCE, SSID, STA, ScriptedAp};

/// Run `body` on a thread whose stack holds the station's unoptimized
/// futures, whose frames and scan table are inline.
fn on_large_stack(body: fn()) {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

/// Virtual monotonic time: waits end when the harness advances it.
#[derive(Default)]
struct VirtualTimer {
    now: Cell<u64>,
    /// The earliest deadline a pending wait asked for since the last poll.
    wanted: Cell<Option<u64>>,
}

impl Clock for VirtualTimer {
    fn now(&self) -> Instant {
        Instant::from_micros(self.now.get())
    }
}

impl Timer for VirtualTimer {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        poll_fn(move |_| {
            if self.now.get() >= deadline.as_micros() {
                Poll::Ready(())
            } else {
                let deadline = deadline.as_micros();
                self.wanted.set(Some(
                    self.wanted.get().map_or(deadline, |w| w.min(deadline)),
                ));
                Poll::Pending
            }
        })
    }
}

/// A seeded xorshift32 source.
struct Seeded(u32);

impl BackoffEntropy for Seeded {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

struct Env<'a>(PhantomData<&'a ()>);

impl<'a> PortStationEnv for Env<'a> {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Timer = &'a VirtualTimer;
    type KeyUnwrap = RsnSoftwareAes;
    type Aggregation = oer_ieee80211_sta_service::port::PortAmpduAggregation;
}

const MANAGEMENT_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm6M);
const DATA_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm54M);

fn channel(number: u8) -> Channel {
    Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()
}

static CHANNELS: [Channel; 3] = [
    match Channel::ghz2_4(1, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("channel 1"),
    },
    match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("channel 6"),
    },
    match Channel::ghz2_4(11, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("channel 11"),
    },
];

static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

fn sa_query_random() -> u32 {
    0x1234
}

fn sae_random() -> u32 {
    static STATE: AtomicU32 = AtomicU32::new(0x0bad_cafe);
    let mut x = STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    STATE.store(x, Ordering::Relaxed);
    x
}

/// The model, tuned and enabled, its event router and the virtual clock.
struct World {
    model: &'static LowerMacModel,
    router: &'static PortRouter<'static, Env<'static>>,
    timer: VirtualTimer,
}

impl World {
    fn new() -> Self {
        let model = LowerMacModel::new();
        model.set_now(RadioInstant::from_micros(1_000));
        model
            .apply(LowerMacSetting::Channel(channel(1)))
            .unwrap()
            .unwrap();
        model.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
        // Every published attempt succeeds at once.
        model.respond(core::iter::repeat_n(ModelOutcome::Success, 100_000));
        let model: &'static LowerMacModel = Box::leak(Box::new(model));
        Self {
            model,
            router: Box::leak(Box::new(EventRouter::new(model, 1))),
            timer: VirtualTimer {
                now: Cell::new(1_000),
                wanted: Cell::new(None),
            },
        }
    }

    /// Move virtual time to `micros`: the station's timer and the model's
    /// radio clock read one time.
    fn advance_to(&self, micros: u64) {
        self.timer.now.set(micros);
        self.model.set_now(RadioInstant::from_micros(micros));
    }

    fn link(&self) -> PortLink<'static, Env<'_>> {
        self.link_at(DATA_RATE)
    }

    fn link_at(&self, data_rate: PhyRate) -> PortLink<'static, Env<'_>> {
        PortLink::new(
            self.router,
            TxPlanner::new(
                [EdcaContention::new(4, 10); 4],
                RetryLimits::IEEE_DEFAULT,
                AmpduRetryPolicy {
                    lifetime: oer_time::RadioDuration::from_micros(1_000_000),
                    aged_margin: oer_time::RadioDuration::from_micros(1_024),
                    retry_limit: 7,
                    retain_single_mpdu: false,
                },
                ProtectionPolicy::new(None),
                ProtectEveryHeTxop,
            ),
            FixedRate,
            Seeded(0x1357_9bdf),
            PortStationConfig {
                vif: VifId(0),
                address: STA,
                management_rate: MANAGEMENT_RATE,
                data_rate,
                power: TxPower::Calibrated,
                coex: CoexPriority::Normal,
                retry_limit: 7,
            },
        )
    }

    fn station<'a>(&'a self, security: StaAttemptSecurity<'a>) -> PortStation<'a, Env<'a>> {
        self.station_with(security, profile())
    }

    fn station_with<'a>(
        &'a self,
        security: StaAttemptSecurity<'a>,
        profile: PortStationProfile<'a>,
    ) -> PortStation<'a, Env<'a>> {
        self.station_at(security, profile, DATA_RATE)
    }

    fn station_at<'a>(
        &'a self,
        security: StaAttemptSecurity<'a>,
        profile: PortStationProfile<'a>,
        data_rate: PhyRate,
    ) -> PortStation<'a, Env<'a>> {
        PortStation::new(
            self.link_at(data_rate).with_beacon_timing(),
            &self.timer,
            RsnSoftwareAes,
            profile,
            security,
        )
    }

    /// Poll `future` to its end: between polls the access point answers
    /// what the station sent, and when neither moves, virtual time
    /// advances to the earliest deadline anyone waits for.
    fn drive<F: Future>(&self, ap: &mut ScriptedAp, future: F) -> F::Output {
        let mut future = pin!(future);
        // The port's one event consumer, beside the station.
        let mut routing = pin!(self.router.run());
        let mut context = Context::from_waker(Waker::noop());
        let mut quiet = false;
        for _ in 0..1_000_000 {
            self.timer.wanted.set(None);
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            if routing.as_mut().poll(&mut context).is_ready() {
                // The router ended on the terminal poisoned event; the
                // station learns of it at its next poll.
                routing.set(self.router.run());
                continue;
            }
            if ap.step(self.model, self.timer.now.get()) {
                quiet = false;
                continue;
            }
            // The router may have answered a wait the station registered
            // after its poll (an idle port it waits to observe): poll once
            // more before moving time.
            if !quiet {
                quiet = true;
                continue;
            }
            quiet = false;
            let wanted = self.timer.wanted.get();
            let next = match (wanted, ap.next_beacon_micros) {
                (Some(a), Some(b)) => a.min(b),
                (a, b) => a
                    .or(b)
                    .expect("the station waits for an event nothing produces"),
            };
            self.advance_to(next.max(self.timer.now.get()));
        }
        panic!("the station did not finish");
    }

    /// Run the station for `millis` of virtual time.
    fn run_for(
        &self,
        ap: &mut ScriptedAp,
        station: &mut PortStation<'_, Env<'_>>,
        millis: u32,
        delivered: &mut Vec<Vec<u8>>,
    ) -> Option<PortDisconnect> {
        let deadline = self
            .timer
            .now()
            .checked_add(Duration::from_millis(millis))
            .unwrap();
        self.drive(
            ap,
            station.run_until(deadline, &mut |ethernet| delivered.push(ethernet.to_vec())),
        )
        .unwrap()
    }
}

fn profile() -> PortStationProfile<'static> {
    PortStationProfile {
        ssid: SSID,
        channels: &CHANNELS,
        scan: StaScanConfig::new(2).unwrap(),
        dwell_tick: Duration::from_millis(10),
        probe: Some(PortProbe {
            ssid: SSID,
            supported_rates: &RATES,
        }),
        capabilities: &CAPABILITIES,
        he_power: None,
        he_packet_padding: oer_espressif_ieee80211_policy::he_txop::packet_padding,
        tx_block_ack: None,
        rx_reorder_gap: Duration::from_micros(
            oer_espressif_ieee80211_policy::block_ack::RX_REORDER_GAP_TIMEOUT_MICROS,
        ),
        phy: PhyMode::Legacy,
        listen_interval: 3,
        ccmp_step: CcmpPacketNumberStep::new(1).unwrap(),
        sa_query_random,
    }
}

fn counters() -> StaTxSequenceCounters {
    StaTxSequenceCounters::new(SequenceNumber::new(0).unwrap())
}

fn open() -> StaAttemptSecurity<'static> {
    StaAttemptSecurity::open(counters())
}

fn wpa2(pmksa: &'static StaSharedPmksa) -> StaAttemptSecurity<'static> {
    StaAttemptSecurity::new(
        StaPersonalCredentials::wpa2(
            Pmk::derive(PASSPHRASE, SSID).unwrap(),
            SaePassword::new(PASSPHRASE).unwrap(),
            sae_random,
            pmksa,
        ),
        SNONCE,
        counters(),
        Wpa2Message4Protection::Unprotected,
    )
}

/// Connect `station` to `ap`, returning the connected station.
fn connect<'a>(
    world: &'a World,
    ap: &mut ScriptedAp,
    station: PortStation<'a, Env<'a>>,
) -> PortStation<'a, Env<'a>> {
    match world.drive(ap, station.connect()) {
        AssociationAttemptOutcome::Connected { connected, .. } => connected,
        AssociationAttemptOutcome::Failed(failure) => {
            panic!(
                "the attempt failed at {:?}: {:?}",
                failure.stage, failure.error
            )
        }
    }
}

/// An Ethernet-II frame from the station's address.
fn ethernet(destination: [u8; 6], ether_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&destination);
    frame.extend_from_slice(&STA);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

const PEER: [u8; 6] = [0x02, 0x77, 0, 0, 0, 9];
const IPV4: u16 = 0x0800;

#[test]
fn an_active_scan_finds_the_access_point_on_its_channel() {
    on_large_stack(an_active_scan_finds_the_access_point_on_its_channel_body);
}

fn an_active_scan_finds_the_access_point_on_its_channel_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut link = world.link();
    let mut sequences = counters();
    let mut table = ScanTable::<8>::new();
    let timer = &world.timer;
    let scan = PortScan::new(
        &mut link,
        &timer,
        sequences.non_qos_mut(),
        &mut table,
        PortScanTarget {
            ssid: SSID,
            policy: oer_ieee80211_mac::security::StaSecurityPolicy::Open,
            probe: Some(PortProbe {
                ssid: SSID,
                supported_rates: &RATES,
            }),
        },
        Duration::from_millis(10),
    );
    let mut service =
        StaCandidateScanService::new(StaScanBackend::new(StaScanConfig::new(2).unwrap()));
    let exit = world.drive(&mut ap, service.run(scan, &CHANNELS));
    let StaCandidateScanExit::Selected {
        candidate,
        progress,
        ..
    } = exit
    else {
        panic!("no candidate");
    };
    assert_eq!(candidate.bssid, AP);
    assert_eq!(candidate.channel, AP_CHANNEL);
    assert_eq!(candidate.ssid_bytes(), SSID);
    assert_eq!(progress.channels_completed, 3);
    // One Probe Request on each channel, the last channel tuned last.
    assert_eq!(ap.probe_requests, [1, 6, 11]);
    assert_eq!(world.model.channel(), Some(channel(11)));
    // The scan left the station interface receiving nothing.
    let vif = world.model.vif_config(VifId(0)).unwrap();
    assert_eq!(vif.receive, ReceiveFilter::NONE);
    assert_eq!(vif.bssid, None);
    // Probe Requests are broadcast and sent without a key.
    let probes: Vec<_> = world
        .model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x40)
        .collect();
    assert_eq!(probes.len(), 3);
    assert!(
        probes
            .iter()
            .all(|attempt| attempt.key == KeySelector::Plaintext
                && attempt.rate == MANAGEMENT_RATE
                && attempt.frames[0][4..10] == [0xff; 6])
    );
}

#[test]
fn an_open_join_connects_and_exchanges_data_both_ways() {
    on_large_stack(an_open_join_connects_and_exchanges_data_both_ways_body);
}

fn an_open_join_connects_and_exchanges_data_both_ways_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    assert_eq!(ap.authentications, 1);
    assert_eq!(ap.associations, 1);
    assert!(station.report().authentication.is_some());
    assert_eq!(world.model.channel(), Some(channel(AP_CHANNEL)));
    let vif = world.model.vif_config(VifId(0)).unwrap();
    assert_eq!(vif.bssid, Some(AP));
    assert_eq!(vif.receive, ReceiveFilter::BSS_MEMBER);
    let connection = station.connection().unwrap();
    assert!(connection.keys().is_none());
    assert!(connection.config().peer_qos);

    // Uplink: QoS data at the user priority's access category, each TID
    // numbering its own sequence.
    for (priority, payload) in [(0, b"best".as_slice()), (6, b"voice"), (0, b"again")] {
        assert_eq!(
            station.send(&ethernet(PEER, IPV4, payload), priority),
            Ok(PortSend::Queued)
        );
    }
    // The receive loop sends the queue.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut Vec::new()),
        None
    );
    assert_eq!(station.connection().unwrap().tx_counters().mpdus, 3);
    ap.absorb(world.model);
    assert_eq!(ap.uplink.len(), 3);
    assert_eq!(ap.uplink[0].payload, b"best");
    assert_eq!(ap.uplink[0].destination, PEER);
    assert_eq!(ap.uplink[0].qos_tid, Some(0));
    assert_eq!(ap.uplink[1].qos_tid, Some(6));
    assert_eq!(
        (
            ap.uplink[0].sequence,
            ap.uplink[1].sequence,
            ap.uplink[2].sequence
        ),
        (0, 0, 1)
    );
    assert!(ap.uplink.iter().all(|uplink| !uplink.protected));
    let data_attempts: Vec<_> = world
        .model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x88)
        .collect();
    assert_eq!(
        data_attempts[1].access_category,
        oer_ieee80211_mac::qos::WmmAccessCategory::Voice
    );
    assert_eq!(data_attempts[0].rate, DATA_RATE);

    // Downlink.
    let frame = ap.data(None, None, false, IPV4, b"hello station", PEER);
    ap.queue(frame);
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert_eq!(delivered.len(), 1);
    assert_eq!(&delivered[0][..6], &STA);
    assert_eq!(&delivered[0][6..12], &PEER);
    assert_eq!(&delivered[0][14..], b"hello station");
}

#[test]
fn a_wpa2_psk_connection_installs_its_keys_through_the_port() {
    on_large_stack(a_wpa2_psk_connection_installs_its_keys_through_the_port_body);
}

fn a_wpa2_psk_connection_installs_its_keys_through_the_port_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    ap.absorb(world.model);
    assert_eq!(
        ap.eapol_from_station,
        [
            oer_ieee80211_rsn::EapolKeyMessage::PairwiseMessage2,
            oer_ieee80211_rsn::EapolKeyMessage::PairwiseMessage4
        ]
    );
    assert!(ap.handshake_complete);
    assert!(station.security().has_connected_wpa2());
    // The connection keeps the access point as the association left it.
    let peer = station.connection().unwrap().config().peer;
    assert_eq!(peer.phy, PhyMode::Legacy);
    assert_eq!(peer.he_peer_state, None);
    let keys = station
        .connection()
        .unwrap()
        .keys()
        .expect("installed keys");
    assert_ne!(keys.pairwise, keys.group);
    assert_eq!(keys.group_key_id, 1);
    assert_eq!(station.report().keys.map(|keys| keys.group_key_id), Some(1));
    // Message 2 and Message 4 left in the clear, before and after the keys.
    let eapol: Vec<_> = world
        .model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x08)
        .collect();
    assert_eq!(eapol.len(), 2);
    assert!(
        eapol
            .iter()
            .all(|attempt| attempt.key == KeySelector::Plaintext)
    );

    // Data leaves under the pairwise key with consecutive packet numbers.
    for payload in [b"one".as_slice(), b"two"] {
        assert_eq!(
            station.send(&ethernet(PEER, IPV4, payload), 0),
            Ok(PortSend::Queued)
        );
    }
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut Vec::new()),
        None
    );
    ap.absorb(world.model);
    assert_eq!(ap.uplink.len(), 2);
    assert!(
        ap.uplink
            .iter()
            .all(|uplink| uplink.protected && uplink.key == KeySelector::Key(keys.pairwise))
    );
    let packet_numbers: Vec<u64> = world
        .model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x88)
        .map(|attempt| {
            let ccmp = &attempt.frames[0][26..34];
            u64::from(ccmp[0])
                | u64::from(ccmp[1]) << 8
                | u64::from(ccmp[4]) << 16
                | u64::from(ccmp[5]) << 24
        })
        .collect();
    assert_eq!(packet_numbers, [1, 2]);

    // A protected downlink frame the backend decrypted is delivered; an
    // unprotected one after the keys is not.
    let protected = ap.data(Some(0), Some(1), false, IPV4, b"secret", PEER);
    ap.queue(protected);
    let plain = ap.data(Some(0), None, false, IPV4, b"forged", PEER);
    ap.queue(plain);
    let mut delivered = Vec::new();
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(delivered.len(), 1);
    assert_eq!(&delivered[0][14..], b"secret");
    assert_eq!(station.connection().unwrap().counters().unprotected, 1);
}

#[test]
fn a_block_ack_window_releases_in_order_and_replays_and_duplicates_are_dropped() {
    on_large_stack(
        a_block_ack_window_releases_in_order_and_replays_and_duplicates_are_dropped_body,
    );
}

fn a_block_ack_window_releases_in_order_and_replays_and_duplicates_are_dropped_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    let mut delivered = Vec::new();

    let request = ap.addba_request(0, 8, 100);
    ap.queue(request);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert!(station.connection().unwrap().block_ack(0));
    // The station answered with a successful ADDBA Response.
    let response = ap.actions.last().unwrap();
    assert_eq!(&response[..3], &[3, 1, 7]);
    assert_eq!(u16::from_le_bytes([response[3], response[4]]), 0);

    // Sequences 101 and 102 arrive before 100: nothing is released until
    // 100 closes the gap, then all three in sequence order.
    for (sequence, packet_number, payload) in [(101, 2, b"b"), (102, 3, b"c")] {
        let frame = ap.data_with_sequence(
            sequence,
            Some(0),
            Some(packet_number),
            false,
            IPV4,
            payload,
            PEER,
        );
        ap.queue(frame);
    }
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert!(delivered.is_empty());
    let frame = ap.data_with_sequence(100, Some(0), Some(1), false, IPV4, b"a", PEER);
    ap.queue(frame);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    let payloads: Vec<&[u8]> = delivered.iter().map(|frame| &frame[14..]).collect();
    assert_eq!(payloads, [b"a", b"b", b"c"]);

    // A new sequence whose packet number does not advance is a replay.
    let replay = ap.data_with_sequence(103, Some(0), Some(2), false, IPV4, b"replay", PEER);
    ap.queue(replay);
    // A retransmission of a received MPDU is a duplicate.
    let fresh = ap.data_with_sequence(104, Some(0), Some(4), false, IPV4, b"d", PEER);
    ap.queue(fresh);
    let retry = ap.data_with_sequence(104, Some(0), Some(4), true, IPV4, b"d", PEER);
    ap.queue(retry);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    let payloads: Vec<&[u8]> = delivered.iter().map(|frame| &frame[14..]).collect();
    assert_eq!(payloads, [b"a".as_slice(), b"b", b"c", b"d"]);
    let counters = station.connection().unwrap().counters();
    assert_eq!(counters.replayed, 1);
    assert_eq!(counters.duplicates, 1);

    // A BlockAckReq moves the window past a gap and releases what waited.
    let frame = ap.data_with_sequence(106, Some(0), Some(5), false, IPV4, b"f", PEER);
    ap.queue(frame);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(delivered.len(), 4);
    let bar = ap.block_ack_request(0, 107);
    ap.queue(bar);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(&delivered[4][14..], b"f");
}

#[test]
fn a_reorder_gap_releases_the_buffered_run_after_its_timeout() {
    on_large_stack(a_reorder_gap_releases_the_buffered_run_after_its_timeout_body);
}

fn a_reorder_gap_releases_the_buffered_run_after_its_timeout_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    let mut delivered = Vec::new();
    let request = ap.addba_request(0, 8, 100);
    ap.queue(request);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);

    // Sequence 100 never arrives: 101 and 102 wait behind it.
    for (sequence, packet_number, payload) in [(101, 2, b"b"), (102, 3, b"c")] {
        let frame = ap.data_with_sequence(
            sequence,
            Some(0),
            Some(packet_number),
            false,
            IPV4,
            payload,
            PEER,
        );
        ap.queue(frame);
    }
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    world.run_for(&mut ap, &mut station, 290, &mut delivered);
    assert!(delivered.is_empty());
    assert_eq!(
        station
            .connection()
            .unwrap()
            .counters()
            .reorder_gap_timeouts,
        0
    );

    // The gap timeout, 300 ms from the first buffered MPDU, releases them.
    world.run_for(&mut ap, &mut station, 10, &mut delivered);
    let payloads: Vec<&[u8]> = delivered.iter().map(|frame| &frame[14..]).collect();
    assert_eq!(payloads, [b"b", b"c"]);
    let counters = station.connection().unwrap().counters();
    assert_eq!(counters.reorder_gap_timeouts, 1);

    // The window moved past the gap: a late 100 is behind it, and 103 is
    // in order.
    let late = ap.data_with_sequence(100, Some(0), Some(1), false, IPV4, b"a", PEER);
    ap.queue(late);
    let next = ap.data_with_sequence(103, Some(0), Some(4), false, IPV4, b"d", PEER);
    ap.queue(next);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(&delivered[2][14..], b"d");
    assert_eq!(delivered.len(), 3);
    assert_eq!(station.connection().unwrap().counters().behind_window, 1);
}

#[test]
fn power_save_dozes_and_wakes_for_buffered_traffic_at_a_tbtt() {
    on_large_stack(power_save_dozes_and_wakes_for_buffered_traffic_at_a_tbtt_body);
}

fn power_save_dozes_and_wakes_for_buffered_traffic_at_a_tbtt_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    world
        .drive(&mut ap, station.enable_power_save(SleepType::MinModem))
        .unwrap();
    let mut delivered = Vec::new();

    // The access point beacons 10 ms from now, then every 102.4 ms. Once
    // the station parsed a beacon and idled past the active timeout, it
    // sends a Null with PM=1 and dozes: the transmit gate closes.
    let start = world.timer.now.get();
    ap.next_beacon_micros = Some(start + 10_000);
    world.run_for(&mut ap, &mut station, 100, &mut delivered);
    ap.absorb(world.model);
    let state = station.connection().unwrap().power_save().unwrap().state();
    assert_eq!(ap.nulls, [true], "{state:?}");
    assert_eq!(state, PmState::Dozing);
    assert!(!world.model.gate_open());

    // The access point buffers traffic: at its next TBTT the station wakes
    // for the beacon, finds its AID in the TIM and leaves power save.
    ap.tim_unicast = true;
    world.run_for(&mut ap, &mut station, 20, &mut delivered);
    ap.absorb(world.model);
    let state = station.connection().unwrap().power_save().unwrap().state();
    assert_eq!(ap.beacons_sent, 2);
    assert_eq!(ap.nulls, [true, false], "{state:?}");
    assert_eq!(state, PmState::Awake);
    assert!(world.model.gate_open());
    ap.tim_unicast = false;
    ap.next_beacon_micros = None;
    let frame = ap.data(None, None, false, IPV4, b"buffered", PEER);
    ap.queue(frame);
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(delivered.len(), 1);
    assert_eq!(&delivered[0][14..], b"buffered");
}

#[test]
fn a_deauthentication_by_the_access_point_ends_the_connection() {
    on_large_stack(a_deauthentication_by_the_access_point_ends_the_connection_body);
}

fn a_deauthentication_by_the_access_point_ends_the_connection_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    let deauthentication = ap.deauthentication(7);
    ap.queue(deauthentication);
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        Some(PortDisconnect::Deauthenticated { reason_code: 7 })
    );
    assert!(station.connection().is_none());
    let vif = world.model.vif_config(VifId(0)).unwrap();
    assert_eq!((vif.bssid, vif.receive), (None, ReceiveFilter::NONE));
    // The station itself sent no Deauthentication.
    assert!(!ap.deauthenticated);
}

#[test]
fn an_sae_authentication_joins_a_wpa3_access_point() {
    on_large_stack(an_sae_authentication_joins_a_wpa3_access_point_body);
}

fn an_sae_authentication_joins_a_wpa3_access_point_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let commit = SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(PASSPHRASE, AP, STA).unwrap(),
        [0x40; 32],
        [0x41; 32],
    )
    .unwrap();
    let mut ap = ScriptedAp::new(ApSecurity::Sae).with_sae_commit(commit);
    let security = StaAttemptSecurity::new(
        StaPersonalCredentials::wpa3(SaePassword::new(PASSPHRASE).unwrap(), sae_random, &PMKSA),
        SNONCE,
        counters(),
        Wpa2Message4Protection::Unprotected,
    );
    let mut station = connect(&world, &mut ap, world.station(security));
    ap.absorb(world.model);
    assert!(station.report().sae);
    assert_eq!(ap.authentications, 2);
    assert!(ap.handshake_complete);
    assert!(station.connection().unwrap().config().management_protection);
    let keys = station.connection().unwrap().keys().unwrap();

    // Under management frame protection an unprotected Deauthentication
    // may be forged: the station asks with a protected SA Query Request
    // instead of leaving.
    let forged = ap.deauthentication(7);
    ap.queue(forged);
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    let request = ap.actions.last().unwrap().clone();
    assert_eq!(&request[..2], &[8, 0]);
    let query = world.model.submitted().into_iter().last().unwrap();
    assert_eq!(query.key, KeySelector::Key(keys.pairwise));
    assert_ne!(query.frames[0][1] & 0x40, 0);
    // The access point confirms the association with a protected Response;
    // the procedure's timeout then passes without ending it.
    let mut response = ap.management(0xd0, STA);
    response[1] |= 0x40;
    response.extend_from_slice(&[1, 0, 0, 0x20, 0, 0, 0, 0]);
    response.extend_from_slice(&[8, 1, request[2], request[3]]);
    ap.queue(response);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 1_500, &mut delivered),
        None
    );
    assert!(station.connection().is_some());
}

/// Serves the connection: stops after its second connection.
struct Application<'a> {
    connections: &'a Cell<u32>,
    disconnects: &'a RefCell<Vec<PortDisconnect>>,
    stop_after: u32,
}

impl PortStationApplication for Application<'_> {
    fn stop_requested(&mut self) -> bool {
        self.connections.get() >= self.stop_after
    }

    fn deliver(&mut self, _ethernet: &[u8]) {}

    fn connected(&mut self, _config: &oer_ieee80211_sta_service::port::PortConnectionConfig) {
        self.connections.set(self.connections.get() + 1);
    }

    fn disconnected(&mut self, reason: PortDisconnect) {
        self.disconnects.borrow_mut().push(reason);
    }
}

#[test]
fn the_lifecycle_rejoins_the_same_access_point_after_a_deauthentication() {
    on_large_stack(the_lifecycle_rejoins_the_same_access_point_after_a_deauthentication_body);
}

fn the_lifecycle_rejoins_the_same_access_point_after_a_deauthentication_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let connections = Cell::new(0);
    let disconnects = RefCell::new(Vec::new());
    let mut service = StaLifecycleService::new(
        PortStationLifecycle::new(
            Application {
                connections: &connections,
                disconnects: &disconnects,
                stop_after: 2,
            },
            Duration::from_millis(1),
        ),
        StaReconnectPolicy::new(3, 10, 100, 20).unwrap(),
    );
    let mut sent_deauthentication = false;
    let exit = {
        let future = service.run(world.station(open()));
        let mut future = pin!(future);
        let mut routing = pin!(world.router.run());
        let mut context = Context::from_waker(Waker::noop());
        loop {
            world.timer.wanted.set(None);
            if let Poll::Ready(exit) = future.as_mut().poll(&mut context) {
                break exit;
            }
            assert!(routing.as_mut().poll(&mut context).is_pending());
            // The access point ends the first association once it is up.
            if !sent_deauthentication && connections.get() == 1 {
                let deauthentication = ap.deauthentication(1);
                ap.queue(deauthentication);
                sent_deauthentication = true;
                continue;
            }
            if ap.step(world.model, world.timer.now.get()) {
                continue;
            }
            let next = world.timer.wanted.get().expect("a deadline");
            world.advance_to(next.max(world.timer.now.get()));
        }
    };
    let StaLifecycleExit::Stopped { progress, .. } = exit else {
        panic!("the lifecycle did not stop");
    };
    assert_eq!(progress.connected_epochs, 1);
    assert_eq!(connections.get(), 2);
    assert_eq!(
        *disconnects.borrow(),
        [PortDisconnect::Deauthenticated { reason_code: 1 }]
    );
    // The second attempt reused the candidate: one scan's worth of Probe
    // Requests, and a Deauthentication when the station stopped.
    ap.absorb(world.model);
    assert_eq!(ap.probe_requests.len(), 3);
    assert_eq!(ap.associations, 2);
    assert!(ap.deauthenticated);
}

/// Route every earlier event and drop what the station has not read, then
/// deliver `frames` to the model at once, past its event queue.
fn burst<'a>(
    world: &'a World,
    ap: &mut ScriptedAp,
    station: &mut PortStation<'a, Env<'a>>,
    frames: Vec<Vec<u8>>,
) {
    let mut routing = pin!(world.router.run());
    assert!(
        routing
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert_eq!(world.model.queued_events(), 0);
    world.drive(ap, station.link_mut().discard_backlog());
    for frame in frames {
        world.model.receive(&frame, scripted_ap::meta(false));
    }
}

#[test]
fn a_receive_loss_is_skipped_and_the_connection_goes_on() {
    on_large_stack(a_receive_loss_is_skipped_and_the_connection_goes_on_body);
}

fn a_receive_loss_is_skipped_and_the_connection_goes_on_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // Six frames at once: the model's queue holds four, then a loss.
    let frames = (0..6)
        .map(|index| ap.data(None, None, false, IPV4, &[b'a' + index; 16], PEER))
        .collect();
    burst(&world, &mut ap, &mut station, frames);
    let mut delivered = Vec::new();
    for _ in 0..20 {
        assert_eq!(
            world.run_for(&mut ap, &mut station, 5, &mut delivered),
            None
        );
    }
    // The four frames before the gap are delivered; the two in it are gone.
    assert_eq!(delivered.len(), 4);
    assert!(station.link().counters().events_lost >= 1);

    // The station goes on receiving and sending after the gap.
    let frame = ap.data(None, None, false, IPV4, b"after", PEER);
    ap.queue(frame);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert_eq!(&delivered.last().unwrap()[14..], b"after");
    assert_eq!(
        station.send(&ethernet(PEER, IPV4, b"up"), 0),
        Ok(PortSend::Queued)
    );
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert_eq!(station.connection().unwrap().tx_counters().acknowledged, 1);
}

#[test]
fn a_completion_lost_in_a_gap_fails_the_send_and_the_next_one_goes_out() {
    on_large_stack(a_completion_lost_in_a_gap_fails_the_send_and_the_next_one_goes_out_body);
}

fn a_completion_lost_in_a_gap_fails_the_send_and_the_next_one_goes_out_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // The model's queue is full when the data frame's completion arrives:
    // the completion falls into the gap. The station cancels its attempt,
    // which already ended, and the exchange ends without a report.
    let frames = (0..5)
        .map(|index| ap.data(None, None, false, IPV4, &[b'a' + index], PEER))
        .collect();
    burst(&world, &mut ap, &mut station, frames);
    assert_eq!(
        station.send(&ethernet(PEER, IPV4, b"lost"), 0),
        Ok(PortSend::Queued)
    );
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    let counters = station.connection().unwrap().tx_counters();
    assert_eq!(
        counters.failed, 1,
        "the completion in the gap is counted lost"
    );
    assert_eq!(
        station.send(&ethernet(PEER, IPV4, b"next"), 0),
        Ok(PortSend::Queued)
    );
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    let counters = station.connection().unwrap().tx_counters();
    assert_eq!((counters.mpdus, counters.acknowledged), (1, 1));
}

#[test]
fn a_poisoned_port_ends_the_connection_and_every_send() {
    on_large_stack(a_poisoned_port_ends_the_connection_and_every_send_body);
}

fn a_poisoned_port_ends_the_connection_and_every_send_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    world.model.poison();
    let deadline = world
        .timer
        .now()
        .checked_add(Duration::from_millis(5))
        .unwrap();
    let ran = world.drive(&mut ap, station.run_until(deadline, &mut |_| {}));
    assert!(matches!(ran, Err(PortLinkError::Poisoned)));
    assert!(world.router.poisoned());
    // A frame still queues; sending it ends on the poisoned port.
    assert_eq!(
        station.send(&ethernet(PEER, IPV4, b"late"), 0),
        Ok(PortSend::Queued)
    );
    let ran = world.drive(&mut ap, station.run_until(deadline, &mut |_| {}));
    let Err(PortLinkError::Tx(UpperMacTxError::Port(error))) = ran else {
        panic!("sending fails on the poisoned port: {ran:?}");
    };
    assert!(oer_ieee80211_lower_mac::PortError::is_poisoned(&error));
}

#[test]
fn the_access_point_replaces_the_group_key_through_the_port() {
    on_large_stack(the_access_point_replaces_the_group_key_through_the_port_body);
}

fn the_access_point_replaces_the_group_key_through_the_port_body() {
    use oer_ieee80211_rsn::EapolKeyMessage;
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    ap.absorb(world.model);
    let before = station.connection().unwrap().keys().unwrap();
    assert_eq!(before.group_key_id, 1);
    let group_message2 = |ap: &ScriptedAp| {
        ap.eapol_from_station
            .iter()
            .filter(|message| **message == EapolKeyMessage::GroupMessage2)
            .count()
    };
    let mut delivered = Vec::new();

    // A Group Message 1 under the pairwise key installs the new group key,
    // removes the old one and is answered under the pairwise key.
    let rsc = [5, 0, 0, 0, 0, 0, 0, 0];
    let message1 = ap.group_message1(2, 0x77, rsc);
    let frame = ap.eapol_to_station(&message1, Some(1));
    ap.queue(frame);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    assert_eq!(group_message2(&ap), 1);
    let answer = world.model.submitted().into_iter().last().unwrap();
    assert_eq!(answer.key, KeySelector::Key(before.pairwise));
    assert_ne!(answer.frames[0][1] & 0x40, 0);
    let connection = station.connection().unwrap();
    let after = connection.keys().unwrap();
    assert_eq!(after.group_key_id, 2);
    assert_eq!(after.group_receive_sequence, rsc);
    assert_ne!(after.group, before.group);
    assert_eq!(after.pairwise, before.pairwise);
    assert_eq!(connection.counters().group_rekeys, 1);
    // EAPOL of the handshake never reaches the caller.
    assert!(delivered.is_empty());

    // The same Group Message 1 again is answered again; the key stays.
    let frame = ap.eapol_to_station(&message1, Some(2));
    ap.queue(frame);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    assert_eq!(group_message2(&ap), 2);
    let connection = station.connection().unwrap();
    assert_eq!(connection.keys().unwrap(), after);
    assert_eq!(connection.counters().group_rekeys, 1);

    // An unprotected Group Message 1 after the keys is dropped unanswered.
    let forged = ap.group_message1(1, 0x99, [9, 0, 0, 0, 0, 0, 0, 0]);
    let frame = ap.eapol_to_station(&forged, None);
    ap.queue(frame);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    assert_eq!(group_message2(&ap), 2);
    let connection = station.connection().unwrap();
    assert_eq!(connection.keys().unwrap(), after);
    assert_eq!(connection.counters().unprotected, 1);
}

#[test]
fn the_station_contends_with_the_access_point_s_edca_parameters() {
    on_large_stack(the_station_contends_with_the_access_point_s_edca_parameters_body);
}

fn the_station_contends_with_the_access_point_s_edca_parameters_body() {
    use oer_ieee80211_mac::{extensions::wmm::parse_wmm_parameter_element, qos::WmmAccessCategory};
    let advertised = scripted_ap::wmm_parameter_element(
        1,
        [(3, 4, 10, 0), (7, 4, 10, 0), (2, 3, 4, 94), (2, 2, 3, 47)],
    );
    let associated = scripted_ap::wmm_parameter_element(
        2,
        [(5, 5, 10, 0), (7, 4, 10, 0), (2, 3, 4, 94), (2, 2, 3, 47)],
    );

    // Only the beacon carries the parameters: the station keeps them.
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.wmm_beacon = Some(advertised.clone());
    let _station = connect(&world, &mut ap, world.station(open()));
    let expected = parse_wmm_parameter_element(&advertised).unwrap();
    assert_eq!(world.model.edca(), Some(expected));

    // The Association Response's set replaces the advertised one.
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.wmm_beacon = Some(advertised);
    ap.wmm_association = Some(associated.clone());
    let _station = connect(&world, &mut ap, world.station(open()));
    let expected = parse_wmm_parameter_element(&associated).unwrap();
    assert_eq!(world.model.edca(), Some(expected));
    assert_eq!(
        expected
            .access_category(WmmAccessCategory::BestEffort)
            .aifsn,
        5
    );
}

#[test]
fn the_station_negotiates_its_tx_block_ack_agreements() {
    on_large_stack(the_station_negotiates_its_tx_block_ack_agreements_body);
}

fn the_station_negotiates_its_tx_block_ack_agreements_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    ap.ht = true;
    let mut station = connect(
        &world,
        &mut ap,
        world.station_with(wpa2(&PMKSA), block_ack_profile()),
    );
    let requests = |ap: &ScriptedAp| {
        ap.actions
            .iter()
            .filter(|action| action.starts_with(&[3, 0]))
            .map(|action| {
                (
                    action[2],
                    (u16::from_le_bytes([action[3], action[4]]) >> 2) as u8 & 0x0f,
                )
            })
            .collect::<Vec<_>>()
    };
    let mut delivered = Vec::new();

    // Connected, the station asks for TID 0 first.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    let first = requests(&ap);
    assert_eq!(first.first().map(|request| request.1), Some(0));

    // The access point agrees; the station then asks for TID 7.
    let mut response = ap.management(0xd0, STA);
    response.extend_from_slice(&scripted_ap::addba_response(first[0].0, 0, 0, 16));
    ap.queue(response);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    let originator = station.connection().unwrap().tx_block_ack().unwrap();
    assert_eq!(
        originator.operational(0).map(|agreement| agreement.window),
        Some(16)
    );
    assert!(requests(&ap).iter().any(|request| request.1 == 7));

    // Unanswered, TID 7 is asked again once after its timeout, then given up.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 1_000, &mut delivered),
        None
    );
    ap.absorb(world.model);
    let tid7 = requests(&ap)
        .iter()
        .filter(|request| request.1 == 7)
        .count();
    assert_eq!(tid7, 2);

    // A DELBA of the access point, as recipient, ends the TID 0 agreement.
    let mut delba = ap.management(0xd0, STA);
    delba.extend_from_slice(&[3, 2, 0x00, 0x00, 1, 0]);
    ap.queue(delba);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    let originator = station.connection().unwrap().tx_block_ack().unwrap();
    assert_eq!(originator.operational(0), None);
}

/// An HT station profile that originates the Espressif TX Block Ack
/// agreements.
fn block_ack_profile() -> PortStationProfile<'static> {
    use oer_ieee80211_mac::station::AssociationCapabilities;
    use oer_ieee80211_sta::block_ack::{StaTxBlockAckConfig, StaTxBlockAckPolicy};
    use oer_ieee80211_sta_service::port::PortTxBlockAck;
    static HT: AssociationCapabilities = AssociationCapabilities {
        ht20: scripted_ap::HT_CAPABILITIES,
        ..CAPABILITIES
    };
    let mut profile = profile();
    profile.capabilities = &HT;
    profile.phy = PhyMode::Ht20;
    profile.tx_block_ack = Some(PortTxBlockAck {
        policy: StaTxBlockAckPolicy {
            tids: &oer_espressif_ieee80211_policy::block_ack::STA_TX_BLOCK_ACK_TIDS,
            first_dialog_token: oer_espressif_ieee80211_policy::block_ack::FIRST_DIALOG_TOKEN,
            next_dialog_token: oer_espressif_ieee80211_policy::block_ack::next_dialog_token,
        },
        config: StaTxBlockAckConfig {
            window: 32,
            negotiation_timeout: Duration::from_millis(100),
            amsdu_tids: 0,
        },
        attempt_limit: 2,
    });
    profile
}

#[test]
fn queued_frames_of_an_agreed_tid_leave_as_one_a_mpdu() {
    on_large_stack(queued_frames_of_an_agreed_tid_leave_as_one_a_mpdu_body);
}

fn queued_frames_of_an_agreed_tid_leave_as_one_a_mpdu_body() {
    use oer_ieee80211_mac::phy::{HtMcs, HtRate, PpduBandwidth};
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let ht = PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap());
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    ap.ht = true;
    let mut station = connect(
        &world,
        &mut ap,
        world.station_at(wpa2(&PMKSA), block_ack_profile(), ht),
    );
    let mut delivered = Vec::new();
    // The access point agrees to TID 0.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    ap.absorb(world.model);
    let request = ap
        .actions
        .iter()
        .find(|action| action.starts_with(&[3, 0]))
        .unwrap()
        .clone();
    let mut response = ap.management(0xd0, STA);
    response.extend_from_slice(&scripted_ap::addba_response(request[2], 0, 0, 16));
    ap.queue(response);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert!(
        station
            .connection()
            .unwrap()
            .tx_block_ack()
            .unwrap()
            .operational(0)
            .is_some()
    );
    let before = world.model.submitted().len();

    // Four frames of TID 0 leave as one A-MPDU; the voice frame alone.
    for payload in [b"a".as_slice(), b"b", b"c", b"d"] {
        assert_eq!(
            station.send(&ethernet(PEER, IPV4, payload), 0),
            Ok(PortSend::Queued)
        );
    }
    assert_eq!(
        station.send(&ethernet(PEER, IPV4, b"voice"), 6),
        Ok(PortSend::Queued)
    );
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    let submitted = world.model.submitted();
    let data: Vec<_> = submitted[before..]
        .iter()
        .filter(|attempt| attempt.frames[0][0] == 0x88)
        .collect();
    assert_eq!(data.len(), 2);
    assert!(data[0].ampdu && data[0].frames.len() == 4);
    assert!(!data[1].ampdu);
    let counters = station.connection().unwrap().tx_counters();
    assert_eq!(
        (counters.aggregates, counters.subframes, counters.mpdus),
        (1, 4, 1)
    );
    assert_eq!(counters.acknowledged, 5);
    // The subframes carry consecutive sequence numbers of TID 0.
    let sequences: Vec<u16> = data[0]
        .frames
        .iter()
        .map(|frame| u16::from_le_bytes([frame[22], frame[23]]) >> 4)
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1));
}

#[test]
fn group_robust_management_counts_only_when_it_verifies_under_the_igtk() {
    on_large_stack(group_robust_management_counts_only_when_it_verifies_under_the_igtk_body);
}

fn group_robust_management_counts_only_when_it_verifies_under_the_igtk_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let commit = SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(PASSPHRASE, AP, STA).unwrap(),
        [0x40; 32],
        [0x41; 32],
    )
    .unwrap();
    let mut ap = ScriptedAp::new(ApSecurity::Sae).with_sae_commit(commit);
    let security = StaAttemptSecurity::new(
        StaPersonalCredentials::wpa3(SaePassword::new(PASSPHRASE).unwrap(), sae_random, &PMKSA),
        SNONCE,
        counters(),
        Wpa2Message4Protection::Unprotected,
    );
    let mut station = connect(&world, &mut ap, world.station(security));
    assert!(station.connection().unwrap().config().management_protection);
    let mut bip = scripted_ap::bip_transmitter();
    let mut delivered = Vec::new();

    // A broadcast Deauthentication without a Management MIC element, one
    // whose MIC fails and a group SA Query without one are all dropped.
    let mut plain = ap.management(0xc0, [0xff; 6]);
    plain.extend_from_slice(&7_u16.to_le_bytes());
    ap.queue(plain);
    let mut forged = ap.bip_deauthentication(&mut bip, 7);
    *forged.last_mut().unwrap() ^= 1;
    ap.queue(forged);
    let mut action = ap.management(0xd0, [0xff; 6]);
    action.extend_from_slice(&[8, 0, 1, 2]);
    ap.queue(action);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert_eq!(station.connection().unwrap().counters().bip_rejected, 3);

    // A verified one ends the association.
    let verified = ap.bip_deauthentication(&mut bip, 3);
    ap.queue(verified);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        Some(PortDisconnect::Deauthenticated { reason_code: 3 })
    );
    assert!(station.connection().is_none());
}
