use oer_ieee80211_lower_mac::TxId;
use oer_ieee80211_mac::phy::LegacyRate;

use super::*;
use crate::{protection::RtsLengthThreshold, retry::AckFailureAccounting};

const LIMITS: RetryLimits = RetryLimits {
    short: 4,
    long: 4,
    ack_failure: AckFailureAccounting::Short,
};

const AMPDU_POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
    lifetime: oer_time::RadioDuration::from_micros(100_000),
    aged_margin: oer_time::RadioDuration::from_micros(1_024),
    retry_limit: 4,
    retain_single_mpdu: false,
};

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

/// Every draw takes the whole window.
fn all_ones() -> u32 {
    u32::MAX
}

fn planner() -> TxPlanner {
    TxPlanner::new(
        [EdcaContention::new(4, 6); 4],
        LIMITS,
        AMPDU_POLICY,
        ProtectionPolicy::new(Some(RtsLengthThreshold::new(1_000))),
        crate::protection::ProtectEveryHeTxop,
    )
}

fn mpdu(length: u32) -> TxRequest {
    TxRequest {
        access_category: WmmAccessCategory::BestEffort,
        initial_rate: OFDM54,
        receiver: TxReceiver::Individual,
        power: TxPower::Calibrated,
        coex: CoexPriority::Normal,
        mpdu_retry_limit: 3,
        body: TxBody::Mpdu(MpduRequest {
            length,
            response: TxResponse::Ack,
        }),
    }
}

fn completion(status: TxStatus, block_ack: Option<BlockAckReport>) -> TxCompletion {
    TxCompletion {
        id: TxId(0),
        status,
        ack_rssi_dbm: None,
        ack_snr_db: (status == TxStatus::Success).then_some(17),
        block_ack,
    }
}

fn attempt(step: TxStep) -> TxAttemptPlan {
    match step {
        TxStep::Attempt(plan) => plan,
        TxStep::Done(report) => panic!("expected an attempt, got {report:?}"),
    }
}

fn now(micros: u64) -> RadioInstant {
    RadioInstant::from_micros(micros)
}

#[test]
fn a_delivered_mpdu_reports_one_attempt_and_resets_contention() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, plan) = planner.begin(mpdu(200), &Ladder, &mut entropy).unwrap();
    assert_eq!(plan.rate, OFDM54);
    assert_eq!(plan.backoff, Backoff::Slots(15));
    assert_eq!(
        plan.content,
        AttemptContent::Mpdu {
            set_retry_bit: false
        }
    );
    assert!(!plan.is_protected());
    let step = planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, None),
        now(10),
        &Ladder,
        &mut entropy,
    );
    let TxStep::Done(TxReport::Mpdu(status)) = step else {
        panic!("the exchange ended");
    };
    assert_eq!(status.result, MacTxResult::Transmitted);
    assert_eq!(status.attempts, 1);
    assert_eq!(status.final_rate, OFDM54);
    assert_eq!(status.acknowledged, Some(true));
    assert_eq!(status.ack_snr_db, Some(17));
    assert!(exchange.is_done());
    assert_eq!(exchange.report(), Some(TxReport::Mpdu(status)));
}

#[test]
fn ack_timeouts_walk_the_ladder_with_the_retry_bit_and_double_the_window() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, _) = planner.begin(mpdu(200), &Ladder, &mut entropy).unwrap();
    let timeout = completion(TxStatus::AckTimeout, None);
    let second = attempt(planner.complete(&mut exchange, &timeout, now(1), &Ladder, &mut entropy));
    assert_eq!(second.rate, OFDM48);
    assert_eq!(second.backoff, Backoff::Slots(31));
    assert_eq!(
        second.content,
        AttemptContent::Mpdu {
            set_retry_bit: true
        }
    );
    let third = attempt(planner.complete(&mut exchange, &timeout, now(2), &Ladder, &mut entropy));
    assert_eq!(third.rate, OFDM24);
    assert_eq!(third.backoff, Backoff::Slots(63));
    let TxStep::Done(TxReport::Mpdu(status)) =
        planner.complete(&mut exchange, &timeout, now(3), &Ladder, &mut entropy)
    else {
        panic!("the MPDU retry limit ended the exchange");
    };
    assert_eq!(status.result, MacTxResult::HardwareTimeout);
    assert_eq!(status.attempts, 3);
    assert_eq!(status.final_rate, OFDM24);
    assert_eq!(status.acknowledged, Some(false));
    // The frame ended: the window is back at CWmin.
    assert_eq!(
        planner
            .contention(WmmAccessCategory::BestEffort)
            .cw_exponent(),
        4
    );
}

#[test]
fn a_cts_timeout_retries_without_the_retry_bit_under_the_same_protection() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, first) = planner.begin(mpdu(1_500), &Ladder, &mut entropy).unwrap();
    assert_eq!(
        first.port_protection(),
        oer_ieee80211_lower_mac::Protection::RtsCts
    );
    let retry = attempt(planner.complete(
        &mut exchange,
        &completion(TxStatus::CtsTimeout, None),
        now(1),
        &Ladder,
        &mut entropy,
    ));
    assert_eq!(
        retry.content,
        AttemptContent::Mpdu {
            set_retry_bit: false
        }
    );
    assert_eq!(
        retry.port_protection(),
        oer_ieee80211_lower_mac::Protection::RtsCts
    );
    assert_eq!(retry.rate, OFDM48);
}

