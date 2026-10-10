//! A station and an access point on one port, against real peers on host
//! models that share an air: an upstream access point the station joins,
//! and a station that joins the pair's access point.

use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_ieee80211_ap::coordinator::{
    ApBands, ApFollowPolicy, ApSearchPolicy, ApUnservable, ChannelCoordinator, UpstreamLossPolicy,
};
use oer_ieee80211_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
};
use oer_ieee80211_ap_service::port::{
    NoSae, PortAccessPoint, PortApAuthenticator, PortApBand, PortApBands, PortApEnv, PortApEvent,
    PortApParts, PortApProfile, PortApStorage,
};
use oer_ieee80211_datapath::{SoftwareTxFrame, memory::MemoryTxQueues};
use oer_ieee80211_lower_mac::{
    Channel, ChannelWidth, CoexPriority, Ieee80211LowerMacPort, LifecycleCommand, LowerMacAmpdu,
    LowerMacBeaconTiming, LowerMacMonitor, LowerMacSetting, MacAddress, RadioPort, TxPower, VifId,
    VifRole,
    model::{LowerMacModel, ModelAir, ready},
};
use oer_ieee80211_mac::{
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    block_ack::TxBlockAckRetry,
    ccmp::CcmpPacketNumberStep,
    channel_switch::ChannelSwitchMode,
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    phy::{LegacyRate, PhyRate},
    sequence::SequenceNumber,
    ssid::WifiSsid,
    station::{AssociationCapabilities, StaTxSequenceCounters, association::Preference},
};
use oer_ieee80211_rsn::aes::RsnSoftwareAes;
use oer_ieee80211_softmac::{BackoffEntropy, EdcaContention};
use oer_ieee80211_sta::{
    attempt::{AssociationAttemptOutcome, StaAttemptSecurity},
    modem_sleep::SleepType,
    scan::StaScanConfig,
};
use oer_ieee80211_sta_ap_service::{PortStaAp, PortStaApError, PortStaApEvent, ProtectedSearch};
use oer_ieee80211_sta_service::port::{
    NoCoexistence, PortDisconnect, PortLink, PortLinkSupervision, PortProbe, PortStation,
    PortStationConfig, PortStationEnv, PortStationError, PortStationEvent, PortStationProfile,
    PortStationStorage,
};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, FixedRate, ProtectEveryHeTxop, ProtectionPolicy, RetryLimits, TxPlanner,
    rate_control::FixedRateControl,
};
use oer_ieee80211_upper_mac_service::{
    PortRouter,
    absence::{AbsenceError, AbsenceState, AbsenceWindow, PortAbsence},
    aggregate::PortAmpduAggregation,
    client::{PortClient, PortClientConfig, PortClientEnv},
    frame::NetworkBody,
};
use oer_network_interface::NetworkInterfaceId;
use oer_time::{Clock, Duration, Instant, RadioInstant, Timer};
use oer_time_virtual::VirtualClock;

/// Run `body` on a thread whose stack holds the drivers' unoptimized
/// futures.
fn on_large_stack(body: fn()) {
    std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

const STATION: VifId = VifId(0);
const ACCESS_POINT: VifId = VifId(1);
const UPSTREAM: MacAddress = [0x02, 0, 0, 0, 0, 0x01];
const PAIR_STATION: MacAddress = [0x02, 0, 0, 0, 0, 0x02];
const PAIR_ACCESS_POINT: MacAddress = [0x02, 0, 0, 0, 0, 0x03];
const CLIENT: MacAddress = [0x02, 0, 0, 0, 0, 0x04];
const UPSTREAM_SSID: &[u8] = b"upstream";
const PAIR_SSID: &[u8] = b"pair";
const MANAGEMENT_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm6M);
const DATA_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);
const RATES: [u8; 8] = [0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c];

/// OFDM rates, 6, 12 and 24 Mbit/s basic: the same in either band.
const ADVERTISEMENT: Advertisement = Advertisement::new(
    LegacyRates::new(RATES, [0x8c, 0x98, 0xb0, 0x6c]),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: 3,
            ecw_min: 4,
            ecw_max: 10,
            txop_limit_units_32_us: 0,
        }; 4],
    ),
    0x0001,
);

fn ghz2_4(number: u8) -> Channel {
    Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()
}

static SCANNED: [Channel; 3] = [
    match Channel::ghz2_4(1, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
    match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
    match Channel::ghz2_4(11, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!(),
    },
];

/// What the stations claim: one-stream HT and WMM, as the access points
/// advertise them.
fn capabilities() -> &'static AssociationCapabilities {
    let ht = |width| {
        oer_ieee80211_mac::ht::ht_capability_ie(
            ADVERTISEMENT.ht,
            Channel::ghz2_4(6, width).unwrap(),
        )
    };
    leak(AssociationCapabilities {
        ht20: ht(ChannelWidth::Mhz20),
        ht40: ht(ChannelWidth::Mhz40Below),
        he20_ht: ht(ChannelWidth::Mhz20),
        he20: [0; 24],
        he20_extended: [0; 14],
        // The WMM Information element: version 1, no U-APSD.
        wmm: [221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00],
    })
}

type VirtualTimer = VirtualClock;

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

/// One of the network's Ethernet-II frames.
struct TestFrame(Vec<u8>);

impl SoftwareTxFrame for TestFrame {
    fn interface(&self) -> NetworkInterfaceId {
        NetworkInterfaceId::new(0)
    }

    fn ethernet(&self) -> &[u8] {
        &self.0
    }
}

type TestFrames = MemoryTxQueues<TestFrame, 32>;
type Model = LowerMacModel<NetworkBody<TestFrame>>;
type TestPair = PortStaAp<'static, Env, Env, &'static VirtualTimer, ProtectedSearch>;
type Router<P = Model> = PortRouter<'static, P>;
type StationJoin<P> = AssociationAttemptOutcome<
    PortStation<'static, Env<P>>,
    PortStation<'static, Env<P>>,
    oer_ieee80211_sta_service::port::PortAttemptError<Env<P>>,
>;

/// One environment for every driver of the test.
trait TestPort:
    LowerMacBeaconTiming
    + LowerMacMonitor
    + LowerMacAmpdu
    + Ieee80211LowerMacPort<TxBody = NetworkBody<TestFrame>>
{
}
impl<P> TestPort for P where
    P: LowerMacBeaconTiming
        + LowerMacMonitor
        + LowerMacAmpdu
        + Ieee80211LowerMacPort<TxBody = NetworkBody<TestFrame>>
{
}

struct Env<P: TestPort = Model>(PhantomData<P>);

impl<P: TestPort> PortClientEnv for Env<P> {
    type NetworkFrame = TestFrame;
    type Port = P;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = PortAmpduAggregation;
}

impl<P: TestPort> PortStationEnv for Env<P> {
    type Timer = &'static VirtualTimer;
    type KeyUnwrap = RsnSoftwareAes;
    type Coex = NoCoexistence;
    type RateControl = FixedRateControl;
    type Frames = TestFrames;
}

struct FixedMaterial;

impl PortApAuthenticator for FixedMaterial {
    fn handshake_material(&mut self) -> ([u8; 32], u64) {
        ([0x33; 32], 9)
    }
}

