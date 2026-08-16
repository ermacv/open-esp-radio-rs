//! Finite ESP32-S31 RX descriptor-frontier ownership.
//!
//! This module owns prepared/live/halted DMA lifecycle and finite descriptor
//! service. Scan, monitor, station and access-point policy wrap this owner but
//! do not duplicate its hardware state machine.

#![forbid(unsafe_code)]

mod lifecycle;
mod state;
mod time;

pub use state::{
    Esp32s31RecycledRxDirective, Esp32s31RecycledRxProgress, Esp32s31RxFrontier,
    Esp32s31RxFrontierContinuation, Esp32s31RxFrontierDirective, Esp32s31RxFrontierError,
    Esp32s31RxFrontierIntoLiveFailure, Esp32s31RxFrontierPhase, Esp32s31RxFrontierProgress,
    Esp32s31RxFrontierSchedulerSnapshot, Esp32s31RxFrontierServiceProgress,
};
pub use time::{EmbassyEsp32s31RxFrontierDelay, Esp32s31RxFrontierDelay};

#[cfg(test)]
mod tests;
