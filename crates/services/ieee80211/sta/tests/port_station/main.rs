//! The station over the lower-MAC port against `oer-ieee80211-lower-mac`'s
//! host model and a scripted access point: scan, Open System and SAE joins,
//! the WPA2-PSK handshake with keys installed through the port, the data
//! plane both ways with a Block Ack window, replay and duplicate checks,
//! power save around a TBTT, and the end of the association.

mod scripted_ap;

use core::{
    cell::Cell,
    future::Future,
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee80211_upper_mac::rate_control::{RateControl, RatePeer};
use oer_time_virtual::VirtualClock;
use std::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
    vec::Vec,
};

use oer_ieee80211_datapath::{SoftwareTxFrame, memory::MemoryTxQueues};
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, CoexPriority, Ieee80211LowerMacPort, KeySelector, LifecycleCommand,
    LowerMacSetting, ReceiveFilter, RxBeaconPriority, TxPower, VifId,
    model::{LowerMacModel, ModelOutcome, ModelRxBuffer},
};
use oer_ieee80211_mac::{
    ccmp::CcmpPacketNumberStep,
    phy::{LegacyRate, PhyRate},
    scan::ScanTable,
    sequence::SequenceNumber,
    station::{
        AssociationCapabilities, StaTxSequenceCounters,
        association::{PhyMode, Preference},
    },
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
    modem_sleep::{CoexView, PmCoexAction, PmCoexEvent, PmState, SleepType},
    pmksa::StaSharedPmksa,
    scan::{StaCandidateScanExit, StaScanConfig},
    station::{StaLifecycleExit, StaReconnectPolicy},
};
use oer_ieee80211_sta_service::{
    port::{
        EventRouter, PortChannelSwitch, PortCoexistence, PortCoexistenceRefused, PortConnection,
        PortConnectionFrame, PortDisconnect, PortLink, PortLinkError, PortLinkSupervision,
        PortProbe, PortRouter, PortScan, PortScanTarget, PortStation, PortStationApplication,
        PortStationConfig, PortStationEnv, PortStationEvent, PortStationLifecycle,
        PortStationProfile, PortStationStorage,
    },
    scan::{StaCandidateScanService, StaScanBackend},
    station::StaLifecycleService,
};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, FixedRate, ProtectEveryHeTxop, ProtectionPolicy, RetryLimits, TxPlanner,
};
use oer_ieee80211_upper_mac_service::client::{PortClientEnv, PortMsdu};
use oer_ieee80211_upper_mac_service::frame::{NetworkBody, PORT_FRAME_CAPACITY};
use oer_ieee80211_upper_mac_service::{
    aggregate::{AmpduSubframes, PORT_AMPDU_SUBFRAMES},
    reorder::PORT_REORDER_SLOTS,
};
use oer_network_interface::NetworkInterfaceId;
use oer_time::{Clock, Duration, Instant, RadioInstant};

use scripted_ap::{AP, AP_CHANNEL, ApSecurity, PASSPHRASE, RATES, SNONCE, SSID, STA, ScriptedAp};

