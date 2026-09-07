use super::rounded_tx_power;

#[test]
fn signed_power_requests_follow_the_reviewed_s31_buckets() {
    let cases = [
        (i8::MIN, 0),
        (-16, 0),
        (-15, 3),
        (-13, 3),
        (-12, 4),
        (-10, 4),
        (-9, 5),
        (-7, 5),
        (-6, 6),
        (-4, 6),
        (-3, 7),
        (-1, 7),
        (0, 8),
        (2, 8),
        (3, 9),
        (5, 9),
        (6, 10),
        (8, 10),
        (9, 11),
        (11, 11),
        (12, 12),
        (14, 12),
        (15, 13),
        (17, 13),
        (18, 14),
        (19, 14),
        (20, 15),
        (i8::MAX, 15),
    ];

    for (dbm, expected_bucket) in cases {
        assert_eq!(rounded_tx_power(dbm), expected_bucket);
    }
}
