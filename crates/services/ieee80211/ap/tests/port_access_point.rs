//! The access point over the lower-MAC host model: virtual time drives its
//! beacon schedule, and the test plays the stations that probe it.

use core::cell::{Cell, RefCell};

use oer_ieee80211_datapath::{SoftwareTxFrame, memory::MemoryTxQueues};
use oer_network_interface::NetworkInterfaceId;

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee80211_mac::station::association::PhyMode;
use oer_ieee80211_upper_mac::rate_control::{FixedRateControl, RateControl, RatePeer};

use oer_ieee80211_ap::sae::{ApSaeCredential, ApSaeRandom, ApSaeResponder};
use oer_ieee80211_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    ApPeerPhase,
};
use oer_ieee80211_ap_service::port::{
    InlineSae, NoSae, PortAccessPoint, PortApAuthenticator, PortApClient, PortApEnv, PortApParts,
    PortApProfile, PortApRouter, PortApStorage,
};
use oer_ieee80211_lower_mac::{
    CoexPriority, Ieee80211LowerMacPort, KeyHandle, KeyScope, KeySelector, LifecycleCommand,
    LowerMacBeaconTiming, MacAddress, PhyRate, ReceiveFilter, RxCryptoStatus, RxEvidence, RxMeta,
    TxPower, VifId, VifRole,
    model::{LowerMacModel, ModelOutcome},
};
use oer_ieee80211_mac::{
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    beacon::dtim,
    block_ack::{ADDBA_ACTION_BODY_LEN, TxBlockAckRetry, write_successful_addba_response},
    channel::{Channel, WifiChannel},
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth},
    qos::WmmAccessCategory,
    ssid::WifiSsid,
};
use oer_ieee80211_mac::{ccmp::CcmpPacketNumberStep, security::rsn::Akm};
use oer_ieee80211_rsn::sae::{SaeCommit, SaeCommitValues, SaePassword, SaePasswordElement};
use oer_ieee80211_rsn::{
    Pmk, Ptk, PtkContext,
    frames::{OwnedAssociationSecurityIes, OwnedRsnIe, RsnGtk, RsnIgtk, RsnTxFrame},
};
use oer_ieee80211_softmac::{BackoffEntropy, EdcaContention};
use oer_ieee80211_upper_mac::{
    AmpduRetryPolicy, ProtectEveryHeTxop, ProtectionPolicy, RateLadder, RetryLimits, TxPlanner,
};
use oer_ieee80211_upper_mac_service::{
    aggregate::PortAmpduAggregation,
    client::{PortClient, PortClientConfig, PortClientEnv},
};
use oer_time::{Clock, Instant, Timer};

const AP: VifId = VifId(1);
const ADDRESS: MacAddress = [0x02, 0, 0, 0, 0, 0x0a];
const STATION: MacAddress = [0x02, 0, 0, 0, 0, 0x5a];
const SSID: &[u8] = b"port-ap";
const RATE: PhyRate = PhyRate::Legacy(LegacyRate::Dsss1M);
const DATA_RATE: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);
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

/// One Ethernet-II frame the network queued: an owner, whose drop counts
/// as its return to the network.
pub struct TestFrame(Vec<u8>);

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

type TestFrames = MemoryTxQueues<TestFrame, 32>;

thread_local! {
    /// The source of this test's access point.
    static FRAMES: Cell<Option<&'static TestFrames>> = const { Cell::new(None) };
    /// Frames returned to the network: sent, dropped or released.
    static RETURNED: Cell<usize> = const { Cell::new(0) };
}

/// A fresh source of the network's frames for this test's access point.
fn frames() -> &'static TestFrames {
    let frames = Box::leak(Box::new(TestFrames::new()));
    FRAMES.with(|current| current.set(Some(frames)));
    frames
}

/// Queue one Ethernet-II frame in the network's source the access point
/// sends from.
fn send(ethernet: &[u8]) {
    FRAMES.with(|current| {
        assert!(
            current
                .get()
                .expect("an access point's source")
                .push(TestFrame(ethernet.to_vec()))
                .is_ok()
        );
    });
}

/// Frames returned to the network so far in this test.
fn returned() -> usize {
    RETURNED.with(Cell::get)
}

struct Env<'a>(core::marker::PhantomData<&'a ()>);

impl PortClientEnv for Env<'_> {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = PortAmpduAggregation;
}

impl<'a> PortApEnv for Env<'a> {
    type Timer = &'a VirtualTimer;
    type Authenticator = FixedMaterial;
    type Sae = NoSae;
    type RateControl = FixedRateControl;
    type Frames = TestFrames;
}

/// The environment of a WPA3 BSS, its SAE responder run inline.
struct Wpa3Env<'a>(core::marker::PhantomData<&'a ()>);

impl PortClientEnv for Wpa3Env<'_> {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = PortAmpduAggregation;
}

impl<'a> PortApEnv for Wpa3Env<'a> {
    type Timer = &'a VirtualTimer;
    type Authenticator = FixedMaterial;
    type Sae = InlineSae<Counter>;
    type RateControl = FixedRateControl;
    type Frames = TestFrames;
}

/// Deterministic SAE scalars.
struct Counter(u8);

impl ApSaeRandom for Counter {
    fn fill(&mut self, bytes: &mut [u8]) {
        for byte in bytes.iter_mut() {
            self.0 = self.0.wrapping_add(0x3b);
            *byte = self.0 | 0x10;
        }
        bytes[0] = 0x11;
    }
}

/// The nonce and replay counter every handshake starts from.
const ANONCE: [u8; 32] = [0x33; 32];
const REPLAY_COUNTER: u64 = 9;

struct FixedMaterial;

