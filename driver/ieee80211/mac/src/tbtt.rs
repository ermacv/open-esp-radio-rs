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
mod tests;
