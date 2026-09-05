use super::*;
use crate::rate::schedule::{RateScheduleKind, RateScheduleRef};

const HE20_POLICY: StaTxRatePolicy = StaTxRatePolicy {
    association_phy: StaAssociationPhy::He20,
    high_throughput_enabled: true,
    fallback_legacy_rate: LegacyRate::Ofdm54M,
    fallback_ht_mcs: HtMcs::Mcs7,
    fallback_ht_guard_interval: HtGuardInterval::Long800Ns,
    ht_mcs_override: None,
    ht_guard_interval_override: None,
    he_mcs_override: None,
    he_guard_interval_and_ltf_override: None,
    he_dcm_override: None,
    he_800ns_gi_ltf: HeGuardIntervalAndLtf::TwoLtf800Ns,
    peer_supports_ht_short_guard_interval: false,
    peer_supports_ldpc: true,
    peer_dcm_receive: HeDcmConstellation::Bpsk,
};

fn state() -> RateControlState {
    RateControlState {
        retry_pressure: 4,
        weighted_retries: 10,
        transmissions: 20,
        completed: 30,
        reevaluate_after_us: 1,
        retry_state_1d: 2,
        retry_state_1e: 3,
        maximum_schedule_index: 5,
        current_schedule: RateScheduleState {
            reference: RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap(),
            retry_limit: 7,
            adaptive: 1,
        },
        legacy_schedule: RateScheduleRef::new(RateScheduleKind::Dot11B, 0).unwrap(),
    }
}

#[test]
fn ack_snr_filter_matches_signed_blob_rounding() {
    let mut filter = AckSnrFilter::new();
    assert_eq!(filter.latest(), None);
    assert_eq!(filter.filtered(), None);

    filter.update(AckSnrFilter::UNINITIALIZED);
    assert_eq!(filter, AckSnrFilter::new());

    // The first valid sample is retained, while the first midpoint is
    // exactly zero because the preceding sample was the sentinel.
    filter.update(-21);
    assert_eq!(filter.latest(), Some(-21));
    assert_eq!(filter.filtered(), Some(0));

    // (-21 + -22) >> 1 is -22 (arithmetic shift), then
    // (3 * 0 + -22) / 4 is -5 (signed division toward zero).
    filter.update(-22);
    assert_eq!(filter.latest(), Some(-22));
    assert_eq!(filter.filtered(), Some(-5));

    // (-22 + 19) >> 1 is -2; (-15 + -2) / 4 is -4.
    filter.update(19);
    assert_eq!(filter.latest(), Some(19));
    assert_eq!(filter.filtered(), Some(-4));
}

#[test]
fn association_installs_selected_schedule_as_owned_runtime_state() {
    let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(70),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.current_schedule(),
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 1).unwrap()
    );
    assert_eq!(
        association.runtime.current_schedule.retry_limit,
        schedule_state(association.current_schedule()).retry_limit
    );

    association.runtime.retry_pressure = 6;
    let update = association.update_tx_per(5);
    assert_eq!(
        update.schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap())
    );
    assert_eq!(
        association.current_schedule(),
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 2).unwrap()
    );
}

#[test]
fn retry_bands_match_the_pinned_transition() {
    let mut low = state();
    assert_eq!(low.update_tx_per(2).schedule, ScheduleSelection::Unchanged);
    assert_eq!(low.retry_pressure, 0);
    assert_eq!(low.transmissions, 21);
    assert_eq!(low.weighted_retries, 13);

    let mut middle = state();
    middle.update_tx_per(4);
    assert_eq!(middle.retry_pressure, 4);

    let mut high = state();
    high.update_tx_per(6);
    assert_eq!(high.retry_pressure, 5);

    let mut very_high = state();
    very_high.update_tx_per(8);
    assert_eq!(very_high.retry_pressure, 6);
    // retry_limit < retries selects retry_limit + 2, not retries + 1.
    assert_eq!(very_high.weighted_retries, 19);
}

#[test]
fn large_counters_are_rescaled_before_accumulation() {
    let mut value = state();
    value.transmissions = 0x01ff_ffff;
    value.weighted_retries = 100;
    value.update_tx_per(0);
    assert_eq!(value.transmissions, 0x0100_0000);
    assert_eq!(value.weighted_retries, 51);
}

#[test]
fn pressure_threshold_clears_state_and_advances_schedule() {
    let mut value = state();
    value.retry_pressure = 6;
    let update = value.update_tx_per(5);
    assert_eq!(
        update.schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11N, 3).unwrap())
    );
    assert_eq!(value.retry_pressure, 0);
    assert_eq!(value.weighted_retries, 0);
    assert_eq!(value.transmissions, 0);
    assert_eq!(value.completed, 0);
    assert_eq!(value.reevaluate_after_us, 500_000);
    assert_eq!(value.retry_state_1d, 0);
    assert_eq!(value.retry_state_1e, 0);
    assert_eq!(value.current_schedule.adaptive, 0);
}

