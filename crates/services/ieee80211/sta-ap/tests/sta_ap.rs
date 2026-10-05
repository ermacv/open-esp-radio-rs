//! A station and an access point on one port, against real peers on host
//! models that share an air: an upstream access point the station joins,
//! and a station that joins the pair's access point.

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_ieee80211_ap::coordinator::{
    ApBands, ApFollowPolicy, ApSearchPolicy, ApUnservable, ChannelCoordinator,
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
    Channel, ChannelWidth, CoexPriority, Ieee80211LowerMacPort, LifecycleCommand, LowerMacSetting,
    MacAddress, TxPower, VifId, VifRole,
    model::{LowerMacModel, ModelAir},
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
use oer_ieee80211_sta_ap_service::{PortStaAp, PortStaApEvent};
use oer_ieee80211_sta_service::port::{
    NoCoexistence, PortDisconnect, PortLink, PortLinkSupervision, PortProbe, PortStation,
    PortStationConfig, PortStationEnv, PortStationEvent, PortStationProfile, PortStationStorage,
};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, FixedRate, ProtectEveryHeTxop, ProtectionPolicy, RetryLimits, TxPlanner,
    rate_control::FixedRateControl,
};
use oer_ieee80211_upper_mac_service::{
    PortRouter,
    aggregate::PortAmpduAggregation,
    client::{PortClient, PortClientConfig, PortClientEnv},
    frame::NetworkBody,
};
use oer_network_interface::NetworkInterfaceId;
use oer_time::{Clock, Duration, Instant, RadioInstant, Timer};

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

/// Virtual monotonic time: waits end when the harness advances it.
#[derive(Default)]
struct VirtualTimer {
    now: Cell<u64>,
    wanted: Cell<Option<u64>>,
}

impl Clock for &VirtualTimer {
    fn now(&self) -> Instant {
        Instant::from_micros(self.now.get())
    }
}

impl Timer for &VirtualTimer {
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
type Router = PortRouter<'static, Model>;

/// One environment for every driver of the test.
struct Env(PhantomData<()>);

impl PortClientEnv for Env {
    type NetworkFrame = TestFrame;
    type Port = Model;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = PortAmpduAggregation;
}

impl PortStationEnv for Env {
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

impl PortApEnv for Env {
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
    model.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    (model, leak(PortRouter::new(model, 1)))
}

fn station(
    router: &'static Router,
    timer: &'static VirtualTimer,
    address: MacAddress,
    ssid: &'static [u8],
) -> PortStation<'static, Env> {
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
            dwell_tick: Duration::from_millis(10),
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
        leak(TestFrames::new()),
    )
}

fn access_point(
    router: &'static Router,
    timer: &'static VirtualTimer,
    address: MacAddress,
    ssid: &'static [u8],
    channel: Channel,
) -> PortAccessPoint<'static, Env> {
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
            frames: leak(TestFrames::new()),
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
}

impl World {
    fn new() -> Self {
        let timer = leak(VirtualTimer {
            now: Cell::new(1_000),
            wanted: Cell::new(None),
        });
        Self {
            timer,
            upstream: radio(),
            pair: radio(),
            client: radio(),
        }
    }

    fn now(&self) -> u64 {
        self.timer.now.get()
    }

    fn after(&self, millis: u64) -> Instant {
        Instant::from_micros(self.now() + millis * 1_000)
    }

    /// Poll `future` to its end beside the three routers: the air carries
    /// what the radios publish, and when nothing moves, virtual time
    /// advances to the earliest deadline anyone waits for.
    fn drive<F: Future>(&self, future: F) -> F::Output {
        let air = ModelAir::new([self.upstream.0, self.pair.0, self.client.0]);
        let mut future = pin!(future);
        let mut routers = [
            pin!(self.upstream.1.run()),
            pin!(self.pair.1.run()),
            pin!(self.client.1.run()),
        ];
        let mut context = Context::from_waker(Waker::noop());
        let mut quiet = 0;
        for _ in 0..50_000_000 {
            self.timer.wanted.set(None);
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            for router in &mut routers {
                assert!(
                    router.as_mut().poll(&mut context).is_pending(),
                    "a radio was poisoned"
                );
            }
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
                .wanted
                .get()
                .expect("the drivers wait for an event nothing produces");
            let next = next.max(self.now());
            self.timer.now.set(next);
            for model in [self.upstream.0, self.pair.0, self.client.0] {
                model.set_now(RadioInstant::from_micros(next));
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
        let now = Instant::from_micros(timer.now.get());
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
async fn serve_pair(
    pair: &mut PortStaAp<'static, Env, Env, &'static VirtualTimer>,
    until: Instant,
) {
    let event = pair
        .run_until(until, &mut |_| {}, &mut |_| {})
        .await
        .unwrap();
    assert_eq!(event, None);
}

fn connected(
    outcome: AssociationAttemptOutcome<
        PortStation<'static, Env>,
        PortStation<'static, Env>,
        oer_ieee80211_sta_service::port::PortAttemptError<Env>,
    >,
) -> PortStation<'static, Env> {
    match outcome {
        AssociationAttemptOutcome::Connected { connected, .. } => connected,
        AssociationAttemptOutcome::Failed(failure) => {
            panic!("the join failed at {:?}", failure.stage)
        }
    }
}

/// The upstream on channel 6, the pair joined to it with its access point
/// started there, and the client joined to the pair's access point.
fn set_up(
    world: &World,
    policy: ApFollowPolicy,
) -> (
    PortAccessPoint<'static, Env>,
    PortStaAp<'static, Env, Env, &'static VirtualTimer>,
    PortStation<'static, Env>,
) {
    let mut upstream = access_point(
        world.upstream.1,
        world.timer,
        UPSTREAM,
        UPSTREAM_SSID,
        ghz2_4(6),
    );
    world
        .drive(upstream.client_mut().retune(ghz2_4(6)))
        .unwrap();
    upstream.start(ghz2_4(6)).unwrap();

    let mut pair = PortStaAp::new(
        station(world.pair.1, world.timer, PAIR_STATION, UPSTREAM_SSID),
        access_point(
            world.pair.1,
            world.timer,
            PAIR_ACCESS_POINT,
            PAIR_SSID,
            ghz2_4(1),
        ),
        ChannelCoordinator::new(policy, ApSearchPolicy::new(&[])),
        world.timer,
    );
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

    let client = station(world.client.1, world.timer, CLIENT, PAIR_SSID);
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
