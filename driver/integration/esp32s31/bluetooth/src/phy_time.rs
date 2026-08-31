//! Embassy time binding for ESP32-S31 PHY delay and PLL tracking.

/// Why one microsecond delay cannot be represented as a future absolute
/// Embassy deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbassyEsp32s31PhyTimeError {
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

const fn validate_tick_rate(ticks_per_second: u64) -> Result<(), EmbassyEsp32s31PhyTimeError> {
    if ticks_per_second == EXPECTED_TICKS_PER_SECOND {
        Ok(())
    } else {
        Err(EmbassyEsp32s31PhyTimeError::UnsupportedTickRate { ticks_per_second })
    }
}

const fn checked_deadline_micros(
    now_micros: u64,
    delay_micros: u64,
) -> Result<u64, EmbassyEsp32s31PhyTimeError> {
    match now_micros.checked_add(delay_micros) {
        Some(deadline_micros) => Ok(deadline_micros),
        None => Err(EmbassyEsp32s31PhyTimeError::DeadlineOverflow {
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
pub struct EmbassyEsp32s31PhyTime;

#[cfg(target_arch = "riscv32")]
impl EmbassyEsp32s31PhyTime {
    /// Verify the board-wide Embassy clock contract before claiming hardware.
    pub fn validate_timebase() -> Result<(), EmbassyEsp32s31PhyTimeError> {
        validate_tick_rate(embassy_time::TICK_HZ)
    }

    /// Validate one relative microsecond delay against the current epoch.
    pub fn validate_delay(micros: u64) -> Result<(), EmbassyEsp32s31PhyTimeError> {
        Self::validate_timebase()?;
        checked_deadline_micros(embassy_time::Instant::now().as_micros(), micros).map(|_| ())
    }
}

#[cfg(target_arch = "riscv32")]
impl open_esp_radio_esp32s31_phy::PhyAsyncDelay for EmbassyEsp32s31PhyTime {
    async fn after_micros(micros: u64) {
        if Self::validate_delay(micros).is_err() {
            core::future::pending::<()>().await;
        }
        embassy_time::Timer::after_micros(micros).await;
    }
}

#[cfg(target_arch = "riscv32")]
impl open_esp_radio_esp32s31_phy::phy_client::PhyPllTrackClock for EmbassyEsp32s31PhyTime {
    fn now_micros(&mut self) -> u64 {
        assert!(
            Self::validate_timebase().is_ok(),
            "ESP32-S31 PHY time requires the one-megahertz Embassy driver"
        );
        embassy_time::Instant::now().as_micros()
    }
}

#[cfg(test)]
mod tests {
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
}
