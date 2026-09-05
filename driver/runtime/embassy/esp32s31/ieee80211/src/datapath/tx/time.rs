//! Embassy time adapter for the executor-independent ESP32-S31 STA TX port.

use embassy_time::{Instant, Timer};
use open_esp_radio_esp32s31_wifi::tx::WifiTxTimer;

/// Production Embassy time adapter for ordinary STA transmit transactions.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyWifiTxTimer;

impl WifiTxTimer for EmbassyWifiTxTimer {
    fn now_micros(&self) -> u64 {
        Instant::now().as_micros()
    }

    fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::at(Instant::from_micros(deadline_micros))
    }

    fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        Timer::after_micros(micros)
    }
}
