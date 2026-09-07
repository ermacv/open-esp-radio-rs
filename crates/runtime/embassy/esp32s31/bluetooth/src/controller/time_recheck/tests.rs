use crate::session::dtm::DtmControllerTimeRecheckStatus;

use embassy_time::Duration;

use super::{
    AbsoluteRecheckSchedule, DtmRecheckDeadline, DtmRecheckPeriod, DtmRecheckPeriodError,
    DtmRecheckScheduleState,
};

#[test]
fn period_is_nonzero_and_keeps_executor_tick_units() {
    assert_eq!(
        DtmRecheckPeriod::from_ticks(0),
        Err(DtmRecheckPeriodError::Zero)
    );
    let period = DtmRecheckPeriod::from_duration(Duration::from_ticks(7))
        .expect("a nonzero duration is one valid period");
    assert_eq!(period.as_ticks(), 7);
    assert_eq!(period.as_duration(), Duration::from_ticks(7));
}

#[test]
fn dropping_a_wait_lease_preserves_the_exact_absolute_deadline() {
    let period = DtmRecheckPeriod::from_ticks(5).unwrap();
    let mut schedule = AbsoluteRecheckSchedule::new(DtmRecheckDeadline::from_ticks(100), period);
    {
        let lease = schedule.begin_wait().expect("the first deadline is armed");
        assert_eq!(lease.deadline().as_ticks(), 100);
    }

    assert_eq!(
        schedule.state(),
        DtmRecheckScheduleState::Scheduled(DtmRecheckDeadline::from_ticks(100))
    );
}

#[test]
fn only_completed_wait_advances_from_the_previous_absolute_deadline() {
    let period = DtmRecheckPeriod::from_ticks(5).unwrap();
    let mut schedule = AbsoluteRecheckSchedule::new(DtmRecheckDeadline::from_ticks(100), period);
    schedule
        .begin_wait()
        .expect("the deadline is armed")
        .complete();
    schedule
        .begin_wait()
        .expect("the next absolute deadline is armed")
        .complete();

    assert_eq!(
        schedule.state(),
        DtmRecheckScheduleState::Scheduled(DtmRecheckDeadline::from_ticks(110))
    );
}

#[test]
fn absolute_timeline_exhaustion_is_typed_instead_of_wrapping() {
    let period = DtmRecheckPeriod::from_ticks(2).unwrap();
    let mut schedule =
        AbsoluteRecheckSchedule::new(DtmRecheckDeadline::from_ticks(u64::MAX - 1), period);
    assert_eq!(schedule.status(), DtmControllerTimeRecheckStatus::Scheduled);
    schedule
        .begin_wait()
        .expect("the final representable deadline is armed")
        .complete();

    assert_eq!(schedule.state(), DtmRecheckScheduleState::TimelineExhausted);
    assert_eq!(
        schedule.status(),
        DtmControllerTimeRecheckStatus::TimelineExhausted
    );
    assert!(schedule.begin_wait().is_none());
}
