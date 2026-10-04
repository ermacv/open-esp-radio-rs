//! The access point over the lower-MAC host model: virtual time drives its
//! beacon schedule, and the test plays the stations that probe it.

use core::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_ieee80211_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    ApPeerPhase,
};
use oer_ieee80211_ap_service::port::{
    PortAccessPoint, PortApBuildError, PortApClient, PortApEnv, PortApProfile, PortApRouter,
};
use oer_ieee80211_lower_mac::{
    CoexPriority, Ieee80211LowerMacPort, LifecycleCommand, LowerMacBeaconTiming, MacAddress,
    PhyRate, ReceiveFilter, RxMeta, TxPower, VifId, VifRole,
    model::{LowerMacModel, ModelOutcome},
};
use oer_ieee80211_mac::{
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    beacon::{AP_BEACON_CAPACITY, dtim},
    channel::{Channel, WifiChannel},
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    phy::LegacyRate,
    qos::WmmAccessCategory,
    ssid::WifiSsid,
};
use oer_ieee80211_softmac::{BackoffEntropy, EdcaContention};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, ProtectEveryHeTxop, ProtectionPolicy, RateLadder, RetryLimits, TxPlanner,
};
use oer_ieee80211_upper_mac_service::client::{PortClient, PortClientConfig, PortClientEnv};
use oer_time::{Clock, Instant, Timer};

const AP: VifId = VifId(1);
const ADDRESS: MacAddress = [0x02, 0, 0, 0, 0, 0x0a];
const STATION: MacAddress = [0x02, 0, 0, 0, 0, 0x5a];
const SSID: &[u8] = b"port-ap";
const RATE: PhyRate = PhyRate::Legacy(LegacyRate::Dsss1M);
/// 100 TU.
const INTERVAL: u64 = 102_400;

const ADVERTISEMENT: Advertisement = Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
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
    0x0421,
);

/// Virtual monotonic time: waits end when the harness advances it.
#[derive(Default)]
struct VirtualTimer {
    now: Cell<u64>,
    /// The earliest deadline a pending wait asked for since the last poll.
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

struct FixedRate;

impl RateLadder for FixedRate {
    fn rate(&self, initial: PhyRate, _failures: u8) -> Option<PhyRate> {
        Some(initial)
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

struct Env<'a>(core::marker::PhantomData<&'a ()>);

impl PortClientEnv for Env<'_> {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
}

impl<'a> PortApEnv for Env<'a> {
    type Timer = &'a VirtualTimer;
}

fn channel() -> WifiChannel {
    WifiChannel::mhz20(6).unwrap()
}

/// A model enabled on another channel, as a composition leaves it.
fn model() -> LowerMacModel {
    let model = LowerMacModel::new();
    model
        .apply(oer_ieee80211_lower_mac::LowerMacSetting::Channel(
            Channel::from_wifi_channel(WifiChannel::mhz20(1).unwrap()),
        ))
        .unwrap()
        .unwrap();
    model.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    // Take the Enabled terminal event.
    {
        let mut next = pin!(model.next_event());
        let Poll::Ready(Ok(_)) = next.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("the model enabled");
        };
    }
    // Every attempt the access point makes succeeds.
    model.respond(core::iter::repeat_n(ModelOutcome::Success, 256));
    model
}

fn client<'a>(router: &'a PortApRouter<'a, Env<'a>>) -> PortApClient<'a, Env<'a>> {
    PortClient::new(
        router,
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
        ),
        FixedRate,
        Seeded(0x2468_ace1),
        PortClientConfig {
            vif: AP,
            address: ADDRESS,
            role: VifRole::AccessPoint,
            power: TxPower::Calibrated,
            retry_limit: 7,
        },
    )
}

/// An Open BSS of four peers whose inactivity closes them after 10 s.
fn service() -> AccessPointService<'static> {
    AccessPointService::new_open(
        ADDRESS,
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    )
}

fn profile(ssid: &WifiSsid) -> PortApProfile<'_> {
    PortApProfile {
        ssid,
        channel: channel(),
        beacon_interval_tu: 100,
        dtim_period: 2,
        advertisement: &ADVERTISEMENT,
        management_rate: RATE,
        coex: CoexPriority::Normal,
    }
}

