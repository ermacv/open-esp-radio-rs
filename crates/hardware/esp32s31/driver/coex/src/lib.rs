#![no_std]
#![forbid(unsafe_code)]

//! Executor-neutral coexistence policy, clock conversion and timer state.
//!
//! Register ownership deliberately lives outside this crate. The radio HAL
//! owns the COEX timer bank and the reviewed shared modem clock fields; this
//! crate implements [`CoexClockHardware`] for the HAL's Wi-Fi MAC capabilities
//! and drives the timer bank only from validation images. Live protocol
//! runtimes do not compose this core into an operational coexistence service.

#[cfg(test)]
extern crate std;

mod clock;
mod core;
mod hal;
mod model;
mod scheduler;
mod timer;

pub use clock::{CoexClockHardware, CoexClockSelector, CoexTimerClock};
pub use core::{CoexCore, CoexStatus};
pub use model::{
    COEX_EVENT_COUNT, COEX_TIMER_COUNT, CoexClient, CoexClientRequest, CoexError,
    CoexEventDurations, CoexEventId, CoexPti, CoexPtiTable, CoexTimerIndex,
};
pub use scheduler::{CoexPhase, CoexSchedule, CoexScheduler};
pub use timer::{CoexTimerHardware, program_timer};

#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(test)]
mod tests;