/// Run `body` on a thread whose stack holds the station's unoptimized
/// futures.
fn on_large_stack(body: fn()) {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
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

/// One of the network's frames: dropping it returns its owner.
struct TestFrame(Vec<u8>);

impl SoftwareTxFrame for TestFrame {
    fn interface(&self) -> NetworkInterfaceId {
        NetworkInterfaceId::new(0)
    }

    fn ethernet(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for TestFrame {
    fn drop(&mut self) {
        RETURNED.with(|returned| returned.set(returned.get() + 1));
    }
}

/// The network's queues the station takes its frames from.
type TestFrames = MemoryTxQueues<TestFrame, 32>;

/// The model, whose bodies are the network's frames.
type Model = LowerMacModel<NetworkBody<TestFrame>>;

thread_local! {
    /// Frames returned to the network: sent, dropped or released.
    static RETURNED: Cell<usize> = const { Cell::new(0) };
}

fn returned() -> usize {
    RETURNED.with(Cell::get)
}

struct Env<'a>(PhantomData<&'a ()>);

/// What the station told its rate control.
#[derive(Default)]
struct RateLog {
    /// The link metric each association started from.
    link_metrics: Vec<Option<i8>>,
    /// Each single MPDU's attempts and acknowledgement.
    mpdus: Vec<(u8, bool)>,
    /// Each A-MPDU's subframes and acknowledged subframes.
    ampdus: Vec<(u16, u16)>,
}

/// One fixed rate that records what the station tells it.
struct ScriptedRate<'a> {
    rate: PhyRate,
    log: &'a RefCell<RateLog>,
}

impl<'a> RateControl for ScriptedRate<'a> {
    type Config = (PhyRate, &'a RefCell<RateLog>);

    fn for_peer(
        (rate, log): (PhyRate, &'a RefCell<RateLog>),
        _peer: &RatePeer,
        link_metric: Option<i8>,
    ) -> Self {
        log.borrow_mut().link_metrics.push(link_metric);
        Self { rate, log }
    }

    fn mpdu_rate(&self) -> PhyRate {
        self.rate
    }

    fn ampdu_rate(&self) -> PhyRate {
        self.rate
    }

    fn observe_mpdu(&mut self, attempts: u8, acknowledged: bool, _ack_snr_db: Option<i8>) {
        self.log.borrow_mut().mpdus.push((attempts, acknowledged));
    }

    fn observe_ampdu(
        &mut self,
        _now: oer_time::Instant,
        attempted: u16,
        acknowledged: u16,
        _ack_snr_db: Option<i8>,
    ) {
        self.log.borrow_mut().ampdus.push((attempted, acknowledged));
    }
}

/// The radio system's coexistence schedule as a script: the view the test
/// sets, the effects the station asked for, and whether it refuses them.
struct ScriptedCoex {
    view: Cell<CoexView>,
    performed: RefCell<Vec<PmCoexAction>>,
    refuse: Cell<bool>,
    /// Its reconnect policy is on: connection frames carry the elevated
    /// priority.
    reconnect: Cell<bool>,
    /// The connection frames the station asked the air for.
    connection_frames: RefCell<Vec<PortConnectionFrame>>,
}

impl PortCoexistence for &ScriptedCoex {
    fn view(&self) -> CoexView {
        self.view.get()
    }

    async fn perform(&mut self, action: PmCoexAction) -> Result<(), PortCoexistenceRefused> {
        if self.refuse.get() {
            return Err(PortCoexistenceRefused);
        }
        self.performed.borrow_mut().push(action);
        Ok(())
    }

    async fn connection_frame(&mut self, frame: PortConnectionFrame) -> bool {
        self.connection_frames.borrow_mut().push(frame);
        self.reconnect.get() && frame != PortConnectionFrame::ProbeRequest
    }
}

impl PortClientEnv for Env<'_> {
    type NetworkFrame = TestFrame;
    type Port = Model;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = oer_ieee80211_upper_mac_service::aggregate::PortAmpduAggregation;
}

impl<'a> PortStationEnv for Env<'a> {
    type Timer = &'a VirtualClock;
    type KeyUnwrap = RsnSoftwareAes;
    type Coex = &'a ScriptedCoex;
    type RateControl = ScriptedRate<'a>;
    type Frames = TestFrames;
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
    model: &'static Model,
    router: &'static PortRouter<'static, Env<'static>>,
    timer: VirtualClock,
    coex: &'static ScriptedCoex,
    rate_log: &'static RefCell<RateLog>,
    frames: &'static TestFrames,
}

impl World {
    fn new() -> Self {
        let model = Model::new();
        model.set_now(RadioInstant::from_micros(1_000));
        model
            .apply(LowerMacSetting::Channel(channel(1)))
            .unwrap()
            .unwrap();
        model.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
        // Every published attempt succeeds at once.
        model.respond(core::iter::repeat_n(ModelOutcome::Success, 100_000));
        let model: &'static Model = Box::leak(Box::new(model));
        Self {
            model,
            router: Box::leak(Box::new(EventRouter::new(model, 1))),
            rate_log: Box::leak(Box::new(RefCell::new(RateLog::default()))),
            frames: Box::leak(Box::new(TestFrames::new())),
            coex: Box::leak(Box::new(ScriptedCoex {
                view: Cell::new(CoexView::INACTIVE),
                performed: RefCell::new(Vec::new()),
                refuse: Cell::new(false),
                reconnect: Cell::new(false),
                connection_frames: RefCell::new(Vec::new()),
            })),
            timer: VirtualClock::starting_at(Instant::from_micros(1_000)),
        }
    }

    /// Move virtual time to `micros`: the station's timer and the model's
    /// radio clock read one time.
    fn advance_to(&self, micros: u64) {
        self.timer.advance_to(Instant::from_micros(micros));
        self.model
            .set_now(RadioInstant::from_micros(self.timer.now().as_micros()));
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
            self.coex,
            (data_rate, self.rate_log),
            PortStationConfig {
                vif: VifId(0),
                address: STA,
                management_rate: MANAGEMENT_RATE,
                power: TxPower::Calibrated,
                coex: CoexPriority::Normal,
                retry_limit: 7,
            },
        )
        .expect("the station's interface is free")
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
            self.link_at(data_rate),
            &self.timer,
            RsnSoftwareAes,
            profile,
            security,
            Box::leak(Box::new(PortStationStorage::new())),
            self.frames,
        )
    }

    /// Hand the station the network's Ethernet-II frame `ethernet`.
    fn send(&self, ethernet: &[u8]) {
        assert!(self.frames.push(TestFrame(ethernet.to_vec())).is_ok());
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
            // Resample the deadlines of live futures at this same instant.
            self.timer.advance_to(self.timer.now());
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            if routing.as_mut().poll(&mut context).is_ready() {
                // The router ended on the terminal poisoned event; the
                // station learns of it at its next poll.
                routing.set(self.router.run());
                continue;
            }
            if ap.step(self.model, self.timer.now().as_micros()) {
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
            let wanted = self.timer.next_deadline().map(Instant::as_micros);
            let next = match (wanted, ap.next_beacon_micros) {
                (Some(a), Some(b)) => a.min(b),
                (a, b) => a
                    .or(b)
                    .expect("the station waits for an event nothing produces"),
            };
            self.advance_to(next.max(self.timer.now().as_micros()));
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
        self.run_events(ap, station, millis, delivered)
            .map(|event| match event {
                PortStationEvent::Ended(disconnect) => disconnect,
                PortStationEvent::ChannelSwitch(switch) => panic!("unexpected {switch:?}"),
            })
    }

    /// Run the connected station for `millis`, or until it reports.
    fn run_events(
        &self,
        ap: &mut ScriptedAp,
        station: &mut PortStation<'_, Env<'_>>,
        millis: u32,
        delivered: &mut Vec<Vec<u8>>,
    ) -> Option<PortStationEvent> {
        let deadline = self
            .timer
            .now()
            .checked_add(Duration::from_millis(millis))
            .unwrap();
        self.drive(
            ap,
            station.run_until(deadline, &mut |msdu| delivered.push(ethernet_of(&msdu))),
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
        link: PortLinkSupervision {
            timeout: oer_espressif_ieee80211_policy::station_link::STATION_INACTIVE_TIME,
            miss_limit: 10,
            probe: oer_espressif_ieee80211_policy::station_link::STATION_LINK_PROBE,
            supported_rates: &RATES,
        },
        sleep_type: SleepType::None,
        rx_reorder_gap: Duration::from_micros(
            oer_espressif_ieee80211_policy::block_ack::RX_REORDER_GAP_TIMEOUT_MICROS,
        ),
        preference: Preference::Automatic,
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

/// The Ethernet-II frame of a delivered MSDU.
fn ethernet_of(msdu: &PortMsdu<'_, ModelRxBuffer>) -> Vec<u8> {
    let parts = msdu.parts();
    let mut ethernet = vec![0; parts.length()];
    parts.copy_to(&mut ethernet).unwrap();
    ethernet
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

/// An IPv4 payload marked Expedited Forwarding (DSCP 46): user priority 6.
fn voice(payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x45, 46 << 2];
    packet.extend_from_slice(payload);
    packet
}

#[test]
fn harness_advancement_keeps_radio_and_monotonic_time_together() {
    let world = World::new();
    world.advance_to(2_000);
    world.advance_to(1_500);
    assert_eq!(world.timer.now(), Instant::from_micros(2_000));
    assert_eq!(world.model.now(), Ok(RadioInstant::from_micros(2_000)));
}

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

#[test]
fn owner_observation_has_an_absolute_deadline_and_never_tunes_or_probes() {
    on_large_stack(owner_observation_has_an_absolute_deadline_and_never_tunes_or_probes_body);
}

fn owner_observation_has_an_absolute_deadline_and_never_tunes_or_probes_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = world.station(open());
    let updates = world.model.channel_updates();
    let lifecycle = world.model.lifecycle_requests();
    let until = Instant::from_micros(21_000);
    assert_eq!(
        world
            .drive(&mut ap, station.observe_on_until(channel(1), until))
            .unwrap(),
        None
    );
    assert_eq!(world.timer.now(), until);
    assert_eq!(world.model.channel_updates(), updates);
    assert_eq!(world.model.lifecycle_requests(), lifecycle);
    assert!(world.model.submitted().is_empty());
    assert!(!world.model.monitoring());
    assert_eq!(
        world.model.vif_config(VifId(0)).unwrap().receive,
        ReceiveFilter::NONE
    );

    world
        .model
        .apply(LowerMacSetting::Channel(channel(AP_CHANNEL)))
        .unwrap()
        .unwrap();
    ap.next_beacon_micros = Some(22_000);
    let until = Instant::from_micros(41_000);
    assert_eq!(
        world
            .drive(
                &mut ap,
                station.observe_on_until(channel(AP_CHANNEL), until)
            )
            .unwrap(),
        Some(channel(AP_CHANNEL))
    );
    assert_eq!(world.timer.now(), Instant::from_micros(22_000));
    assert!(world.model.submitted().is_empty());
    assert_eq!(
        world.model.vif_config(VifId(0)).unwrap().receive,
        ReceiveFilter::NONE
    );
}

#[test]
fn passive_recovery_identifies_only_the_known_hidden_bssid_and_joins_without_a_probe() {
    on_large_stack(
        passive_recovery_identifies_only_the_known_hidden_bssid_and_joins_without_a_probe_body,
    );
}

fn passive_recovery_identifies_only_the_known_hidden_bssid_and_joins_without_a_probe_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let AssociationAttemptOutcome::Connected {
        connected: mut station,
        ..
    } = world.drive(&mut ap, world.station(open()).connect())
    else {
        panic!("initial join");
    };
    world.drive(&mut ap, station.disconnect()).unwrap();
    ap.next_beacon_micros = None;
    let mut hidden = ap.beacon(0x80, [0xff; 6], 0);
    assert_eq!(hidden[36], 0);
    let ssid_len = usize::from(hidden[37]);
    hidden.drain(38..38 + ssid_len);
    hidden[37] = 0;
    let mut unknown = hidden.clone();
    unknown[16..22].copy_from_slice(&[0x02, 0, 0, 0, 0, 99]);
    let mut wrong_security = hidden.clone();
    wrong_security[34] |= 0x10;
    for rejected in [unknown, wrong_security] {
        ap.queue(rejected);
        let until = world
            .timer
            .now()
            .checked_add(Duration::from_millis(20))
            .unwrap();
        assert_eq!(
            world
                .drive(
                    &mut ap,
                    station.observe_on_until(channel(AP_CHANNEL), until)
                )
                .unwrap(),
            None
        );
    }
    let probes = ap.probe_requests.len();
    ap.queue(hidden);
    let until = world
        .timer
        .now()
        .checked_add(Duration::from_millis(20))
        .unwrap();
    assert_eq!(
        world
            .drive(
                &mut ap,
                station.observe_on_until(channel(AP_CHANNEL), until)
            )
            .unwrap(),
        Some(channel(AP_CHANNEL))
    );
    // The scan table keeps the beacon's empty SSID; only the join candidate
    // uses the configured SSID of this previously joined BSSID.
    assert!(station.scan_table().records()[0].ssid_bytes().is_empty());
    assert_eq!(station.candidate().unwrap().ssid_bytes(), SSID);
    let outcome = world.drive(&mut ap, station.connect_observed_on(channel(AP_CHANNEL)));
    assert!(matches!(
        outcome,
        AssociationAttemptOutcome::Connected { .. }
    ));
    assert_eq!(ap.probe_requests.len(), probes);
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
    for payload in [b"best".to_vec(), voice(b"voice"), b"again".to_vec()] {
        world.send(&ethernet(PEER, IPV4, &payload));
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
        world.send(&ethernet(PEER, IPV4, payload));
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
    // The window keeps copies: every port buffer went back.
    assert_eq!(world.model.rx_buffers_lent(), 0);
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

/// Coexistence with a Bluetooth LE schedule: one beacon interval per
/// period, half of phase 0 Wi-Fi's.
const ACTIVE_COEX: CoexView = CoexView {
    active: true,
    current_period: 1,
    flexible_period: 1,
    interval: 1024,
    phase0_share_percent: 50,
};

#[test]
fn the_power_manager_asks_the_radio_system_for_the_beacon_window() {
    on_large_stack(the_power_manager_asks_the_radio_system_for_the_beacon_window_body);
}

fn the_power_manager_asks_the_radio_system_for_the_beacon_window_body() {
    let world = World::new();
    world.coex.view.set(ACTIVE_COEX);
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // The power manager runs for the association without being asked.
    // Starting under an active schedule sets its interval to the beacon
    // interval, in 100 us units, and the flexible period.
    let power = station.connection().unwrap().power_save().unwrap();
    assert_eq!(power.state(), PmState::Awake);
    assert_eq!(
        *world.coex.performed.borrow(),
        [
            PmCoexAction::SetInterval(1024),
            PmCoexAction::SetFlexiblePeriod(1),
        ]
    );
    // Beacon reception asks for the air at the beacon-window priority.
    assert_eq!(
        world.model.rx_beacon_priority(),
        Some(RxBeaconPriority::BeaconWindow)
    );

    // At each TBTT the station restarts the schedule and requests the air
    // for the beacon window.
    let mut delivered = Vec::new();
    ap.next_beacon_micros = Some(world.timer.now().as_micros() + 10_000);
    world.run_for(&mut ap, &mut station, 250, &mut delivered);
    let performed = world.coex.performed.borrow();
    assert!(performed.contains(&PmCoexAction::RestartPhases));
    assert!(performed.iter().any(|action| matches!(
        action,
        PmCoexAction::Request {
            event: PmCoexEvent::BeaconWindow,
            ..
        }
    )));
}

#[test]
fn a_refused_coexistence_effect_fails_the_power_manager() {
    on_large_stack(a_refused_coexistence_effect_fails_the_power_manager_body);
}

fn a_refused_coexistence_effect_fails_the_power_manager_body() {
    let world = World::new();
    world.coex.view.set(ACTIVE_COEX);
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // A new sleep type restarts the manager, whose stop sets the flexible
    // period; the radio system refuses it.
    world.coex.refuse.set(true);
    assert_eq!(
        world.drive(&mut ap, station.set_sleep_type(SleepType::MinModem)),
        Err(PortLinkError::Coexistence)
    );
}

#[test]
fn an_ht40_access_point_is_joined_on_its_40_mhz_channel() {
    on_large_stack(an_ht40_access_point_is_joined_on_its_40_mhz_channel_body);
}

fn an_ht40_access_point_is_joined_on_its_40_mhz_channel_body() {
    use oer_ieee80211_mac::station::AssociationCapabilities;
    static HT40: AssociationCapabilities = AssociationCapabilities {
        ht20: scripted_ap::HT_CAPABILITIES,
        ht40: scripted_ap::HT_CAPABILITIES,
        ..CAPABILITIES
    };
    let world = World::new();
    let mut profile = profile();
    profile.capabilities = &HT40;

    // A 20 MHz HT access point is joined in HT20 on its primary channel.
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.ht = true;
    let station = connect(&world, &mut ap, world.station_with(open(), profile));
    let peer = station.connection().unwrap().config().peer;
    assert_eq!(peer.phy, PhyMode::Ht20);
    assert_eq!(
        world.model.channel().map(|channel| channel.width()),
        Some(ChannelWidth::Mhz20)
    );

    // A 40 MHz one is joined in HT40, tuned with its secondary channel.
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.ht40 = true;
    let station = connect(&world, &mut ap, world.station_with(open(), profile));
    let peer = station.connection().unwrap().config().peer;
    assert_eq!(peer.phy, PhyMode::Ht40);
    assert_eq!(
        world.model.channel().map(|channel| channel.width()),
        Some(ChannelWidth::Mhz40Above)
    );

    // A station that prefers HT20 stays at 20 MHz.
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.ht40 = true;
    let mut ht20 = profile;
    ht20.preference = Preference::ForceHt20;
    let station = connect(&world, &mut ap, world.station_with(open(), ht20));
    assert_eq!(
        station.connection().unwrap().config().peer.phy,
        PhyMode::Ht20
    );
    assert_eq!(
        world.model.channel().map(|channel| channel.width()),
        Some(ChannelWidth::Mhz20)
    );
}

#[test]
fn an_owner_grant_cannot_be_changed_by_the_joined_candidates_width() {
    on_large_stack(an_owner_grant_cannot_be_changed_by_the_joined_candidates_width_body);
}

fn an_owner_grant_cannot_be_changed_by_the_joined_candidates_width_body() {
    static HT40: AssociationCapabilities = AssociationCapabilities {
        ht20: scripted_ap::HT_CAPABILITIES,
        ht40: scripted_ap::HT_CAPABILITIES,
        ..CAPABILITIES
    };
    let world = World::new();
    let granted = channel(AP_CHANNEL);
    let required = Channel::ghz2_4(AP_CHANNEL, ChannelWidth::Mhz40Above).unwrap();
    world
        .model
        .apply(LowerMacSetting::Channel(granted))
        .unwrap()
        .unwrap();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.ht40 = true;
    let mut profile = profile();
    profile.capabilities = &HT40;
    let outcome = world.drive(
        &mut ap,
        world.station_with(open(), profile).connect_on(granted),
    );
    let AssociationAttemptOutcome::Failed(failure) = outcome else {
        panic!("the candidate's 40 MHz channel was not granted");
    };
    let (mut station, _, _, error, _) = failure.into_parts();
    assert!(
        matches!(error, oer_ieee80211_sta_service::port::PortStationError::ChannelMismatch {
        granted: actual_grant, required: actual_required,
    } if actual_grant == granted && actual_required == required)
    );
    assert_eq!(world.model.channel(), Some(granted));
    assert_eq!(ap.authentications, 0);
    assert_eq!(ap.associations, 0);
    assert_eq!(ap.probe_requests, [AP_CHANNEL]);
    // The same returned owner can join once its composition grants 40 MHz.
    world
        .drive(&mut ap, station.link_mut().retune(required))
        .unwrap();
    let outcome = world.drive(&mut ap, station.connect_on(required));
    let AssociationAttemptOutcome::Connected { connected, .. } = outcome else {
        panic!("the granted candidate must connect");
    };
    assert_eq!(connected.connection().unwrap().config().channel, required);
    assert_eq!(world.model.channel(), Some(required));
}

#[test]
fn an_he_association_sets_the_bss_color_of_its_access_point() {
    on_large_stack(an_he_association_sets_the_bss_color_of_its_access_point_body);
}

fn an_he_association_sets_the_bss_color_of_its_access_point_body() {
    use oer_ieee80211_mac::station::{
        AssociationCapabilities,
        association::{HeUlMuPowerCapability, StaPowerCapability},
    };
    use oer_ieee80211_sta_service::port::PortHePower;
    static HE: AssociationCapabilities = AssociationCapabilities {
        ht20: scripted_ap::HT_CAPABILITIES,
        he20_ht: scripted_ap::HT_CAPABILITIES,
        he20: scripted_ap::HE_CAPABILITIES,
        ..CAPABILITIES
    };
    let world = World::new();
    let mut profile = profile();
    profile.capabilities = &HE;
    profile.preference = Preference::PreferHe20;
    profile.he_power = Some(PortHePower {
        power_capability: StaPowerCapability::new(-11, 20).unwrap(),
        ul_mu: HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap(),
    });
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    ap.he_bss_color = Some(0x2a);
    let station = connect(&world, &mut ap, world.station_with(open(), profile));
    let peer = station.connection().unwrap().config().peer;
    assert_eq!(peer.phy, PhyMode::He20);
    assert_eq!(peer.he_bss_color, 0x2a);
    assert_eq!(world.model.he_bss_color(), Some((VifId(0), 0x2a)));
}

#[test]
fn an_aggregate_and_its_block_ack_stay_within_the_txop_limit() {
    on_large_stack(an_aggregate_and_its_block_ack_stay_within_the_txop_limit_body);
}

fn an_aggregate_and_its_block_ack_stay_within_the_txop_limit_body() {
    use oer_ieee80211_mac::phy::{HtMcs, HtRate, PpduBandwidth};
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let ht = PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap());
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    ap.ht = true;
    // Best effort may hold the air for 160 us: at HT20 MCS 7 with the
    // short GI three of the four frames and the BlockAck fit, four do not.
    ap.wmm_association = Some(scripted_ap::wmm_parameter_element(
        1,
        [(3, 4, 10, 5), (7, 4, 10, 0), (2, 3, 4, 94), (2, 2, 3, 47)],
    ));
    let mut station = connect(
        &world,
        &mut ap,
        world.station_at(wpa2(&PMKSA), block_ack_profile(), ht),
    );
    let mut delivered = Vec::new();
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
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
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    let before = world.model.submitted().len();

    for payload in [b"a".as_slice(), b"b", b"c", b"d"] {
        world.send(&ethernet(PEER, IPV4, payload));
    }
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
    assert!(data[0].ampdu && data[0].frames.len() == 3);
    assert!(!data[1].ampdu);
    assert!(station.connection().is_some());
}

#[test]
fn a_silent_access_point_is_probed_and_then_left() {
    on_large_stack(a_silent_access_point_is_probed_and_then_left_body);
}

fn a_silent_access_point_is_probed_and_then_left_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    ap.answers_probes = false;
    let probes_before = ap.probe_destinations.len();
    let mut delivered = Vec::new();

    // No beacon for 6 s: nothing yet.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5_900, &mut delivered),
        None
    );
    assert_eq!(ap.probe_destinations.len(), probes_before);
    // Then five probes 500 ms apart, three to the access point and two
    // broadcast, and the station leaves after the last.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 3_000, &mut delivered),
        Some(PortDisconnect::BeaconLoss)
    );
    ap.absorb(world.model);
    assert_eq!(
        ap.probe_destinations[probes_before..],
        [AP, AP, AP, [0xff; 6], [0xff; 6]]
    );
    assert!(station.connection().is_none());
}

