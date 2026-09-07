use super::*;

const POLICY: StaPowerSavePolicy = match StaPowerSavePolicy::new(100, 2_000) {
    Ok(policy) => policy,
    Err(_) => panic!("valid test policy"),
};

fn beacon(tim: Option<StaTimObservation>) -> StaBeaconObservation {
    StaBeaconObservation {
        timestamp_tsf: 1_000_000,
        // Deliberately different: policy must retain the association's
        // protected interval rather than trust this beacon field.
        interval_tu: 500,
        capability_information: 0,
        tim,
    }
}

fn tim() -> StaTimObservation {
    StaTimObservation {
        dtim_count: 1,
        dtim_period: 3,
        unicast_buffered: false,
        group_buffered: false,
    }
}

fn opportunity(
    tim: Option<StaTimObservation>,
    station_tsf: u64,
    traffic: StaTrafficState,
) -> StaPowerSaveOpportunity {
    StaPowerSaveOpportunity {
        beacon: beacon(tim),
        station_tsf,
        traffic,
    }
}

#[test]
fn policy_rejects_vacuous_or_unwakeable_intervals() {
    assert_eq!(
        StaPowerSavePolicy::new(0, 1),
        Err(StaPowerSavePolicyError::ZeroBeaconInterval)
    );
    assert_eq!(
        StaPowerSavePolicy::new(1, 1_024),
        Err(StaPowerSavePolicyError::WakeGuardOutsideBeaconInterval)
    );
    assert_eq!(POLICY.beacon_interval_micros(), 102_400);
}

#[test]
fn doze_requires_acknowledged_pm_one_and_uses_association_interval() {
    let mut planner = StaPowerSavePlanner::new(POLICY);
    assert_eq!(
        planner.observe_beacon(opportunity(
            Some(tim()),
            1_000_100,
            StaTrafficState::Quiescent,
        )),
        StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::PowerSave)
    );
    assert_eq!(planner.state(), StaPowerSaveState::AdvertisingPowerSave);

    let permit = StaDozePermit {
        beacon_timestamp_tsf: 1_000_000,
        next_listen_tsf: 1_102_400,
        next_dtim_tsf: 1_102_400,
        wake_tsf: 1_100_400,
        wake_after_beacons: 1,
        wake_reason: StaDozeWakeReason::ListenIntervalAndDtim,
        dtim_count: 1,
        dtim_period: 3,
    };
    assert_eq!(planner.candidate(), Some(permit));
    assert_eq!(
        planner
            .complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 1_001_000,
            })
            .unwrap(),
        StaPowerSaveDecision::PermitDoze(permit)
    );
    assert_eq!(planner.state(), StaPowerSaveState::PowerSave);
}

#[test]
fn failed_or_late_pm_one_never_grants_doze() {
    let mut failed = StaPowerSavePlanner::new(POLICY);
    failed.observe_beacon(opportunity(
        Some(tim()),
        1_000_100,
        StaTrafficState::Quiescent,
    ));
    assert_eq!(
        failed
            .complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::PowerSave,
                outcome: StaPowerManagementTxOutcome::Failed,
                station_tsf: 1_001_000,
            })
            .unwrap(),
        StaPowerSaveDecision::StayAwake(StaStayAwakeReason::NoFreshDozeWindow)
    );
    assert_eq!(failed.state(), StaPowerSaveState::Awake);

    let mut late = StaPowerSavePlanner::new(POLICY);
    late.observe_beacon(opportunity(
        Some(tim()),
        1_000_100,
        StaTrafficState::Quiescent,
    ));
    assert_eq!(
        late.complete_power_management(StaPowerManagementTxCompletion {
            advertised: StaPowerManagement::PowerSave,
            outcome: StaPowerManagementTxOutcome::Acknowledged,
            station_tsf: 1_100_400,
        })
        .unwrap(),
        StaPowerSaveDecision::StayAwake(StaStayAwakeReason::WakeDeadlinePassed)
    );
    assert_eq!(late.state(), StaPowerSaveState::PowerSave);
}