impl PortApAuthenticator for FixedMaterial {
    fn handshake_material(&mut self) -> ([u8; 32], u64) {
        (ANONCE, REPLAY_COUNTER)
    }
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

fn client<'a, X>(router: &'a PortApRouter<'a, X>) -> PortApClient<'a, X>
where
    X: PortClientEnv<
            Port = LowerMacModel,
            Budget = ProtectEveryHeTxop,
            Ladder = FixedRate,
            Entropy = Seeded,
        >,
{
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

/// Three TX Block Ack offers, a failed one followed by the next a second
/// later.
const RETRY: TxBlockAckRetry = TxBlockAckRetry {
    attempts: 3,
    interval: oer_time::Duration::from_millis(1_000),
};

fn profile(ssid: &WifiSsid) -> PortApProfile<'_> {
    PortApProfile {
        ssid,
        channel: channel(),
        beacon_interval_tu: 100,
        dtim_period: 2,
        advertisement: &ADVERTISEMENT,
        management_rate: RATE,
        ccmp_step: CcmpPacketNumberStep::new(1).unwrap(),
        rx_reorder_gap: oer_time::Duration::from_millis(300),
        tx_block_ack_retry: RETRY,
        coex: CoexPriority::Normal,
    }
}

/// Poll `future` to its end beside the router; when neither moves, virtual
/// time advances to the earliest of the deadlines a wait asked for and the
/// `stops` still ahead, and `at` runs at each new time.
fn drive<T>(
    model: &LowerMacModel,
    router: &oer_ieee80211_upper_mac_service::EventRouter<'_, LowerMacModel, 2, 4>,
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
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
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
        access_point.run_until(deadline, &mut |_| {}),
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
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
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
        access_point.run_until(deadline, &mut |_| {}),
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
fn serve<X: PortApEnv<Port = LowerMacModel>>(
    model: &LowerMacModel,
    router: &oer_ieee80211_upper_mac_service::EventRouter<'_, LowerMacModel, 2, 4>,
    timer: &VirtualTimer,
    access_point: &mut PortAccessPoint<'_, X>,
    frames: &[(u64, Vec<u8>)],
    until: u64,
) -> Vec<Vec<u8>> {
    let stops: Vec<u64> = frames.iter().map(|(at, _)| *at).collect();
    let mut sent = 0;
    let mut delivered = Vec::new();
    drive(
        model,
        router,
        timer,
        access_point.run_until(Instant::from_micros(until), &mut |msdu| {
            let parts = msdu.parts();
            let mut ethernet = vec![0; parts.length()];
            parts.copy_to(&mut ethernet).unwrap();
            delivered.push(ethernet);
        }),
        &stops,
        |now| {
            while sent < frames.len() && frames[sent].0 <= now {
                // A protected data MPDU arrives decrypted by the backend.
                let frame = &frames[sent].1;
                let meta = if frame[0] & 0x0c == 0x08 && frame[1] & 0x40 != 0 {
                    RxMeta {
                        crypto: RxEvidence::HardwareObserved(
                            RxCryptoStatus::DecryptedAndIntegrityVerified,
                        ),
                        ..meta()
                    }
                } else {
                    meta()
                };
                model.receive(frame, meta);
                sent += 1;
            }
        },
    )
    .unwrap();
    delivered
}

#[test]
fn an_open_station_authenticates_associates_and_leaves() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
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
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
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
fn a_wpa3_station_authenticates_by_sae_while_the_bss_goes_on() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Wpa3Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let wpa3 = AccessPointService::new_wpa3(
        ADDRESS,
        RsnGtk::new(1, true, [9; 16]).unwrap(),
        RsnIgtk::new(4, [0; 6], [0x66; 16]).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    let sae = InlineSae::new(
        ApSaeResponder::new(
            ADDRESS,
            ApSaeCredential::derive(SSID, SaePassword::new(PASSPHRASE).unwrap()),
        ),
        Counter(0),
    );
    let mut access_point = PortAccessPoint::<Wpa3Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        wpa3,
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();

    // The station's Commit, built from the primitives the station role
    // uses: the access point answers with its own Commit.
    let pwe = SaePasswordElement::hunting_and_pecking(PASSPHRASE, STATION, ADDRESS).unwrap();
    let commit = SaeCommit::new(pwe, [0x21; 32], [0x43; 32]).unwrap();
    let mut body = [0; 160];
    let length = commit.values().encode(None, false, &mut body).unwrap();
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 1_000, sae_authentication(1, &body[..length]))],
        start + 2_000,
    );
    let (transaction, status, reply) = last_sae_reply(&model);
    assert_eq!((transaction, status), (1, 0));
    let keys = commit
        .process(SaeCommitValues::parse(&reply, false).unwrap())
        .unwrap();

    // Its Confirm: the access point accepts the station and confirms.
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 3_000, sae_authentication(2, &keys.own_confirm(1)))],
        start + 4_000,
    );
    let (transaction, status, reply) = last_sae_reply(&model);
    assert_eq!((transaction, status), (2, 0));
    assert_eq!(keys.verify_peer_confirm(&reply), Ok(1));
    assert_eq!(access_point.counters().sae_accepted, 1);
    assert_eq!(
        access_point.service().peer_status(STATION).unwrap().phase,
        ApPeerPhase::Authenticated
    );
    // The BSS went on beaconing throughout.
    assert!(access_point.counters().beacons >= 1);
}

/// The RSN element of a WPA2-Personal station: CCMP, PSK.
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const PASSPHRASE: &[u8] = b"port-ap-password";
const SNONCE: [u8; 32] = [0x44; 32];

/// An Association Request of a WPA2-Personal station.
fn rsn_association() -> Vec<u8> {
    let mut body = vec![0x31, 0x04, 10, 0];
    body.extend_from_slice(&[0, SSID.len() as u8]);
    body.extend_from_slice(SSID);
    body.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    body.extend_from_slice(&RSN);
    management(0, false, &body)
}

