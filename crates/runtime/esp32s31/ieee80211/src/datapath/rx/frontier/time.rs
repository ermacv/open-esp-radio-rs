use core::future::Future;

use embassy_time::Timer;

pub use oer_esp32s31_wifi::rx::time::RxFrontierDelay;

/// Production Embassy-time delay adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyRxFrontierDelay;

impl RxFrontierDelay for EmbassyRxFrontierDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()> {
        Timer::after_micros(u64::from(micros))
    }
}
