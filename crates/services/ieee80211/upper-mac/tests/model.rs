//! The transmit driver over the lower-MAC host model: each test queues the
//! outcomes the model gives the next attempts and checks what the driver
//! submitted and reported.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::{Context, Poll, Waker},
};

use oer_ieee80211_lower_mac::Ieee80211Instant;
use oer_ieee80211_lower_mac::{
    Backoff, BlockAckReport, Channel, ChannelWidth, CoexPriority, Ieee80211LowerMacPort,
    KeySelector, LifecycleCommand, LowerMacSetting, MacAddress, Protection, ReceiveFilter, TxPower,
    TxResponse, TxStatus, VifConfig, VifId, VifRole,
    model::{LowerMacModel, ModelOutcome},
};
use oer_ieee80211_lower_mac::{LowerMacEvent, RxMeta};
use oer_ieee80211_mac::{
    ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumberStep, CcmpTxPacketNumber},
    phy::{LegacyRate, PhyRate},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};
use oer_ieee80211_softmac::{BackoffEntropy, EdcaContention, MacAmpduTxResult, MacTxResult};
use oer_ieee80211_upper_mac::{
    AckFailureAccounting, AmpduRequest, AmpduRetryPolicy, MpduRequest, ProtectEveryHeTxop,
    ProtectionPolicy, RateLadder, RetryLimits, RtsLengthThreshold, TxBody, TxPlanner, TxReceiver,
    TxReport, TxRequest,
};
use oer_ieee80211_upper_mac_service::{
    AmpduFrames, EventRouter, MpduParts, UpperMacTx, UpperMacTxError,
    client::{PortClient, PortClientConfig, PortClientEnv, PortClientError},
};

const STATION: VifId = VifId(0);
const ADDRESS: MacAddress = [0x02, 0, 0, 0, 0, 1];
const PEER: MacAddress = [0x02, 0, 0, 0, 0, 2];

const OFDM54: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm54M);
const OFDM48: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm48M);
const OFDM24: PhyRate = PhyRate::Legacy(LegacyRate::Ofdm24M);

/// 54, 48, then 24 Mb/s.
struct Ladder;

impl RateLadder for Ladder {
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate> {
        Some(match failures {
            0 => initial,
            1 => OFDM48,
            _ => OFDM24,
        })
    }
}

/// A seeded xorshift32 source: the same seed draws the same backoffs.
#[derive(Clone)]
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

/// Run a future whose every wait the model has already satisfied.
fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..1_000 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("the driver waits for an event the model never produces");
}

fn enabled_station() -> LowerMacModel {
    let model = LowerMacModel::new();
    let channel = Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap();
    model
        .apply(LowerMacSetting::Channel(channel))
        .unwrap()
        .unwrap();
    model.lifecycle(LifecycleCommand::Enable).unwrap().unwrap();
    // Take the Enabled terminal event.
    run(model.next_event()).unwrap();
    model
        .apply(LowerMacSetting::Vif {
            vif: STATION,
            config: Some(VifConfig {
                address: ADDRESS,
                role: VifRole::Station,
                bssid: Some(PEER),
                receive: ReceiveFilter::BSS_MEMBER,
            }),
        })
        .unwrap()
        .unwrap();
    model
}

const LIMITS: RetryLimits = RetryLimits {
    short: 7,
    long: 4,
    ack_failure: AckFailureAccounting::Short,
};

/// The router of a test: four waiting exchanges, four received frames.
type Router<'m> = EventRouter<'m, LowerMacModel, 4, 4>;

type Driver<'r, 'm> = UpperMacTx<'r, 'm, LowerMacModel, ProtectEveryHeTxop, 4, 4>;

/// Run `exchange` while the router takes the port's events, as a
/// composition polls the router beside its exchanges.
async fn exchange<T>(router: &Router<'_>, exchange: impl Future<Output = T>) -> T {
    let mut exchange = pin!(exchange);
    let mut routing = pin!(router.run());
    poll_fn(|context| {
        if let Poll::Ready(output) = exchange.as_mut().poll(context) {
            return Poll::Ready(output);
        }
        assert!(
            routing.as_mut().poll(context).is_pending(),
            "the model was poisoned"
        );
        Poll::Pending
    })
    .await
}

