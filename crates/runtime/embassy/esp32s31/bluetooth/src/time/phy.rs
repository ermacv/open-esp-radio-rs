//! Embassy time binding for ESP32-S31 PHY delay and PLL tracking.

/// Why one microsecond delay cannot be represented as a future absolute
/// Embassy deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyPhyTimeError {
    /// This integration requires the board's one-tick-per-microsecond driver.
    UnsupportedTickRate {
        /// Configured Embassy ticks per second.
        ticks_per_second: u64,
    },
    /// Adding the requested delay would wrap the monotonic microsecond epoch.
    DeadlineOverflow {
        /// Current monotonic time in microseconds since boot.
        now_micros: u64,
        /// Requested relative delay in the same microsecond unit.
        delay_micros: u64,
    },
}

const EXPECTED_TICKS_PER_SECOND: u64 = 1_000_000;

const fn validate_tick_rate(ticks_per_second: u64) -> Result<(), EmbassyPhyTimeError> {
    if ticks_per_second == EXPECTED_TICKS_PER_SECOND {
        Ok(())
    } else {
        Err(EmbassyPhyTimeError::UnsupportedTickRate { ticks_per_second })
    }
}

const fn checked_deadline_micros(
    now_micros: u64,
    delay_micros: u64,
) -> Result<u64, EmbassyPhyTimeError> {
    match now_micros.checked_add(delay_micros) {
        Some(deadline_micros) => Ok(deadline_micros),
        None => Err(EmbassyPhyTimeError::DeadlineOverflow {
            now_micros,
            delay_micros,
        }),
    }
}

/// Zero-sized production Embassy clock for finite PHY operations.
///
/// Both lower traits use `u64` microseconds, exactly matching Embassy's
/// `Timer::after_micros` and `Instant::as_micros` interfaces. There is no unit
/// narrowing or integer cast. The explicit validation method lets composition
/// reject a relative delay whose absolute deadline would wrap. The infallible
/// lower delay trait fail-stops on that impossible long-uptime condition rather
/// than wrapping the monotonic epoch or completing the hardware wait early.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyPhyTime;

#[cfg(target_arch = "riscv32")]
impl EmbassyPhyTime {
    /// Verify the board-wide Embassy clock contract before claiming hardware.
    pub fn validate_timebase() -> Result<(), EmbassyPhyTimeError> {
        validate_tick_rate(embassy_time::TICK_HZ)
    }

    /// Validate one relative microsecond delay against the current epoch.
    pub fn validate_delay(micros: u64) -> Result<(), EmbassyPhyTimeError> {
        Self::validate_timebase()?;
        checked_deadline_micros(embassy_time::Instant::now().as_micros(), micros).map(|_| ())
    }
}

#[cfg(target_arch = "riscv32")]
impl oer_esp32s31_phy::PhyAsyncDelay for EmbassyPhyTime {
    fn now_micros() -> Option<u64> {
        Some(embassy_time::Instant::now().as_micros())
    }

    async fn after_micros(micros: u64) {
        if Self::validate_delay(micros).is_err() {
            core::future::pending::<()>().await;
        }
        embassy_time::Timer::after_micros(micros).await;
    }
}

#[cfg(target_arch = "riscv32")]
impl oer_esp32s31_phy::state::client::PhyPllTrackClock for EmbassyPhyTime {
    fn now_micros(&mut self) -> u64 {
        assert!(
            Self::validate_timebase().is_ok(),
            "ESP32-S31 PHY time requires the one-megahertz Embassy driver"
        );
        embassy_time::Instant::now().as_micros()
    }
}

#[cfg(test)]
mod tests;
