use super::*;
use crate::rate_schedule::{RateScheduleKind, RateScheduleRef};

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
        let mcs = HeMcs::new(mcs as u8).unwrap();
        assert_eq!(
            StaRateControlPeerHighestRate::he20_one_spatial_stream(mcs).vendor_half_mbps(),
            expected
        );
    }
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
            HeMcs::new(9).unwrap(),
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
    // Long-range codes select the 802.11g arena's long-range records.
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 0x29), 12);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11G, 0x2a), 11);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x21), 0);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11N, 0x29), 13);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x23), 0);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Dot11Ax, 0x2a), 14);
    assert_eq!(rate_to_schedule_index(RateIndexMap::Lora, 0x28), 0xff);
}

fn association(phy: StaRateControlPhy, p2p: bool) -> StaRateControlAssociation {
    StaRateControlAssociation::new(StaRateControlAssociationInput {
        phy,
        link_metric: StaLinkMetric::from_estimator(70),
        p2p,
        peer_highest_rate: None,
        long_range_rates_present: false,
        he_low_metric_report: HeLowMetricReportFeatures::default(),
    })
}

#[test]
fn non_data_frames_use_the_association_control_schedule() {
    let basic_ofdm = RateScheduleRef::new(RateScheduleKind::BasicOfdm, 0).unwrap();
    for phy in [
        StaRateControlPhy::Dot11G,
        StaRateControlPhy::Ht,
        StaRateControlPhy::He,
    ] {
        assert_eq!(association(phy, false).control_schedule(), basic_ofdm);
    }
    assert_eq!(
        association(StaRateControlPhy::Dot11B, false).control_schedule(),
        DEFAULT_CONTROL_SCHEDULE
    );
    assert_eq!(
        association(StaRateControlPhy::Dot11G, true).control_schedule(),
        RateScheduleRef::new(RateScheduleKind::P2pDot11G, 7).unwrap()
    );
    assert_eq!(
        association(StaRateControlPhy::He, true).control_schedule(),
        DEFAULT_CONTROL_SCHEDULE
    );
}

mod seam {
    use oer_ieee80211_mac::{
        ht::ht_peer_capabilities,
        phy::{HtMcs, HtRate, PhyRate, PpduBandwidth},
        station::association::PhyMode,
    };
    use oer_ieee80211_sta::{association::StaAssociatedPeer, rate_control::StaRateControl};
    use oer_ieee80211_upper_mac::BssProtection;

    use super::super::{EspressifRateControl, RateScheduleKind, RateScheduleRef, schedule_state};

    /// An HT20 access point, with or without the 20 MHz short guard
    /// interval.
    fn ht20_peer(short_gi: bool) -> StaAssociatedPeer {
        let mut element = [0_u8; 28];
        element[..6].copy_from_slice(&[45, 26, if short_gi { 0x20 } else { 0 }, 0, 0x17, 0xff]);
        StaAssociatedPeer {
            phy: PhyMode::Ht20,
            ht_capabilities: ht_peer_capabilities(&element),
            ht_ampdu_parameters: 0x17,
            he_capabilities: None,
            he_peer_state: None,
            he_bss_color: 0,
            protection: BssProtection::UNPROTECTED,
        }
    }

    #[test]
    fn an_unknown_link_metric_starts_at_the_weakest_link_s_rate() {
        let peer = ht20_peer(true);
        let unknown = EspressifRateControl::associate((), &peer, None);
        let weakest = EspressifRateControl::associate((), &peer, Some(i8::MIN));
        let strong = EspressifRateControl::associate((), &peer, Some(60));
        assert_eq!(unknown.mpdu_rate(), weakest.mpdu_rate());
        assert!(unknown.mpdu_rate().nominal_kbps() < strong.mpdu_rate().nominal_kbps());
    }

    #[test]
    fn a_short_guard_interval_is_used_only_where_the_peer_supports_it() {
        let with = EspressifRateControl::associate((), &ht20_peer(true), Some(60)).decode;
        let without = EspressifRateControl::associate((), &ht20_peer(false), Some(60)).decode;
        let mut short = 0;
        for index in 0..14 {
            let Some(schedule) = RateScheduleRef::new(RateScheduleKind::Dot11N, index) else {
                continue;
            };
            let raw = crate::rate_code::phy_rate(
                schedule.kind,
                schedule_state(schedule).rate,
                PpduBandwidth::Mhz20,
                oer_ieee80211_mac::phy::HeGiLtf::Ltf2xGi800Ns,
            );
            if let Some(raw @ PhyRate::Ht(rate)) = raw {
                assert_eq!(with.rate(schedule), raw);
                let PhyRate::Ht(qualified) = without.rate(schedule) else {
                    panic!("an HT schedule decodes to HT");
                };
                assert!(!qualified.short_gi());
                assert_eq!(qualified.mcs(), rate.mcs());
                short += usize::from(rate.short_gi());
            }
        }
        // The 802.11n schedules do name short-GI rates.
        assert!(short > 0);
        let _ = (HtMcs::new(0), HtRate::new);
    }

    #[test]
    fn retry_pressure_lowers_the_rate() {
        let mut control = EspressifRateControl::associate((), &ht20_peer(true), Some(60));
        let before = control.mpdu_rate().nominal_kbps();
        // Exchanges of ten attempts each add two to the retry pressure; past
        // six the controller moves to the next, slower schedule.
        for _ in 0..4 {
            control.observe_mpdu(10, false, None);
        }
        assert!(control.mpdu_rate().nominal_kbps() < before);
    }
}
