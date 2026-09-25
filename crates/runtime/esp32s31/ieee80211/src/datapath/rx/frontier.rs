//! Finite ESP32-S31 RX descriptor-frontier ownership.
//!
//! This module owns prepared/live/halted DMA lifecycle and finite descriptor
//! service. Scan, monitor, station and access-point policy wrap this owner but
//! do not duplicate its hardware state machine.

#![forbid(unsafe_code)]

mod time;

pub use oer_esp32s31_wifi::rx::frontier::{
    ReceiveFrontier, RxFrontierContinuation, RxFrontierDirective, RxFrontierError,
    RxFrontierIntoLiveFailure, RxFrontierPhase, RxFrontierProgress, RxFrontierSchedulerSnapshot,
    RxFrontierServiceProgress,
};

pub use time::{EmbassyRxFrontierDelay, RxFrontierDelay};