/// An EAPOL frame of the station to the access point, in a data MPDU.
fn eapol(frame: &[u8]) -> Vec<u8> {
    let mut mpdu = vec![0x08, 0x01, 0, 0];
    mpdu.extend_from_slice(&ADDRESS);
    mpdu.extend_from_slice(&STATION);
    mpdu.extend_from_slice(&ADDRESS);
    mpdu.extend_from_slice(&[0x10, 0]);
    mpdu.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    mpdu.extend_from_slice(frame);
    mpdu
}

fn ptk() -> Ptk {
    Pmk::derive(PASSPHRASE, SSID).unwrap().derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: ADDRESS,
            supplicant_address: STATION,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    )
}

#[test]
fn a_wpa2_station_completes_the_four_way_handshake_and_gets_its_key() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let wpa2 = AccessPointService::new(
        ADDRESS,
        Pmk::derive(PASSPHRASE, SSID).unwrap(),
        RsnGtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        wpa2,
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    // The group key is the port's from the start.
    assert_eq!(model.installed_keys(), [KeyScope::Group { key_id: 1 }]);

    let ptk = ptk();
    let rsn_ie = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let security_ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, &[]).unwrap();
    let message2 = RsnTxFrame::<512>::message2_with_security_ies(
        Akm::Psk,
        ADDRESS,
        REPLAY_COUNTER,
        SNONCE,
        &security_ies,
    )
    .unwrap()
    .authenticate(&ptk);
    let message4 = RsnTxFrame::<512>::message4(Akm::Psk, ADDRESS, REPLAY_COUNTER + 1)
        .unwrap()
        .authenticate(&ptk);
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, rsn_association()),
            (start + 3_000, eapol(message2.as_bytes())),
            (start + 4_000, eapol(message4.as_bytes())),
        ],
        start + 5_000,
    );
    // Message 1 followed the association, Message 3 Message 2: EAPOL data
    // MPDUs from the DS to the station.
    let eapol_sent: Vec<_> = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .filter(|frame| frame[0] == 0x08 && frame[4..10] == STATION)
        .collect();
    assert_eq!(eapol_sent.len(), 2);
    assert!(
        eapol_sent
            .iter()
            .all(|frame| frame[1] & 0x02 != 0 && frame[30..32] == [0x88, 0x8e])
    );
    assert_eq!(access_point.counters().handshakes, 1);
    assert_eq!(
        access_point.service().peer_status(STATION).unwrap().phase,
        ApPeerPhase::Authorized
    );
    // The pairwise key was installed before the peer was authorized.
    assert_eq!(
        model.installed_keys(),
        [
            KeyScope::Group { key_id: 1 },
            KeyScope::Pairwise { peer: STATION }
        ]
    );

    // The station leaves: its pairwise key goes with it.
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 6_000, management(12, false, &[3, 0]))],
        start + 7_000,
    );
    assert_eq!(model.installed_keys(), [KeyScope::Group { key_id: 1 }]);
}

#[test]
fn a_silent_station_gets_message_1_again_and_is_closed_when_its_retries_run_out() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let wpa2 = AccessPointService::new(
        ADDRESS,
        Pmk::derive(PASSPHRASE, SSID).unwrap(),
        RsnGtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(60).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        wpa2,
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
            (start + 2_000, rsn_association()),
        ],
        start + 6_000_000,
    );
    // Message 1 and its three retransmissions, then the close: a
    // Disassociation and a Deauthentication, both for an authentication no
    // longer valid.
    assert_eq!(access_point.counters().eapol_sent, 4);
    let teardown: Vec<_> = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .filter(|frame| frame[4..10] == STATION && matches!(frame[0], 0xa0 | 0xc0))
        .map(|frame| (frame[0], u16::from_le_bytes([frame[24], frame[25]])))
        .collect();
    assert_eq!(teardown, [(0xa0, 2), (0xc0, 2)]);
    assert!(access_point.service().peer_status(STATION).is_none());
}

/// An SAE Authentication frame of the station.
fn sae_authentication(transaction: u16, body: &[u8]) -> Vec<u8> {
    let mut fixed = vec![3, 0];
    fixed.extend_from_slice(&transaction.to_le_bytes());
    fixed.extend_from_slice(&[0, 0]);
    fixed.extend_from_slice(body);
    management(11, false, &fixed)
}

/// The transaction, status and body of the last SAE Authentication frame
/// the access point sent the station.
fn last_sae_reply(model: &LowerMacModel) -> (u16, u16, Vec<u8>) {
    let frame = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .rfind(|frame| frame[0] == 0xb0 && frame[4..10] == STATION)
        .expect("an SAE reply");
    assert_eq!(u16::from_le_bytes([frame[24], frame[25]]), 3);
    (
        u16::from_le_bytes([frame[26], frame[27]]),
        u16::from_le_bytes([frame[28], frame[29]]),
        frame[30..].to_vec(),
    )
}

/// An Ethernet-II frame between `destination` and `source`.
fn ethernet(destination: [u8; 6], source: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = destination.to_vec();
    frame.extend_from_slice(&source);
    frame.extend_from_slice(&[0x08, 0x00]);
    frame.extend_from_slice(payload);
    frame
}

/// A data MPDU of the station to the distribution system, carrying `body`
/// after its header (an LLC/SNAP IPv4 payload, behind a CCMP header when
/// `ccmp` names its packet number).
fn uplink(sequence: u16, ccmp: Option<u64>, payload: &[u8]) -> Vec<u8> {
    let mut mpdu = vec![0x08, if ccmp.is_some() { 0x41 } else { 0x01 }, 0, 0];
    mpdu.extend_from_slice(&ADDRESS);
    mpdu.extend_from_slice(&STATION);
    mpdu.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x99]);
    mpdu.extend_from_slice(&(sequence << 4).to_le_bytes());
    if let Some(packet_number) = ccmp {
        let pn = packet_number.to_le_bytes();
        mpdu.extend_from_slice(&[pn[0], pn[1], 0, 0x20, pn[2], pn[3], pn[4], pn[5]]);
    }
    mpdu.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    mpdu.extend_from_slice(payload);
    mpdu
}

