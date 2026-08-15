//! RX descriptor ownership across Authentication, Association and WPA2.
//!
//! This facade keeps the stable owner path while executor time, type-state
//! vocabulary and finite DMA transitions remain separate.

#![forbid(unsafe_code)]

mod lifecycle;
mod state;
mod time;

pub use state::{
    Esp32s31PreconnectedRx, Esp32s31PreconnectedRxContinuation, Esp32s31PreconnectedRxDirective,
    Esp32s31PreconnectedRxError, Esp32s31PreconnectedRxIntoLiveFailure,
    Esp32s31PreconnectedRxPhase, Esp32s31PreconnectedRxProgress,
    Esp32s31PreconnectedRxSchedulerSnapshot, Esp32s31RecycledRxDirective,
    Esp32s31RecycledRxProgress,
};
pub use time::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRxDelay};

#[cfg(test)]
mod tests;