impl<P: TestPort> PortApEnv for Env<P> {
    type Timer = &'static VirtualTimer;
    type Authenticator = FixedMaterial;
    type Sae = NoSae;
    type RateControl = FixedRateControl;
    type Frames = TestFrames;
}

fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn planner() -> TxPlanner<ProtectEveryHeTxop> {
    TxPlanner::new(
        [EdcaContention::new(2, 3); 4],
        RetryLimits::IEEE_DEFAULT,
        AmpduRetryPolicy {
            lifetime: oer_time::RadioDuration::from_micros(1_000_000),
            aged_margin: oer_time::RadioDuration::from_micros(1_024),
            retry_limit: 7,
            retain_single_mpdu: false,
        },
        ProtectionPolicy::new(None),
        ProtectEveryHeTxop,
    )
}

/// A radio, enabled on channel 1, and its event router.
fn radio() -> (&'static Model, &'static Router) {
    let model = leak(Model::new());
    model.set_now(RadioInstant::from_micros(1_000));
    model
        .apply(LowerMacSetting::Channel(ghz2_4(1)))
        .unwrap()
        .unwrap();
    ready(model.lifecycle(LifecycleCommand::Enable))
        .unwrap()
        .unwrap();
    (model, leak(PortRouter::new(model, 1)))
}

fn station<P: TestPort + 'static>(
    router: &'static Router<P>,
    timer: &'static VirtualTimer,
    address: MacAddress,
    ssid: &'static [u8],
    frames: &'static TestFrames,
) -> PortStation<'static, Env<P>> {
    let link = PortLink::new(
        router,
        planner(),
        FixedRate,
        Seeded(0x1357_9bdf ^ u32::from(address[5])),
        NoCoexistence,
        DATA_RATE,
        PortStationConfig {
            vif: STATION,
            address,
            management_rate: MANAGEMENT_RATE,
            power: TxPower::Calibrated,
            coex: CoexPriority::Normal,
            retry_limit: 7,
        },
    )
    .expect("the station's interface is free");
    let probe = leak(PortProbe {
        ssid,
        supported_rates: &RATES,
    });
    PortStation::new(
        link,
        timer,
        RsnSoftwareAes,
        PortStationProfile {
            ssid,
            channels: &SCANNED,
            scan: StaScanConfig::new(2).unwrap(),
            dwell_tick: Duration::from_millis(100),
            probe: Some(*probe),
            capabilities: capabilities(),
            he_power: None,
            he_packet_padding: oer_espressif_ieee80211_policy::he_txop::packet_padding,
            tx_block_ack: None,
            link: PortLinkSupervision {
                timeout: Duration::from_secs(6),
                miss_limit: 10,
                probe: oer_espressif_ieee80211_policy::station_link::STATION_LINK_PROBE,
                supported_rates: &RATES,
            },
            sleep_type: SleepType::None,
            rx_reorder_gap: Duration::from_millis(100),
            preference: Preference::Automatic,
            listen_interval: 3,
            ccmp_step: CcmpPacketNumberStep::new(1).unwrap(),
            sa_query_random: || 0x1234,
        },
        StaAttemptSecurity::open(StaTxSequenceCounters::new(SequenceNumber::new(0).unwrap())),
        Box::leak(Box::new(PortStationStorage::new())),
        frames,
    )
}

fn access_point<P: TestPort + 'static>(
    router: &'static Router<P>,
    timer: &'static VirtualTimer,
    address: MacAddress,
    ssid: &'static [u8],
    channel: Channel,
    frames: &'static TestFrames,
) -> PortAccessPoint<'static, Env<P>> {
    let client = PortClient::new(
        router,
        planner(),
        FixedRate,
        Seeded(0x2468_ace1 ^ u32::from(address[5])),
        PortClientConfig {
            vif: ACCESS_POINT,
            address,
            role: VifRole::AccessPoint,
            power: TxPower::Calibrated,
            retry_limit: 7,
        },
    )
    .expect("the access point's interface is free");
    let band = PortApBand {
        advertisement: &ADVERTISEMENT,
        management_rate: MANAGEMENT_RATE,
    };
    PortAccessPoint::new(
        PortApParts {
            client,
            timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames,
        },
        PortApProfile {
            ssid: leak(WifiSsid::new(ssid).unwrap()),
            channel,
            beacon_interval_tu: 100,
            dtim_period: 1,
            bands: PortApBands {
                ghz2_4: Some(band),
                ghz5: Some(band),
            },
            coex: CoexPriority::Normal,
            ccmp_step: CcmpPacketNumberStep::new(1).unwrap(),
            rx_reorder_gap: Duration::from_millis(100),
            tx_block_ack_retry: TxBlockAckRetry {
                attempts: 3,
                interval: Duration::from_millis(1_000),
            },
        },
        AccessPointService::new_open(
            address,
            AccessPointClientLimit::new(4).unwrap(),
            AccessPointInactiveTimeout::new(60).unwrap(),
            Box::leak(Box::new(AccessPointPeerStorage::new())),
        ),
        Box::leak(Box::new(PortApStorage::<8, TestFrame>::new())),
    )
    .unwrap()
}

/// The three radios on one air, their routers and one virtual clock.
struct World {
    timer: &'static VirtualTimer,
    upstream: (&'static Model, &'static Router),
    pair: (&'static Model, &'static Router),
    client: (&'static Model, &'static Router),
    pair_frames: &'static TestFrames,
    client_frames: &'static TestFrames,
}

impl World {
    fn new() -> Self {
        let timer = leak(VirtualTimer::starting_at(Instant::from_micros(1_000)));
        Self {
            timer,
            upstream: radio(),
            pair: radio(),
            client: radio(),
            pair_frames: leak(TestFrames::new()),
            client_frames: leak(TestFrames::new()),
        }
    }

    fn now(&self) -> u64 {
        self.timer.now().as_micros()
    }

    fn after(&self, millis: u64) -> Instant {
        Instant::from_micros(self.now() + millis * 1_000)
    }

    /// Poll `future` to its end beside the three routers: the air carries
    /// what the radios publish, and when nothing moves, virtual time
    /// advances to the earliest deadline anyone waits for.
    fn drive<F: Future>(&self, future: F) -> F::Output {
        self.drive_observing(future, || {})
    }

    fn drive_observing<F: Future>(&self, future: F, observe: impl FnMut()) -> F::Output {
        self.drive_with_pair_router(future, self.pair.1, observe)
    }

