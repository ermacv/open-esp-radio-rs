//! Embassy timer binding for finite ESP32-S31 PHY operations.

use embassy_time::Timer;
use open_esp_radio_esp32s31_phy::target_executor::PhyAsyncDelay;

/// Production Embassy delay used by the recovered finite PHY transitions.
///
/// The PHY crate remains executor-independent. Board applications and HIL
/// fixtures select this zero-sized adapter when Embassy owns target time.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31PhyDelay;

impl PhyAsyncDelay for EmbassyEsp32s31PhyDelay {
    fn after_micros(micros: u64) -> impl core::future::Future<Output = ()> {
        Timer::after_micros(micros)
    }
}