#[test]
fn an_answered_probe_keeps_the_link() {
    on_large_stack(an_answered_probe_keeps_the_link_body);
}

fn an_answered_probe_keeps_the_link_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    let probes_before = ap.probe_destinations.len();
    let mut delivered = Vec::new();
    // The access point sends no beacon but answers the first probe; the
    // link holds and probing starts over only after another window.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 6_400, &mut delivered),
        None
    );
    ap.absorb(world.model);
    assert_eq!(ap.probe_destinations[probes_before..], [AP]);
    assert_eq!(station.connection().unwrap().link().probes_sent(), 0);
}

#[test]
fn connection_frames_carry_the_elevated_priority_under_the_reconnect_policy() {
    on_large_stack(connection_frames_carry_the_elevated_priority_under_the_reconnect_policy_body);
}

fn connection_frames_carry_the_elevated_priority_under_the_reconnect_policy_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    world.coex.reconnect.set(true);
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    // The station asked the air for its probes, its Authentication and
    // Association and both of its handshake messages.
    let frames = world.coex.connection_frames.borrow().clone();
    assert!(frames.contains(&PortConnectionFrame::ProbeRequest));
    assert_eq!(
        frames
            .iter()
            .filter(|frame| **frame != PortConnectionFrame::ProbeRequest)
            .copied()
            .collect::<Vec<_>>(),
        [
            PortConnectionFrame::Authentication,
            PortConnectionFrame::Association,
            PortConnectionFrame::Eapol,
            PortConnectionFrame::Eapol,
        ]
    );
    // They went out at the elevated priority, the probes at the ordinary.
    let coex_of = |frame_control: u8| {
        world
            .model
            .submitted()
            .iter()
            .filter(|attempt| attempt.frames[0][0] == frame_control)
            .map(|attempt| attempt.coex)
            .collect::<Vec<_>>()
    };
    assert!(
        coex_of(0x40)
            .iter()
            .all(|coex| *coex == CoexPriority::Normal)
    );
    assert_eq!(coex_of(0xb0), [CoexPriority::Elevated]);
    assert_eq!(coex_of(0x00), [CoexPriority::Elevated]);
    assert_eq!(coex_of(0x08), [CoexPriority::Elevated; 2]);

    // Data is ordinary.
    let before = world.model.submitted().len();
    world.send(&ethernet(PEER, IPV4, b"data"));
    let mut delivered = Vec::new();
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert!(
        world.model.submitted()[before..]
            .iter()
            .all(|attempt| attempt.coex == CoexPriority::Normal)
    );
}

