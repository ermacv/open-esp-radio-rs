#![no_std]
#![forbid(unsafe_code)]

//! Embassy-time binding for ESP32-S31 PHY waits and tracking clocks.
//!
//! Every protocol runtime drives PHY registration, tracking and lifecycle
//! transactions through the one zero-sized [`EmbassyPhyTime`]:
//!
//! - Minimum hardware settles of at most the ROM delay limit (20 us) run
//!   synchronously through the ESP32-S31 ROM `ets_delay_us` loop, matching the
//!   recovered vendor execution shape. Readiness retries keep their timer
//!   cadence regardless of size.
//! - Every other wait uses an absolute Embassy deadline. An already elapsed
//!   deadline completes without an executor yield; a future deadline keeps the
//!   ordinary timer wake registration.
//! - The binding requires the board's one-megahertz Embassy tick. A deadline
//!   that cannot be represented in the monotonic microsecond epoch never
//!   completes, rather than wrapping or finishing a hardware wait early.
//!
//! The PHY tracking scheduler and the transaction deadline read the same
//! Embassy clock, so both observe one time domain.

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod delay;
mod time;

pub use time::{EmbassyPhyTime, EmbassyPhyTimeError};
