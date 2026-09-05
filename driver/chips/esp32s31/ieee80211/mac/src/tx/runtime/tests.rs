use super::*;
use crate::{
    tx::ampdu::{HtAmpduTxCompletion, HtBlockAckObservation},
    tx::{HtGuardInterval, HtMcs, HtRate, LegacyRate, TxCompletion, TxCookie},
};
use open_esp_radio_ieee80211::extensions::wmm::parse_wmm_parameter_element;

const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];

fn ipv4_with_dscp(dscp: u8) -> [u8; 16] {
    let mut frame = [0_u8; 16];
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[14] = 0x45;
    frame[15] = dscp << 2;
    frame
}

#[test]
fn sta_runtime_policy_owns_peer_and_vendor_edca_state() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    assert_eq!(policy.he_bss_color(), 0);
    assert_eq!(
        policy.contention_parameters(LegacyTxQueue::BestEffort),
        EdcaContentionParameters::new(3, 4, 10).unwrap()
    );
    assert_eq!(policy.contention_exponent(LegacyTxQueue::BestEffort), 4);

    policy.install_he_bss_color(0xff);
    policy.install_ht_ampdu(HtPeerAmpduParameters::from_capability_byte(0x17));
    assert_eq!(policy.he_bss_color(), 0x3f);
    assert_eq!(
        policy.ht_ampdu(),
        HtPeerAmpduParameters::from_capability_byte(0x17)
    );
    assert_eq!(
        policy.select_backoff(LegacyTxQueue::BestEffort, u32::MAX),
        15
    );
}

#[test]
fn negotiated_acm_downgrades_tid_and_queue_without_claiming_admission() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
        .unwrap();

    let voice = policy
        .select_network_traffic(&ipv4_with_dscp(46), true)
        .unwrap();
    assert_eq!(voice.requested.access_category, WmmAccessCategory::Voice);
    assert_eq!(voice.access_category, WmmAccessCategory::Video);
    assert_eq!(voice.user_priority, WmmUserPriority::UP5);
    assert_eq!(voice.queue(), LegacyTxQueue::Video);
    assert_eq!(
        voice.admission,
        WmmAdmissionDisposition::Downgraded {
            requested: WmmAccessCategory::Voice
        }
    );
    assert_eq!(voice.txop_limit_units_32_us(), 94);

    let non_qos = policy
        .select_network_traffic(&ipv4_with_dscp(46), false)
        .unwrap();
    assert_eq!(non_qos.tid(), 0);
    assert_eq!(non_qos.queue(), LegacyTxQueue::BestEffort);
    assert_eq!(non_qos.admission, WmmAdmissionDisposition::NonQosBestEffort);
}

#[test]
fn all_mandatory_access_categories_fail_closed() {
    let mut element = STANDARD_WMM;
    for offset in [10, 14, 18, 22] {
        element[offset] |= 0x10;
    }
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_wmm(parse_wmm_parameter_element(&element).unwrap())
        .unwrap();
    assert_eq!(
        policy.select_network_traffic(&ipv4_with_dscp(0), true),
        Err(WifiTxTrafficError::AdmissionControlRequired {
            requested: WmmAccessCategory::BestEffort
        })
    );
}

#[test]
fn negotiated_txop_is_bounded_and_medium_ownership_stays_unsupported() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    policy
        .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
        .unwrap();
    let video = policy
        .select_network_traffic(&ipv4_with_dscp(40), true)
        .unwrap();
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
        Ok(HeEdcaTxopLimit::from_units_32_us(94).unwrap())
    );
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
        Ok(HeEdcaTxopLimit::from_units_32_us(47).unwrap())
    );
    assert_eq!(
        video.require_ht_txop_support(),
        Err(WmmTxopUnsupported::HtAggregateDurationBudget { units_32_us: 94 })
    );
    assert_eq!(
        policy.request_hardware_medium(WmmHardwareMediumRequest::RtsCtsProtection),
        Err(WmmTxopUnsupported::RtsCtsProtection)
    );
    assert_eq!(
        policy.request_hardware_medium(WmmHardwareMediumRequest::MultiPpduTxop),
        Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
    );

    let mut wide = STANDARD_WMM;
    wide[21] = 1;
    policy
        .install_wmm(parse_wmm_parameter_element(&wide).unwrap())
        .unwrap();
    let video = policy
        .select_network_traffic(&ipv4_with_dscp(40), true)
        .unwrap();
    assert_eq!(
        video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
        Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us: 350 })
    );
}