fn driver<'r, 'm>(router: &'r Router<'m>) -> Driver<'r, 'm> {
    UpperMacTx::new(
        router,
        STATION,
        TxPlanner::new(
            [EdcaContention::new(4, 7); 4],
            LIMITS,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(1_000_000),
                aged_margin: oer_time::RadioDuration::from_micros(1_024),
                retry_limit: 7,
                retain_single_mpdu: false,
            },
            ProtectionPolicy::new(Some(RtsLengthThreshold::new(500))),
            ProtectEveryHeTxop,
        ),
    )
}

/// A protected QoS Data MPDU to the peer, header to end of body.
fn qos_data(sequence: u16, packet_number: [u8; 8], body: usize) -> Vec<u8> {
    let mut frame = vec![0_u8; 26 + 8 + body];
    frame[0] = 0x88;
    frame[1] = 0x41;
    frame[4..10].copy_from_slice(&PEER);
    frame[10..16].copy_from_slice(&ADDRESS);
    frame[16..22].copy_from_slice(&PEER);
    frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
    frame[26..34].copy_from_slice(&packet_number);
    frame
}

fn mpdu_request(frame: &[u8], mpdu_retry_limit: u8) -> TxRequest {
    TxRequest {
        access_category: WmmAccessCategory::BestEffort,
        initial_rate: OFDM54,
        receiver: TxReceiver::Individual,
        power: TxPower::Calibrated,
        coex: CoexPriority::Normal,
        mpdu_retry_limit,
        body: TxBody::Mpdu(MpduRequest {
            length: frame.len() as u32 + 12,
            response: TxResponse::Ack,
        }),
    }
}

fn retry_bit(frame: &[u8]) -> bool {
    frame[1] & 0x08 != 0
}

#[test]
fn a_frame_acknowledged_at_the_first_attempt_is_sent_once() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(7, [3, 0, 0, 0x20, 0, 0, 0, 0], 40);
    model.respond([ModelOutcome::Success]);
    let report = run(exchange(
        &router,
        tx.send_mpdu(
            MpduParts::whole(&frame),
            KeySelector::Plaintext,
            mpdu_request(&frame, 4),
            &Ladder,
            &mut Seeded(1),
        ),
    ))
    .unwrap();
    let TxReport::Mpdu(status) = report else {
        panic!("an MPDU report");
    };
    assert_eq!(status.result, MacTxResult::Transmitted);
    assert_eq!(status.attempts, 1);
    assert_eq!(status.acknowledged, Some(true));
    let submitted = model.submitted();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].rate, OFDM54);
    assert_eq!(submitted[0].protection, Protection::None);
    assert_eq!(submitted[0].frames, core::slice::from_ref(&frame));
    assert!(!retry_bit(&submitted[0].frames[0]));
    assert_eq!(model.buffers_lent(), 0);
}

#[test]
fn a_missing_ack_retries_down_the_ladder_with_the_retry_bit_until_the_limit() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(8, [6, 0, 0, 0x20, 0, 0, 0, 0], 40);
    model.respond([ModelOutcome::Fail(TxStatus::AckTimeout); 3]);
    let report = run(exchange(
        &router,
        tx.send_mpdu(
            MpduParts::whole(&frame),
            KeySelector::Plaintext,
            mpdu_request(&frame, 3),
            &Ladder,
            &mut Seeded(1),
        ),
    ))
    .unwrap();
    let TxReport::Mpdu(status) = report else {
        panic!("an MPDU report");
    };
    assert_eq!(status.result, MacTxResult::HardwareTimeout);
    assert_eq!(status.attempts, 3);
    assert_eq!(status.final_rate, OFDM24);
    assert_eq!(status.acknowledged, Some(false));

    let submitted = model.submitted();
    let rates: Vec<_> = submitted.iter().map(|attempt| attempt.rate).collect();
    assert_eq!(rates, [OFDM54, OFDM48, OFDM24]);
    let retries: Vec<_> = submitted
        .iter()
        .map(|attempt| retry_bit(&attempt.frames[0]))
        .collect();
    assert_eq!(retries, [false, true, true]);
    // Each attempt had its own identity.
    assert_eq!(submitted[2].id.0, submitted[0].id.0 + 2);
}

