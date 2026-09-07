use super::next_tbtt_delay;

fn iterative(send_tick: u32, interval: u32, now: u32) -> (u32, u32) {
    let mut next = send_tick;
    loop {
        next = next.wrapping_add(interval);
        let remaining = next.wrapping_sub(now);
        if remaining <= interval {
            return (next, remaining / 1_000 + 1);
        }
    }
}

#[test]
fn matches_the_recovered_loop_for_bounded_catch_up_cases() {
    for interval in [1, 999, 1_000, 4_096, 102_400] {
        for elapsed in [0, 1, interval - 1, interval, interval + 1, interval * 7] {
            let send_tick = 0x1000_0000_u32;
            let now = send_tick.wrapping_add(elapsed);
            assert_eq!(
                next_tbtt_delay(send_tick, interval, now),
                Some(iterative(send_tick, interval, now))
            );
        }
    }
}

#[test]
fn handles_tsf_wrap_and_full_u32_catch_up_in_constant_time() {
    assert_eq!(
        next_tbtt_delay(0xffff_f000, 0x1000, 0x1000),
        Some((0x1000, 1))
    );
    assert_eq!(next_tbtt_delay(1, 1, 0), Some((0, 1)));
}

#[test]
fn rejects_zero_interval_instead_of_spinning() {
    assert_eq!(next_tbtt_delay(0, 0, 0), None);
}