#[test]
fn incomplete_unsafe_or_busy_observation_fails_closed() {
    let cases = [
        (
            opportunity(None, 1_000_100, StaTrafficState::Quiescent),
            StaStayAwakeReason::MissingTim,
        ),
        (
            opportunity(
                Some(StaTimObservation {
                    dtim_count: 3,
                    ..tim()
                }),
                1_000_100,
                StaTrafficState::Quiescent,
            ),
            StaStayAwakeReason::InvalidTimPhase,
        ),
        (
            opportunity(
                Some(StaTimObservation {
                    unicast_buffered: true,
                    ..tim()
                }),
                1_000_100,
                StaTrafficState::Quiescent,
            ),
            StaStayAwakeReason::UnicastBuffered,
        ),
        (
            opportunity(Some(tim()), 1_000_100, StaTrafficState::Pending),
            StaStayAwakeReason::TrafficPending,
        ),
    ];
    for (input, reason) in cases {
        let mut planner = StaPowerSavePlanner::new(POLICY);
        assert_eq!(
            planner.observe_beacon(input),
            StaPowerSaveDecision::StayAwake(reason)
        );
        assert_eq!(planner.state(), StaPowerSaveState::Awake);
        assert_eq!(planner.candidate(), None);
    }
}

#[test]
fn power_save_beacon_can_grant_one_fresh_permit() {
    let mut planner = StaPowerSavePlanner::new(POLICY);
    planner.observe_beacon(opportunity(
        Some(tim()),
        1_000_100,
        StaTrafficState::Quiescent,
    ));
    planner
        .complete_power_management(StaPowerManagementTxCompletion {
            advertised: StaPowerManagement::PowerSave,
            outcome: StaPowerManagementTxOutcome::Acknowledged,
            station_tsf: 1_100_400,
        })
        .unwrap();

    let decision = planner.observe_beacon(opportunity(
        Some(tim()),
        1_000_200,
        StaTrafficState::Quiescent,
    ));
    assert!(matches!(decision, StaPowerSaveDecision::PermitDoze(_)));
}

#[test]
fn active_transition_is_also_ack_gated() {
    let mut planner = StaPowerSavePlanner::new(POLICY);
    planner.observe_beacon(opportunity(
        Some(tim()),
        1_000_100,
        StaTrafficState::Quiescent,
    ));
    planner
        .complete_power_management(StaPowerManagementTxCompletion {
            advertised: StaPowerManagement::PowerSave,
            outcome: StaPowerManagementTxOutcome::Acknowledged,
            station_tsf: 1_001_000,
        })
        .unwrap();
    assert_eq!(
        planner.request_active(),
        StaPowerSaveDecision::SendPowerManagement(StaPowerManagement::Active)
    );
    assert_eq!(planner.state(), StaPowerSaveState::AdvertisingActive);
    assert_eq!(
        planner
            .complete_power_management(StaPowerManagementTxCompletion {
                advertised: StaPowerManagement::Active,
                outcome: StaPowerManagementTxOutcome::Acknowledged,
                station_tsf: 1_002_000,
            })
            .unwrap(),
        StaPowerSaveDecision::StayAwake(StaStayAwakeReason::AlreadyAwake)
    );
    assert_eq!(planner.state(), StaPowerSaveState::Awake);
}

#[test]
fn unexpected_completion_does_not_mutate_state() {
    let mut planner = StaPowerSavePlanner::new(POLICY);
    assert_eq!(
        planner.complete_power_management(StaPowerManagementTxCompletion {
            advertised: StaPowerManagement::PowerSave,
            outcome: StaPowerManagementTxOutcome::Acknowledged,
            station_tsf: 0,
        }),
        Err(UnexpectedStaPowerManagementCompletion {
            state: StaPowerSaveState::Awake,
            advertised: StaPowerManagement::PowerSave,
        })
    );
    assert_eq!(planner.state(), StaPowerSaveState::Awake);
}
