#![no_std]
#![forbid(unsafe_code)]

//! Executor-neutral coexistence policy, clock conversion and timer state.
//!
//! Register ownership deliberately lives outside this crate. The radio
//! arbiter owns the COEX timer bank, the event priority table and the reviewed
//! shared modem clock fields; this crate is the policy that drives them
//! through its ports. [`CoexCore`] programs event requests on the timer bank;
//! [`CoexSchedule`] is the recovered time-slice schedule that selects a
//! scheme from the radios' status and steps its phases. Live protocol
//! runtimes do not compose either into an operational coexistence service
//! yet.

#[cfg(test)]
extern crate std;

mod clock;
mod core;
mod hal;
mod model;
mod schedule;
mod timer;

pub use clock::{CoexClockHardware, CoexClockSelector, CoexTimerClock};
pub use core::{CoexCore, CoexStatus};
pub use model::{
    COEX_EVENT_COUNT, COEX_TIMER_COUNT, CoexClient, CoexClientRequest, CoexError,
    CoexEventDurations, CoexEventId, CoexPti, CoexPtiTable, CoexTimerIndex, timer_index,
};
pub use schedule::{
    CoexPhase, CoexPhaseStep, CoexSchedule, CoexScheduleIdle, CoexScheme, CoexSchemeId,
    CoexStatusType, CoexStatusWords, wifi_status,
};
pub use timer::{CoexTimerHardware, program_timer};

#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;

#[cfg(test)]
mod tests;