#[test]
fn last_schedule_falls_back_to_the_legacy_table() {
    let mut value = state();
    value.retry_pressure = 6;
    value.maximum_schedule_index = 2;
    value.current_schedule.reference = RateScheduleRef::new(RateScheduleKind::Dot11N, 2).unwrap();
    assert_eq!(
        value.update_tx_per(5).schedule,
        ScheduleSelection::Selected(RateScheduleRef::new(RateScheduleKind::Dot11B, 2).unwrap())
    );
}

#[test]
fn invalid_schedule_transition_is_explicit() {
    let mut value = state();
    value.retry_pressure = 6;
    value.maximum_schedule_index = 6;
    value.current_schedule.reference = RateScheduleRef::new(RateScheduleKind::Dot11N, 13).unwrap();
    assert_eq!(value.update_tx_per(5).schedule, ScheduleSelection::Invalid);
}

#[test]
fn byte_pressure_wrap_is_preserved() {
    let mut value = state();
    value.retry_pressure = 0xff;
    value.update_tx_per(8);
    assert_eq!(value.retry_pressure, 1);
}

#[test]
fn beamforming_policy_has_three_exact_modes() {
    assert_eq!(
        beamforming_report_rate(40, 20, true, true),
        BeamformingReportRate {
            mode: 1,
            rate: 16,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    );
    assert_eq!(
        beamforming_report_rate(20, 20, true, true),
        BeamformingReportRate {
            mode: 2,
            rate: 16,
            dcm: true,
            ersu: true,
            ersu_ack: true,
        }
    );
    assert_eq!(
        beamforming_report_rate(20, 20, true, false),
        BeamformingReportRate {
            mode: 0,
            rate: 11,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    );
}

fn selection_input(phy_type: u8) -> PhyModeSelectionInput {
    PhyModeSelectionInput {
        phy_type,
        he_type: 0,
        metric: 20,
        p2p: false,
        supplied_highest_rate: 0,
        use_supplied_highest_rate: false,
        long_range_rates_present: false,
    }
}

#[test]
fn highest_rate_tables_match_recovered_boundaries() {
    assert_eq!(highest_rate_index(0, 0, 2, true), 3);
    assert_eq!(highest_rate_index(0, 0, 22, true), 0);
    assert_eq!(highest_rate_index(1, 0, 12, true), 7);
    assert_eq!(highest_rate_index(1, 0, 108, true), 0);
    assert_eq!(highest_rate_index(2, 0, 13, true), 8);
    assert_eq!(highest_rate_index(2, 0, 144, true), 0);
    assert_eq!(highest_rate_index(2, 7, 17, true), 9);
    assert_eq!(highest_rate_index(2, 7, 229, true), 0);
    assert_eq!(highest_rate_index(2, 0, 0, false), 1);
    assert_eq!(highest_rate_index(3, 0, 0, false), 0);
    assert_eq!(highest_rate_index(4, 0, 0, false), 1);
}

#[test]
fn typed_he_peer_maximum_covers_the_complete_mcs0_to_mcs9_table() {
    let expected = [17, 34, 51, 68, 104, 137, 154, 172, 206, 229];
    for (mcs, expected) in expected.into_iter().enumerate() {
        let mcs = HeMcs::from_index(mcs as u8).unwrap();
        assert_eq!(
            StaRateControlPeerHighestRate::he20_one_spatial_stream(mcs).vendor_half_mbps(),
            expected
        );
    }
}

#[test]
fn sta_tx_policy_joins_he_schedule_peer_ltf_and_ldpc() {
    let rate =
        HE20_POLICY.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap());
    assert_eq!(
        rate,
        TxPhyRate::He(HeRate::ldpc(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
        ))
    );
}

#[test]
fn sta_tx_policy_keeps_hil_override_and_unknown_arena_explicit() {
    let ht40 = StaTxRatePolicy {
        association_phy: StaAssociationPhy::Ht40,
        ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
        peer_supports_ht_short_guard_interval: true,
        peer_supports_ldpc: false,
        ..HE20_POLICY
    };
    assert_eq!(
        ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    let fixed_mcs = StaTxRatePolicy {
        ht_mcs_override: Some(HtMcs::Mcs3),
        ..ht40
    };
    assert_eq!(
        fixed_mcs.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs3,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        ht40.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Lora, 0).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        StaTxRatePolicy {
            high_throughput_enabled: false,
            ..HE20_POLICY
        }
        .rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap()),
        TxPhyRate::Legacy(LegacyRate::Ofdm54M)
    );
}

