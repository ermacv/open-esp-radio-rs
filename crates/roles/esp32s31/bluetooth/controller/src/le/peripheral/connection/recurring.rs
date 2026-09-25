//! Recurring peripheral-connection timing.

#![forbid(unsafe_code)]

mod timing;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use timing::PeripheralConnectionRecurringPhase;
#[cfg(any(target_arch = "riscv32", test))]
pub use timing::PeripheralConnectionRecurringTimingError;

pub(crate) use timing::{
    PeripheralConnectionLocalSleepClockAccuracy, PeripheralConnectionRecurringTimingPolicy,
    PeripheralConnectionWindowWideningMode,
};
