use super::{
    EmbassyEsp32s31PhyTime, EmbassyEsp32s31PhyTimeError, checked_deadline_micros,
    validate_tick_rate,
};

#[test]
fn production_time_adapter_is_zero_sized() {
    assert_eq!(size_of::<EmbassyEsp32s31PhyTime>(), 0);
}

#[test]
fn only_the_board_microsecond_timebase_is_admitted() {
    assert_eq!(validate_tick_rate(1_000_000), Ok(()));
    assert_eq!(
        validate_tick_rate(1_000),
        Err(EmbassyEsp32s31PhyTimeError::UnsupportedTickRate {
            ticks_per_second: 1_000,
        })
    );
}

#[test]
fn delay_and_deadline_use_the_same_microsecond_unit() {
    assert_eq!(checked_deadline_micros(1_000_000, 37), Ok(1_000_037));
    assert_eq!(checked_deadline_micros(0, 0), Ok(0));
}

#[test]
fn absolute_deadline_overflow_is_typed_without_wrapping() {
    assert_eq!(
        checked_deadline_micros(u64::MAX - 2, 3),
        Err(EmbassyEsp32s31PhyTimeError::DeadlineOverflow {
            now_micros: u64::MAX - 2,
            delay_micros: 3,
        })
    );
    assert_eq!(checked_deadline_micros(u64::MAX - 2, 2), Ok(u64::MAX));
}