/// The data MPDUs the access point sent `destination`.
fn downlink(model: &LowerMacModel, destination: [u8; 6]) -> Vec<(Vec<u8>, KeySelector, PhyRate)> {
    model
        .submitted()
        .into_iter()
        .filter(|attempt| {
            attempt.frames[0][0] & 0x0c == 0x08 && attempt.frames[0][4..10] == destination
        })
        .filter(|attempt| {
            // Not the handshake's EAPOL.
            !attempt.frames[0]
                .windows(2)
                .any(|pair| pair == [0x88, 0x8e])
        })
        .map(|attempt| (attempt.frames[0].clone(), attempt.key, attempt.rate))
        .collect()
}

#[test]
fn an_open_bss_carries_data_both_ways_for_its_associated_peers() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    let start = timer.now.get();

    // Before the station associates, its downlink has nowhere to go.
    send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], b"early"));
    serve(&model, &router, &timer, &mut access_point, &[], start + 500);
    assert_eq!(access_point.counters().data_dropped, 1);

    let delivered = serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, association()),
            (start + 3_000, uplink(1, None, b"up")),
            // The station's retransmission of it is a duplicate.
            (start + 3_500, {
                let mut retry = uplink(1, None, b"up");
                retry[1] |= 0x08;
                retry
            }),
        ],
        start + 4_000,
    );
    assert_eq!(
        delivered,
        [ethernet([0x02, 0, 0, 0, 0, 0x99], STATION, b"up")]
    );
    assert_eq!(access_point.counters().duplicates, 1);

    send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], b"down"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 5_000,
    );
    let sent = downlink(&model, STATION);
    assert_eq!(sent.len(), 1);
    let (frame, key, rate) = &sent[0];
    // From the DS, plaintext, at the data rate.
    assert_eq!(
        (frame[1] & 0x43, *key, *rate),
        (0x02, KeySelector::Plaintext, DATA_RATE)
    );
    assert!(frame.ends_with(b"down"));
}

#[test]
fn a_wpa2_bss_carries_data_under_each_key_and_drops_replays() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let wpa2 = AccessPointService::new(
        ADDRESS,
        Pmk::derive(PASSPHRASE, SSID).unwrap(),
        RsnGtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        wpa2,
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    let ptk = ptk();
    let rsn_ie = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let security_ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, &[]).unwrap();
    let message2 = RsnTxFrame::<512>::message2_with_security_ies(
        Akm::Psk,
        ADDRESS,
        REPLAY_COUNTER,
        SNONCE,
        &security_ies,
    )
    .unwrap()
    .authenticate(&ptk);
    let message4 = RsnTxFrame::<512>::message4(Akm::Psk, ADDRESS, REPLAY_COUNTER + 1)
        .unwrap()
        .authenticate(&ptk);
    let start = timer.now.get();
    let delivered = serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, rsn_association()),
            (start + 3_000, eapol(message2.as_bytes())),
            (start + 4_000, eapol(message4.as_bytes())),
            (start + 5_000, uplink(1, Some(1), b"one")),
            // A new sequence whose packet number does not advance.
            (start + 6_000, uplink(2, Some(1), b"replay")),
            // Plaintext data after the keys are in.
            (start + 7_000, uplink(3, None, b"plain")),
        ],
        start + 8_000,
    );
    assert_eq!(
        delivered,
        [ethernet([0x02, 0, 0, 0, 0, 0x99], STATION, b"one")]
    );
    assert_eq!(access_point.counters().replayed, 1);
    assert_eq!(access_point.counters().rx_rejected, 1);
    send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], b"down"));
    send(&ethernet([0xff; 6], [0x02, 0, 0, 0, 0, 0x99], b"all"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 9_000,
    );
    // To the peer under its pairwise key with its first packet number; to
    // the group under the group key at the management rate.
    let unicast = downlink(&model, STATION);
    let (frame, key, rate) = &unicast[0];
    assert_eq!((frame[1] & 0x40, *rate), (0x40, DATA_RATE));
    assert_eq!(
        *key,
        KeySelector::Key(oer_ieee80211_lower_mac::KeyHandle(1))
    );
    assert_eq!((frame[24], frame[27] & 0x20, frame[27] >> 6), (1, 0x20, 0));
    let group = downlink(&model, [0xff; 6]);
    let (frame, key, rate) = &group[0];
    assert_eq!(
        *key,
        KeySelector::Key(oer_ieee80211_lower_mac::KeyHandle(0))
    );
    assert_eq!((frame[27] >> 6, *rate), (1, RATE));
}

/// A Null Data frame of the station, its power-management bit `dozing`.
fn null_data(dozing: bool) -> Vec<u8> {
    let mut frame = vec![0x48, if dozing { 0x11 } else { 0x01 }, 0, 0];
    frame.extend_from_slice(&ADDRESS);
    frame.extend_from_slice(&STATION);
    frame.extend_from_slice(&ADDRESS);
    frame.extend_from_slice(&[0x20, 0]);
    frame
}

/// A PS-Poll of the station for `association_id`.
fn ps_poll(association_id: u16) -> Vec<u8> {
    let mut frame = vec![0xa4, 0x00];
    frame.extend_from_slice(&(association_id | 0xc000).to_le_bytes());
    frame.extend_from_slice(&ADDRESS);
    frame.extend_from_slice(&STATION);
    frame
}

/// The TIM's bitmap control and first partial-bitmap octet of the last
/// beacon sent.
fn last_tim(model: &LowerMacModel) -> (u8, u8) {
    let beacon = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .rfind(|frame| frame[0] == 0x80)
        .unwrap();
    let (offset, _, _) = dtim(&beacon).unwrap();
    (beacon[offset + 4], beacon[offset + 5])
}