#[test]
fn a_cts_timeout_resends_under_the_same_protection_without_the_retry_bit() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    // Longer than the 500-octet RTS threshold.
    let frame = qos_data(9, [9, 0, 0, 0x20, 0, 0, 0, 0], 600);
    model.respond([
        ModelOutcome::Fail(TxStatus::CtsTimeout),
        ModelOutcome::Success,
    ]);
    let report = run(exchange(
        &router,
        tx.send_mpdu(
            MpduParts::whole(&frame),
            KeySelector::Plaintext,
            mpdu_request(&frame, 4),
            &Ladder,
            &mut Seeded(1),
        ),
    ))
    .unwrap();
    let TxReport::Mpdu(status) = report else {
        panic!("an MPDU report");
    };
    assert_eq!(status.result, MacTxResult::Transmitted);
    assert_eq!(status.attempts, 2);
    let submitted = model.submitted();
    assert_eq!(submitted[0].protection, Protection::RtsCts);
    assert_eq!(submitted[1].protection, Protection::RtsCts);
    // The MPDU never reached the receiver: no Retry bit.
    assert!(!retry_bit(&submitted[1].frames[0]));
    assert_eq!(submitted[1].rate, OFDM48);
}

fn ampdu_request(frames: &[Vec<u8>], first_sequence: u16) -> TxRequest {
    let lengths: Vec<u16> = frames.iter().map(|frame| frame.len() as u16 + 12).collect();
    TxRequest {
        access_category: WmmAccessCategory::Video,
        initial_rate: PhyRate::Ht(
            oer_ieee80211_mac::phy::HtRate::new(
                oer_ieee80211_mac::phy::HtMcs::new(7).unwrap(),
                oer_ieee80211_mac::phy::PpduBandwidth::Mhz20,
                true,
            )
            .unwrap(),
        ),
        receiver: TxReceiver::Individual,
        power: TxPower::Calibrated,
        coex: CoexPriority::Normal,
        mpdu_retry_limit: 4,
        body: TxBody::Ampdu(
            AmpduRequest::new(
                0,
                SequenceNumber::new(first_sequence).unwrap(),
                &lengths,
                Ieee80211Instant::from_micros(0),
                true,
            )
            .unwrap(),
        ),
    }
}

#[test]
fn a_partial_block_ack_resends_only_the_unacknowledged_subframes() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frames: Vec<Vec<u8>> = (0..4)
        .map(|index| {
            qos_data(
                200 + index,
                [3 * (index as u8 + 1), 0, 0, 0x20, 0, 0, 0, 0],
                50,
            )
        })
        .collect();
    // Each subframe in two parts the port's buffer gathers.
    let slices: Vec<MpduParts<'_>> = frames
        .iter()
        .map(|frame| MpduParts::new(&frame[..24], &frame[24..]))
        .collect();
    // 200 and 202 acknowledged, then the rest.
    model.respond([
        ModelOutcome::BlockAck(BlockAckReport {
            start_sequence: SequenceNumber::new(200).unwrap(),
            bitmap: 0b0101,
        }),
        ModelOutcome::Success,
    ]);
    let report = run(exchange(
        &router,
        tx.send_ampdu(
            AmpduFrames {
                subframes: &slices,
                key: KeySelector::Plaintext,
                min_mpdu_start_spacing: 0,
            },
            ampdu_request(&frames, 200),
            &Ladder,
            &mut Seeded(1),
        ),
    ))
    .unwrap();
    let TxReport::Ampdu(status) = report else {
        panic!("an A-MPDU report");
    };
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(status.aggregate_attempts, 2);
    assert_eq!(status.block_acknowledged_subframes, 4);

    let submitted = model.submitted();
    assert_eq!(submitted.len(), 2);
    assert!(submitted.iter().all(|attempt| attempt.ampdu));
    assert_eq!(submitted[0].frames, frames);
    // The second aggregate holds 201 and 203 only, each with the Retry bit,
    // their sequence numbers and packet numbers unchanged.
    let resent = &submitted[1].frames;
    assert_eq!(resent.len(), 2);
    for (resent, original) in resent.iter().zip([&frames[1], &frames[3]]) {
        assert!(retry_bit(resent));
        assert_eq!(resent[2..], original[2..]);
    }
    assert_eq!(model.ampdu_buffers_lent(), 0);
}