fn ampdu_request(subframes: usize) -> TxRequest {
    let lengths = [100_u16; 64];
    TxRequest {
        access_category: WmmAccessCategory::Video,
        initial_rate: OFDM54,
        receiver: TxReceiver::Individual,
        power: TxPower::Calibrated,
        coex: CoexPriority::Normal,
        mpdu_retry_limit: 2,
        body: TxBody::Ampdu(
            AmpduRequest::new(
                0,
                SequenceNumber::new(100).unwrap(),
                &lengths[..subframes],
                now(0),
                true,
            )
            .unwrap(),
        ),
    }
}

fn report(start: u16, bitmap: u64) -> Option<BlockAckReport> {
    Some(BlockAckReport {
        start_sequence: SequenceNumber::new(start).unwrap(),
        bitmap,
    })
}

#[test]
fn a_partial_block_ack_resends_only_the_missing_subframes() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, first) = planner
        .begin(ampdu_request(4), &Ladder, &mut entropy)
        .unwrap();
    assert_eq!(
        first.content,
        AttemptContent::Ampdu {
            subframes: 0b1111,
            retry: 0
        }
    );
    let second = attempt(planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, report(100, 0b0101)),
        now(10),
        &Ladder,
        &mut entropy,
    ));
    assert_eq!(
        second.content,
        AttemptContent::Ampdu {
            subframes: 0b1010,
            retry: 0b1010
        }
    );
    assert_eq!(second.rate, OFDM54);
    let TxStep::Done(TxReport::Ampdu(status)) = planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, report(101, 0b101)),
        now(20),
        &Ladder,
        &mut entropy,
    ) else {
        panic!("every subframe was acknowledged");
    };
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(status.aggregate_attempts, 2);
    assert_eq!(status.block_acknowledged_subframes, 4);
    assert!(status.fully_delivered());
}

#[test]
fn one_missing_subframe_leaves_the_aggregate_for_an_individual_retry() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, _) = planner
        .begin(ampdu_request(3), &Ladder, &mut entropy)
        .unwrap();
    let individual = attempt(planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, report(100, 0b011)),
        now(10),
        &Ladder,
        &mut entropy,
    ));
    assert_eq!(individual.content, AttemptContent::Subframe { index: 2 });
    let TxStep::Done(TxReport::Ampdu(status)) = planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, None),
        now(20),
        &Ladder,
        &mut entropy,
    ) else {
        panic!("the individual retry delivered the last subframe");
    };
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(status.block_acknowledged_subframes, 2);
    assert_eq!(status.individual_retries.transmitted, 1);
    assert!(status.fully_delivered());
}

#[test]
fn repeated_protection_failures_end_in_a_block_ack_request() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, _) = planner
        .begin(ampdu_request(2), &Ladder, &mut entropy)
        .unwrap();
    let cts = completion(TxStatus::CtsTimeout, None);
    for _ in 0..3 {
        let plan = attempt(planner.complete(&mut exchange, &cts, now(1), &Ladder, &mut entropy));
        assert_eq!(
            plan.content,
            AttemptContent::Ampdu {
                subframes: 0b11,
                retry: 0
            }
        );
    }
    let request = attempt(planner.complete(&mut exchange, &cts, now(1), &Ladder, &mut entropy));
    assert_eq!(
        request.content,
        AttemptContent::BlockAckRequest {
            tid: 0,
            starting_sequence: SequenceNumber::new(100).unwrap()
        }
    );
    assert!(matches!(request.rate, PhyRate::Legacy(_)));
    let TxStep::Done(TxReport::Ampdu(status)) = planner.complete(
        &mut exchange,
        &completion(TxStatus::Success, report(100, 0b11)),
        now(2),
        &Ladder,
        &mut entropy,
    ) else {
        panic!("the BlockAck acknowledged both subframes");
    };
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
}

#[test]
fn an_unanswered_aggregate_ends_as_a_hardware_timeout() {
    let mut planner = planner();
    let mut entropy = all_ones;
    let (mut exchange, _) = planner
        .begin(ampdu_request(2), &Ladder, &mut entropy)
        .unwrap();
    let timeout = completion(TxStatus::AckTimeout, None);
    for _ in 0..3 {
        let plan =
            attempt(planner.complete(&mut exchange, &timeout, now(1), &Ladder, &mut entropy));
        assert_eq!(
            plan.content,
            AttemptContent::Ampdu {
                subframes: 0b11,
                retry: 0b11
            }
        );
    }
    let TxStep::Done(TxReport::Ampdu(status)) =
        planner.complete(&mut exchange, &timeout, now(1), &Ladder, &mut entropy)
    else {
        panic!("the retry limit ended the aggregate");
    };
    assert_eq!(status.result, MacAmpduTxResult::HardwareTimeout);
    assert_eq!(status.aggregate_attempts, 4);
}

#[test]
fn an_aggregate_length_counts_delimiters_and_padding() {
    let request = AmpduRequest::new(
        0,
        SequenceNumber::ZERO,
        &[101, 102, 103],
        RadioInstant::from_micros(0),
        true,
    )
    .unwrap();
    // 4 + 101 -> 108, 4 + 102 -> 108, last 4 + 103 unpadded.
    assert_eq!(request.aggregate_length(0b111), 108 + 108 + 107);
    assert_eq!(request.aggregate_length(0b100), 107);
    assert!(AmpduRequest::new(0, SequenceNumber::ZERO, &[], now(0), true).is_none());
    assert_eq!(
        acknowledged_subframes(&request, report(0, 0b101).unwrap()),
        0b101
    );
}