    fn drive_with_pair_router<F: Future, P: Ieee80211LowerMacPort>(
        &self,
        future: F,
        pair_router: &'static Router<P>,
        mut observe: impl FnMut(),
    ) -> F::Output {
        let air = ModelAir::new([self.upstream.0, self.pair.0, self.client.0]);
        let mut future = pin!(future);
        let mut upstream_router = pin!(self.upstream.1.run());
        let mut pair_router = pin!(pair_router.run());
        let mut client_router = pin!(self.client.1.run());
        let mut context = Context::from_waker(Waker::noop());
        let mut quiet = 0;
        for _ in 0..50_000_000 {
            self.timer.advance_to(self.timer.now());
            let polled = future.as_mut().poll(&mut context);
            observe();
            if let Poll::Ready(output) = polled {
                return output;
            }
            assert!(upstream_router.as_mut().poll(&mut context).is_pending());
            assert!(pair_router.as_mut().poll(&mut context).is_pending());
            assert!(client_router.as_mut().poll(&mut context).is_pending());
            if air.step() {
                quiet = 0;
                continue;
            }
            let queued = [self.upstream.0, self.pair.0, self.client.0]
                .iter()
                .any(|model| model.queued_events() > 0);
            if queued || quiet < 2 {
                quiet += 1;
                continue;
            }
            quiet = 0;
            let next = self
                .timer
                .next_deadline()
                .into_iter()
                .chain(
                    [self.upstream.0, self.pair.0, self.client.0]
                        .into_iter()
                        .filter_map(|model| {
                            let until = model.nav_until()?;
                            let sample = model.clock_sample().unwrap().unwrap();
                            model
                                .clock_info()
                                .to_monotonic_with(
                                    oer_ieee80211_lower_mac::Ieee80211Stamp {
                                        at: until,
                                        generation: sample.generation,
                                    },
                                    &sample,
                                )
                                .ok()
                                .map(|projected| projected.at)
                        }),
                )
                .min()
                .expect("the drivers wait for an event nothing produces");
            self.timer.advance_to(next);
            for model in [self.upstream.0, self.pair.0, self.client.0] {
                model.set_now(RadioInstant::from_micros(next.as_micros()));
            }
        }
        panic!("the drivers did not finish");
    }
}

/// Run both futures to their ends at once.
async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let (mut a_out, mut b_out) = (None, None);
    poll_fn(|context| {
        if a_out.is_none()
            && let Poll::Ready(out) = a.as_mut().poll(context)
        {
            a_out = Some(out);
        }
        if b_out.is_none()
            && let Poll::Ready(out) = b.as_mut().poll(context)
        {
            b_out = Some(out);
        }
        if a_out.is_some() && b_out.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    (a_out.unwrap(), b_out.unwrap())
}

/// The upstream access point's owner: it serves its BSS until `until` and
/// moves its port when its own announced switch is due.
async fn serve_upstream(access_point: &mut PortAccessPoint<'static, Env>, until: Instant) {
    loop {
        match access_point.run_until(until, &mut |_| {}).await.unwrap() {
            Some(PortApEvent::ChannelSwitch { target }) => {
                access_point.client_mut().retune(target).await.unwrap();
                access_point.channel_switched().unwrap();
            }
            None => return,
        }
    }
}

/// The client station's owner, alone on its port: it serves the station
/// until `until` and follows its access point's announced moves.
async fn serve_client(
    station: &mut PortStation<'static, Env>,
    timer: &VirtualTimer,
    until: Instant,
) {
    let mut switch = None;
    loop {
        let now = timer.now();
        if let Some(due) =
            switch.filter(|due: &oer_ieee80211_sta_service::port::PortChannelSwitch| due.at <= now)
        {
            switch = None;
            station.link_mut().retune(due.target).await.unwrap();
            station.channel_switched(due.target).unwrap();
            continue;
        }
        if now >= until {
            return;
        }
        let deadline = switch.map_or(until, |due| due.at.min(until));
        match station.run_until(deadline, &mut |_| {}).await.unwrap() {
            Some(PortStationEvent::ChannelSwitch(announced)) => switch = Some(announced),
            Some(PortStationEvent::Ended(reason)) => panic!("the client lost its BSS: {reason:?}"),
            None => {}
        }
    }
}

/// The pair, served until `until`: no event ends it early.
async fn serve_pair(pair: &mut TestPair, until: Instant) {
    let event = pair
        .run_until(until, &mut |_| {}, &mut |_| {})
        .await
        .unwrap();
    assert_eq!(event, None);
}

fn connected<P: TestPort>(outcome: StationJoin<P>) -> PortStation<'static, Env<P>> {
    match outcome {
        AssociationAttemptOutcome::Connected { connected, .. } => connected,
        AssociationAttemptOutcome::Failed(failure) => {
            panic!("the join failed at {:?}", failure.stage)
        }
    }
}

fn unconnected_pair(world: &World, policy: ApFollowPolicy) -> TestPair {
    unconnected_pair_with_search(world, policy, ApSearchPolicy::new(&[]))
}

fn unconnected_pair_with_search(
    world: &World,
    policy: ApFollowPolicy,
    search: ApSearchPolicy<'static>,
) -> TestPair {
    PortStaAp::new_with_search(
        station(
            world.pair.1,
            world.timer,
            PAIR_STATION,
            UPSTREAM_SSID,
            leak(TestFrames::new()),
        ),
        access_point(
            world.pair.1,
            world.timer,
            PAIR_ACCESS_POINT,
            PAIR_SSID,
            ghz2_4(1),
            world.pair_frames,
        ),
        ChannelCoordinator::new(policy, search),
        world.timer,
    )
}

/// The upstream on channel 6, the pair joined to it with its access point
/// started there, and the client joined to the pair's access point.
fn set_up(
    world: &World,
    policy: ApFollowPolicy,
) -> (
    PortAccessPoint<'static, Env>,
    TestPair,
    PortStation<'static, Env>,
) {
    set_up_with_search(world, policy, ApSearchPolicy::new(&[]))
}

fn set_up_with_search(
    world: &World,
    policy: ApFollowPolicy,
    search: ApSearchPolicy<'static>,
) -> (
    PortAccessPoint<'static, Env>,
    TestPair,
    PortStation<'static, Env>,
) {
    let mut upstream = access_point(
        world.upstream.1,
        world.timer,
        UPSTREAM,
        UPSTREAM_SSID,
        ghz2_4(6),
        leak(TestFrames::new()),
    );
    world
        .drive(upstream.client_mut().retune(ghz2_4(6)))
        .unwrap();
    upstream.start(ghz2_4(6)).unwrap();
    let mut pair = unconnected_pair_with_search(world, policy, search);
    let until = world.after(3_000);
    let (_, joined) = world.drive(join(serve_upstream(&mut upstream, until), pair.connect()));
    joined.unwrap();
    assert!(pair.station().connection().is_some());
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
    assert!(
        pair.access_point()
            .schedule()
            .is_none_or(|schedule| schedule.channel == ghz2_4(6))
    );

    let client = station(
        world.client.1,
        world.timer,
        CLIENT,
        PAIR_SSID,
        world.client_frames,
    );
    let until = world.after(4_000);
    let (_, (_, client)) = world.drive(join(
        serve_upstream(&mut upstream, until),
        join(serve_pair(&mut pair, until), client.connect()),
    ));
    let client = connected(client);
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
    (upstream, pair, client)
}

#[test]
fn the_pair_follows_its_upstream_s_move_and_takes_its_peer_along() {
    on_large_stack(the_pair_follows_its_upstream_s_move_and_takes_its_peer_along_body);
}

