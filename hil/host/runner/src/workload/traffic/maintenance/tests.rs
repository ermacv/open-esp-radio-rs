use super::validate_pause;
use open_esp_radio_hil_protocol::{
    StationPauseEvidence, StationPauseOperation, StationPauseResult, StationPhyTrackingEvidence,
};

fn parent_timing(common: bool, wifi: bool) -> open_esp_radio_hil_protocol::PhyTimingEvidence {
    use open_esp_radio_hil_protocol::{PhyOperationTiming, PhyTimingEvidence};
    let complete = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 1,
        maximum_micros: 1,
        ..Default::default()
    };
    let absent = PhyOperationTiming::default();
    PhyTimingEvidence {
        wifi_i2c: complete,
        wifi_power: complete,
        calibration: complete,
        temperature: complete,
        dcode: if common { complete } else { absent },
        rx_gain: if common { complete } else { absent },
        channel_restore: if common { complete } else { absent },
        tx_dc_pwdet: if wifi { complete } else { absent },
        tx_gain_publication: if wifi { complete } else { absent },
        dcode_polls: open_esp_radio_hil_protocol::PhyPollTiming {
            polls: u32::from(common),
            ..Default::default()
        },
        rx_gain_polls: open_esp_radio_hil_protocol::PhyPollTiming {
            polls: u32::from(common),
            ..Default::default()
        },
        tx_dc_pwdet_polls: open_esp_radio_hil_protocol::PhyPollTiming {
            polls: u32::from(wifi),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn calibration_requires_success_and_both_committed_branches() {
    for result in [StationPauseResult::Resumed, StationPauseResult::PhyTracking] {
        for inhibited in [false, true] {
            for common_calibrated in [false, true] {
                for wifi_calibrated in [false, true] {
                    let evidence = StationPauseEvidence {
                        timings: Some(parent_timing(common_calibrated, wifi_calibrated)),
                        result,
                        elapsed_micros: 1,
                        tracking: Some(StationPhyTrackingEvidence {
                            inhibited,
                            common_calibrated,
                            wifi_calibrated,
                            bluetooth_ieee802154_calibrated: false,
                        }),
                    };
                    assert_eq!(
                        validate_pause(StationPauseOperation::Calibration, evidence).is_ok(),
                        result == StationPauseResult::Resumed
                            && !inhibited
                            && common_calibrated
                            && wifi_calibrated
                    );
                    assert_eq!(
                        validate_pause(StationPauseOperation::Tracking, evidence).is_ok(),
                        result == StationPauseResult::Resumed && !inhibited
                    );
                }
            }
        }
    }
}

#[test]
fn parent_rejects_missing_duplicated_or_unrelated_children_and_false_commit_flags() {
    let evidence = StationPauseEvidence {
        result: StationPauseResult::Resumed,
        elapsed_micros: 20,
        tracking: Some(StationPhyTrackingEvidence {
            inhibited: false,
            common_calibrated: true,
            wifi_calibrated: true,
            bluetooth_ieee802154_calibrated: false,
        }),
        timings: Some(parent_timing(true, true)),
    };
    for operation in [
        StationPauseOperation::Tracking,
        StationPauseOperation::Calibration,
    ] {
        assert!(validate_pause(operation, evidence).is_ok());
        assert!(
            validate_pause(
                operation,
                StationPauseEvidence {
                    timings: None,
                    ..evidence
                }
            )
            .is_err()
        );
        for mutation in 0..7 {
            let mut changed = evidence;
            let timing = changed.timings.as_mut().unwrap();
            match mutation {
                0 => timing.temperature = Default::default(),
                1 => {
                    timing.wifi_power.started = 2;
                    timing.wifi_power.completed = 2;
                }
                2 => timing.rfpll = timing.wifi_power,
                3 => timing.bluetooth_ieee802154_power = timing.wifi_power,
                4 => timing.channel_restore = Default::default(),
                5 => timing.tx_gain_publication = Default::default(),
                6 => changed.tracking.as_mut().unwrap().common_calibrated = false,
                _ => unreachable!(),
            }
            assert!(
                validate_pause(operation, changed).is_err(),
                "accepted mutation {mutation}"
            );
        }
    }
}

#[test]
fn access_alone_accepts_no_tracking_work() {
    let evidence = StationPauseEvidence {
        timings: None,
        result: StationPauseResult::Resumed,
        elapsed_micros: 1,
        tracking: None,
    };
    assert!(validate_pause(StationPauseOperation::Access, evidence).is_ok());
    assert!(validate_pause(StationPauseOperation::Tracking, evidence).is_err());
    assert!(validate_pause(StationPauseOperation::Calibration, evidence).is_err());
}

#[test]
fn resumed_pause_rejects_incomplete_failed_or_invalid_timing() {
    for (invalid, started, completed, failed) in
        [(true, 1, 1, 0), (false, 1, 0, 0), (false, 1, 0, 1)]
    {
        let mut timings = open_esp_radio_hil_protocol::PhyTimingEvidence {
            invalid,
            ..Default::default()
        };
        timings.dcode = open_esp_radio_hil_protocol::PhyOperationTiming {
            started,
            completed,
            failed,
            ..Default::default()
        };
        let evidence = StationPauseEvidence {
            result: StationPauseResult::Resumed,
            elapsed_micros: 20,
            tracking: None,
            timings: Some(timings),
        };
        assert!(validate_pause(StationPauseOperation::Access, evidence).is_err());
    }
}

#[test]
fn tx_detail_is_required_exactly_when_operation_timing_is_available() {
    use super::validate_tx_waits;
    use open_esp_radio_hil_protocol::{PhyTimingEvidence, PhyTxWaitEvidence};
    assert!(validate_tx_waits(None, None).is_ok());
    assert!(validate_tx_waits(Some(PhyTimingEvidence::default()), None).is_err());
    assert!(validate_tx_waits(None, Some(PhyTxWaitEvidence::default())).is_err());
    assert!(
        validate_tx_waits(
            Some(PhyTimingEvidence::default()),
            Some(PhyTxWaitEvidence::default())
        )
        .is_ok()
    );
    // An access-only pause cannot claim even a status sample from TX calibration.
    assert!(
        validate_tx_waits(
            Some(PhyTimingEvidence::default()),
            Some(PhyTxWaitEvidence {
                sar_ready: 1,
                ..Default::default()
            })
        )
        .is_err()
    );
}

#[test]
fn service_window_requires_repeated_observations_and_rejects_faults_or_unrelated_detail() {
    let report = open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
        operations: [4, 0, 0, 0, 0, 0],
        elapsed_micros: open_esp_radio_hil_protocol::STATION_TRACKING_SERVICE_WINDOW_MICROS,
        pause_micros: 100,
        maximum_pause_micros: 25,
        ..Default::default()
    };
    assert!(super::validate_service(StationPauseOperation::TrackingService, Some(report)).is_ok());
    assert!(super::validate_service(StationPauseOperation::TrackingService, None).is_err());
    for bad in [
        open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
            operations: [1, 0, 0, 0, 0, 0],
            ..report
        },
        open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
            suspended: true,
            ..report
        },
        open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
            failed: true,
            ..report
        },
        open_esp_radio_hil_protocol::StationTrackingServiceEvidence {
            invalid: true,
            ..report
        },
    ] {
        assert!(
            super::validate_service(StationPauseOperation::TrackingService, Some(bad)).is_err()
        );
    }
    assert!(super::validate_service(StationPauseOperation::Temperature, Some(report)).is_err());
}

