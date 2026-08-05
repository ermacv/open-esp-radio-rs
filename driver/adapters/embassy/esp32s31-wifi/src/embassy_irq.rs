//! Embassy wake adapter for the executor-neutral S31 interrupt state.
//!
//! Hardware acknowledgement and interrupt classification remain in the chip
//! MAC crate. This facade exposes only executor wake runtimes and the finite
//! platform-route epoch owner.

mod epoch;
mod mac_runtime;
mod power_runtime;

pub use epoch::{
    Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
    Esp32s31MacInterruptEpochDrain, Esp32s31MacInterruptEpochQuiesceError,
    Esp32s31MacInterruptEpochStateError,
};
pub use mac_runtime::{EmbassyMacIrqDrain, EmbassyMacIrqRuntime};
pub use power_runtime::EmbassyPowerIrqRuntime;

#[cfg(test)]
mod tests;