fn the_pair_follows_its_upstream_s_move_and_takes_its_peer_along_body() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, ApFollowPolicy::DEFAULT);

    // The upstream moves to channel 11 after three beacons.
    upstream
        .announce_channel_switch(ghz2_4(11), ChannelSwitchMode::Continue, 3)
        .unwrap();
    let until = world.after(2_000);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            serve_pair(&mut pair, until),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    // Everyone is on channel 11, and every association held.
    for model in [world.upstream.0, world.pair.0, world.client.0] {
        assert_eq!(model.channel(), Some(ghz2_4(11)));
    }
    assert!(pair.station().connection().is_some());
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
    assert!(client.connection().is_some());
    assert_eq!(pair.access_point().schedule().unwrap().channel, ghz2_4(11));
}

#[test]
fn the_access_point_stops_where_its_policy_does_not_follow_and_starts_again() {
    on_large_stack(the_access_point_stops_where_its_policy_does_not_follow_and_starts_again_body);
}

fn the_access_point_stops_where_its_policy_does_not_follow_and_starts_again_body() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, ApFollowPolicy::DEFAULT);
    let ghz5 = Channel::ghz5(36, ChannelWidth::Mhz20).unwrap();

    // The upstream moves to 5 GHz, which the default policy does not serve:
    // the pair's access point releases its peer and stops; the station
    // follows the upstream alone.
    upstream
        .announce_channel_switch(ghz5, ChannelSwitchMode::Continue, 3)
        .unwrap();
    let until = world.after(1_000);
    let (_, (_, ended)) = world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            serve_pair(&mut pair, until),
            client.run_until(until, &mut |_| {}),
        ),
    ));
    // The client heard the access point leave (reason 3).
    assert_eq!(
        ended.unwrap(),
        Some(PortStationEvent::Ended(PortDisconnect::Deauthenticated {
            reason_code: 3
        }))
    );
    assert_eq!(world.pair.0.channel(), Some(ghz5));
    assert!(pair.station().connection().is_some());
    assert!(pair.access_point().schedule().is_none());
    assert!(pair.access_point().service().peer_status(CLIENT).is_none());

    // Back in 2.4 GHz, the access point starts again where the port moved.
    upstream
        .announce_channel_switch(ghz2_4(1), ChannelSwitchMode::Continue, 3)
        .unwrap();
    let until = world.after(1_000);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        serve_pair(&mut pair, until),
    ));
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(1)));
    assert!(pair.station().connection().is_some());
    assert_eq!(pair.access_point().schedule().unwrap().channel, ghz2_4(1));
}

#[test]
fn the_leave_upstream_policy_keeps_the_access_point_and_its_peer() {
    on_large_stack(the_leave_upstream_policy_keeps_the_access_point_and_its_peer_body);
}