#[test]
fn sta_tx_policy_fixed_ht_matrix_preserves_negotiated_width() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap();
    for association_phy in [StaAssociationPhy::Ht20, StaAssociationPhy::Ht40] {
        let width = match association_phy {
            StaAssociationPhy::Ht20 => HtChannelWidth::Mhz20,
            StaAssociationPhy::Ht40 => HtChannelWidth::Mhz40,
            _ => unreachable!(),
        };
        for mcs_index in 0..=7 {
            let mcs = HtMcs::from_index(mcs_index).unwrap();
            for guard_interval in [HtGuardInterval::Long800Ns, HtGuardInterval::Short400Ns] {
                let policy = StaTxRatePolicy {
                    association_phy,
                    ht_mcs_override: Some(mcs),
                    ht_guard_interval_override: Some(guard_interval),
                    peer_supports_ht_short_guard_interval: true,
                    peer_supports_ldpc: false,
                    ..HE20_POLICY
                };
                assert_eq!(
                    policy.rate_for_schedule(schedule),
                    TxPhyRate::Ht(HtRate::new(mcs, guard_interval, width))
                );
            }
        }
    }
}

#[test]
fn sta_tx_policy_never_publishes_unadvertised_ht_short_gi() {
    let policy = StaTxRatePolicy {
        association_phy: StaAssociationPhy::Ht40,
        fallback_ht_guard_interval: HtGuardInterval::Short400Ns,
        ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
        peer_supports_ht_short_guard_interval: false,
        ..HE20_POLICY
    };
    assert_eq!(
        policy.rate_for_schedule(RateScheduleRef::new(RateScheduleKind::Dot11N, 1).unwrap()),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))
    );
    assert_eq!(
        policy.fallback_rate(),
        TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))
    );
}

#[test]
fn sta_tx_policy_fixed_he_su_matrix_preserves_peer_coding() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
    let gi_ltf_values = [
        HeGuardIntervalAndLtf::OneLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];
    for mcs_index in 0..=9 {
        let mcs = HeMcs::from_index(mcs_index).unwrap();
        for guard_interval_and_ltf in gi_ltf_values {
            let policy = StaTxRatePolicy {
                he_mcs_override: Some(mcs),
                he_guard_interval_and_ltf_override: Some(guard_interval_and_ltf),
                ..HE20_POLICY
            };
            assert_eq!(
                policy.rate_for_schedule(schedule),
                TxPhyRate::He(HeRate::ldpc(mcs, guard_interval_and_ltf))
            );
        }
    }
}

#[test]
fn sta_tx_policy_dcm_override_is_capability_gated_and_preserves_coding() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap();
    let gi = HeGuardIntervalAndLtf::TwoLtf800Ns;
    let bpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs0, gi);
    let bpsk_policy = StaTxRatePolicy {
        he_dcm_override: Some(bpsk),
        peer_dcm_receive: HeDcmConstellation::Bpsk,
        ..HE20_POLICY
    };
    assert!(bpsk_policy.he_dcm_override_is_supported());
    assert_eq!(
        bpsk_policy.rate_for_schedule(schedule),
        TxPhyRate::He(bpsk.rate())
    );
    assert!(!bpsk.rate().is_ldpc());

    let qpsk = HeDcmRate::bcc(crate::tx::HeBccDcmMcs::Mcs1, gi);
    let unsupported_qpsk = StaTxRatePolicy {
        he_dcm_override: Some(qpsk),
        peer_dcm_receive: HeDcmConstellation::Bpsk,
        ..HE20_POLICY
    };
    assert!(!unsupported_qpsk.he_dcm_override_is_supported());
    let TxPhyRate::He(fallback) = unsupported_qpsk.rate_for_schedule(schedule) else {
        panic!("HE association retains the ordinary HE schedule");
    };
    assert!(!fallback.is_dcm());

    let supported_qpsk = StaTxRatePolicy {
        peer_dcm_receive: HeDcmConstellation::Qpsk,
        ..unsupported_qpsk
    };
    assert_eq!(
        supported_qpsk.rate_for_schedule(schedule),
        TxPhyRate::He(qpsk.rate())
    );

    let ldpc_16qam = HeDcmRate::ldpc(crate::tx::HeLdpcDcmMcs::Mcs4, gi);
    let no_ldpc = StaTxRatePolicy {
        he_dcm_override: Some(ldpc_16qam),
        peer_dcm_receive: HeDcmConstellation::Qam16,
        peer_supports_ldpc: false,
        ..HE20_POLICY
    };
    assert!(!no_ldpc.he_dcm_override_is_supported());
    let with_ldpc = StaTxRatePolicy {
        peer_supports_ldpc: true,
        ..no_ldpc
    };
    assert!(with_ldpc.he_dcm_override_is_supported());
    assert_eq!(
        with_ldpc.rate_for_schedule(schedule),
        TxPhyRate::He(ldpc_16qam.rate())
    );
}

