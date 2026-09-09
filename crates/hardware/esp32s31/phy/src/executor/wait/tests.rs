use super::*;

fn completed(recorder: &mut Recorder, scope: Scope, kind: Kind, requested: u64, elapsed: u64) {
    recorder.observe(
        scope,
        kind,
        Event::Started {
            requested_micros: requested,
        },
    );
    recorder.observe(
        scope,
        kind,
        Event::Completed {
            elapsed_micros: elapsed,
            lateness_micros: elapsed - requested,
        },
    );
}

#[test]
fn direct_i2c_nested_i2c_and_settling_are_separate() {
    let mut recorder = Recorder::default();
    completed(&mut recorder, Scope::Dcode, Kind::Completion, 1, 3);
    completed(&mut recorder, Scope::Rfpll, Kind::BusBusy, 1, 5);
    completed(&mut recorder, Scope::Rfpll, Kind::Completion, 1, 7);
    completed(&mut recorder, Scope::Rfpll, Kind::Settle, 20, 35);
    recorder.pll_lock(false);
    recorder.pll_lock(true);
    assert!(recorder.is_complete());
    let report = recorder.report();
    assert_eq!(report.i2c.timing.count, 1);
    assert_eq!(report.i2c.timing.elapsed_micros, 3);
    assert_eq!(report.rfpll_i2c.bus_busy, 1);
    assert_eq!(report.rfpll_i2c.timing.count, 2);
    assert_eq!(report.rfpll_i2c.timing.requested_micros, 2);
    assert_eq!(report.rfpll_i2c.timing.elapsed_micros, 12);
    assert_eq!(report.rfpll_i2c.timing.maximum_lateness_micros, 6);
    assert_eq!(
        report.rfpll_settle,
        Timing {
            count: 1,
            requested_micros: 20,
            elapsed_micros: 35,
            maximum_lateness_micros: 15,
        }
    );
    assert_eq!((report.pll_locked, report.pll_unlocked), (1, 1));
}

#[test]
fn cancellation_unsupported_and_inconsistent_deadlines_are_incomplete() {
    for events in [
        std::vec![Event::Started {
            requested_micros: 1
        }],
        std::vec![Event::Unsupported],
        std::vec![Event::Completed {
            elapsed_micros: 5,
            lateness_micros: 4
        }],
        std::vec![
            Event::Started {
                requested_micros: 10
            },
            Event::Completed {
                elapsed_micros: 9,
                lateness_micros: 0
            }
        ],
        std::vec![
            Event::Started {
                requested_micros: 1
            },
            Event::Completed {
                elapsed_micros: 5,
                lateness_micros: 5
            }
        ],
    ] {
        let mut recorder = Recorder::default();
        for event in events {
            recorder.observe(Scope::Dcode, Kind::Completion, event);
        }
        assert!(!recorder.is_complete());
    }
}

#[test]
fn scopes_cannot_complete_each_others_waits() {
    let mut recorder = Recorder::default();
    recorder.observe(
        Scope::Dcode,
        Kind::Completion,
        Event::Started {
            requested_micros: 1,
        },
    );
    recorder.observe(
        Scope::Rfpll,
        Kind::Completion,
        Event::Completed {
            elapsed_micros: 2,
            lateness_micros: 1,
        },
    );
    assert!(!recorder.is_complete());
}

#[test]
fn count_overflow_invalidates_evidence_without_wrapping() {
    let mut recorder = Recorder::default();
    for _ in 0..=u16::MAX {
        completed(&mut recorder, Scope::Rfpll, Kind::Settle, 1, 1);
    }
    assert!(!recorder.is_complete());
    assert_eq!(recorder.report().rfpll_settle.count, u16::MAX);
    assert_eq!(
        recorder.report().rfpll_settle.elapsed_micros,
        u32::from(u16::MAX)
    );
    let mut recorder = Recorder::default();
    for _ in 0..=u16::MAX {
        recorder.pll_lock(false);
    }
    assert!(!recorder.is_complete());
    assert_eq!(recorder.report().pll_unlocked, u16::MAX);
}