#[test]
fn ordinary_retry_owns_rate_ladder_and_edca_transitions() {
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    let queue = LegacyTxQueue::BestEffort;
    let mut retry = OrdinaryMpduRetryState::new(
        queue,
        TxPhyRate::Legacy(LegacyRate::Ofdm54M),
        4,
        OrdinaryFrameClass::Short,
    )
    .unwrap();

    assert_eq!(
        retry.current_rate(),
        Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
    );
    assert_eq!(
        retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
        OrdinaryRetryDecision::Retry {
            set_retry_bit: true
        }
    );
    assert_eq!(retry.publications(), 2);
    assert_eq!(
        retry.counters(),
        OrdinaryRetryCounters {
            mpdu: 1,
            short: 1,
            long: 0
        }
    );
    assert_eq!(policy.contention_exponent(queue), 5);
    assert_eq!(
        retry.current_rate(),
        Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
    );

    assert_eq!(
        retry.observe_completion(&mut policy, TxCompletionDisposition::CtsTimeout),
        OrdinaryRetryDecision::Retry {
            set_retry_bit: false
        }
    );
    assert_eq!(retry.publications(), 3);
    assert_eq!(
        retry.counters(),
        OrdinaryRetryCounters {
            mpdu: 1,
            short: 2,
            long: 0
        }
    );
    assert_eq!(policy.contention_exponent(queue), 6);
    assert_eq!(
        retry.current_rate(),
        Ok(TxPhyRate::Legacy(LegacyRate::Ofdm48M))
    );

    assert_eq!(
        retry.observe_completion(&mut policy, TxCompletionDisposition::Success),
        OrdinaryRetryDecision::Complete
    );
    assert_eq!(policy.contention_exponent(queue), 4);
}

#[test]
fn ordinary_retry_limit_collision_and_abort_restore_the_minimum_cw() {
    let queue = LegacyTxQueue::Voice;
    let mut policy = WifiTxRuntimePolicy::vendor_defaults();
    let mut retry = OrdinaryMpduRetryState::new(
        queue,
        TxPhyRate::Legacy(LegacyRate::Ofdm6M),
        2,
        OrdinaryFrameClass::Long,
    )
    .unwrap();
    assert_eq!(
        retry.observe_collision(&mut policy),
        OrdinaryRetryDecision::Retry {
            set_retry_bit: false
        }
    );
    assert_eq!(retry.publications(), 2);
    assert_eq!(retry.counters().long, 1);
    assert_eq!(retry.counters().mpdu, 0);
    assert_eq!(retry.counters().short, 0);
    assert_eq!(
        retry.current_rate(),
        Ok(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(policy.contention_exponent(queue), 3);
    assert_eq!(
        retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
        OrdinaryRetryDecision::Retry {
            set_retry_bit: true
        }
    );
    assert_eq!(retry.counters().mpdu, 1);
    assert_eq!(retry.counters().long, 2);

    retry.abort(&mut policy);
    assert_eq!(policy.contention_exponent(queue), 2);
    assert_eq!(
        OrdinaryMpduRetryState::new(
            queue,
            TxPhyRate::Legacy(LegacyRate::Ofdm6M),
            0,
            OrdinaryFrameClass::Short,
        )
        .err(),
        Some(OrdinaryRetryError::ZeroMpduRetryLimit)
    );
}

#[test]
fn p2p_ht20_sgi_mcs0_through_mcs6_enter_same_mcs_lgi_retry_records() {
    for mcs_index in 0..=6 {
        let mcs = HtMcs::from_index(mcs_index).unwrap();
        let initial = TxPhyRate::Ht(HtRate::new(
            mcs,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        ));
        let schedule = RateScheduleRef::new(
            RateScheduleKind::P2pDot11N,
            8_u8.checked_sub(mcs_index).unwrap(),
        )
        .unwrap();
        let schedule = P2pRetryRateSchedule::new(schedule).unwrap();
        let mut retry = OrdinaryMpduRetryState::new_with_rate_policy(
            LegacyTxQueue::Voice,
            initial,
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule),
            4,
            OrdinaryFrameClass::Short,
        )
        .unwrap();
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();

        assert_eq!(retry.current_rate(), Ok(initial));
        assert_eq!(
            retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
            OrdinaryRetryDecision::Retry {
                set_retry_bit: true
            }
        );
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Ht(HtRate::new(
                mcs,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz20,
            )))
        );
    }
}