#[test]
fn association_owns_ordinary_ampdu_and_completion_rate_transitions() {
    let mut association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.tx_rate(HE20_POLICY),
        HE20_POLICY.rate_for_schedule(association.current_schedule())
    );
    assert_eq!(
        association.ampdu_tx_rate(HE20_POLICY),
        HE20_POLICY.rate_for_schedule(association.current_ampdu_schedule().unwrap())
    );
    association.observe_tx_completion(
        TxCompletion::new_model(crate::tx::TxCookie(1), 0, 0).with_ack_snr_encoded_model(0xeb),
    );
    assert_eq!(association.latest_ack_snr(), Some(75));
}

#[test]
fn ampdu_thresholds_match_the_he_mcs9_oracle_endpoints() {
    assert_eq!(ampdu_rssi_margin(0x19, 75), 32);
    assert_eq!(ampdu_up_threshold(0x19, 75), 89);
    assert_eq!(ampdu_down_threshold(0x19, 75), 82);
    assert_eq!(
        ampdu_rssi_margin(0x23, AckSnrFilter::UNINITIALIZED as u8),
        0
    );
    assert_eq!(
        ampdu_up_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
        121
    );
    assert_eq!(
        ampdu_down_threshold(0x23, AckSnrFilter::UNINITIALIZED as u8),
        114
    );
}

#[test]
fn ampdu_owner_promotes_after_two_clean_vendor_windows() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 1, 0).unwrap();
    assert_eq!(
        state.current_schedule(),
        schedule(RateScheduleKind::Dot11Ax, 1)
    );
    assert_eq!(
        state.observe_block_ack(600_000, 500, 500, Some(75)),
        Ok(AmpduRateDecision::Retain {
            raw_success_ratio: 128,
            filtered_success_ratio: 110,
        })
    );
    assert_eq!(
        state.observe_block_ack(700_001, 500, 500, Some(75)),
        Ok(AmpduRateDecision::Promote {
            from: schedule(RateScheduleKind::Dot11Ax, 1),
            to: schedule(RateScheduleKind::Dot11Ax, 0),
            raw_success_ratio: 128,
            filtered_success_ratio: 114,
        })
    );
    assert_eq!(
        state.current_schedule(),
        schedule(RateScheduleKind::Dot11Ax, 0)
    );
    assert_eq!(state.filtered_success_ratio(), None);
}

#[test]
fn ampdu_owner_lowers_only_after_two_filtered_bad_windows() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
    assert!(matches!(
        state.observe_block_ack(100_001, 500, 0, Some(75)),
        Ok(AmpduRateDecision::Retain {
            filtered_success_ratio: 93,
            ..
        })
    ));
    assert!(matches!(
        state.observe_block_ack(200_002, 500, 0, Some(75)),
        Ok(AmpduRateDecision::Retain {
            filtered_success_ratio: 69,
            ..
        })
    ));
    assert_eq!(
        state.observe_block_ack(300_003, 500, 0, Some(75)),
        Ok(AmpduRateDecision::Lower {
            from: schedule(RateScheduleKind::Dot11Ax, 0),
            to: schedule(RateScheduleKind::Dot11Ax, 1),
            raw_success_ratio: 0,
            filtered_success_ratio: 51,
        })
    );
}

#[test]
fn ampdu_owner_accumulates_and_rejects_impossible_block_ack_counts() {
    let mut state = AmpduRateControlState::new(RateScheduleKind::Dot11Ax, 0, 0).unwrap();
    assert_eq!(
        state.observe_block_ack(10, 16, 16, None),
        Ok(AmpduRateDecision::Accumulating)
    );
    assert_eq!(
        state.observe_block_ack(11, 0, 0, None),
        Err(AmpduRateObservationError::NoAttemptedMpdu)
    );
    assert_eq!(
        state.observe_block_ack(12, 15, 16, None),
        Err(AmpduRateObservationError::AcknowledgedExceedsAttempted)
    );
    assert_eq!(vendor_duration(4, u32::MAX - 5), 9);
}

