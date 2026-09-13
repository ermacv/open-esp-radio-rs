use super::*;

#[test]
fn role_transition_is_exact() {
    let evidence = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 3,
    };
    assert!(require_transition(evidence, WifiRole::Station, WifiRole::Idle).is_ok());
    assert!(require_transition(evidence, WifiRole::Idle, WifiRole::Station).is_err());
}

#[test]
fn radio_restart_requires_idle_ownership_and_the_next_generation() {
    let stopped = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 7,
    };
    let restarted = WifiRadioRestartEvidence {
        generation: 8,
        phy_registration_generation: 1,
        calibration_path: WifiRadioCalibrationPath::RestoredCache,
    };
    assert!(require_radio_restart(stopped, restarted).is_ok());
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                generation: 7,
                ..restarted
            }
        )
        .is_err()
    );
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                calibration_path: WifiRadioCalibrationPath::Full,
                ..restarted
            }
        )
        .is_err()
    );
}

#[test]
fn station_after_radio_restart_requires_the_next_generation() {
    let restarted = WifiRadioRestartEvidence {
        generation: 8,
        phy_registration_generation: 1,
        calibration_path: WifiRadioCalibrationPath::RestoredCache,
    };
    let started = WifiRoleTransitionEvidence {
        previous: WifiRole::Idle,
        current: WifiRole::Station,
        generation: 9,
    };
    assert!(require_station_after_lifecycle(restarted.generation, started).is_ok());
    assert!(
        require_station_after_lifecycle(
            restarted.generation,
            WifiRoleTransitionEvidence {
                generation: 8,
                ..started
            }
        )
        .is_err()
    );
}

#[test]
fn retained_radio_cycle_requires_the_next_generation_and_preserves_phy_epoch() {
    let stopped = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 7,
    };
    let retained = WifiRadioRetainedCycleEvidence {
        generation: 8,
        phy_registration_generation: 3,
    };
    assert!(require_retained_radio_cycle(stopped, retained, None).is_ok());
    assert!(require_retained_radio_cycle(stopped, retained, Some(3)).is_ok());
    assert!(
        require_retained_radio_cycle(
            stopped,
            WifiRadioRetainedCycleEvidence {
                generation: 7,
                ..retained
            },
            Some(3),
        )
        .is_err()
    );
    assert!(require_retained_radio_cycle(stopped, retained, Some(2)).is_err());
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            monitor_channel: Some(14),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            snapshot_length: 2305,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
