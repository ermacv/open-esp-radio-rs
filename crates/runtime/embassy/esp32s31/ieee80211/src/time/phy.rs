//! Embassy timer binding for finite ESP32-S31 PHY operations.

use super::delay::{Deadline, HardwareDelay, synchronous_settle};
use embassy_time::{Duration, Instant, Timer};
use oer_esp32s31_phy::executor::wait::Kind;
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
    type ShortDelay = oer_esp32s31_phy::RomShortDelay;

    fn now_micros() -> Option<u64> {
        Some(embassy_time::Instant::now().as_micros())
    }

    fn after_micros_observed(
        kind: Kind,
        micros: u64,
        enabled: bool,
        observe: impl FnMut(oer_esp32s31_phy::executor::wait::Event),
    ) -> impl core::future::Future<Output = ()> {
        let start = Instant::now();
        super::delay::measure(
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

type PlatformHardwareDelay = HardwareDelay<Timer, fn() -> Instant, fn(u32)>;

// Long and scheduling waits retain the Embassy timer. Recovered minimum
// settles up to 20 us are part of the synchronous hardware transaction.
fn hardware_delay(kind: Kind, micros: u64, start: Instant) -> PlatformHardwareDelay {
    let limit = u64::from(oer_esp32s31_phy::RomShortDelay::MAX_MICROS);
    if synchronous_settle(kind, micros, limit) {
        HardwareDelay::Settle {
            micros: u32::try_from(micros).expect("short settle must fit u32"),
            delay: rom_settle,
        }
    } else {
        let deadline = start + Duration::from_micros(micros);
        HardwareDelay::Timer(Deadline {
            deadline,
            timer: Timer::at(deadline),
            now: Instant::now,
        })
    }
}

fn rom_settle(micros: u32) {
    assert!(oer_esp32s31_phy::RomShortDelay::settle_micros(micros));
}