#[test]
fn p2p_ht_sgi_bridge_rejects_a_different_mcs_retry_record() {
    let initial = TxPhyRate::Ht(HtRate::new(
        HtMcs::Mcs4,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz20,
    ));
    let wrong =
        P2pRetryRateSchedule::new(RateScheduleRef::new(RateScheduleKind::P2pDot11N, 3).unwrap())
            .unwrap();
    assert_eq!(
        OrdinaryMpduRetryState::new_with_rate_policy(
            LegacyTxQueue::Voice,
            initial,
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(wrong),
            4,
            OrdinaryFrameClass::Short,
        )
        .err(),
        Some(OrdinaryRetryError::P2pHtSgiFallbackMismatch {
            initial,
            scheduled: TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs5,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz20,
            )),
        })
    );
}

fn completion_with_tx(
    tx: TxCompletion,
    starting_sequence: u16,
    bitmap: u64,
) -> HtAmpduTxCompletion {
    HtAmpduTxCompletion {
        tx,
        block_ack: HtBlockAckObservation::new(0, starting_sequence, bitmap),
        block_ack_received: true,
    }
}

fn completion(status: u8, starting_sequence: u16, bitmap: u64) -> HtAmpduTxCompletion {
    completion_with_tx(
        TxCompletion::new_model(TxCookie(1), status, 0),
        starting_sequence,
        bitmap,
    )
}

const HT_POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
    attempt_limit: 4,
    retain_single_mpdu: false,
};

#[test]
fn partial_block_ack_compacts_sequences_across_retained_attempts() {
    let mut state = AmpduRetryState::<32>::new(0x0ffe, 4, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(0, 0x0ffe, 0b0101), 4),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 })
    );
    assert_eq!(state.current_subframes(), 2);
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.block_ack_mpdu_attempts(), 4);

    // The retained sequences are 0x0fff and 1. A BlockAck starting at
    // 0x0fff acknowledges both across the 12-bit wrap.
    assert_eq!(
        state.observe(completion(0, 0x0fff, 0b0101), 2),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 4);
    assert_eq!(state.aggregate_attempts(), 2);
    assert_eq!(state.block_ack_mpdu_attempts(), 6);
}

#[test]
fn advanced_block_ack_ssn_completes_preceding_mpdu_without_retry() {
    let mut state = AmpduRetryState::<4>::new(100, 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(0, 102, 0), 2),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 2);
    assert_eq!(state.aggregate_attempts(), 1);
}

#[test]
fn ack_timeout_completion_applies_a_received_block_ack_bitmap() {
    let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(5, 20, u64::MAX), 2),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
    );
    assert_eq!(state.acknowledged(), 2);
}

#[test]
fn missing_block_ack_result_ignores_a_stale_success_bitmap() {
    let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
    let mut stale = completion(5, 20, u64::MAX);
    stale.block_ack_received = false;
    assert_eq!(
        state.observe(stale, 2),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
    );
    assert_eq!(state.acknowledged(), 0);
}