#[test]
fn triggers_and_sounding_announcements_are_not_answered() {
    on_large_stack(triggers_and_sounding_announcements_are_not_answered_body);
}

fn triggers_and_sounding_announcements_are_not_answered_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    let before = world.model.submitted().len();
    let counters = station.connection().unwrap().counters();

    // A Basic Trigger for every station and an HE NDP Announcement naming
    // this one: the station has no HE-TB or beamformee path (#153, #154)
    // and answers neither.
    let mut trigger = vec![0x24, 0x00, 0, 0];
    trigger.extend_from_slice(&[0xff; 6]);
    trigger.extend_from_slice(&AP);
    trigger.extend_from_slice(&[0; 8]);
    trigger.extend_from_slice(&[1, 0, 0, 0, 0]);
    ap.queue(trigger);
    let mut announcement = vec![0x54, 0x00, 0, 0];
    announcement.extend_from_slice(&STA);
    announcement.extend_from_slice(&AP);
    announcement.extend_from_slice(&[0x02, 1, 0, 0, 0]);
    ap.queue(announcement);
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut delivered),
        None
    );
    assert_eq!(world.model.submitted().len(), before);
    assert!(delivered.is_empty());
    assert_eq!(station.connection().unwrap().counters(), counters);
}

#[test]
fn the_rate_control_starts_from_the_association_response_and_learns_from_each_frame() {
    on_large_stack(
        the_rate_control_starts_from_the_association_response_and_learns_from_each_frame_body,
    );
}

