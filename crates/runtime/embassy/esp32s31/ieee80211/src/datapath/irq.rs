//! Embassy wake adapter for the executor-neutral S31 interrupt state.
//!
//! Hardware acknowledgement and interrupt classification remain in the chip
//! MAC crate. This facade exposes only executor wake runtimes and the finite
//! platform-route epoch owner.
//!
//! `InterruptEpoch::try_pause` retains a separate `MacInterruptPauseRoute::Paused`
//! capability; it never returns cold setup. The S31 esp-hal backend detaches
//! CPU routing without writing the peripheral masks or clearing status. Resume
//! restores the same RX moderation policy before rebinding the level routes,
//! merges retained executor work with new arrivals, and requests one RX probe.
//! `PausedInterruptEpoch::into_stopped` explicitly performs terminal cleanup.
//! This boundary establishes neither MAC/DMA idle nor exclusive RF access.
//! The connected supervisor composes this with a scheduler boundary and a
//! preserved RX walker. Hardware behavior requires the dedicated pause HIL
//! scenario; ordinary disconnect/reassociation does not exercise this path.

mod epoch;
mod mac_runtime;
mod power_runtime;

pub use epoch::{
    InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochDrain,
    MacInterruptEpochQuiesceError, MacInterruptEpochStateError,
};

pub use mac_runtime::{EmbassyMacIrqDrain, EmbassyMacIrqRuntime};

mod pause;
pub use pause::{PausedInterruptEpoch, PausedInterruptOperationFailure};

pub use power_runtime::EmbassyPowerIrqRuntime;

#[cfg(test)]
mod tests;