fn the_leave_upstream_policy_keeps_the_access_point_and_its_peer_body() {
    let world = World::new();
    let policy = ApFollowPolicy {
        bands: ApBands::GHZ2_4,
        unservable: ApUnservable::LeaveUpstream,
        announce_count: 5,
    };
    let (mut upstream, mut pair, mut client) = set_up(&world, policy);
    let ghz5 = Channel::ghz5(36, ChannelWidth::Mhz20).unwrap();
    upstream
        .announce_channel_switch(ghz5, ChannelSwitchMode::Continue, 3)
        .unwrap();
    let until = world.after(1_000);
    let (_, (event, _)) = world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            pair.run_until(until, &mut |_| {}, &mut |_| {}),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    assert_eq!(event.unwrap(), Some(PortStaApEvent::UpstreamLeft));
    assert!(pair.station().connection().is_none());
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

fn policy() -> ApFollowPolicy {
    ApFollowPolicy {
        bands: ApBands {
            ghz2_4: true,
            ghz5: true,
        },
        announce_count: 3,
        unservable: ApUnservable::StopAccessPoint,
    }
}

fn lose_upstream(world: &World, upstream: &mut PortAccessPoint<'static, Env>, pair: &mut TestPair) {
    world.drive(upstream.stop()).unwrap();
    let event = world
        .drive(pair.run_until(world.after(1_000), &mut |_| {}, &mut |_| {}))
        .unwrap();
    assert!(matches!(
        event,
        Some(PortStaApEvent::StationEnded(
            PortDisconnect::Deauthenticated { .. }
        ))
    ));
    assert!(pair.station().connection().is_none());
}

fn ethernet(destination: MacAddress, source: MacAddress, payload: &[u8]) -> Vec<u8> {
    let mut frame = destination.to_vec();
    frame.extend_from_slice(&source);
    frame.extend_from_slice(&0x0800_u16.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Drop an attempt after it has sent its discovery probe and the radio
/// has completed every published transmission, while the dwell still waits.
async fn cancel_during_discovery(world: &World, attempt: impl Future) {
    let submitted = world.pair.0.submitted().len();
    let mut attempt = pin!(attempt);
    poll_fn(|context| {
        assert!(attempt.as_mut().poll(context).is_pending());
        let probed = world
            .pair
            .0
            .submitted()
            .iter()
            .skip(submitted)
            .any(|attempt| attempt.vif == STATION && attempt.frames[0][0] == 0x40);
        if probed && world.pair.0.in_flight() == 0 {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

#[test]
fn a_cancelled_initial_join_returns_no_station_on_the_next_attempt() {
    on_large_stack(a_cancelled_initial_join_returns_no_station_on_the_next_attempt_inner);
}

fn a_cancelled_initial_join_returns_no_station_on_the_next_attempt_inner() {
    let world = World::new();
    let mut pair = unconnected_pair(&world, policy());
    world.drive(cancel_during_discovery(&world, pair.connect()));
    assert!(matches!(
        world.drive(pair.connect()),
        Err(PortStaApError::NoStation)
    ));
    assert!(matches!(
        world.drive(pair.reconnect(&mut |_| {})),
        Err(PortStaApError::NoStation)
    ));
}

#[test]
fn a_cancelled_rejoin_returns_no_station_on_the_next_attempt() {
    on_large_stack(a_cancelled_rejoin_returns_no_station_on_the_next_attempt_inner);
}

fn a_cancelled_rejoin_returns_no_station_on_the_next_attempt_inner() {
    let world = World::new();
    let (mut upstream, mut pair, _) = set_up(&world, policy());
    lose_upstream(&world, &mut upstream, &mut pair);
    world.drive(cancel_during_discovery(&world, pair.reconnect(&mut |_| {})));
    assert!(matches!(
        world.drive(pair.connect()),
        Err(PortStaApError::NoStation)
    ));
    assert!(matches!(
        world.drive(pair.reconnect(&mut |_| {})),
        Err(PortStaApError::NoStation)
    ));
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
}

#[test]
fn current_channel_retries_keep_the_access_point_and_bidirectional_data() {
    on_large_stack(current_channel_retries_keep_the_access_point_and_bidirectional_data_inner);
}

fn current_channel_retries_keep_the_access_point_and_bidirectional_data_inner() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, policy());
    let association_id = client.connection().unwrap().config().association_id;
    let peer = pair.access_point().service().peer_status(CLIENT).unwrap();
    let ap_vif = world.pair.0.vif_config(ACCESS_POINT);
    lose_upstream(&world, &mut upstream, &mut pair);
    let channel_updates = world.pair.0.channel_updates();
    let lifecycle_requests = world.pair.0.lifecycle_requests();
    let observe = || {
        assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
        assert_eq!(world.pair.0.channel_updates(), channel_updates);
        assert_eq!(world.pair.0.lifecycle_requests(), lifecycle_requests);
        assert_eq!(world.pair.0.vif_config(ACCESS_POINT), ap_vif);
        assert!(world.pair.0.gate_open());
    };
    // An unconstrained scan would find this upstream on channel 11 and
    // take the pair's radio away from its existing access-point peer.
    world
        .drive(upstream.client_mut().retune(ghz2_4(11)))
        .unwrap();
    upstream.start(ghz2_4(11)).unwrap();
    for _ in 0..2 {
        let beacons = pair.access_point().counters().beacons;
        let until = world.after(500);
        let (_, (result, _)) = world.drive_observing(
            join(
                serve_upstream(&mut upstream, until),
                join(
                    pair.reconnect(&mut |_| {}),
                    serve_client(&mut client, world.timer, until),
                ),
            ),
            observe,
        );
        assert!(
            matches!(
                result,
                Err(PortStaApError::Join(PortStationError::NoCandidate))
            ),
            "retry: {result:?}"
        );
        assert!(pair.access_point().counters().beacons > beacons);
        assert!(pair.station().connection().is_none());
    }
    // The AP also serves by itself between explicit retry attempts.
    let until = world.after(500);
    world.drive_observing(
        join(
            serve_pair(&mut pair, until),
            serve_client(&mut client, world.timer, until),
        ),
        observe,
    );
    world.drive(upstream.stop()).unwrap();
    world
        .drive(upstream.client_mut().retune(ghz2_4(6)))
        .unwrap();
    upstream.start(ghz2_4(6)).unwrap();
    let to_ap = ethernet(PAIR_ACCESS_POINT, CLIENT, b"during upstream join");
    let to_client = ethernet(CLIENT, PAIR_ACCESS_POINT, b"access point stays up");
    assert!(world.client_frames.push(TestFrame(to_ap.clone())).is_ok());
    assert!(world.pair_frames.push(TestFrame(to_client.clone())).is_ok());
    let mut received_ap = Vec::new();
    let mut received_client = Vec::new();
    let until = world.after(500);
    let (_, (result, client_result)) = world.drive_observing(
        join(
            serve_upstream(&mut upstream, until),
            join(
                pair.reconnect(&mut |msdu| {
                    let parts = msdu.parts();
                    let mut frame = vec![0; parts.length()];
                    parts.copy_to(&mut frame).unwrap();
                    received_ap.push(frame);
                }),
                client.run_until(until, &mut |msdu| {
                    let parts = msdu.parts();
                    let mut frame = vec![0; parts.length()];
                    parts.copy_to(&mut frame).unwrap();
                    received_client.push(frame);
                }),
            ),
        ),
        observe,
    );
    result.unwrap();
    assert_eq!(client_result.unwrap(), None);
    assert_eq!(received_ap, [to_ap]);
    assert_eq!(received_client, [to_client]);
    assert!(world.client_frames.is_empty());
    assert!(world.pair_frames.is_empty());
    assert_eq!(
        client.connection().unwrap().config().association_id,
        association_id
    );
    let after = pair.access_point().service().peer_status(CLIENT).unwrap();
    assert_eq!(after.association_id, peer.association_id);
    assert_eq!(after.association_epoch, peer.association_epoch);
    assert_eq!(after.phase, peer.phase);
    assert!(pair.station().connection().is_some());
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(6)
    );
}

#[test]
fn initial_connect_cannot_rescan_a_running_access_point() {
    on_large_stack(initial_connect_cannot_rescan_a_running_access_point_inner);
}

fn initial_connect_cannot_rescan_a_running_access_point_inner() {
    let world = World::new();
    let (mut upstream, mut pair, _) = set_up(&world, policy());
    assert!(matches!(
        world.drive(pair.connect()),
        Err(PortStaApError::AlreadyConnected)
    ));
    assert!(matches!(
        world.drive(pair.reconnect(&mut |_| {})),
        Err(PortStaApError::AlreadyConnected)
    ));
    lose_upstream(&world, &mut upstream, &mut pair);
    let submitted = world.pair.0.submitted().len();
    assert!(matches!(
        world.drive(pair.connect()),
        Err(PortStaApError::AccessPointRunning)
    ));
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
    assert_eq!(world.pair.0.submitted().len(), submitted);
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

#[test]
fn reconnect_refreshes_the_candidate_after_a_coordinated_channel_switch() {
    on_large_stack(reconnect_refreshes_the_candidate_after_a_coordinated_channel_switch_inner);
}

fn reconnect_refreshes_the_candidate_after_a_coordinated_channel_switch_inner() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, policy());
    upstream
        .announce_channel_switch(ghz2_4(11), ChannelSwitchMode::Continue, 3)
        .unwrap();
    let until = world.after(1_000);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            serve_pair(&mut pair, until),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    assert_eq!(pair.station().candidate().unwrap().channel, 6);
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(11)
    );
    lose_upstream(&world, &mut upstream, &mut pair);
    upstream.start(ghz2_4(11)).unwrap();
    let until = world.after(500);
    let (_, (result, _)) = world.drive_observing(
        join(
            serve_upstream(&mut upstream, until),
            join(
                pair.reconnect(&mut |_| {}),
                serve_client(&mut client, world.timer, until),
            ),
        ),
        || assert_eq!(world.pair.0.channel(), Some(ghz2_4(11))),
    );
    result.unwrap();
    assert_eq!(pair.station().candidate().unwrap().channel, 11);
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(11)
    );
    assert!(client.connection().is_some());
}

#[test]
fn reconnect_waits_for_an_announced_move_after_the_upstream_is_lost() {
    on_large_stack(reconnect_waits_for_an_announced_move_after_the_upstream_is_lost_inner);
}

fn reconnect_waits_for_an_announced_move_after_the_upstream_is_lost_inner() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, policy());
    upstream
        .announce_channel_switch(ghz2_4(11), ChannelSwitchMode::Continue, 5)
        .unwrap();
    let until = world.after(120);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        serve_pair(&mut pair, until),
    ));
    lose_upstream(&world, &mut upstream, &mut pair);
    assert!(matches!(
        world.drive(pair.reconnect(&mut |_| {})),
        Err(PortStaApError::ChannelSwitchPending)
    ));
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
    world
        .drive(upstream.client_mut().retune(ghz2_4(11)))
        .unwrap();
    upstream.start(ghz2_4(11)).unwrap();
    // The owner finishes its move while serving just the access point.
    let until = world.after(1_000);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            serve_pair(&mut pair, until),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(11)));
    let until = world.after(500);
    let (_, result) = world.drive(join(
        serve_upstream(&mut upstream, until),
        pair.reconnect(&mut |_| {}),
    ));
    result.unwrap();
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(11)
    );
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
    assert!(client.connection().is_some());
}

#[test]
fn a_failed_authentication_retains_the_station_and_keeps_publishing_beacons() {
    on_large_stack(a_failed_authentication_retains_the_station_and_keeps_publishing_beacons_inner);
}

#[test]
fn passive_absences_are_nav_protected_dense_then_sparse_and_keep_peer_data() {
    on_large_stack(passive_absences_are_nav_protected_dense_then_sparse_and_keep_peer_data_inner);
}