#[test]
fn one_unacknowledged_subframe_is_resent_alone() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frames: Vec<Vec<u8>> = (0..3)
        .map(|index| qos_data(10 + index, [0; 8], 30))
        .collect();
    // Each subframe in two parts the port's buffer gathers.
    let slices: Vec<MpduParts<'_>> = frames
        .iter()
        .map(|frame| MpduParts::new(&frame[..24], &frame[24..]))
        .collect();
    model.respond([
        ModelOutcome::BlockAck(BlockAckReport {
            start_sequence: SequenceNumber::new(10).unwrap(),
            bitmap: 0b011,
        }),
        ModelOutcome::Success,
    ]);
    let report = run(exchange(
        &router,
        tx.send_ampdu(
            AmpduFrames {
                subframes: &slices,
                key: KeySelector::Plaintext,
                min_mpdu_start_spacing: 0,
            },
            ampdu_request(&frames, 10),
            &Ladder,
            &mut Seeded(1),
        ),
    ))
    .unwrap();
    let TxReport::Ampdu(status) = report else {
        panic!("an A-MPDU report");
    };
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(status.individual_retries.transmitted, 1);
    let submitted = model.submitted();
    assert!(!submitted[1].ampdu);
    assert!(retry_bit(&submitted[1].frames[0]));
    assert_eq!(submitted[1].frames[0][2..], frames[2][2..]);
}

fn slots(backoff: Backoff) -> u16 {
    match backoff {
        Backoff::Slots(slots) => slots,
        Backoff::HardwareDraw { .. } => panic!("the planner draws in software"),
    }
}

#[test]
fn the_contention_window_doubles_on_failures_and_resets_after_the_frame() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(1, [0; 8], 40);
    let seed = Seeded(0x1234_5678);
    let mut entropy = seed.clone();
    model.respond([
        ModelOutcome::Fail(TxStatus::AckTimeout),
        ModelOutcome::Fail(TxStatus::AckTimeout),
        ModelOutcome::Success,
        ModelOutcome::Success,
    ]);
    for _ in 0..2 {
        run(exchange(
            &router,
            tx.send_mpdu(
                MpduParts::whole(&frame),
                KeySelector::Plaintext,
                mpdu_request(&frame, 4),
                &Ladder,
                &mut entropy,
            ),
        ))
        .unwrap();
    }
    // The same seed reproduces every draw: CW 15, 31, 63 for the first
    // frame, and 15 again for the next frame.
    let mut expected = seed;
    let windows = [15_u32, 31, 63, 15];
    let backoffs: Vec<u16> = model
        .submitted()
        .iter()
        .map(|attempt| slots(attempt.backoff))
        .collect();
    let drawn: Vec<u16> = windows
        .iter()
        .map(|window| (expected.next_u32() & window) as u16)
        .collect();
    assert_eq!(backoffs, drawn);
    assert_eq!(
        tx.planner()
            .contention(WmmAccessCategory::BestEffort)
            .cw_exponent(),
        4
    );
}

