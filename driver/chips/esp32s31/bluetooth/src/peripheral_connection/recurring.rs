//! Recurring peripheral-connection timing.

#![forbid(unsafe_code)]

mod timing;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use timing::BluetoothPeripheralConnectionRecurringPhase;
#[cfg(any(target_arch = "riscv32", test))]
pub use timing::BluetoothPeripheralConnectionRecurringTimingError;
pub(crate) use timing::{
    BluetoothPeripheralConnectionLocalSleepClockAccuracy,
    BluetoothPeripheralConnectionRecurringTimingPolicy,
    BluetoothPeripheralConnectionWindowWideningMode,
};
