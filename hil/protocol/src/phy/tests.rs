use super::*;

fn rx_execution() -> PhyRxGainExecutionEvidence {
    PhyRxGainExecutionEvidence {
        minimum_searches: 121,
        minimum_operations: 191,
        outer_operations: 58,
        settle_1us: 100,
        settle_2us: 1,
        settle_10us: 121,
    }
}

#[test]
fn rx_gain_detail_rejects_orphan_incomplete_and_overlapping_evidence() {
    let stage = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 40,
        maximum_micros: 40,
        ..Default::default()
    };
    let mut evidence = PhyRxGainEvidence {
        dc_phase: stage,
        ..Default::default()
    };
    assert!(!evidence.fits(PhyOperationTiming::default()));
    let operation = PhyOperationTiming {
        elapsed_micros: 100,
        maximum_micros: 100,
        ..stage
    };
    evidence.execution = Some(rx_execution());
    evidence.control_phase = stage;
    assert!(evidence.fits(operation));
    evidence.publish_phase = stage;
    assert!(!evidence.fits(operation));
    evidence.publish_phase = PhyOperationTiming::default();
    evidence.control_phase.completed = 0;
    assert!(!evidence.fits(operation));
    evidence.control_phase = PhyOperationTiming {
        elapsed_micros: u32::MAX,
        ..stage
    };
    assert!(!evidence.fits(operation));
}

#[test]
fn successful_timing_requires_polls_within_the_observed_operation() {
    let mut evidence = PhyTimingEvidence::default();
    assert!(evidence.is_complete());
    evidence.dcode = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 100,
        maximum_micros: 100,
        ..Default::default()
    };
    assert!(!evidence.is_complete());
    evidence.dcode_polls = PhyPollTiming {
        polls: 2,
        pending: 1,
        suspended_micros: 20,
        maximum_suspension_micros: 20,
        elapsed_micros: 10,
        maximum_micros: 6,
    };
    assert!(evidence.is_complete());
    for polls in [
        PhyPollTiming {
            polls: 2,
            pending: 1,
            suspended_micros: 20,
            maximum_suspension_micros: 20,
            elapsed_micros: 101,
            maximum_micros: 6,
        },
        PhyPollTiming {
            polls: 2,
            pending: 1,
            suspended_micros: 20,
            maximum_suspension_micros: 20,
            elapsed_micros: 10,
            maximum_micros: 11,
        },
        PhyPollTiming::default(),
    ] {
        evidence.dcode_polls = polls;
        assert!(!evidence.is_complete());
    }
    evidence = PhyTimingEvidence::default();
    evidence.rx_gain_polls.polls = 1;
    assert!(!evidence.is_complete());
}

#[test]
fn pending_gaps_must_fit_and_each_completed_child_requires_ready() {
    let operation = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 100,
        maximum_micros: 100,
        ..Default::default()
    };
    let valid = PhyPollTiming {
        polls: 3,
        pending: 2,
        elapsed_micros: 30,
        maximum_micros: 20,
        suspended_micros: 60,
        maximum_suspension_micros: 40,
    };
    assert!(valid.fits(operation));
    for invalid in [
        PhyPollTiming {
            pending: 3,
            ..valid
        },
        PhyPollTiming {
            pending: 4,
            ..valid
        },
        PhyPollTiming {
            suspended_micros: 71,
            ..valid
        },
        PhyPollTiming {
            maximum_suspension_micros: 61,
            ..valid
        },
        PhyPollTiming {
            suspended_micros: u32::MAX,
            ..valid
        },
        PhyPollTiming {
            pending: 0,
            polls: 1,
            ..valid
        },
    ] {
        assert!(!invalid.fits(operation));
    }
}

#[test]
fn deadline_measurements_require_nonoverlapping_valid_intervals() {
    let operation = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 100,
        maximum_micros: 100,
        ..Default::default()
    };
    let timing = PhyWaitTiming {
        count: 2,
        requested_micros: 2,
        elapsed_micros: 30,
        maximum_lateness_micros: 20,
    };
    let valid = PhyDcodeWaitEvidence {
        i2c: PhyBusWaitEvidence {
            bus_busy: 0,
            timing,
        },
        rfpll_i2c: PhyBusWaitEvidence {
            bus_busy: 1,
            timing,
        },
        rfpll_settle: timing,
        pll_locked: 1,
        pll_unlocked: 3,
    };
    assert!(valid.fits(operation));
    for invalid in [
        PhyWaitTiming {
            elapsed_micros: 1,
            ..timing
        },
        PhyWaitTiming {
            elapsed_micros: 41,
            ..timing
        },
        PhyWaitTiming {
            maximum_lateness_micros: 29,
            ..timing
        },
        PhyWaitTiming { count: 0, ..timing },
        PhyWaitTiming {
            elapsed_micros: u32::MAX,
            ..timing
        },
    ] {
        assert!(
            !PhyDcodeWaitEvidence {
                rfpll_settle: invalid,
                ..valid
            }
            .fits(operation)
        );
    }
    assert!(
        !PhyDcodeWaitEvidence {
            i2c: PhyBusWaitEvidence {
                bus_busy: 3,
                timing
            },
            ..valid
        }
        .fits(operation)
    );
    assert!(!valid.fits(PhyOperationTiming::default()));
}

#[test]
fn tx_waits_must_fit_active_operation_without_overflow() {
    let operation = PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: 50,
        maximum_micros: 50,
        ..Default::default()
    };
    let wait = PhyWaitTiming {
        count: 1,
        requested_micros: 1,
        elapsed_micros: 10,
        maximum_lateness_micros: 9,
    };
    let valid = PhyTxWaitEvidence {
        pbus: PhyBusWaitEvidence {
            bus_busy: 1,
            timing: wait,
        },
        search: wait,
        tone: wait,
        sar: wait,
        root: wait,
        sar_ready: 1,
        sar_not_ready: 0,
    };
    assert!(valid.fits(operation));
    assert!(!valid.fits(PhyOperationTiming::default()));
    assert!(PhyTxWaitEvidence::default().fits(PhyOperationTiming::default()));
    for invalid in [
        PhyTxWaitEvidence {
            pbus: PhyBusWaitEvidence {
                bus_busy: 2,
                timing: wait,
            },
            ..valid
        },
        PhyTxWaitEvidence {
            tone: PhyWaitTiming {
                elapsed_micros: 11,
                ..wait
            },
            ..valid
        },
        PhyTxWaitEvidence {
            tone: PhyWaitTiming {
                elapsed_micros: u32::MAX,
                ..wait
            },
            ..valid
        },
        PhyTxWaitEvidence {
            tone: PhyWaitTiming { count: 0, ..wait },
            ..valid
        },
    ] {
        assert!(!invalid.fits(operation));
    }
}

#[test]
fn rx_regions_validate_disjoint_phases() {
    let timing = |us| PhyOperationTiming {
        started: 1,
        completed: 1,
        elapsed_micros: us,
        maximum_micros: us,
        ..Default::default()
    };
    let mut evidence = PhyRxGainEvidence {
        execution: Some(rx_execution()),
        dc_phase: timing(70),
        publish_phase: timing(20),
        control_phase: timing(5),
    };
    assert!(evidence.fits(timing(100)));
    evidence.publish_phase = timing(30);
    assert!(!evidence.fits(timing(100)));
    evidence.publish_phase = timing(20);
    evidence.dc_phase.completed = 0;
    assert!(!evidence.fits(timing(100)));
}