#[test]
fn a_retry_repeats_its_packet_number_and_the_next_frame_takes_a_higher_one() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let mut packet_numbers = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE);
    let first = qos_data(
        20,
        packet_numbers.next_header(CcmpKeyId::PAIRWISE).unwrap(),
        40,
    );
    let second = qos_data(
        21,
        packet_numbers.next_header(CcmpKeyId::PAIRWISE).unwrap(),
        40,
    );
    model.respond([
        ModelOutcome::Fail(TxStatus::AckTimeout),
        ModelOutcome::Success,
        ModelOutcome::Success,
    ]);
    for frame in [&first, &second] {
        run(exchange(
            &router,
            tx.send_mpdu(
                MpduParts::whole(frame),
                KeySelector::Plaintext,
                mpdu_request(frame, 4),
                &Ladder,
                &mut Seeded(7),
            ),
        ))
        .unwrap();
    }
    let packet_number = |frame: &[u8]| {
        CcmpHeader::parse(frame[26..34].try_into().unwrap())
            .unwrap()
            .packet_number()
    };
    let submitted = model.submitted();
    assert_eq!(submitted.len(), 3);
    // The retransmission carries the PN and sequence number of its first
    // attempt; only the Retry bit differs.
    assert_eq!(
        packet_number(&submitted[0].frames[0]),
        packet_number(&submitted[1].frames[0])
    );
    assert_eq!(
        submitted[0].frames[0][22..24],
        submitted[1].frames[0][22..24]
    );
    assert!(packet_number(&submitted[2].frames[0]) > packet_number(&submitted[1].frames[0]));
}

#[test]
fn a_refused_attempt_releases_its_buffer_and_reports_the_refusal() {
    let model = LowerMacModel::new();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(1, [0; 8], 40);
    let result = run(exchange(
        &router,
        tx.send_mpdu(
            MpduParts::whole(&frame),
            KeySelector::Plaintext,
            mpdu_request(&frame, 4),
            &Ladder,
            &mut Seeded(1),
        ),
    ));
    assert_eq!(
        result,
        Err(UpperMacTxError::Refused(
            oer_ieee80211_lower_mac::SubmitError::Disabled
        ))
    );
    assert_eq!(model.buffers_lent(), 0);
}

/// A data frame from the peer to the station, admitted by its filter.
fn received_frame() -> [u8; 24] {
    let mut frame = [0_u8; 24];
    frame[0] = 0x08;
    frame[1] = 0x02;
    frame[4..10].copy_from_slice(&ADDRESS);
    frame[10..16].copy_from_slice(&PEER);
    frame[16..22].copy_from_slice(&PEER);
    frame
}

fn receive(model: &LowerMacModel) {
    model.receive(
        &received_frame(),
        RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()),
    );
}

fn request_on(frame: &[u8], access_category: WmmAccessCategory) -> TxRequest {
    TxRequest {
        access_category,
        ..mpdu_request(frame, 4)
    }
}

fn queue(access_category: WmmAccessCategory) -> u8 {
    oer_ieee80211_lower_mac::model::MODEL_CAPABILITIES.tx_queue(access_category)
}