#[test]
fn rfpll_requires_one_completed_measured_operation_and_no_other_branch() {
    use open_esp_radio_hil_protocol::{PhyOperationTiming, PhyTimingEvidence};
    let timing = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 10,
        maximum_micros: 10,
        ..Default::default()
    };
    let evidence = StationPauseEvidence {
        result: StationPauseResult::Resumed,
        elapsed_micros: 20,
        tracking: Some(StationPhyTrackingEvidence {
            inhibited: false,
            common_calibrated: false,
            wifi_calibrated: false,
            bluetooth_ieee802154_calibrated: false,
        }),
        timings: Some(PhyTimingEvidence {
            rfpll: timing,
            ..Default::default()
        }),
    };
    assert!(validate_pause(StationPauseOperation::Rfpll, evidence).is_ok());
    for timings in [
        None,
        Some(PhyTimingEvidence::default()),
        Some(PhyTimingEvidence {
            rfpll: timing,
            wifi_power: timing,
            ..Default::default()
        }),
        Some(PhyTimingEvidence {
            rfpll: PhyOperationTiming {
                completed: 0,
                ..timing
            },
            ..Default::default()
        }),
    ] {
        assert!(
            validate_pause(
                StationPauseOperation::Rfpll,
                StationPauseEvidence {
                    timings,
                    ..evidence
                }
            )
            .is_err()
        );
    }
}

