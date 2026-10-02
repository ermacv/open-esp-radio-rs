use super::*;

#[test]
fn a_matching_response_ends_the_procedure() {
    let mut query = StationSaQuery::new();
    assert_eq!(query.start(1_000, 65_525 + 7), Some([7, 0]));
    assert_eq!(query.start(1_100, 3), None);
    assert!(!query.response([6, 0]));
    assert!(query.is_active());
    assert!(query.response([7, 0]));
    assert!(!query.is_active());
    assert_eq!(query.step(2_000_000), SaQueryStep::Idle);
}

#[test]
fn retries_every_200_ms_and_accepts_any_request_of_the_procedure() {
    let mut query = StationSaQuery::new();
    query.start(0, 100);
    assert_eq!(query.deadline_micros(), Some(200_000));
    assert_eq!(query.step(199_999), SaQueryStep::Idle);
    assert_eq!(query.step(200_000), SaQueryStep::Request([101, 0]));
    assert_eq!(query.step(400_000), SaQueryStep::Request([102, 0]));
    assert!(!query.response([99, 0]));
    assert!(!query.response([103, 0]));
    assert!(query.response([100, 0]));
}

#[test]
fn no_response_within_1024_ms_times_out() {
    let mut query = StationSaQuery::new();
    query.start(0, 0);
    for expected in 1..=5_u8 {
        assert_eq!(
            query.step(u64::from(expected) * 200_000),
            SaQueryStep::Request([expected, 0])
        );
    }
    assert_eq!(query.deadline_micros(), Some(1_024_000));
    assert_eq!(query.step(1_024_000), SaQueryStep::TimedOut);
    assert!(!query.is_active());
    assert_eq!(query.start(1_100_000, 9), Some([9, 0]));
}
