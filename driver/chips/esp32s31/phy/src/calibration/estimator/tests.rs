use super::*;

const REQUEST: PhyDcIqEstimateRequest = PhyDcIqEstimateRequest {
    iteration: 3,
    chain: 1,
    control: 0x0fa0,
    mode: 0,
};

fn reach_readiness(transition: &mut PhyDcIqEstimateTransition) {
    transition
        .advance(PhyDcIqCompletion::Configured(REQUEST))
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: true,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: true,
        })
        .unwrap();
}

fn finish_disable_tail(transition: &mut PhyDcIqEstimateTransition) {
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::DelayElapsed {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Stop,
            micros: 1,
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::EnableSet {
            request: REQUEST,
            phase: PhyDcIqEnablePhase::Start,
            enabled: false,
        })
        .unwrap();
}

#[test]
fn linear_to_db_matches_rom_table_boundaries() {
    assert_eq!(phy_linear_to_db(0, 0), 0);
    assert_eq!(phy_linear_to_db(1, 0), 28);
    assert_eq!(phy_linear_to_db(2, 0), 48);
    assert_eq!(phy_linear_to_db(3, 0), 76);
    assert_eq!(phy_linear_to_db(1, 3), 4);
}

#[test]
fn accumulator_transform_matches_reachable_mode_zero_equations() {
    assert_eq!(
        calculate_dc_iq_estimate(
            REQUEST,
            PhyDcIqAccumulatorSnapshot {
                i: 4001 * 64 * 2,
                q: -(4001 * 64 * 3),
                power: 4001 * 32,
            },
        ),
        PhyDcIqEstimate {
            i: 2,
            q: -3,
            power: 24,
        }
    );
}

#[test]
fn readiness_requires_external_edges_and_owns_diagnostic_count() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    reach_readiness(&mut transition);
    for activity in [false, true, true] {
        transition
            .advance(PhyDcIqCompletion::ReadinessObserved {
                request: REQUEST,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: false,
                    activity,
                },
            })
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyDcIqAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 2,
            readiness_samples: 3,
        }
    );
    transition
        .advance(PhyDcIqCompletion::ReadinessObserved {
            request: REQUEST,
            snapshot: PhyDcIqReadinessSnapshot {
                ready: true,
                activity: true,
            },
        })
        .unwrap();
    transition
        .advance(PhyDcIqCompletion::AccumulatorsRead {
            request: REQUEST,
            snapshot: PhyDcIqAccumulatorSnapshot {
                i: 0,
                q: 0,
                power: 0,
            },
        })
        .unwrap();
    finish_disable_tail(&mut transition);
    let PhyDcIqAction::Complete(outcome) = transition.action() else {
        panic!("DC/IQ transition did not complete");
    };
    assert_eq!(outcome.readiness_activity_edges, 2);
    assert_eq!(
        outcome.estimate,
        PhyDcIqEstimate {
            i: 0,
            q: 0,
            power: 0,
        }
    );
}

#[test]
fn timeout_runs_complete_disable_tail_before_failure() {
    let mut transition = PhyDcIqEstimateTransition::new(REQUEST);
    reach_readiness(&mut transition);
    transition
        .advance(PhyDcIqCompletion::ReadinessTimedOut(REQUEST))
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyDcIqAction::SetEnable {
            phase: PhyDcIqEnablePhase::Measurement,
            enabled: false,
            ..
        }
    ));
    finish_disable_tail(&mut transition);
    assert_eq!(
        transition.action(),
        PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 0,
        })
    );
}

#[test]
fn external_lowering_separates_mmio_timer_readiness_and_terminal() {
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::Configure(REQUEST)),
        Ok(PhyDcIqExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::DelayMicros {
            request: REQUEST,
            phase: PhyDcIqDelayPhase::Start,
            micros: 1,
        }),
        Ok(PhyDcIqExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::AwaitReadinessEdge {
            request: REQUEST,
            readiness_activity_edges: 0,
            readiness_samples: 7,
        }),
        Ok(PhyDcIqExternalBinding::Readiness(binding)) if binding.samples() == 7
    ));
    assert!(matches!(
        PhyDcIqExternalBinding::lower(PhyDcIqAction::Failed(PhyDcIqFailure::ReadinessTimedOut {
            request: REQUEST,
            readiness_activity_edges: 0,
        })),
        Err(PhyDcIqBindingError::UnsupportedAction)
    ));
}
