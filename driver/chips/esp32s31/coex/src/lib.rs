#![no_std]
#![forbid(unsafe_code)]

//! Executor-neutral coexistence policy, clock conversion and timer state.
//!
//! Platform register ownership deliberately lives outside this crate. The
//! custom radio PAC owns the COEX timer bank, while the platform PAC owns the
//! shared low-power clock selector sampled by [`CoexClockHardware`].

#[cfg(test)]
extern crate std;

mod clock;
mod core;
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

#[cfg(test)]
mod tests;