/// An Open access point with the station associated.
fn associated<'a, const HELD: usize>(
    model: &'a LowerMacModel,
    router: &'a PortApRouter<'a, Env<'a>>,
    timer: &'a VirtualTimer,
    ssid: &'a WifiSsid,
    storage: &'a mut PortApStorage<HELD, TestFrame>,
) -> PortAccessPoint<'a, Env<'a>> {
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(router),
            timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: DATA_RATE,
            frames: frames(),
        },
        profile(ssid),
        service(),
        storage,
    )
    .unwrap();
    drive(model, router, timer, access_point.start(), &[], |_| {}).unwrap();
    let start = timer.now.get();
    serve(
        model,
        router,
        timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, association()),
        ],
        start + 3_000,
    );
    access_point
}

#[test]
fn a_dozing_peer_s_frames_wait_for_its_ps_poll_or_its_wake_up() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = associated(&model, &router, &timer, &ssid, &mut storage);
    let aid = access_point
        .service()
        .peer_status(STATION)
        .unwrap()
        .association_id;
    let from = [0x02, 0, 0, 0, 0, 0x99];

    // The station dozes: its frame is held, and the next beacon's TIM says
    // so.
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 1_000, null_data(true))],
        start + 2_000,
    );
    send(&ethernet(STATION, from, b"held"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 120_000,
    );
    assert!(downlink(&model, STATION).is_empty());
    assert_eq!(access_point.counters().held, 1);
    let (_, bitmap) = last_tim(&model);
    assert_ne!(bitmap & (1 << aid), 0);

    // Its PS-Poll gets the one frame, More Data clear.
    let now = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(now + 1_000, ps_poll(aid))],
        now + 2_000,
    );
    let sent = downlink(&model, STATION);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].0.ends_with(b"held"));
    assert_eq!(sent[0].0[1] & 0x20, 0);

    // Two more wait; when it wakes both go out, More Data set on the first.
    send(&ethernet(STATION, from, b"first"));
    send(&ethernet(STATION, from, b"second"));
    let now = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(now + 5_000, null_data(false))],
        now + 6_000,
    );
    let sent = downlink(&model, STATION);
    assert_eq!(sent.len(), 3);
    assert!(sent[1].0.ends_with(b"first") && sent[2].0.ends_with(b"second"));
    assert_eq!((sent[1].0[1] & 0x20, sent[2].0[1] & 0x20), (0x20, 0));
    assert_eq!(access_point.counters().released, 3);
}

#[test]
fn group_frames_wait_for_the_dtim_while_a_peer_dozes() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = associated(&model, &router, &timer, &ssid, &mut storage);
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 1_000, null_data(true))],
        start + 2_000,
    );

    send(&ethernet([0xff; 6], [0x02, 0, 0, 0, 0, 0x99], b"group"));
    // Run past two TBTTs: one of them is the DTIM (period 2).
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 250_000,
    );
    let frames: Vec<_> = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .collect();
    let group = frames
        .iter()
        .position(|frame| frame[0] == 0x08 && frame[4..10] == [0xff; 6])
        .expect("the group frame went out");
    // It followed a DTIM beacon that announced it.
    let beacon = &frames[group - 1];
    let (offset, count, _) = dtim(beacon).unwrap();
    assert_eq!((beacon[0], count, beacon[offset + 4] & 1), (0x80, 0, 1));
    assert_eq!(access_point.counters().released, 1);
}

/// An ADDBA Request of the station for `tid`.
fn addba_request(tid: u8, window: u16, start: u16) -> Vec<u8> {
    let parameters: u16 = 0x0002 | (u16::from(tid) << 2) | (window << 6);
    let mut body = vec![3, 0, 7];
    body.extend_from_slice(&parameters.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body.extend_from_slice(&(start << 4).to_le_bytes());
    management(13, false, &body)
}

/// A QoS data MPDU of the station for `tid`, `sequence`.
fn qos_uplink(tid: u8, sequence: u16, payload: &[u8]) -> Vec<u8> {
    let mut mpdu = vec![0x88, 0x01, 0, 0];
    mpdu.extend_from_slice(&ADDRESS);
    mpdu.extend_from_slice(&STATION);
    mpdu.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x99]);
    mpdu.extend_from_slice(&(sequence << 4).to_le_bytes());
    mpdu.extend_from_slice(&[tid, 0]);
    mpdu.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    mpdu.extend_from_slice(payload);
    mpdu
}

#[test]
fn a_peer_s_block_ack_agreement_reorders_its_data_until_it_ends() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = associated(&model, &router, &timer, &ssid, &mut storage);
    let now = timer.now.get();
    let delivered = serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (now + 1_000, addba_request(0, 16, 100)),
            // 101 and 102 wait for 100.
            (now + 2_000, qos_uplink(0, 101, b"b")),
            (now + 3_000, qos_uplink(0, 102, b"c")),
            (now + 4_000, qos_uplink(0, 100, b"a")),
        ],
        now + 5_000,
    );
    // The ADDBA Response succeeded and the port holds the agreement.
    let response = model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .rfind(|frame| frame[0] == 0xd0 && frame[4..10] == STATION)
        .unwrap();
    assert_eq!(
        (
            &response[24..27],
            u16::from_le_bytes([response[27], response[28]])
        ),
        (&[3_u8, 1, 7][..], 0)
    );
    assert_eq!(model.rx_block_acks().len(), 1);
    let payloads: Vec<&[u8]> = delivered.iter().map(|frame| &frame[14..]).collect();
    assert_eq!(payloads, [b"a", b"b", b"c"]);

    // The station ends it: the port drops the agreement.
    let now = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(
            now + 1_000,
            management(13, false, &[3, 2, 0x00, 0x08, 1, 0]),
        )],
        now + 2_000,
    );
    assert!(model.rx_block_acks().is_empty());
    assert_eq!(access_point.counters().rx_agreements, 1);
}

