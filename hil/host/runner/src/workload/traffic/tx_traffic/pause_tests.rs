use super::validate_pause;
use open_esp_radio_hil_protocol::{
    StationPauseEvidence, StationPauseOperation, StationPauseResult, StationPhyTrackingEvidence,
};

#[test]
fn calibration_requires_success_and_both_committed_branches() {
    for result in [StationPauseResult::Resumed, StationPauseResult::PhyTracking] {
        for inhibited in [false, true] {
            for common_calibrated in [false, true] {
                for wifi_calibrated in [false, true] {
                    let evidence = StationPauseEvidence {
                        timings: None,
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