#[test]
fn rfpll_detail_cannot_be_missing_or_confuse_forced_work_with_thermal_skip() {
    use open_esp_radio_hil_protocol::{
        PhyOperationTiming, PhyTimingEvidence, RfpllCorrectionEvidence, RfpllEvidence,
    };
    let timing = PhyTimingEvidence {
        rfpll: PhyOperationTiming {
            started: 1,
            completed: 1,
            elapsed_micros: 10,
            maximum_micros: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let skipped = RfpllEvidence {
        sample_age_micros: None,
        temperature: 10,
        reference_before: 10,
        reference_after: 10,
        threshold: 15,
        channel: 13,
        correction: None,
    };
    let ran = RfpllEvidence {
        sample_age_micros: Some(100),
        threshold: 0,
        correction: Some(RfpllCorrectionEvidence {
            initial_cap: 100,
            selected_cap: 100,
            accepted_samples: 0,
            entries_updated: 0,
            restored_frequency_index: None,
        }),
        ..skipped
    };
    for (operation, detail, expected) in [
        (StationPauseOperation::Rfpll, Some(ran), true),
        (StationPauseOperation::Rfpll, Some(skipped), false),
        (StationPauseOperation::Rfpll, None, false),
        (StationPauseOperation::RfpllCheck, Some(skipped), true),
        (StationPauseOperation::RfpllCheck, Some(ran), false),
        (StationPauseOperation::RfpllCheck, None, false),
    ] {
        assert_eq!(
            super::validate_rfpll(operation, Some(timing), detail).is_ok(),
            expected
        );
    }
    assert!(super::validate_rfpll(StationPauseOperation::Access, None, Some(ran)).is_err());
    assert!(super::validate_rfpll(StationPauseOperation::Access, None, None).is_ok());
}

#[test]
fn observed_rfpll_rejects_missing_or_stale_sensor_age() {
    use open_esp_radio_hil_protocol::{
        PhyTimingEvidence, RfpllEvidence, STATION_RFPLL_SAMPLE_MAX_AGE_MICROS,
    };
    let mut timings = PhyTimingEvidence::default();
    timings.rfpll.started = 1;
    timings.rfpll.completed = 1;
    for (sample_age_micros, valid) in [
        (None, false),
        (Some(0), true),
        (Some(STATION_RFPLL_SAMPLE_MAX_AGE_MICROS), true),
        (Some(STATION_RFPLL_SAMPLE_MAX_AGE_MICROS + 1), false),
        (Some(u64::MAX), false),
    ] {
        let detail = RfpllEvidence {
            temperature: 20,
            reference_before: 20,
            reference_after: 20,
            threshold: 15,
            channel: 11,
            correction: None,
            sample_age_micros,
        };
        assert_eq!(
            super::validate_rfpll(
                StationPauseOperation::RfpllObserved,
                Some(timings),
                Some(detail)
            )
            .is_ok(),
            valid
        );
        let measured = open_esp_radio_hil_protocol::RfpllEvidence {
            threshold: 0,
            correction: Some(open_esp_radio_hil_protocol::RfpllCorrectionEvidence {
                initial_cap: 100,
                selected_cap: 101,
                accepted_samples: 3,
                entries_updated: 85,
                restored_frequency_index: Some(11),
            }),
            ..detail
        };
        assert_eq!(
            super::validate_rfpll(StationPauseOperation::Rfpll, Some(timings), Some(measured))
                .is_ok(),
            valid
        );
        assert!(
            super::validate_rfpll(
                StationPauseOperation::RfpllCheck,
                Some(timings),
                Some(detail)
            )
            .is_ok()
        );
    }
}

#[test]
fn temperature_prerequisite_requires_a_completed_acquisition_only() {
    use open_esp_radio_hil_protocol::PhyTimingEvidence;
    let mut timings = PhyTimingEvidence::default();
    let evidence = StationPauseEvidence {
        result: StationPauseResult::Resumed,
        elapsed_micros: 1,
        timings: Some(timings),
        tracking: Some(StationPhyTrackingEvidence {
            inhibited: false,
            common_calibrated: false,
            wifi_calibrated: false,
            bluetooth_ieee802154_calibrated: false,
        }),
    };
    assert!(validate_pause(StationPauseOperation::Temperature, evidence).is_err());
    timings.temperature.started = 1;
    timings.temperature.completed = 1;
    assert!(
        validate_pause(
            StationPauseOperation::Temperature,
            StationPauseEvidence {
                timings: Some(timings),
                ..evidence
            }
        )
        .is_ok()
    );
    timings.rfpll.started = 1;
    timings.rfpll.completed = 1;
    assert!(
        validate_pause(
            StationPauseOperation::Temperature,
            StationPauseEvidence {
                timings: Some(timings),
                ..evidence
            }
        )
        .is_err()
    );
}

#[test]
fn synthetic_pause_requires_its_hold_and_cannot_hide_phy_work() {
    let operation = StationPauseOperation::Synthetic {
        duration_micros: 10_000,
        notify_ap: true,
    };
    let mut evidence = StationPauseEvidence {
        timings: None,
        tracking: None,
        result: open_esp_radio_hil_protocol::StationPauseResult::Resumed,
        elapsed_micros: 9_999,
    };
    assert!(validate_pause(operation, evidence).is_err());
    evidence.elapsed_micros = 10_500;
    assert!(validate_pause(operation, evidence).is_ok());
    evidence.tracking = Some(open_esp_radio_hil_protocol::StationPhyTrackingEvidence {
        inhibited: false,
        common_calibrated: false,
        wifi_calibrated: false,
        bluetooth_ieee802154_calibrated: false,
    });
    assert!(validate_pause(operation, evidence).is_err());
}

#[test]
fn rx_gain_detail_is_required_exactly_when_timing_is_available() {
    use super::validate_rx_gain;
    use open_esp_radio_hil_protocol::{PhyRxGainEvidence, PhyTimingEvidence};
    assert!(validate_rx_gain(None, None).is_ok());
    assert!(validate_rx_gain(Some(PhyTimingEvidence::default()), None).is_err());
    assert!(validate_rx_gain(None, Some(PhyRxGainEvidence::default())).is_err());
    assert!(
        validate_rx_gain(
            Some(PhyTimingEvidence::default()),
            Some(PhyRxGainEvidence::default())
        )
        .is_ok()
    );
}