#[test]
fn ampdu_initial_rate_uses_the_vendor_index_eight_floor() {
    let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    });
    assert_eq!(
        association.current_schedule(),
        schedule(RateScheduleKind::Dot11Ax, 13)
    );
    assert_eq!(
        association.current_ampdu_schedule(),
        Some(schedule(RateScheduleKind::Dot11Ax, 8))
    );
}

#[test]
fn phy_mode_selector_covers_legacy_p2p_ht_he_and_lora() {
    let dot11b = select_phy_mode(selection_input(0));
    assert_eq!(dot11b.current, schedule(RateScheduleKind::Dot11B, 3));
    assert_eq!(dot11b.maximum_index, 3);
    assert_eq!(dot11b.schedule_count, 6);

    let mut dot11g_input = selection_input(1);
    dot11g_input.metric = 12;
    dot11g_input.p2p = true;
    let dot11g = select_phy_mode(dot11g_input);
    assert_eq!(dot11g.current, schedule(RateScheduleKind::P2pDot11G, 5));
    assert_eq!(dot11g.secondary, schedule(RateScheduleKind::P2pDot11G, 7));
    assert_eq!(dot11g.maximum_index, 7);

    let mut ht_input = selection_input(2);
    ht_input.metric = 8;
    let ht = select_phy_mode(ht_input);
    assert_eq!(ht.current, schedule(RateScheduleKind::Dot11N, 11));
    assert_eq!(ht.ampdu_limit_rate, Some(0x10));

    let mut he_input = selection_input(4);
    he_input.he_type = 7;
    he_input.metric = 8;
    he_input.long_range_rates_present = true;
    let he = select_phy_mode(he_input);
    assert_eq!(he.current, schedule(RateScheduleKind::Dot11Ax, 13));
    assert_eq!(he.maximum_index, 15);
    assert_eq!(he.ampdu_limit_rate, Some(0x12));
    assert_eq!(he.fallback, schedule(RateScheduleKind::Lora, 1));

    let lora = select_phy_mode(selection_input(6));
    assert_eq!(lora.current, schedule(RateScheduleKind::Lora, 0));
    assert_eq!(lora.schedule_count, 2);
}

#[test]
fn associated_he_owner_joins_schedule_and_report_rate_without_c_layout() {
    let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(20),
        p2p: false,
        peer_highest_rate: None,
        long_range_rates_present: true,
        he_low_metric_report: HeLowMetricReportFeatures {
            dcm_receive_supported: true,
            extended_range_single_user_permitted: true,
        },
    });

    assert_eq!(
        association.current_schedule(),
        schedule(RateScheduleKind::Dot11Ax, 7)
    );
    assert_eq!(
        association.fallback_schedule(),
        schedule(RateScheduleKind::Lora, 1)
    );
    assert_eq!(association.maximum_schedule_index(), 15);
    assert_eq!(association.schedule_count(), 16);
    assert_eq!(association.ampdu_limit_rate(), Some(0x13));
    assert_eq!(
        association.beamforming_report(),
        beamforming_report_rate_for_metric(20, true, true)
    );
}

#[test]
fn associated_he_owner_keeps_low_metric_feature_gates_explicit() {
    let mut input = StaRateControlAssociationInput {
        phy: StaRateControlPhy::He,
        link_metric: StaLinkMetric::from_estimator(8),
        p2p: false,
        peer_highest_rate: Some(StaRateControlPeerHighestRate::he20_one_spatial_stream(
            HeMcs::Mcs9,
        )),
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    };
    let ordinary = StaRateControlAssociation::new(input);
    assert_eq!(
        ordinary.current_schedule(),
        schedule(RateScheduleKind::Dot11Ax, 0)
    );
    assert_eq!(
        ordinary.beamforming_report(),
        beamforming_report_rate_for_metric(8, false, false)
    );

    input.he_low_metric_report = HeLowMetricReportFeatures {
        dcm_receive_supported: true,
        extended_range_single_user_permitted: true,
    };
    assert_eq!(
        StaRateControlAssociation::new(input).beamforming_report(),
        beamforming_report_rate_for_metric(8, true, true)
    );
}

#[test]
fn sta_link_metric_preserves_the_blob_signed_byte_subtraction() {
    assert_eq!(
        StaLinkMetric::from_rssi_and_noise_floor(-30, -96).value(),
        66
    );
    assert_eq!(
        StaLinkMetric::from_rssi_and_noise_floor(100, -100).value(),
        -56
    );
}

#[test]
fn recovered_rate_callbacks_and_ampdu_table_are_finite() {
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 0), 3);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11B, 42), 4);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 15), 6);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x21), 0);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x29), 13);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x23), 0);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x2a), 14);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Lora, 0x28), 0xff);
}