fn passive_absences_are_nav_protected_dense_then_sparse_and_keep_peer_data_inner() {
    let world = World::new();
    let search = ApSearchPolicy::new(&SCANNED);
    let (mut upstream, mut pair, mut client) = set_up_with_search(&world, policy(), search);
    let peer = pair.access_point().service().peer_status(CLIENT).unwrap();
    lose_upstream(&world, &mut upstream, &mut pair);
    let lost = world.timer.now();
    let submitted = world.pair.0.submitted().len();
    let lifecycle = world.pair.0.lifecycle_requests();
    let to_ap = ethernet(PAIR_ACCESS_POINT, CLIENT, b"uplink during absence");
    let to_client = ethernet(CLIENT, PAIR_ACCESS_POINT, b"downlink during absence");
    let mut received_ap = Vec::new();
    let mut received_client = Vec::new();
    let mut queued = false;
    let mut away = None;
    let mut windows = Vec::new();
    let until = world.after(33_000);
    let (pair_result, client_result) = world.drive_observing(
        join(
            pair.run_until(until, &mut |_| {}, &mut |msdu| {
                let parts = msdu.parts();
                let mut frame = vec![0; parts.length()];
                parts.copy_to(&mut frame).unwrap();
                received_ap.push(frame);
            }),
            client.run_until(until, &mut |msdu| {
                let parts = msdu.parts();
                let mut frame = vec![0; parts.length()];
                parts.copy_to(&mut frame).unwrap();
                received_client.push(frame);
            }),
        ),
        || {
            let channel = world.pair.0.channel().unwrap();
            if channel != ghz2_4(6) {
                if away.is_none() {
                    assert!(world.client.0.nav_until().is_some(), "departure before CTS");
                    away = Some((world.timer.now(), channel));
                }
                assert!(
                    world.pair.0.published().is_empty(),
                    "AP TX in a receive-only visit"
                );
                if !queued {
                    assert!(world.client_frames.push(TestFrame(to_ap.clone())).is_ok());
                    assert!(world.pair_frames.push(TestFrame(to_client.clone())).is_ok());
                    queued = true;
                }
            } else if let Some((start, channel)) = away.take() {
                let end = world.timer.now();
                assert!(end.saturating_duration_since(start) <= search.dwell);
                windows.push((start, end, channel));
            }
            assert_eq!(world.pair.0.lifecycle_requests(), lifecycle);
        },
    );
    assert_eq!(pair_result.unwrap(), None);
    assert_eq!(client_result.unwrap(), None);
    assert!(away.is_none());
    assert_eq!(received_ap, [to_ap]);
    assert_eq!(received_client, [to_client]);
    assert!(world.pair_frames.is_empty());
    assert!(world.client_frames.is_empty());
    assert!(pair.station().connection().is_none());
    let after = pair.access_point().service().peer_status(CLIENT).unwrap();
    assert_eq!(after.association_id, peer.association_id);
    assert_eq!(after.association_epoch, peer.association_epoch);
    let dense_end = lost.checked_add(search.dense_period).unwrap();
    let dense: Vec<_> = windows
        .iter()
        .filter(|(start, _, _)| *start < dense_end)
        .collect();
    let sparse: Vec<_> = windows
        .iter()
        .filter(|(start, _, _)| *start >= dense_end)
        .collect();
    assert!(
        (290..=295).contains(&dense.len()),
        "{} dense visits",
        dense.len()
    );
    assert!(
        (2..=4).contains(&sparse.len()),
        "{} sparse visits",
        sparse.len()
    );
    for pair in dense.windows(2) {
        assert_eq!(
            pair[1].0.saturating_duration_since(pair[0].0),
            Duration::from_micros(102_400)
        );
    }
    for pair in sparse.windows(2) {
        let gap = pair[1].0.saturating_duration_since(pair[0].0);
        assert!(gap >= search.sparse_interval);
        assert!(
            gap < search
                .sparse_interval
                .checked_add(Duration::from_micros(102_400))
                .unwrap()
        );
    }
    for (index, (_, _, channel)) in windows.iter().enumerate() {
        assert_eq!(*channel, ghz2_4(if index % 2 == 0 { 1 } else { 11 }));
    }
    let attempts = world.pair.0.submitted();
    let attempts = &attempts[submitted..];
    assert!(
        attempts.iter().all(|attempt| attempt.frames[0][0] != 0x40),
        "passive search sent a probe"
    );
    let reservations: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.frames[0][0] == 0xc4)
        .collect();
    assert_eq!(reservations.len(), windows.len());
    for reservation in reservations {
        let frame = &reservation.frames[0];
        assert_eq!(&frame[4..10], &PAIR_ACCESS_POINT);
        let duration = u16::from_le_bytes([frame[2], frame[3]]);
        assert!((1..=20_000).contains(&duration));
    }
}

#[test]
fn passive_search_rejoins_an_upstream_returning_on_the_same_channel() {
    on_large_stack(|| passive_recovery(6));
}

#[test]
fn passive_search_moves_the_access_point_by_csa_before_joining_a_different_channel() {
    on_large_stack(|| passive_recovery(11));
}

fn passive_recovery(number: u8) {
    let world = World::new();
    let search = ApSearchPolicy::new(&SCANNED);
    let (mut upstream, mut pair, mut client) = set_up_with_search(&world, policy(), search);
    let peer = pair.access_point().service().peer_status(CLIENT).unwrap();
    let aid = client.connection().unwrap().config().association_id;
    lose_upstream(&world, &mut upstream, &mut pair);
    let lifecycle = world.pair.0.lifecycle_requests();
    let submitted = world.pair.0.submitted().len();
    // Give the returning AP a beacon phase inside the passive window.
    // Equal beacon intervals with a phase outside it are intentionally
    // not a promise of eventual passive discovery.
    let restart = pair
        .access_point()
        .schedule()
        .unwrap()
        .next_tbtt
        .checked_add(Duration::from_millis(7))
        .unwrap();
    world.drive(world.timer.wait_until(restart));
    world
        .drive(upstream.client_mut().retune(ghz2_4(number)))
        .unwrap();
    upstream.start(ghz2_4(number)).unwrap();
    let until = world.after(2_000);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            serve_pair(&mut pair, until),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(number)
    );
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(number)));
    assert_eq!(world.client.0.channel(), Some(ghz2_4(number)));
    assert_eq!(client.connection().unwrap().config().association_id, aid);
    assert_eq!(world.pair.0.lifecycle_requests(), lifecycle);
    let after = pair.access_point().service().peer_status(CLIENT).unwrap();
    assert_eq!(after.association_id, peer.association_id);
    assert_eq!(after.association_epoch, peer.association_epoch);
    let attempts = world.pair.0.submitted();
    assert!(
        attempts[submitted..]
            .iter()
            .all(|attempt| attempt.frames[0][0] != 0x40)
    );
    // Data still crosses the preserved downstream association after the move.
    assert!(
        world
            .pair_frames
            .push(TestFrame(ethernet(
                CLIENT,
                PAIR_ACCESS_POINT,
                b"after recovery"
            )))
            .is_ok()
    );
    assert!(
        world
            .client_frames
            .push(TestFrame(ethernet(
                PAIR_ACCESS_POINT,
                CLIENT,
                b"after recovery"
            )))
            .is_ok()
    );
    let mut down = 0;
    let mut up = 0;
    let until = world.after(500);
    let (_, (pair_result, client_result)) = world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            pair.run_until(until, &mut |_| {}, &mut |_| up += 1),
            client.run_until(until, &mut |_| down += 1),
        ),
    ));
    assert_eq!(pair_result.unwrap(), None);
    assert_eq!(client_result.unwrap(), None);
    assert_eq!((up, down), (1, 1));
}