#[test]
fn the_composition_sizes_the_frames_held_for_dozing_peers() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    // Room for one held frame only.
    let mut storage = PortApStorage::<1, TestFrame>::new();
    let mut access_point = associated(&model, &router, &timer, &ssid, &mut storage);
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 1_000, null_data(true))],
        start + 2_000,
    );
    let from = [0x02, 0, 0, 0, 0, 0x99];
    send(&ethernet(STATION, from, b"kept"));
    send(&ethernet(STATION, from, b"dropped"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 3_000,
    );
    assert_eq!(
        (
            access_point.counters().held,
            access_point.counters().held_dropped
        ),
        (1, 1)
    );
}

/// HT MCS 7 at 20 MHz: a rate the port aggregates at.
const HT_DATA_RATE: PhyRate = PhyRate::Ht(
    match HtRate::new(
        match HtMcs::new(7) {
            Some(mcs) => mcs,
            None => panic!("MCS 7"),
        },
        PpduBandwidth::Mhz20,
        false,
    ) {
        Some(rate) => rate,
        None => panic!("an HT rate"),
    },
);

/// An Association Request of a WPA2-Personal HT station: a Maximum A-MPDU
/// Length of 65 535 octets and a Minimum MPDU Start Spacing of 4 µs (5).
fn ht_rsn_association() -> Vec<u8> {
    let mut frame = rsn_association();
    let mut ht = vec![45, 26, 0x0c, 0x00, 0x03 | (5 << 2), 0xff];
    ht.resize(28, 0);
    frame.extend_from_slice(&ht);
    frame
}

/// A Block Ack action of the station.
fn block_ack_action(body: &[u8]) -> Vec<u8> {
    management(13, false, body)
}

/// Run a WPA2 BSS whose HT station authenticates, associates and completes
/// its handshake, then `test` it from the time after the handshake.
fn with_ht_peer(
    test: impl FnOnce(
        &LowerMacModel,
        &oer_ieee80211_upper_mac_service::EventRouter<'_, LowerMacModel, 2, 4>,
        &VirtualTimer,
        &mut PortAccessPoint<'_, Env<'_>>,
        u64,
    ),
) {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let wpa2 = AccessPointService::new(
        ADDRESS,
        Pmk::derive(PASSPHRASE, SSID).unwrap(),
        RsnGtk::new(1, true, [0x55; 16]).unwrap(),
        AccessPointClientLimit::new(4).unwrap(),
        AccessPointInactiveTimeout::new(10).unwrap(),
        Box::leak(Box::new(AccessPointPeerStorage::new())),
    );
    let mut access_point = PortAccessPoint::<Env<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: HT_DATA_RATE,
            frames: frames(),
        },
        profile(&ssid),
        wpa2,
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    let ptk = ptk();
    let rsn_ie = OwnedRsnIe::<64>::try_copy(&RSN).unwrap();
    let security_ies = OwnedAssociationSecurityIes::<128>::try_copy(&rsn_ie, &[]).unwrap();
    let message2 = RsnTxFrame::<512>::message2_with_security_ies(
        Akm::Psk,
        ADDRESS,
        REPLAY_COUNTER,
        SNONCE,
        &security_ies,
    )
    .unwrap()
    .authenticate(&ptk);
    let message4 = RsnTxFrame::<512>::message4(Akm::Psk, ADDRESS, REPLAY_COUNTER + 1)
        .unwrap()
        .authenticate(&ptk);
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 1_000, authentication(false)),
            (start + 2_000, ht_rsn_association()),
            (start + 3_000, eapol(message2.as_bytes())),
            (start + 4_000, eapol(message4.as_bytes())),
        ],
        start + 5_000,
    );
    assert_eq!(access_point.counters().handshakes, 1);
    test(&model, &router, &timer, &mut access_point, start + 5_000);
}

/// The ADDBA Requests the access point sent the station: each Dialog Token
/// and Starting Sequence Number.
fn addba_requests(model: &LowerMacModel) -> Vec<(u8, u16)> {
    model
        .submitted()
        .into_iter()
        .map(|attempt| attempt.frames[0].clone())
        .filter(|frame| frame[0] == 0xd0 && frame[4..10] == STATION && frame[24..26] == [3, 0])
        .map(|frame| (frame[26], u16::from_le_bytes([frame[31], frame[32]]) >> 4))
        .collect()
}

/// The protected data attempts to the station: each one's subframes, as
/// their sequence numbers, and whether it was an A-MPDU.
fn data_attempts(model: &LowerMacModel) -> Vec<(Vec<u16>, bool)> {
    model
        .submitted()
        .into_iter()
        .filter(|attempt| {
            attempt.frames[0][0] == 0x88
                && attempt.frames[0][4..10] == STATION
                && attempt.frames[0][1] & 0x40 != 0
        })
        .map(|attempt| {
            (
                attempt
                    .frames
                    .iter()
                    .map(|frame| u16::from_le_bytes([frame[22], frame[23]]) >> 4)
                    .collect(),
                attempt.ampdu,
            )
        })
        .collect()
}