/// Poll `future` once without an executor.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn concurrent_exchanges_on_two_access_categories_keep_their_own_completions() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut best_effort = driver(&router);
    let mut voice = driver(&router);
    let be_frame = qos_data(30, [1, 0, 0, 0x20, 0, 0, 0, 0], 40);
    let vo_frame = qos_data(31, [2, 0, 0, 0x20, 0, 0, 0, 0], 40);
    let mut entropy_1 = Seeded(1);
    let mut be = pin!(best_effort.send_mpdu(
        MpduParts::whole(&be_frame),
        KeySelector::Plaintext,
        request_on(&be_frame, WmmAccessCategory::BestEffort),
        &Ladder,
        &mut entropy_1,
    ));
    let mut entropy_2 = Seeded(2);
    let mut vo = pin!(voice.send_mpdu(
        MpduParts::whole(&vo_frame),
        KeySelector::Plaintext,
        request_on(&vo_frame, WmmAccessCategory::Voice),
        &Ladder,
        &mut entropy_2,
    ));
    let mut routing = pin!(router.run());
    let mut received = 0;
    macro_rules! step {
        ($be:expr, $vo:expr) => {{
            let be = poll_once($be.as_mut());
            let vo = poll_once($vo.as_mut());
            assert!(poll_once(routing.as_mut()).is_pending());
            (be, vo)
        }};
    }

    // Both attempts are in flight on their own queues.
    assert!(matches!(step!(be, vo), (Poll::Pending, Poll::Pending)));
    assert_eq!(model.in_flight(), 2);

    // Received frames and the voice completion arrive while best effort
    // still waits: neither exchange takes the other's completion.
    receive(&model);
    model.complete(queue(WmmAccessCategory::Voice), TxStatus::AckTimeout);
    receive(&model);
    assert!(matches!(step!(be, vo), (Poll::Pending, Poll::Pending)));
    // The voice exchange retries after its failed attempt; best effort
    // still waits for its own completion.
    assert!(matches!(step!(be, vo), (Poll::Pending, Poll::Pending)));
    assert_eq!(model.in_flight(), 2);
    model.complete(queue(WmmAccessCategory::BestEffort), TxStatus::Success);
    model.complete(queue(WmmAccessCategory::Voice), TxStatus::Success);
    assert!(matches!(step!(be, vo), (Poll::Pending, Poll::Pending)));
    let Poll::Ready(be_report) = poll_once(be.as_mut()) else {
        panic!("best effort completes");
    };
    let Poll::Ready(vo_report) = poll_once(vo.as_mut()) else {
        panic!("voice completes");
    };
    let (Ok(TxReport::Mpdu(be_status)), Ok(TxReport::Mpdu(vo_status))) = (be_report, vo_report)
    else {
        panic!("two MPDU reports");
    };
    assert_eq!(
        (be_status.result, be_status.attempts),
        (MacTxResult::Transmitted, 1)
    );
    assert_eq!(
        (vo_status.result, vo_status.attempts),
        (MacTxResult::Transmitted, 2)
    );
    assert_eq!(router.unclaimed_completions(), 0);

    // The received frames waited in the router's receive queue.
    for _ in 0..2 {
        let Poll::Ready(Some(Ok(event))) = poll_once(pin!(router.received())) else {
            panic!("a received frame");
        };
        assert!(matches!(
            LowerMacModel::view(&event),
            LowerMacEvent::Received { .. }
        ));
        received += 1;
    }
    assert_eq!(received, 2);
    assert!(poll_once(pin!(router.received())).is_pending());
}

#[test]
fn a_loss_is_recovered_by_cancelling_the_attempt_in_flight() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(40, [5, 0, 0, 0x20, 0, 0, 0, 0], 40);
    let mut entropy_3 = Seeded(1);
    let mut send = pin!(tx.send_mpdu(
        MpduParts::whole(&frame),
        KeySelector::Plaintext,
        mpdu_request(&frame, 4),
        &Ladder,
        &mut entropy_3,
    ));
    let mut routing = pin!(router.run());
    assert!(poll_once(send.as_mut()).is_pending());
    // A receive burst overflows the port's queue while the attempt flies.
    for _ in 0..6 {
        receive(&model);
    }
    assert!(poll_once(routing.as_mut()).is_pending());
    // The exchange learns of the loss and cancels its published attempt,
    // which ends with its own completion.
    assert!(poll_once(send.as_mut()).is_pending());
    model.complete(0, TxStatus::Success);
    assert!(poll_once(routing.as_mut()).is_pending());
    let Poll::Ready(Ok(TxReport::Mpdu(status))) = poll_once(send.as_mut()) else {
        panic!("the exchange recovers its completion");
    };
    assert_eq!(status.result, MacTxResult::Transmitted);
    // The receive queue reports its own loss after the frames it kept.
    let mut frames = 0;
    loop {
        match poll_once(pin!(router.received())) {
            Poll::Ready(Some(Ok(_))) => frames += 1,
            Poll::Ready(Some(Err(_))) => break,
            other => panic!("unexpected {other:?}", other = other.is_pending()),
        }
    }
    assert_eq!(frames, 4);
}