/// Poll `future` to its end beside the router; when neither moves, virtual
/// time advances to the earliest of the deadlines a wait asked for and the
/// `stops` still ahead, and `at` runs at each new time.
fn drive<T>(
    model: &LowerMacModel,
    router: &PortApRouter<'_, Env<'_>>,
    timer: &VirtualTimer,
    future: impl Future<Output = T>,
    stops: &[u64],
    mut at: impl FnMut(u64),
) -> T {
    let mut future = pin!(future);
    let mut routing = pin!(router.run());
    let mut context = Context::from_waker(Waker::noop());
    let mut quiet = false;
    for _ in 0..10_000 {
        timer.wanted.set(None);
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        // Events the access point's poll produced are progress, whether
        // or not the router takes them now.
        let produced = model.queued_events() > 0;
        assert!(routing.as_mut().poll(&mut context).is_pending());
        if produced || model.queued_events() > 0 {
            quiet = false;
            continue;
        }
        // The router may have answered a wait the access point registered
        // after its poll: poll once more before moving time.
        if !quiet {
            quiet = true;
            continue;
        }
        quiet = false;
        let stop = stops.iter().copied().find(|stop| *stop > timer.now.get());
        let next = match (timer.wanted.get(), stop) {
            (Some(wanted), Some(stop)) => wanted.min(stop),
            (wanted, stop) => wanted
                .or(stop)
                .expect("the access point waits for an event nothing produces"),
        };
        timer.now.set(next.max(timer.now.get()));
        model.set_now(oer_time::RadioInstant::from_micros(timer.now.get()));
        at(timer.now.get());
    }
    panic!("the access point did not finish");
}

/// A Probe Request from the station to `receiver`, BSSID `bssid`, for
/// `ssid` (empty: the wildcard SSID).
fn probe_request(receiver: MacAddress, bssid: MacAddress, ssid: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x40, 0, 0, 0];
    frame.extend_from_slice(&receiver);
    frame.extend_from_slice(&STATION);
    frame.extend_from_slice(&bssid);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&[0, ssid.len() as u8]);
    frame.extend_from_slice(ssid);
    frame.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    frame
}

fn meta() -> RxMeta {
    RxMeta::unavailable(Channel::from_wifi_channel(channel()))
}

#[test]
fn the_access_point_starts_its_bss_and_beacons_at_every_tbtt() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        client(&router),
        &timer,
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();

    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    assert_eq!(model.channel(), Some(Channel::from_wifi_channel(channel())));
    let vif = model.vif_config(AP).unwrap();
    assert_eq!(
        (vif.address, vif.role, vif.bssid),
        (ADDRESS, VifRole::AccessPoint, None)
    );
    assert_eq!(
        vif.receive,
        ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS)
    );
    // The interface's TSF restarted.
    assert_eq!(model.tsf(AP).unwrap().unwrap().at.as_micros(), 0);

    // The first beacon starts the schedule; three more follow at its TBTTs.
    let start = timer.now.get();
    let deadline = Instant::from_micros(start + 3 * INTERVAL + 1);
    drive(
        &model,
        &router,
        &timer,
        access_point.run_until(deadline),
        &[],
        |_| {},
    )
    .unwrap();
    let beacons: Vec<_> = model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x80)
        .collect();
    assert_eq!(beacons.len(), 4);
    assert_eq!(access_point.counters().beacons, 4);
    let mut counts = Vec::new();
    for (index, beacon) in beacons.iter().enumerate() {
        assert_eq!(beacon.access_category, WmmAccessCategory::Voice);
        assert_eq!(beacon.rate, RATE);
        let frame = &beacon.frames[0];
        assert_eq!(&frame[4..10], &[0xff; 6]);
        // One management sequence; the DTIM count runs down the period.
        assert_eq!(
            u16::from_le_bytes([frame[22], frame[23]]) >> 4,
            index as u16
        );
        let (_, count, period) = dtim(frame).unwrap();
        assert_eq!(period, 2);
        counts.push(count);
    }
    // The DTIM count runs down the period at every TBTT.
    assert!(
        counts
            .windows(2)
            .all(|pair| pair[0] != pair[1] && pair[0] < 2)
    );
}

#[test]
fn a_probe_request_for_the_bss_or_any_ssid_is_answered_once_per_interval() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        client(&router),
        &timer,
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();

    // A wildcard request, a directed one inside the response interval, then
    // one for another SSID, and a directed one after the interval.
    let start = timer.now.get();
    let requests = [
        (start + 1_000, probe_request([0xff; 6], [0xff; 6], b"")),
        (start + 2_000, probe_request(ADDRESS, ADDRESS, SSID)),
        (
            start + 20_000,
            probe_request([0xff; 6], [0xff; 6], b"other"),
        ),
        (start + 30_000, probe_request(ADDRESS, ADDRESS, SSID)),
    ];
    let stops: Vec<u64> = requests.iter().map(|(at, _)| *at).collect();
    let mut sent = 0;
    let deadline = Instant::from_micros(start + 50_000);
    drive(
        &model,
        &router,
        &timer,
        access_point.run_until(deadline),
        &stops,
        |now| {
            while sent < requests.len() && requests[sent].0 <= now {
                model.receive(&requests[sent].1, meta());
                sent += 1;
            }
        },
    )
    .unwrap();
    let responses: Vec<_> = model
        .submitted()
        .into_iter()
        .filter(|attempt| attempt.frames[0][0] == 0x50)
        .collect();
    assert_eq!(responses.len(), 2);
    for response in &responses {
        assert_eq!(&response.frames[0][4..10], &STATION);
        assert_eq!(response.rate, RATE);
    }
    assert_eq!(access_point.counters().probe_responses, 2);
    assert_eq!(access_point.counters().probes_ignored, 2);
}