#[test]
fn a_peer_that_accepts_the_tx_block_ack_agreement_gets_aggregates_until_it_ends_it() {
    with_ht_peer(|model, router, timer, access_point, at| {
        // The access point offers its agreement once the peer is authorized.
        let requests = addba_requests(model);
        assert_eq!(requests.len(), 1);
        let (token, start) = requests[0];
        assert_eq!(access_point.counters().tx_agreements_offered, 1);
        let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
        write_successful_addba_response(&mut response, token, 0, 16).unwrap();
        serve(
            model,
            router,
            timer,
            access_point,
            &[(at + 1_000, block_ack_action(&response))],
            at + 2_000,
        );
        assert_eq!(access_point.counters().tx_agreements, 1);

        // Three frames for the peer go as one A-MPDU of consecutive
        // sequence numbers from the agreement's start, under its key, at
        // the data rate.
        for payload in [&b"one"[..], b"two", b"three"] {
            send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], payload));
        }
        serve(model, router, timer, access_point, &[], at + 3_000);
        assert_eq!(
            data_attempts(model),
            [(vec![start, start + 1, start + 2], true)]
        );
        let aggregate = model.submitted().into_iter().last().unwrap();
        assert_eq!(
            (aggregate.key, aggregate.rate),
            (KeySelector::Key(KeyHandle(1)), HT_DATA_RATE)
        );
        let counters = access_point.counters();
        assert_eq!(
            (
                counters.aggregates,
                counters.aggregated_acknowledged,
                counters.data_sent
            ),
            (1, 3, 3)
        );

        // The peer, as recipient, ends the agreement: frames go alone again.
        let delba = [3, 2, 0, 0, 37, 0];
        serve(
            model,
            router,
            timer,
            access_point,
            &[(at + 4_000, block_ack_action(&delba))],
            at + 5_000,
        );
        for payload in [&b"four"[..], b"five"] {
            send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], payload));
        }
        serve(model, router, timer, access_point, &[], at + 6_000);
        assert_eq!(
            data_attempts(model)[1..],
            [(vec![start + 3], false), (vec![start + 4], false)]
        );
    });
}

#[test]
fn an_unanswered_offer_is_made_again_with_the_peer_s_data_after_the_interval() {
    with_ht_peer(|model, router, timer, access_point, at| {
        assert_eq!(addba_requests(model).len(), 1);
        // Past the negotiation timeout, the offer has failed.
        serve(model, router, timer, access_point, &[], at + 200_000);
        let counters = access_point.counters();
        assert_eq!(
            (counters.tx_agreements, counters.tx_agreements_failed),
            (0, 1)
        );
        assert!(
            access_point
                .service()
                .peer_status(STATION)
                .unwrap()
                .tx_block_ack
                .is_none()
        );
        // Within the interval the peer's frames go alone, offering nothing.
        let send_all = |payloads: &[&[u8]]| {
            for payload in payloads {
                send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], payload));
            }
        };
        send_all(&[b"one", b"two"]);
        serve(model, router, timer, access_point, &[], at + 201_000);
        let attempts = data_attempts(model);
        assert_eq!(attempts.len(), 2);
        assert!(
            attempts
                .iter()
                .all(|(subframes, ampdu)| subframes.len() == 1 && !ampdu)
        );
        assert_eq!(addba_requests(model).len(), 1);

        // After it, the peer's data brings the second offer first.
        serve(model, router, timer, access_point, &[], at + 1_150_000);
        assert_eq!(addba_requests(model).len(), 1);
        send_all(&[b"three"]);
        serve(model, router, timer, access_point, &[], at + 1_151_000);
        let requests = addba_requests(model);
        assert_eq!(requests.len(), 2);
        let (token, start) = requests[1];
        assert_ne!(token, requests[0].0);
        assert_eq!(access_point.counters().tx_agreements_offered, 2);
        // The peer accepts it, and its frames go as an A-MPDU.
        let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
        write_successful_addba_response(&mut response, token, 0, 16).unwrap();
        serve(
            model,
            router,
            timer,
            access_point,
            &[(at + 1_152_000, block_ack_action(&response))],
            at + 1_153_000,
        );
        assert_eq!(access_point.counters().tx_agreements, 1);
        send_all(&[b"four", b"five"]);
        serve(model, router, timer, access_point, &[], at + 1_154_000);
        assert_eq!(
            data_attempts(model).last().unwrap(),
            &(vec![start + 1, start + 2], true)
        );
    });
}

/// What the recording rate control saw: each peer's controller as built,
/// and each exchange it learned from.
#[derive(Default)]
struct RateLog {
    peers: Vec<(PhyMode, bool, Option<i8>)>,
    mpdus: Vec<(u8, bool)>,
}

/// A controller that keeps one rate and records what it is told.
struct Recorded<'a> {
    rate: PhyRate,
    log: &'a RefCell<RateLog>,
}

impl<'a> RateControl for Recorded<'a> {
    type Config = (PhyRate, &'a RefCell<RateLog>);

    fn for_peer((rate, log): Self::Config, peer: &RatePeer, link_metric: Option<i8>) -> Self {
        log.borrow_mut()
            .peers
            .push((peer.phy, peer.ht_capabilities.is_some(), link_metric));
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
        _now: Instant,
        _attempted: u16,
        _acknowledged: u16,
        _ack_snr_db: Option<i8>,
    ) {
    }
}

/// The environment of an Open BSS whose rate control records.
struct RateEnv<'a>(core::marker::PhantomData<&'a ()>);

impl PortClientEnv for RateEnv<'_> {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = FixedRate;
    type Entropy = Seeded;
    type Aggregation = PortAmpduAggregation;
}

impl<'a> PortApEnv for RateEnv<'a> {
    type Timer = &'a VirtualTimer;
    type Authenticator = FixedMaterial;
    type Sae = NoSae;
    type RateControl = Recorded<'a>;
    type Frames = TestFrames;
}

/// An Association Request of an Open BSS HT station.
fn ht_association() -> Vec<u8> {
    let mut frame = association();
    let mut ht = vec![45, 26, 0x0c, 0x00, 0x03, 0xff];
    ht.resize(28, 0);
    frame.extend_from_slice(&ht);
    frame
}

