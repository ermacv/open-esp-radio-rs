use core::future::Future;

use embassy_time::Timer;

pub use open_esp_radio_esp32s31_wifi::rx::time::Esp32s31RxFrontierDelay;

/// Production Embassy-time delay adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RxFrontierDelay;

impl Esp32s31RxFrontierDelay for EmbassyEsp32s31RxFrontierDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()> {
        Timer::after_micros(u64::from(micros))
    }
}
