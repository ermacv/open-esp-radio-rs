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
        previous_phy_registration_generation: 0,
        phy_registration_generation: 1,
        calibration_path: WifiRadioCalibrationPath::RestoredCache,
    };
    assert!(require_radio_restart(stopped, restarted, None).is_ok());
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                generation: 7,
                ..restarted
            },
            None,
        )
        .is_err()
    );
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                calibration_path: WifiRadioCalibrationPath::Full,
                ..restarted
            },
            None,
        )
        .is_err()
    );
}

#[test]
fn station_after_radio_restart_requires_the_next_generation() {
    let restarted = WifiRadioRestartEvidence {
        generation: 8,
        previous_phy_registration_generation: 0,
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
        previous_phy_registration_generation: 3,
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
fn first_retained_cycle_must_not_establish_its_own_phy_baseline() {
    let stopped = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 7,
    };
    let first_cycle_after_an_unobserved_phy_change = WifiRadioRetainedCycleEvidence {
        generation: 8,
        previous_phy_registration_generation: 3,
        phy_registration_generation: 4,
    };
    assert!(
        require_retained_radio_cycle(stopped, first_cycle_after_an_unobserved_phy_change, None)
            .is_err(),
        "a missing pre-operation PHY anchor cannot prove retention on cycle one"
    );
}

#[test]
fn lifecycle_stop_requires_the_current_station_link_disconnect() {
    let disconnect = StationLifecycleEvent::Disconnected {
        generation: u32::MAX,
        reason: StationDisconnectReason::LinkPolicy,
    };
    assert!(require_lifecycle_disconnect(disconnect, u32::MAX).is_ok());
    assert!(require_lifecycle_disconnect(disconnect, 0).is_err());
    assert!(
        require_lifecycle_disconnect(
            StationLifecycleEvent::Disconnected {
                generation: u32::MAX,
                reason: StationDisconnectReason::BeaconLoss,
            },
            u32::MAX,
        )
        .is_err()
    );
    assert!(
        require_lifecycle_disconnect(
            StationLifecycleEvent::Connected {
                generation: u32::MAX,
                association_bandwidth_mhz: Some(40),
                security: Some(oer_hil_protocol::StationLinkSecurity::Wpa2Personal),
            },
            u32::MAX,
        )
        .is_err()
    );
}

#[test]
fn lifecycle_connected_link_requires_negotiated_phy_and_security() {
    let ht40_wpa2 = StationConnectionObservation {
        generation: 4,
        association_bandwidth_mhz: Some(40),
        security: Some(oer_hil_protocol::StationLinkSecurity::Wpa2Personal),
        event_cursor_after: 3,
    };
    assert!(require_station_link(ht40_wpa2, PhyExpectation::Ht40).is_ok());
    assert!(require_station_link(ht40_wpa2, PhyExpectation::Ht20).is_err());
    assert!(
        require_station_link(
            StationConnectionObservation {
                security: Some(oer_hil_protocol::StationLinkSecurity::Open),
                ..ht40_wpa2
            },
            PhyExpectation::Ht40,
        )
        .is_err()
    );
    assert!(
        require_station_link(
            StationConnectionObservation {
                association_bandwidth_mhz: None,
                ..ht40_wpa2
            },
            PhyExpectation::Ht40,
        )
        .is_err()
    );
}

#[test]
fn all_three_cold_and_retained_cycles_require_continuous_owner_epochs() {
    let mut role_generation = u32::MAX - 1;
    let mut cold_phy_generation = u32::MAX - 1;
    let retained_phy_generation = 17;
    for _cycle in 1..=3 {
        let stopped = WifiRoleTransitionEvidence {
            previous: WifiRole::Station,
            current: WifiRole::Idle,
            generation: role_generation,
        };
        let cold = WifiRadioRestartEvidence {
            generation: role_generation.wrapping_add(1),
            previous_phy_registration_generation: cold_phy_generation,
            phy_registration_generation: cold_phy_generation.wrapping_add(1),
            calibration_path: WifiRadioCalibrationPath::RestoredCache,
        };
        assert!(require_radio_restart(stopped, cold, Some(cold_phy_generation)).is_ok());
        let retained = WifiRadioRetainedCycleEvidence {
            generation: role_generation.wrapping_add(1),
            previous_phy_registration_generation: retained_phy_generation,
            phy_registration_generation: retained_phy_generation,
        };
        assert!(
            require_retained_radio_cycle(stopped, retained, Some(retained_phy_generation)).is_ok()
        );
        let started = WifiRoleTransitionEvidence {
            previous: WifiRole::Idle,
            current: WifiRole::Station,
            generation: role_generation.wrapping_add(2),
        };
        assert!(require_station_after_lifecycle(cold.generation, started).is_ok());
        role_generation = started.generation;
        cold_phy_generation = cold.phy_registration_generation;
    }

    let stale_cold = WifiRadioRestartEvidence {
        generation: role_generation.wrapping_add(1),
        previous_phy_registration_generation: cold_phy_generation.wrapping_sub(1),
        phy_registration_generation: cold_phy_generation,
        calibration_path: WifiRadioCalibrationPath::RestoredCache,
    };
    assert!(
        require_radio_restart(
            WifiRoleTransitionEvidence {
                previous: WifiRole::Station,
                current: WifiRole::Idle,
                generation: role_generation,
            },
            stale_cold,
            Some(cold_phy_generation),
        )
        .is_err(),
        "a missing predecessor cycle cannot supply the current PHY baseline"
    );
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