#[test]
fn a_delayed_or_failed_cts_never_departures_without_a_live_reservation() {
    on_large_stack(a_delayed_or_failed_cts_never_departures_without_a_live_reservation_inner);
}

fn a_delayed_or_failed_cts_never_departures_without_a_live_reservation_inner() {
    let world = World::new();
    let mut ap = access_point(
        world.pair.1,
        world.timer,
        PAIR_ACCESS_POINT,
        PAIR_SSID,
        ghz2_4(6),
        world.pair_frames,
    );
    world.drive(ap.client_mut().retune(ghz2_4(6))).unwrap();
    ap.start(ghz2_4(6)).unwrap();
    let lifecycle = world.pair.0.lifecycle_requests();
    let window = AbsenceWindow {
        home: ghz2_4(6),
        channel: ghz2_4(11),
        start: world.timer.now(),
        until: world.after(20),
    };
    let mut delayed = false;
    let mut state = AbsenceState::new();
    let begun = world
        .drive_observing(
            PortAbsence::begin(
                ap.client_mut(),
                world.timer,
                &mut state,
                window,
                MANAGEMENT_RATE,
                CoexPriority::Normal,
            ),
            || {
                assert_eq!(world.pair.0.channel(), Some(window.home));
                if !delayed && world.pair.0.in_flight() > 0 {
                    delayed = true;
                    let late = window.until.checked_add(Duration::from_millis(1)).unwrap();
                    world.timer.advance_to(late);
                    for model in [world.upstream.0, world.pair.0, world.client.0] {
                        model.set_now(RadioInstant::from_micros(late.as_micros()));
                    }
                }
            },
        )
        .unwrap();
    assert!(begun.is_none());
    drop(begun);
    assert!(delayed);
    assert_eq!(world.pair.0.channel(), Some(window.home));
    assert_eq!(world.pair.0.in_flight(), 0);

    // An unsuccessful completion also leaves the operating channel alone.
    world
        .pair
        .0
        .respond([oer_ieee80211_lower_mac::model::ModelOutcome::Fail(
            oer_ieee80211_lower_mac::TxStatus::Aborted,
        )]);
    let window = AbsenceWindow {
        start: world.timer.now(),
        until: world.after(20),
        ..window
    };
    assert!(matches!(
        world.drive(PortAbsence::begin(
            ap.client_mut(),
            world.timer,
            &mut state,
            window,
            MANAGEMENT_RATE,
            CoexPriority::Normal,
        )),
        Err(AbsenceError::Reservation(
            oer_ieee80211_lower_mac::TxStatus::Aborted
        ))
    ));
    assert_eq!(world.pair.0.channel(), Some(window.home));
    assert_eq!(world.pair.0.lifecycle_requests(), lifecycle);
}

#[test]
fn dropping_a_passive_visit_returns_home_without_an_in_flight_attempt() {
    on_large_stack(dropping_a_passive_visit_returns_home_without_an_in_flight_attempt_inner);
}