#[test]
fn a_completion_lost_in_the_gap_ends_the_exchange_without_a_report() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(41, [6, 0, 0, 0x20, 0, 0, 0, 0], 40);
    let mut entropy_4 = Seeded(1);
    let mut send = pin!(tx.send_mpdu(
        MpduParts::whole(&frame),
        KeySelector::Plaintext,
        mpdu_request(&frame, 4),
        &Ladder,
        &mut entropy_4,
    ));
    let mut routing = pin!(router.run());
    assert!(poll_once(send.as_mut()).is_pending());
    for _ in 0..6 {
        receive(&model);
    }
    // The completion falls into the gap.
    model.complete(0, TxStatus::Success);
    let result = run(poll_fn(|context| {
        if let Poll::Ready(result) = send.as_mut().poll(context) {
            return Poll::Ready(result);
        }
        assert!(routing.as_mut().poll(context).is_pending());
        Poll::Pending
    }));
    assert_eq!(
        result,
        Err(UpperMacTxError::CompletionLost {
            attempt: oer_ieee80211_lower_mac::TxId(100)
        })
    );
}

#[test]
fn a_poisoned_port_ends_every_exchange() {
    let model = enabled_station();
    let router = Router::new(&model, 100);
    let mut tx = driver(&router);
    let frame = qos_data(42, [7, 0, 0, 0x20, 0, 0, 0, 0], 40);
    let mut entropy_5 = Seeded(1);
    let mut send = pin!(tx.send_mpdu(
        MpduParts::whole(&frame),
        KeySelector::Plaintext,
        mpdu_request(&frame, 4),
        &Ladder,
        &mut entropy_5,
    ));
    let mut routing = pin!(router.run());
    assert!(poll_once(send.as_mut()).is_pending());
    model.poison();
    assert!(poll_once(routing.as_mut()).is_ready());
    assert!(router.poisoned());
    assert_eq!(
        poll_once(send.as_mut()),
        Poll::Ready(Err(UpperMacTxError::Poisoned))
    );
}

/// The client test's environment over the model.
struct ClientEnv;

impl PortClientEnv for ClientEnv {
    type Port = LowerMacModel;
    type Budget = ProtectEveryHeTxop;
    type Ladder = Ladder;
    type Entropy = Seeded;
    type Aggregation = oer_ieee80211_upper_mac_service::aggregate::PortAmpduAggregation;
}

fn client<'r>(router: &'r Router<'r>, role: VifRole) -> PortClient<'r, ClientEnv, 4, 4> {
    PortClient::new(
        router,
        TxPlanner::new(
            [EdcaContention::new(4, 7); 4],
            LIMITS,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(1_000_000),
                aged_margin: oer_time::RadioDuration::from_micros(1_024),
                retry_limit: 7,
                retain_single_mpdu: false,
            },
            ProtectionPolicy::new(None),
            ProtectEveryHeTxop,
        ),
        Ladder,
        Seeded(0x1357_9bdf),
        PortClientConfig {
            vif: STATION,
            address: ADDRESS,
            role,
            power: TxPower::Calibrated,
            retry_limit: 7,
        },
    )
}

#[test]
fn a_client_configures_its_interface_in_its_role() {
    let model = enabled_station();
    let router = Router::new(&model, 1);
    let client = client(&router, VifRole::AccessPoint);
    assert_eq!(
        client.configure(Some(ADDRESS), ReceiveFilter::BSS_MEMBER),
        Ok(())
    );
    assert_eq!(
        model.vif_config(STATION),
        Some(VifConfig {
            address: ADDRESS,
            role: VifRole::AccessPoint,
            bssid: Some(ADDRESS),
            receive: ReceiveFilter::BSS_MEMBER,
        })
    );
}

#[test]
fn a_frame_without_its_first_address_is_refused_before_the_port() {
    let model = enabled_station();
    let router = Router::new(&model, 1);
    let mut client = client(&router, VifRole::Station);
    let short = [0x08, 0, 0, 0, 0x02, 0];
    assert_eq!(
        run(exchange(
            &router,
            client.transmit(
                MpduParts::whole(&short),
                KeySelector::Plaintext,
                WmmAccessCategory::Voice,
                OFDM24,
                CoexPriority::Normal,
            ),
        )),
        Err(PortClientError::FrameTooShort)
    );
    assert!(model.submitted().is_empty());
}
