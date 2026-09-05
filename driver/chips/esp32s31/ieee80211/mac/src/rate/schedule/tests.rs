use super::*;

#[test]
fn dot11g_54m_record_has_the_vendor_retry_ladder() {
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11G, 0).unwrap();
    assert_eq!(schedule_publication_limit(schedule), 32);
    assert_eq!(schedule_rate_after_failures(schedule, 0), Some(0x0c));
    assert_eq!(schedule_rate_after_failures(schedule, 1), Some(0x0c));
    assert_eq!(schedule_rate_after_failures(schedule, 2), Some(0x08));
    assert_eq!(schedule_rate_after_failures(schedule, 3), Some(0x08));
    assert_eq!(schedule_rate_after_failures(schedule, 4), Some(0x0b));
    assert_eq!(schedule_rate_after_failures(schedule, 6), Some(0x0b));
    assert_eq!(schedule_rate_after_failures(schedule, 7), Some(0x06));
    assert_eq!(schedule_rate_after_failures(schedule, 31), Some(0x06));
    assert_eq!(schedule_rate_after_failures(schedule, 32), None);
}

#[test]
fn post_attach_indices_are_materialized_in_every_arena() {
    let mut total = 0;
    for kind in RATE_SCHEDULE_KINDS {
        for index in 0..kind.record_count() {
            let schedule = RateScheduleRef::new(kind, index as u8).unwrap();
            assert_eq!(schedule_state(schedule).index, index as u8);
            total += 1;
        }
    }
    assert_eq!(total, 71);
}

#[test]
fn recovered_boundary_records_match_the_pinned_tables() {
    assert_eq!(
        schedule_state(RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap()),
        RateScheduleRecordState {
            rate: 0x23,
            retry_limit: 2,
            index: 0,
            adaptive: 0,
        }
    );
    assert_eq!(
        schedule_state(RateScheduleRef::new(RateScheduleKind::Dot11N, 13).unwrap()),
        RateScheduleRecordState {
            rate: 0x29,
            retry_limit: 2,
            index: 13,
            adaptive: 0,
        }
    );
    assert!(RateScheduleRef::new(RateScheduleKind::Lora, 2).is_none());
}