#[test]
fn each_associated_peer_gets_its_own_rate_control_from_its_association() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<RateEnv<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let log = RefCell::new(RateLog::default());
    let peer_rate = PhyRate::Legacy(LegacyRate::Ofdm36M);
    let mut access_point = PortAccessPoint::<RateEnv<'_>>::new(
        PortApParts {
            client: client(&router),
            timer: &timer,
            authenticator: FixedMaterial,
            sae: NoSae,
            rate_control: (peer_rate, &log),
            frames: frames(),
        },
        profile(&ssid),
        service(),
        &mut storage,
    )
    .unwrap();
    drive(&model, &router, &timer, access_point.start(), &[], |_| {}).unwrap();
    let start = timer.now.get();
    // The Association Request arrives 56 dB over the noise floor.
    let metered = RxMeta {
        rssi_dbm: RxEvidence::HardwareObserved(-40),
        noise_floor_dbm: RxEvidence::HardwareObserved(-96),
        ..meta()
    };
    let frames = [
        (start + 1_000, authentication(false), meta()),
        (start + 2_000, ht_association(), metered),
    ];
    let stops: Vec<u64> = frames.iter().map(|(at, _, _)| *at).collect();
    let mut sent = 0;
    drive(
        &model,
        &router,
        &timer,
        access_point.run_until(Instant::from_micros(start + 3_000), &mut |_| {}),
        &stops,
        |now| {
            while sent < frames.len() && frames[sent].0 <= now {
                model.receive(&frames[sent].1, frames[sent].2);
                sent += 1;
            }
        },
    )
    .unwrap();
    // One controller, for an HT20 link, from the request's link metric.
    assert_eq!(log.borrow().peers, [(PhyMode::Ht20, true, Some(56))]);

    // The peer's data goes at its controller's rate, which learns from it;
    // the group's at the management rate, which teaches it nothing.
    send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], b"down"));
    send(&ethernet([0xff; 6], [0x02, 0, 0, 0, 0, 0x99], b"all"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 4_000,
    );
    assert_eq!(downlink(&model, STATION)[0].2, peer_rate);
    assert_eq!(downlink(&model, [0xff; 6])[0].2, RATE);
    assert_eq!(log.borrow().mpdus, [(1, true)]);

    // A peer that leaves and associates again starts over.
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[
            (start + 5_000, management(12, false, &[3, 0])),
            (start + 6_000, authentication(false)),
            (start + 7_000, association()),
        ],
        start + 8_000,
    );
    assert_eq!(log.borrow().peers[1..], [(PhyMode::Legacy, false, None)]);
}

#[test]
fn frames_held_for_a_dozing_peer_stay_the_network_s_owners_until_it_leaves() {
    let model = model();
    let timer = VirtualTimer::default();
    let router = PortApRouter::<Env<'_>>::new(&model, 1);
    let ssid = WifiSsid::new(SSID).unwrap();
    let mut storage = PortApStorage::<8, TestFrame>::new();
    let mut access_point = associated(&model, &router, &timer, &ssid, &mut storage);
    let from = [0x02, 0, 0, 0, 0, 0x99];
    let start = timer.now.get();
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 1_000, null_data(true))],
        start + 2_000,
    );
    let before = returned();
    send(&ethernet(STATION, from, b"one"));
    send(&ethernet(STATION, from, b"two"));
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[],
        start + 3_000,
    );
    // Held: taken from the network's queue, not copied, not returned.
    assert_eq!(access_point.counters().held, 2);
    assert_eq!(returned(), before);
    FRAMES.with(|frames| assert!(frames.get().unwrap().is_empty()));
    // The peer leaves: the access point returns both owners.
    serve(
        &model,
        &router,
        &timer,
        &mut access_point,
        &[(start + 4_000, management(12, false, &[3, 0]))],
        start + 5_000,
    );
    assert_eq!(returned(), before + 2);
    assert!(downlink(&model, STATION).is_empty());
}

#[test]
fn an_aggregate_carries_more_than_eight_subframes_each_its_owner_s_payload() {
    with_ht_peer(|model, router, timer, access_point, at| {
        let (token, start) = addba_requests(model)[0];
        let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
        write_successful_addba_response(&mut response, token, 0, 32).unwrap();
        serve(
            model,
            router,
            timer,
            access_point,
            &[(at + 1_000, block_ack_action(&response))],
            at + 2_000,
        );
        let before = returned();
        let payloads: Vec<Vec<u8>> = (0..12_u8)
            .map(|n| vec![0x40 + n; 30 + usize::from(n)])
            .collect();
        for payload in &payloads {
            send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], payload));
        }
        serve(model, router, timer, access_point, &[], at + 3_000);
        // One aggregate of all twelve.
        assert_eq!(
            data_attempts(model),
            [((start..start + 12).collect::<Vec<_>>(), true)]
        );
        // Each subframe ends in its frame's payload, gathered from the
        // network's owner after the header.
        let aggregate = model
            .submitted()
            .into_iter()
            .find(|attempt| attempt.ampdu)
            .unwrap();
        for (subframe, payload) in aggregate.frames.iter().zip(&payloads) {
            assert!(subframe.ends_with(payload));
        }
        // The owners went back once the exchange ended.
        assert_eq!(returned(), before + 12);
        FRAMES.with(|frames| assert!(frames.get().unwrap().is_empty()));
    });
}

#[test]
fn a_frame_the_aggregate_does_not_admit_stays_with_the_network_for_the_next_turn() {
    with_ht_peer(|model, router, timer, access_point, at| {
        let (token, start) = addba_requests(model)[0];
        // An agreement of two frames.
        let mut response = [0_u8; ADDBA_ACTION_BODY_LEN];
        write_successful_addba_response(&mut response, token, 0, 2).unwrap();
        serve(
            model,
            router,
            timer,
            access_point,
            &[(at + 1_000, block_ack_action(&response))],
            at + 2_000,
        );
        let before = returned();
        for payload in [&b"one"[..], b"two", b"three"] {
            send(&ethernet(STATION, [0x02, 0, 0, 0, 0, 0x99], payload));
        }
        serve(model, router, timer, access_point, &[], at + 3_000);
        // Two go as the window's aggregate; the third waits in the network's
        // queue and goes on its own turn, alone.
        assert_eq!(
            data_attempts(model),
            [(vec![start, start + 1], true), (vec![start + 2], false)]
        );
        assert_eq!(returned(), before + 3);
    });
}
