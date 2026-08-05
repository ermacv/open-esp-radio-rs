/// Advance the wrapping beacon tick to the first interval after `now` and
/// return the vendor-compatible microsecond alarm delay.
///
/// This is the closed-form equivalent of repeatedly adding `interval` until
/// `next_tick.wrapping_sub(now) <= interval`. It performs no catch-up loop.
pub const fn next_tbtt_delay(send_tick: u32, interval: u32, now: u32) -> Option<(u32, u32)> {
    if interval == 0 {
        return None;
    }

    // The vendor body always adds once, but an exact interval multiple may
    // stop at zero remaining distance. The quotient can be u32::MAX only when
    // the remainder is zero, so the ceiling increment cannot overflow.
    let elapsed = now.wrapping_sub(send_tick);
    let steps = if elapsed == 0 {
        1
    } else {
        let quotient = elapsed / interval;
        quotient.wrapping_add((!elapsed.is_multiple_of(interval)) as u32)
    };
    let advance = steps.wrapping_mul(interval);
    let next_tick = send_tick.wrapping_add(advance);
    let remaining = next_tick.wrapping_sub(now);
    Some((next_tick, remaining / 1_000 + 1))
}

#[cfg(test)]
mod tests {
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
}
