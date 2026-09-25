//! The zero-sized Embassy time owner shared by every PHY caller.

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

#[cfg(any(target_arch = "riscv32", test))]
const EXPECTED_TICKS_PER_SECOND: u64 = 1_000_000;

#[cfg(any(target_arch = "riscv32", test))]
const fn validate_tick_rate(ticks_per_second: u64) -> Result<(), EmbassyPhyTimeError> {
    if ticks_per_second == EXPECTED_TICKS_PER_SECOND {
        Ok(())
    } else {
        Err(EmbassyPhyTimeError::UnsupportedTickRate { ticks_per_second })
    }
}

#[cfg(any(target_arch = "riscv32", test))]
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

/// Zero-sized production Embassy clock, delay and tracking timer for PHY
/// operations.
///
/// Every interface uses `u64` microseconds, exactly matching Embassy's
/// `Instant::as_micros` and `Instant::from_micros`; there is no unit narrowing.
/// Composition can reject an unsupported timebase or a delay whose absolute
/// deadline would wrap before claiming hardware. The infallible lower delay
/// trait fail-stops on that impossible long-uptime condition instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyPhyTime;

#[cfg(target_arch = "riscv32")]
mod target {
    use embassy_time::{Instant, Timer};
    use oer_esp32s31_phy::{
        PhyAsyncDelay, RomShortDelay,
        executor::wait::{Event, Kind},
        state::client::{PhyPllTrackClock, PhyTrackingTimer},
    };

    use super::{EmbassyPhyTime, EmbassyPhyTimeError, checked_deadline_micros, validate_tick_rate};
    use crate::delay::{Deadline, HardwareDelay, measure, synchronous_settle};

    impl EmbassyPhyTime {
        /// Verify the board-wide Embassy clock contract before claiming hardware.
        pub fn validate_timebase() -> Result<(), EmbassyPhyTimeError> {
            validate_tick_rate(embassy_time::TICK_HZ)
        }

        /// Validate one relative microsecond delay against the current epoch.
        pub fn validate_delay(micros: u64) -> Result<(), EmbassyPhyTimeError> {
            Self::validate_timebase()?;
            checked_deadline_micros(Instant::now().as_micros(), micros).map(|_| ())
        }

        fn now() -> u64 {
            assert!(
                Self::validate_timebase().is_ok(),
                "ESP32-S31 PHY time requires the one-megahertz Embassy driver"
            );
            Instant::now().as_micros()
        }
    }

    impl PhyAsyncDelay for EmbassyPhyTime {
        type ShortDelay = RomShortDelay;

        fn now_micros() -> Option<u64> {
            Some(Self::now())
        }

        fn after_micros_observed(
            kind: Kind,
            micros: u64,
            enabled: bool,
            observe: impl FnMut(Event),
        ) -> impl core::future::Future<Output = ()> {
            let start = Instant::now();
            measure(
                hardware_delay(kind, micros, start),
                start,
                micros,
                Instant::now,
                enabled,
                observe,
            )
        }

        fn after_micros(kind: Kind, micros: u64) -> impl core::future::Future<Output = ()> {
            hardware_delay(kind, micros, Instant::now())
        }
    }

    impl PhyPllTrackClock for EmbassyPhyTime {
        fn now_micros(&mut self) -> u64 {
            Self::now()
        }
    }

    impl PhyTrackingTimer for EmbassyPhyTime {
        async fn wait_until_micros(&mut self, deadline: u64) {
            Timer::at(Instant::from_micros(deadline)).await;
        }
    }

    type PlatformHardwareDelay = HardwareDelay<Timer, fn() -> Instant, fn(u32)>;

    // Long and scheduling waits retain the Embassy timer. Recovered minimum
    // settles up to 20 us are part of the synchronous hardware transaction.
    fn hardware_delay(kind: Kind, micros: u64, start: Instant) -> PlatformHardwareDelay {
        let limit = u64::from(RomShortDelay::MAX_MICROS);
        if synchronous_settle(kind, micros, limit) {
            return HardwareDelay::Settle {
                micros: u32::try_from(micros).expect("short settle must fit u32"),
                delay: rom_settle,
            };
        }
        match checked_deadline_micros(start.as_micros(), micros) {
            Ok(deadline) => {
                let deadline = Instant::from_micros(deadline);
                HardwareDelay::Timer(Deadline {
                    deadline,
                    timer: Timer::at(deadline),
                    now: Instant::now,
                })
            }
            Err(_) => HardwareDelay::Unrepresentable,
        }
    }

    fn rom_settle(micros: u32) {
        assert!(RomShortDelay::settle_micros(micros));
    }
}

#[cfg(test)]
mod tests;