/// A management frame of the station to the access point.
fn management(subtype: u8, retry: bool, body: &[u8]) -> Vec<u8> {
    let mut frame = vec![subtype << 4, if retry { 0x08 } else { 0 }, 0, 0];
    frame.extend_from_slice(&ADDRESS);
    frame.extend_from_slice(&STATION);
    frame.extend_from_slice(&ADDRESS);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(body);
    frame
}

/// An Open System Authentication Request.
fn authentication(retry: bool) -> Vec<u8> {
    management(11, retry, &[0, 0, 1, 0, 0, 0])
}

/// An Association Request of an Open BSS station with the 802.11b/g rates.
fn association() -> Vec<u8> {
    let mut body = vec![0x21, 0x04, 10, 0];
    body.extend_from_slice(&[0, SSID.len() as u8]);
    body.extend_from_slice(SSID);
    body.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    management(0, false, &body)
}

/// The subtypes and first body word of what the access point sent to the
/// station, in order.
fn sent_to_station(model: &LowerMacModel) -> Vec<(u8, u16)> {
    model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .filter(|frame| frame[4..10] == STATION)
        .map(|frame| (frame[0] >> 4, u16::from_le_bytes([frame[24], frame[25]])))
        .collect()
}

/// Run the access point until `until`, delivering each `(time, frame)` as
/// the port receives it.
fn serve(
    model: &LowerMacModel,
    router: &PortApRouter<'_, Env<'_>>,
    timer: &VirtualTimer,
    access_point: &mut PortAccessPoint<'_, Env<'_>>,
    frames: &[(u64, Vec<u8>)],
    until: u64,
) {
    let stops: Vec<u64> = frames.iter().map(|(at, _)| *at).collect();
    let mut sent = 0;
    drive(
        model,
        router,
        timer,
        access_point.run_until(Instant::from_micros(until)),
        &stops,
        |now| {
            while sent < frames.len() && frames[sent].0 <= now {
                model.receive(&frames[sent].1, meta());
                sent += 1;
            }
        },
    )
    .unwrap();
}

#[test]
fn an_open_station_authenticates_associates_and_leaves() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        client(&router),
        &timer,
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();

    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, association()),
            // The station lost the acknowledgement of both responses and
            // repeats them: each is answered again, the peer kept.
            (start + 3_000, authentication(true)),
            (start + 4_000, association()),
        ],
        start + 5_000,
    );
    let peer = access_point.service().peer_status(STATION).unwrap();
    assert_eq!(peer.phase, ApPeerPhase::Authorized);
    assert_ne!(peer.association_id, 0);
    // Authentication (11) and Association Response (1) twice, both
    // successful: status 0 after the fixed fields.
    let sent = sent_to_station(&model);
    assert_eq!(
        sent.iter().map(|(subtype, _)| *subtype).collect::<Vec<_>>(),
        [11, 1, 11, 1]
    );
    let responses: Vec<_> = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .filter(|frame| frame[4..10] == STATION)
        .collect();
    // The authentication's status, the association's status and AID.
    assert_eq!(u16::from_le_bytes([responses[0][28], responses[0][29]]), 0);
    assert_eq!(u16::from_le_bytes([responses[1][26], responses[1][27]]), 0);
    assert_eq!(
        u16::from_le_bytes([responses[1][28], responses[1][29]]) & 0x3fff,
        peer.association_id
    );
    assert_eq!(responses[1][28..30], responses[3][28..30]);

    // The station deauthenticates: the access point forgets it.
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 6_000, management(12, false, &[3, 0]))],
        start + 7_000,
    );
    assert!(access_point.service().peer_status(STATION).is_none());
    assert_eq!(access_point.counters().peers_left, 1);
}

#[test]
fn an_inactive_peer_is_disassociated_and_deauthenticated() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        client(&router),
        &timer,
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, association()),
        ],
        start + 12_000_000,
    );
    // Silent for the 10 s timeout: a Disassociation for inactivity (10,
    // reason 4), then a Deauthentication (12, reason 2).
    let sent = sent_to_station(&model);
    assert_eq!(&sent[2..], &[(10, 4), (12, 2)]);
    assert!(access_point.service().peer_status(STATION).is_none());
    assert_eq!(access_point.counters().peers_closed, 1);
}

#[test]
fn a_protected_bss_is_refused_until_its_handshake_is_served() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = [0; AP_BEACON_CAPACITY];
    let pmk = oer_ieee80211_rsn::Pmk::from_bytes([7; 32]);
    let gtk = oer_ieee80211_rsn::frames::RsnGtk::new(1, true, [9; 16]).unwrap();
    let wpa2 = AccessPointService::new(
        ADDRESS,
        pmk,
        gtk,
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    assert!(matches!(
        PortAccessPoint::<Env<'_>>::new(
            client(&router),
            &timer,
            profile(&ssid),
            wpa2,
            &mut storage
        ),
        Err(PortApBuildError::SecurityUnsupported)
    ));
}
