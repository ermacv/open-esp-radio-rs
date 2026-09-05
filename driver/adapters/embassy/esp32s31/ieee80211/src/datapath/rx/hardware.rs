//! ESP32-S31 RX append completion and independent link-release delays.

use core::future::Future;

use embassy_time::Timer;

/// Timer capability for cooperative RX ownership probes and walker settle.
pub trait RxDmaObservationDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_;
}

/// Executor timer used by a normal ESP32-S31 connected RX owner.
///
/// HIL compositions may wrap the same edge to collect timing evidence, but
/// the driver itself only requires finite asynchronous delays at explicitly
/// schedulable ownership edges. The reload suffix is intentionally not one.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyEsp32s31RxDmaObservationDelay;

impl RxDmaObservationDelay for EmbassyEsp32s31RxDmaObservationDelay {
    fn after_micros(&mut self, micros: u32) -> impl Future<Output = ()> + '_ {
        Timer::after_micros(u64::from(micros))
    }
}

#[cfg(test)]
mod tests;
