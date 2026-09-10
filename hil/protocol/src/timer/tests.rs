use super::*;

fn completed() -> TimerWindowEvidence {
    let phase = TimerPhaseTiming {
        count: 1,
        total_micros: 5,
        maximum_micros: 5,
    };
    TimerWindowEvidence {
        elapsed_micros: 100,
        irq_ack: phase,
        programming: phase,
        alarm_to_irq: phase,
        deadline_lateness: phase,
        irq_to_dispatch: phase,
        dispatch: phase,
        interrupts: 1,
        ..Default::default()
    }
}

#[test]
fn timer_report_reconciles_alarms_interrupts_and_dispatches() {
    let valid = completed();
    assert!(valid.is_valid());
    assert!(TimerWindowEvidence::default().is_valid());
    for bad in [
        TimerWindowEvidence {
            interrupts: 2,
            ..valid
        },
        TimerWindowEvidence {
            stopped: 1,
            ..valid
        },
        TimerWindowEvidence {
            replaced: 1,
            ..valid
        },
        TimerWindowEvidence {
            invalid: true,
            ..valid
        },
        TimerWindowEvidence {
            unmatched_dispatches: 1,
            ..valid
        },
        TimerWindowEvidence {
            coalesced_interrupts: 1,
            ..valid
        },
        TimerWindowEvidence {
            early_interrupts: 2,
            ..valid
        },
        TimerWindowEvidence {
            programming: TimerPhaseTiming {
                count: 0,
                ..valid.programming
            },
            ..valid
        },
        TimerWindowEvidence {
            dispatch: TimerPhaseTiming {
                total_micros: 101,
                ..valid.dispatch
            },
            ..valid
        },
        TimerWindowEvidence {
            replaced: u32::MAX,
            ..valid
        },
    ] {
        assert!(!bad.is_valid());
    }
}

#[test]
fn partial_boundary_records_are_accounted_for() {
    let valid = completed();
    let partial = TimerWindowEvidence {
        irq_ack: TimerPhaseTiming {
            count: 3,
            ..valid.irq_ack
        },
        interrupts: 3,
        unmatched_interrupts: 2,
        coalesced_interrupts: 1,
        irq_pending_at_end: true,
        armed_at_end: true,
        programming: TimerPhaseTiming {
            count: 2,
            ..valid.programming
        },
        ..valid
    };
    assert!(partial.is_valid());
}

#[test]
fn deadline_counters_and_acknowledgements_must_reconcile() {
    let valid = TimerWindowEvidence {
        registrations: 3,
        due_at_registration: 2,
        due_at_program_start: 0,
        due_at_program_return: 1,
        ..completed()
    };
    assert!(valid.is_valid()); // Registration and programming are different populations.
    for invalid in [
        TimerWindowEvidence {
            due_at_registration: 4,
            ..valid
        },
        TimerWindowEvidence {
            due_at_program_start: 2,
            ..valid
        },
        TimerWindowEvidence {
            due_at_program_return: 2,
            ..valid
        },
        TimerWindowEvidence {
            irq_ack: TimerPhaseTiming::default(),
            ..valid
        },
    ] {
        assert!(!invalid.is_valid());
    }
}

#[test]
fn overlapping_irq_retires_an_alarm_without_a_latency_sample() {
    let report = TimerWindowEvidence {
        overlapping_interrupts: 1,
        alarm_to_irq: TimerPhaseTiming::default(),
        deadline_lateness: TimerPhaseTiming::default(),
        ..completed()
    };
    assert!(report.is_valid());
    assert!(
        !TimerWindowEvidence {
            overlapping_interrupts: 2,
            ..report
        }
        .is_valid()
    );
    assert!(
        !TimerWindowEvidence {
            alarm_to_irq: completed().alarm_to_irq,
            ..report
        }
        .is_valid()
    );
    assert!(
        !TimerWindowEvidence {
            programming: TimerPhaseTiming::default(),
            ..report
        }
        .is_valid()
    );
}
