//! Embassy timer binding for finite ESP32-S31 PHY operations.

use super::delay::Deadline;
use embassy_time::{Duration, Instant, Timer};
use oer_esp32s31_phy::state::client::{PhyPllTrackClock, PhyTrackingTimer};
use oer_esp32s31_phy::target_executor::PhyAsyncDelay;

/// Absolute Embassy timer for PHY maintenance deadlines.
#[derive(Default)]
pub struct EmbassyPhyClock;

impl PhyPllTrackClock for EmbassyPhyClock {
    fn now_micros(&mut self) -> u64 {
        embassy_time::Instant::now().as_micros()
    }
}

impl PhyTrackingTimer for EmbassyPhyClock {
    async fn wait_until_micros(&mut self, deadline: u64) {
        Timer::at(embassy_time::Instant::from_micros(deadline)).await;
    }
}

/// Production Embassy delay used by the recovered finite PHY transitions.
///
/// The PHY crate remains executor-independent. Board applications and HIL
/// fixtures select this zero-sized adapter when Embassy owns target time.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyPhyDelay;

impl PhyAsyncDelay for EmbassyPhyDelay {
    fn now_micros() -> Option<u64> {
        Some(embassy_time::Instant::now().as_micros())
    }

    fn after_micros_observed(
        micros: u64,
        enabled: bool,
        observe: impl FnMut(oer_esp32s31_phy::executor::wait::Event),
    ) -> impl core::future::Future<Output = ()> {
        let start = Instant::now();
        let deadline = start + Duration::from_micros(micros);
        super::delay::measure(
            Deadline {
                deadline,
                timer: Timer::at(deadline),
                now: Instant::now,
            },
            start,
            micros,
            Instant::now,
            enabled,
            observe,
        )
    }

    fn after_micros(micros: u64) -> impl core::future::Future<Output = ()> {
        let deadline = Instant::now() + Duration::from_micros(micros);
        Deadline {
            deadline,
            timer: Timer::at(deadline),
            now: Instant::now,
        }
    }
}