fn dropping_a_passive_visit_returns_home_without_an_in_flight_attempt_inner() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) =
        set_up_with_search(&world, policy(), ApSearchPolicy::new(&SCANNED));
    lose_upstream(&world, &mut upstream, &mut pair);
    let until = world.after(1_000);
    world.drive(async {
        let mut station_deliver = |_: oer_ieee80211_upper_mac_service::client::PortMsdu<
            '_,
            oer_ieee80211_lower_mac::model::ModelRxBuffer,
        >| {};
        let mut ap_deliver = |_: oer_ieee80211_upper_mac_service::client::PortMsdu<
            '_,
            oer_ieee80211_lower_mac::model::ModelRxBuffer,
        >| {};
        let mut visit = pin!(pair.run_until(until, &mut station_deliver, &mut ap_deliver));
        poll_fn(|context| {
            assert!(visit.as_mut().poll(context).is_pending());
            if world.pair.0.channel() != Some(ghz2_4(6)) {
                assert_eq!(world.pair.0.in_flight(), 0);
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    });
    assert_eq!(world.pair.0.channel(), Some(ghz2_4(6)));
    assert_eq!(world.pair.0.in_flight(), 0);
    assert!(pair.station().connection().is_none());
    let until = world.after(500);
    world.drive(join(
        serve_pair(&mut pair, until),
        serve_client(&mut client, world.timer, until),
    ));
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

#[test]
fn a_failed_passive_return_blocks_clients_until_port_recovery() {
    on_large_stack(a_failed_passive_return_blocks_clients_until_port_recovery_inner);
}

fn a_failed_passive_return_blocks_clients_until_port_recovery_inner() {
    let world = World::new();
    let mut ap = access_point(
        world.pair.1,
        world.timer,
        PAIR_ACCESS_POINT,
        PAIR_SSID,
        ghz2_4(6),
        world.pair_frames,
    );
    world.drive(ap.client_mut().retune(ghz2_4(6))).unwrap();
    ap.start(ghz2_4(6)).unwrap();
    let mut state = AbsenceState::new();
    let window = AbsenceWindow {
        home: ghz2_4(6),
        channel: ghz2_4(11),
        start: world.timer.now(),
        until: world.after(20),
    };
    let guard = world
        .drive(PortAbsence::begin(
            ap.client_mut(),
            world.timer,
            &mut state,
            window,
            MANAGEMENT_RATE,
            CoexPriority::Normal,
        ))
        .unwrap()
        .unwrap();
    world.pair.0.poison();
    drop(guard);
    assert!(state.recovery_required());
    assert!(matches!(
        world.drive(PortAbsence::begin(
            ap.client_mut(),
            world.timer,
            &mut state,
            window,
            MANAGEMENT_RATE,
            CoexPriority::Normal,
        )),
        Err(AbsenceError::RecoveryRequired)
    ));
}

#[test]
fn an_explicit_reconnect_can_complete_a_pending_current_channel_recovery() {
    on_large_stack(an_explicit_reconnect_can_complete_a_pending_current_channel_recovery_inner);
}

fn an_explicit_reconnect_can_complete_a_pending_current_channel_recovery_inner() {
    let world = World::new();
    let (mut upstream, mut pair, _) = set_up(&world, policy());
    lose_upstream(&world, &mut upstream, &mut pair);
    let updates = world.pair.0.channel_updates();
    upstream.start(ghz2_4(6)).unwrap();
    let until = world.after(10);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        serve_pair(&mut pair, until),
    ));
    // Discovery completed at the caller's boundary; the auto-join is still
    // pending, so the caller can explicitly choose fresh discovery instead.
    assert!(pair.station().connection().is_none());
    let until = world.after(500);
    let (_, result) = world.drive(join(
        serve_upstream(&mut upstream, until),
        pair.reconnect(&mut |_| {}),
    ));
    result.unwrap();
    let until = world.after(500);
    world.drive(join(
        serve_upstream(&mut upstream, until),
        serve_pair(&mut pair, until),
    ));
    assert!(pair.station().connection().is_some());
    assert_eq!(world.pair.0.channel_updates(), updates);
    assert_eq!(
        pair.station().connection().unwrap().config().channel,
        ghz2_4(6)
    );
}

#[test]
fn invalid_search_windows_are_refused_before_reserving_or_retuning() {
    on_large_stack(invalid_search_windows_are_refused_before_reserving_or_retuning_inner);
}

fn invalid_search_windows_are_refused_before_reserving_or_retuning_inner() {
    let world = World::new();
    let mut search = ApSearchPolicy::new(&SCANNED);
    search.dwell = Duration::ZERO;
    let (mut upstream, mut pair, _) = set_up_with_search(&world, policy(), search);
    lose_upstream(&world, &mut upstream, &mut pair);
    let updates = world.pair.0.channel_updates();
    let submitted = world.pair.0.submitted().len();
    assert!(matches!(
        world.drive(pair.run_until(world.after(500), &mut |_| {}, &mut |_| {})),
        Err(PortStaApError::Absence(AbsenceError::InvalidWindow))
    ));
    assert_eq!(world.pair.0.channel_updates(), updates);
    assert!(
        world.pair.0.submitted()[submitted..]
            .iter()
            .all(|attempt| attempt.frames[0][0] != 0xc4)
    );
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

#[test]
fn passive_absences_use_the_configured_dwell() {
    on_large_stack(passive_absences_use_the_configured_dwell_inner);
}

fn passive_absences_use_the_configured_dwell_inner() {
    let world = World::new();
    let mut search = ApSearchPolicy::new(&SCANNED);
    search.dwell = Duration::from_millis(25);
    let (mut upstream, mut pair, _) = set_up_with_search(&world, policy(), search);
    lose_upstream(&world, &mut upstream, &mut pair);
    let mut away = None;
    let mut visits = Vec::new();
    let result = world.drive_observing(
        pair.run_until(world.after(500), &mut |_| {}, &mut |_| {}),
        || {
            if world.pair.0.channel() != Some(ghz2_4(6)) {
                away.get_or_insert(world.timer.now());
            } else if let Some(start) = away.take() {
                visits.push(world.timer.now().saturating_duration_since(start));
            }
        },
    );
    assert_eq!(result.unwrap(), None);
    assert!(visits.len() >= 3);
    assert!(visits.iter().all(|duration| *duration == search.dwell));
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

#[test]
fn windows_reaching_the_next_tbtt_are_skipped_while_the_ap_keeps_serving() {
    on_large_stack(windows_reaching_the_next_tbtt_are_skipped_while_the_ap_keeps_serving_inner);
}

fn windows_reaching_the_next_tbtt_are_skipped_while_the_ap_keeps_serving_inner() {
    let world = World::new();
    let mut search = ApSearchPolicy::new(&SCANNED);
    search.guard = Duration::from_millis(90);
    search.dwell = Duration::from_millis(25);
    let (mut upstream, mut pair, _) = set_up_with_search(&world, policy(), search);
    lose_upstream(&world, &mut upstream, &mut pair);
    let beacons = pair.access_point().counters().beacons;
    let submitted = world.pair.0.submitted().len();
    let updates = world.pair.0.channel_updates();
    // End at a TBTT so clipping to the caller's deadline cannot turn the
    // last overlapping window into a safe shorter visit.
    let until = pair
        .access_point()
        .schedule()
        .unwrap()
        .next_tbtt
        .checked_add(Duration::from_micros(4 * 102_400))
        .unwrap();
    assert_eq!(
        world
            .drive(pair.run_until(until, &mut |_| {}, &mut |_| {}))
            .unwrap(),
        None
    );
    assert!(pair.access_point().counters().beacons >= beacons + 4);
    assert_eq!(world.pair.0.channel_updates(), updates);
    assert!(
        world.pair.0.submitted()[submitted..]
            .iter()
            .all(|attempt| attempt.frames[0][0] != 0xc4)
    );
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

#[test]
fn a_beacon_delayed_past_the_search_window_skips_it_without_ending_the_run() {
    on_large_stack(a_beacon_delayed_past_the_search_window_skips_it_without_ending_the_run_inner);
}

fn a_beacon_delayed_past_the_search_window_skips_it_without_ending_the_run_inner() {
    let world = World::new();
    let (mut upstream, mut pair, _) =
        set_up_with_search(&world, policy(), ApSearchPolicy::new(&SCANNED));
    lose_upstream(&world, &mut upstream, &mut pair);
    let next_tbtt = pair.access_point().schedule().unwrap().next_tbtt;
    let late = next_tbtt.checked_add(Duration::from_millis(30)).unwrap();
    let until = late.checked_add(Duration::from_millis(10)).unwrap();
    let updates = world.pair.0.channel_updates();
    let submitted = world.pair.0.submitted().len();
    let mut delayed = false;
    let result = world.drive_observing(pair.run_until(until, &mut |_| {}, &mut |_| {}), || {
        if !delayed
            && world
                .pair
                .0
                .published()
                .iter()
                .any(|(_, attempt)| attempt.frames[0][0] == 0x80)
        {
            delayed = true;
            world.timer.advance_to(late);
            for model in [world.upstream.0, world.pair.0, world.client.0] {
                model.set_now(RadioInstant::from_micros(late.as_micros()));
            }
        }
    });
    assert!(delayed);
    assert_eq!(result.unwrap(), None);
    assert_eq!(world.pair.0.channel_updates(), updates);
    assert!(
        world.pair.0.submitted()[submitted..]
            .iter()
            .all(|attempt| attempt.frames[0][0] != 0xc4)
    );
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
}

fn a_failed_authentication_retains_the_station_and_keeps_publishing_beacons_inner() {
    let world = World::new();
    let (mut upstream, mut pair, mut client) = set_up(&world, policy());
    lose_upstream(&world, &mut upstream, &mut pair);
    upstream.start(ghz2_4(6)).unwrap();
    let beacons = pair.access_point().counters().beacons;
    // Answer the discovery probe, then stop processing upstream requests
    // before the 200 ms dwell ends: MAC ACKs cannot complete authentication.
    let until = world.after(100);
    let (_, result) = world.drive_observing(
        join(
            serve_upstream(&mut upstream, until),
            pair.reconnect(&mut |_| {}),
        ),
        || assert_eq!(world.pair.0.channel(), Some(ghz2_4(6))),
    );
    assert!(matches!(
        result,
        Err(PortStaApError::Join(PortStationError::Join(_)))
    ));
    assert!(pair.access_point().counters().beacons >= beacons + 3);
    assert!(pair.station().connection().is_none());
    assert!(pair.access_point().service().peer_status(CLIENT).is_some());
    let until = world.after(500);
    let (_, (result, _)) = world.drive(join(
        serve_upstream(&mut upstream, until),
        join(
            pair.reconnect(&mut |_| {}),
            serve_client(&mut client, world.timer, until),
        ),
    ));
    result.unwrap();
    assert!(pair.station().connection().is_some());
    assert!(client.connection().is_some());
}

#[path = "sta_ap/current_channel.rs"]
mod current_channel;