#[test]
fn ht_finishes_one_missing_mpdu_but_he_retains_it() {
    let mut ht = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
    assert_eq!(
        ht.observe(completion(0, 20, 0b01), 2),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b10 })
    );

    let mut he = AmpduRetryState::<4>::new(
        20,
        2,
        AmpduRetryPolicy {
            retain_single_mpdu: true,
            ..HT_POLICY
        },
    )
    .unwrap();
    assert_eq!(
        he.observe(completion(0, 20, 0b01), 2),
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
    );
    assert_eq!(he.current_subframes(), 1);
}

#[test]
fn attempt_limit_finishes_the_last_failed_aggregate() {
    let mut state = AmpduRetryState::<4>::new(
        100,
        2,
        AmpduRetryPolicy {
            attempt_limit: 2,
            retain_single_mpdu: false,
        },
    )
    .unwrap();
    assert!(matches!(
        state.observe(completion(5, 100, 0), 2),
        Ok(AmpduRetryDecision::RetainAggregate { .. })
    ));
    assert_eq!(
        state.observe(completion(5, 100, 0), 2),
        Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
    );
    assert_eq!(state.aggregate_attempts(), 2);
}

#[test]
fn construction_and_dma_count_disagreements_fail_closed() {
    assert!(matches!(
        AmpduRetryState::<33>::new(0, 1, HT_POLICY),
        Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: 33 })
    ));
    assert!(matches!(
        AmpduRetryState::<2>::new(0, 3, HT_POLICY),
        Err(AmpduRetryError::AggregateExceedsCapacity { .. })
    ));
    let mut state = AmpduRetryState::<2>::new(0, 2, HT_POLICY).unwrap();
    assert_eq!(
        state.observe(completion(0, 0, 0b11), 1),
        Err(AmpduRetryError::FrameCountChanged {
            expected: 2,
            observed: 1
        })
    );
}

#[test]
fn vendor_trigger_timeout_finishes_without_fabricating_block_ack() {
    let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
    let trigger_timeout = completion_with_tx(
        TxCompletion::new_model(TxCookie(1), TxCompletion::ACK_TIMEOUT_STATUS, 0)
            .with_trigger_flow_model(true),
        20,
        u64::MAX,
    );

    assert_eq!(
        state.observe(trigger_timeout, 2),
        Ok(AmpduRetryDecision::FinishTriggerFlow)
    );
    assert_eq!(state.trigger_flow_completions(), 1);
    assert_eq!(state.acknowledged(), 0);
    assert_eq!(state.block_ack_mpdu_attempts(), 0);
    assert_eq!(state.aggregate_attempts(), 1);
}

#[test]
fn trigger_timeout_with_reported_packets_stays_on_retry_path() {
    for (primary, last_tx_was_trigger_based, secondary) in
        [(1, false, 0), (0, true, 1), (1, true, 0)]
    {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        let completion = completion_with_tx(
            TxCompletion::new_model(TxCookie(1), TxCompletion::ACK_TIMEOUT_STATUS, 0)
                .with_trigger_flow_model(true)
                .with_trigger_packet_counts_model(primary, last_tx_was_trigger_based, secondary),
            20,
            u64::MAX,
        );
        assert_eq!(
            state.observe(completion, 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert_eq!(state.trigger_flow_completions(), 0);
        assert_eq!(state.block_ack_mpdu_attempts(), 2);
    }
}

#[test]
fn trigger_success_predicate_rejects_wrong_status_or_queue_state() {
    for (status, trigger_flow) in [(0, true), (4, true), (5, false)] {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        let completion = completion_with_tx(
            TxCompletion::new_model(TxCookie(1), status, 0).with_trigger_flow_model(trigger_flow),
            20,
            0,
        );
        assert_ne!(
            state.observe(completion, 2),
            Ok(AmpduRetryDecision::FinishTriggerFlow)
        );
    }
}
