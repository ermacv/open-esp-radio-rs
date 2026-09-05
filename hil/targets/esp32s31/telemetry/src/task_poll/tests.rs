use super::*;

#[test]
fn interval_delta_retains_lifetime_maximum() {
    let counters = TaskPollCounters::new();
    let before = counters.snapshot();
    counters.record(101);
    counters.record(5_001);
    let delta = counters.snapshot().wrapping_delta_since(before);
    assert_eq!(delta.polls, 2);
    assert_eq!(delta.poll_micros, 5_102);
    assert_eq!(delta.lifetime_max_micros, 5_001);
    assert_eq!(delta.over_100_micros, 2);
    assert_eq!(delta.over_5_000_micros, 1);
}

#[test]
fn preaggregated_batch_preserves_every_counter() {
    let counters = TaskPollCounters::new();
    counters.record_batch(TaskPollSnapshot {
        polls: 256,
        poll_micros: 8_192,
        lifetime_max_micros: 731,
        over_100_micros: 3,
        over_500_micros: 1,
        over_1_000_micros: 0,
        over_5_000_micros: 0,
    });
    counters.record_batch(TaskPollSnapshot {
        polls: 17,
        poll_micros: 411,
        lifetime_max_micros: 89,
        over_100_micros: 0,
        over_500_micros: 0,
        over_1_000_micros: 0,
        over_5_000_micros: 0,
    });

    assert_eq!(
        counters.snapshot(),
        TaskPollSnapshot {
            polls: 273,
            poll_micros: 8_603,
            lifetime_max_micros: 731,
            over_100_micros: 3,
            over_500_micros: 1,
            over_1_000_micros: 0,
            over_5_000_micros: 0,
        }
    );
}
