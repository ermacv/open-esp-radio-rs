use core::future::Future;

use embassy_time::Timer;

/// Executor edge between walker publication and its first live observation.
pub trait Esp32s31RxFrontierDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()>;
}

/// Production Embassy-time delay adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RxFrontierDelay;

impl Esp32s31RxFrontierDelay for EmbassyEsp32s31RxFrontierDelay {
    fn after_micros(micros: u32) -> impl Future<Output = ()> {
        Timer::after_micros(u64::from(micros))
    }
}