fn the_rate_control_starts_from_the_association_response_and_learns_from_each_frame_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // -40 dBm over a -96 dBm noise floor.
    assert_eq!(world.rate_log.borrow().link_metrics, [Some(56)]);
    world.send(&ethernet(PEER, IPV4, b"data"));
    let mut delivered = Vec::new();
    world.run_for(&mut ap, &mut station, 5, &mut delivered);
    assert_eq!(world.rate_log.borrow().mpdus, [(1, true)]);
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
        .drive(&mut ap, station.set_sleep_type(SleepType::MinModem))
        .unwrap();
    let mut delivered = Vec::new();

    // The access point beacons 10 ms from now, then every 102.4 ms. Once
    // the station parsed a beacon and idled past the active timeout, it
    // sends a Null with PM=1 and dozes: the transmit gate closes.
    let start = world.timer.now().as_micros();
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

/// Serves the connection: stops after its `stop_after`th connection, or
/// when the test says so.
struct Application<'a> {
    connections: &'a Cell<u32>,
    disconnects: &'a RefCell<Vec<PortDisconnect>>,
    stop_after: u32,
    stop: &'a Cell<bool>,
}

impl PortStationApplication<ModelRxBuffer> for Application<'_> {
    fn stop_requested(&mut self) -> bool {
        self.stop.get() || self.connections.get() >= self.stop_after
    }

    fn deliver(&mut self, _msdu: PortMsdu<'_, ModelRxBuffer>) {}

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
    let stop = Cell::new(false);
    let mut service = StaLifecycleService::new(
        PortStationLifecycle::new(
            Application {
                connections: &connections,
                disconnects: &disconnects,
                stop_after: 2,
                stop: &stop,
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
            world.timer.advance_to(world.timer.now());
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
            if ap.step(world.model, world.timer.now().as_micros()) {
                continue;
            }
            let next = world
                .timer
                .next_deadline()
                .map(Instant::as_micros)
                .expect("a deadline");
            world.advance_to(next.max(world.timer.now().as_micros()));
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
    world.send(&ethernet(PEER, IPV4, b"up"));
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
    world.send(&ethernet(PEER, IPV4, b"lost"));
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
    world.send(&ethernet(PEER, IPV4, b"next"));
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
    world.send(&ethernet(PEER, IPV4, b"late"));
    let ran = world.drive(&mut ap, station.run_until(deadline, &mut |_| {}));
    assert!(
        matches!(ran, Err(PortLinkError::Poisoned)),
        "sending fails on the poisoned port: {ran:?}"
    );
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
    use oer_ieee80211_mac::block_ack::{
        TxBlockAckOriginatorConfig, TxBlockAckOriginatorPolicy, TxBlockAckRetry,
    };
    use oer_ieee80211_mac::station::AssociationCapabilities;
    use oer_ieee80211_sta_service::port::PortTxBlockAck;
    static HT: AssociationCapabilities = AssociationCapabilities {
        ht20: scripted_ap::HT_CAPABILITIES,
        ..CAPABILITIES
    };
    let mut profile = profile();
    profile.capabilities = &HT;
    profile.preference = Preference::ForceHt20;
    profile.tx_block_ack = Some(PortTxBlockAck {
        policy: TxBlockAckOriginatorPolicy {
            tids: &oer_espressif_ieee80211_policy::block_ack::STA_TX_BLOCK_ACK_TIDS,
            first_dialog_token: oer_espressif_ieee80211_policy::block_ack::FIRST_DIALOG_TOKEN,
            next_dialog_token: oer_espressif_ieee80211_policy::block_ack::next_dialog_token,
        },
        config: TxBlockAckOriginatorConfig {
            window: 32,
            negotiation_timeout: Duration::from_millis(100),
            amsdu_tids: 0,
        },
        retry: TxBlockAckRetry {
            attempts: 2,
            interval: Duration::ZERO,
        },
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

    // Twelve frames of TID 0, more than the eight subframes an aggregate
    // once had, leave as one A-MPDU; the voice frame alone.
    let payloads: Vec<Vec<u8>> = (0..12_u8)
        .map(|n| vec![0x60 + n; 20 + usize::from(n)])
        .collect();
    for payload in &payloads {
        world.send(&ethernet(PEER, IPV4, payload));
    }
    world.send(&ethernet(PEER, IPV4, &voice(b"voice")));
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
    assert!(data[0].ampdu && data[0].frames.len() == 12);
    assert!(!data[1].ampdu);
    // Each subframe ends in its frame's payload, gathered from the
    // network's owner after the header; every owner went back.
    for (subframe, payload) in data[0].frames.iter().zip(&payloads) {
        assert!(subframe.ends_with(payload));
    }
    assert!(data[1].frames[0].ends_with(&voice(b"voice")));
    assert!(world.frames.is_empty());
    assert_eq!(world.model.bodies_held(), 0);
    let counters = station.connection().unwrap().tx_counters();
    assert_eq!(
        (counters.aggregates, counters.subframes, counters.mpdus),
        (1, 12, 1)
    );
    // The rate control saw the aggregate's BlockAck and the voice frame.
    assert_eq!(world.rate_log.borrow().ampdus, [(12, 12)]);
    assert_eq!(world.rate_log.borrow().mpdus.last(), Some(&(1, true)));
    assert_eq!(counters.acknowledged, 13);
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

#[test]
fn a_new_connection_takes_the_storage_s_buffers_back_empty() {
    on_large_stack(a_new_connection_takes_the_storage_s_buffers_back_empty_body);
}

fn a_new_connection_takes_the_storage_s_buffers_back_empty_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    // Frames the connection never ran to send.
    for _ in 0..4 {
        world.send(&ethernet(PEER, IPV4, b"data"));
    }
    let before = returned();
    assert_eq!(world.drive(&mut ap, station.disconnect()), Ok(()));
    assert!(station.connection().is_none());
    // The station took none of them: they stay the network's.
    assert_eq!((world.frames.len(), returned()), (4, before));

    // The next association borrows the same buffers and sends them.
    let mut station = connect(&world, &mut ap, station);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 5, &mut Vec::new()),
        None
    );
    assert!(world.frames.is_empty());
    assert_eq!(returned(), before + 4);
}

#[test]
fn the_station_holds_no_frame_buffer_of_its_own() {
    type Station = PortStation<'static, Env<'static>>;
    type Connection = PortConnection<
        'static,
        <Env<'static> as PortClientEnv>::Port,
        <Env<'static> as PortStationEnv>::RateControl,
    >;
    // The scan table and every frame buffer are in the composition's
    // storage; the station and its connection are protocol state. The one
    // frame a connection keeps is an EAPOL frame awaiting the Group Key
    // Handshake.
    let frame = PORT_FRAME_CAPACITY;
    assert!(
        core::mem::size_of::<Connection>()
            < frame + oer_ieee80211_rsn::runner::RSN_HANDSHAKE_EAPOL_CAPACITY
    );
    assert!(core::mem::size_of::<Station>() < 2 * frame);
    assert!(core::mem::size_of::<PortStationStorage<TestFrame>>() > PORT_REORDER_SLOTS * frame);
    // An A-MPDU's subframes are headers and the network's owners, no copy
    // of a frame.
    assert_eq!(PORT_AMPDU_SUBFRAMES, 32);
    assert!(core::mem::size_of::<AmpduSubframes<TestFrame>>() < 2 * frame);
}

#[test]
fn an_in_order_msdu_is_handed_on_in_the_port_s_buffer() {
    on_large_stack(an_in_order_msdu_is_handed_on_in_the_port_s_buffer_body);
}

fn an_in_order_msdu_is_handed_on_in_the_port_s_buffer_body() {
    static PMKSA: StaSharedPmksa = StaSharedPmksa::new();
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Wpa2Psk);
    let mut station = connect(&world, &mut ap, world.station(wpa2(&PMKSA)));
    let request = ap.addba_request(0, 8, 100);
    ap.queue(request);
    world.run_for(&mut ap, &mut station, 5, &mut Vec::new());
    assert!(station.connection().unwrap().block_ack(0));

    // 101 arrives before 100: the window keeps a copy of it. 100 is in
    // order and travels in the port's buffer, which the application holds
    // until it lets go; 101 follows from the copy as parts.
    for (sequence, packet_number, payload) in [(101, 2, b"b"), (100, 1, b"a")] {
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
    let mut delivered = Vec::new();
    let deadline = world
        .timer
        .now()
        .checked_add(Duration::from_millis(5))
        .unwrap();
    world
        .drive(
            &mut ap,
            station.run_until(deadline, &mut |msdu| {
                let in_buffer = matches!(msdu, PortMsdu::Buffer { .. });
                delivered.push((
                    in_buffer,
                    world.model.rx_buffers_lent(),
                    ethernet_of(&msdu)[14..].to_vec(),
                ));
            }),
        )
        .unwrap();
    assert_eq!(
        delivered,
        [(true, 1, b"a".to_vec()), (false, 0, b"b".to_vec())]
    );
    assert_eq!(world.model.rx_buffers_lent(), 0);
}

#[test]
fn an_announced_channel_switch_silences_the_station_until_its_owner_moves_it() {
    on_large_stack(an_announced_channel_switch_silences_the_station_until_its_owner_moves_it_body);
}

fn an_announced_channel_switch_silences_the_station_until_its_owner_moves_it_body() {
    use oer_ieee80211_mac::channel_switch::ChannelSwitchMode;

    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let mut station = connect(&world, &mut ap, world.station(open()));
    ap.next_beacon_micros = Some(world.timer.now().as_micros());
    let mut delivered = Vec::new();
    assert_eq!(
        world.run_for(&mut ap, &mut station, 300, &mut delivered),
        None
    );

    // The access point moves to channel 11 in three beacon intervals and
    // tells its stations to stop transmitting.
    ap.beacon_extra = vec![37, 3, 1, 11, 3];
    let event = world.run_events(&mut ap, &mut station, 300, &mut delivered);
    let Some(PortStationEvent::ChannelSwitch(PortChannelSwitch { target, mode, at })) = event
    else {
        panic!(
            "no announcement: {event:?} {:?} {:?}",
            station.connection().unwrap().counters(),
            station
                .connection()
                .unwrap()
                .link()
                .beacons()
                .last_observation()
        );
    };
    assert_eq!(target, channel(11));
    assert_eq!(mode, ChannelSwitchMode::StopTransmitting);
    let interval = u64::from(scripted_ap::BEACON_INTERVAL_TU) * 1_024;
    assert_eq!(at.as_micros(), world.timer.now().as_micros() + 3 * interval);
    assert_eq!(
        station
            .connection()
            .unwrap()
            .channel_switch()
            .map(|switch| switch.target),
        Some(channel(11))
    );

    // Every beacon counts down to the same switch: nothing new to report,
    // and the network's frame waits.
    ap.absorb(world.model);
    let before = world.model.submitted().len();
    world.send(&ethernet(PEER, IPV4, b"held"));
    assert_eq!(
        world.run_events(&mut ap, &mut station, 250, &mut delivered),
        None
    );
    assert_eq!(world.model.submitted().len(), before);

    // The access point moves; the owner retunes the port and tells the
    // station, whose frame then goes out on the new channel.
    ap.channel = 11;
    ap.beacon_extra.clear();
    world
        .drive(&mut ap, station.link_mut().retune(channel(11)))
        .unwrap();
    station.channel_switched(channel(11)).unwrap();
    assert_eq!(station.connection().unwrap().channel_switch(), None);
    assert_eq!(
        world.run_for(&mut ap, &mut station, 50, &mut delivered),
        None
    );
    assert!(
        world.model.submitted()[before..]
            .iter()
            .any(|attempt| attempt.frames[0][0] & 0x0c == 0x08)
    );
    // Beacons on the new channel keep the link well past its loss window.
    assert_eq!(
        world.run_for(&mut ap, &mut station, 12_000, &mut delivered),
        None
    );
    assert!(station.connection().is_some());
}

#[test]
fn the_lifecycle_follows_its_access_point_to_the_announced_channel() {
    on_large_stack(the_lifecycle_follows_its_access_point_to_the_announced_channel_body);
}

fn the_lifecycle_follows_its_access_point_to_the_announced_channel_body() {
    let world = World::new();
    let mut ap = ScriptedAp::new(ApSecurity::Open);
    let connections = Cell::new(0);
    let disconnects = RefCell::new(Vec::new());
    let stop = Cell::new(false);
    let mut service = StaLifecycleService::new(
        PortStationLifecycle::new(
            Application {
                connections: &connections,
                disconnects: &disconnects,
                stop_after: 2,
                stop: &stop,
            },
            Duration::from_millis(1),
        ),
        StaReconnectPolicy::new(3, 10, 100, 20).unwrap(),
    );
    let interval = u64::from(scripted_ap::BEACON_INTERVAL_TU) * 1_024;
    // When the first beacon announcing channel 11 in two intervals went out.
    let mut announced_at = None;
    // When the station's port reached channel 11.
    let mut retuned_at = None;
    let exit = {
        let future = service.run(world.station(open()));
        let mut future = pin!(future);
        let mut routing = pin!(world.router.run());
        let mut context = Context::from_waker(Waker::noop());
        loop {
            world.timer.advance_to(world.timer.now());
            if let Poll::Ready(exit) = future.as_mut().poll(&mut context) {
                break exit;
            }
            assert!(routing.as_mut().poll(&mut context).is_pending());
            let now = world.timer.now().as_micros();
            if announced_at.is_none() && connections.get() == 1 {
                ap.beacon_extra = vec![37, 3, 0, 11, 2];
                ap.next_beacon_micros = Some(now);
                announced_at = Some(now);
                continue;
            }
            // The access point moves with the station's port.
            if announced_at.is_some()
                && retuned_at.is_none()
                && world.model.channel() == Some(channel(11))
            {
                retuned_at = Some(now);
                ap.channel = 11;
                ap.beacon_extra.clear();
            }
            // Well past the beacon loss window on the new channel.
            if retuned_at.is_some_and(|retuned| now > retuned + 12_000_000) {
                stop.set(true);
            }
            if ap.step(world.model, now) {
                continue;
            }
            let next = world
                .timer
                .next_deadline()
                .map(Instant::as_micros)
                .expect("a deadline");
            world.advance_to(next.max(now));
        }
    };
    assert!(matches!(exit, StaLifecycleExit::Stopped { .. }));
    assert_eq!(connections.get(), 1);
    assert!(disconnects.borrow().is_empty());
    let (announced, retuned) = (announced_at.unwrap(), retuned_at.unwrap());
    assert!(retuned >= announced + 2 * interval, "{announced} {retuned}");
    assert!(retuned < announced + 3 * interval, "{announced} {retuned}");
}
