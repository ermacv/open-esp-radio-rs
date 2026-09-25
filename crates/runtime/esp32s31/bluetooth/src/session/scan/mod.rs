//! Passive-scanning first-event and recurring-session adaptation.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first;

#[cfg(target_arch = "riscv32")]
pub use active::{
    PassiveScanActiveDrive, PassiveScanRecurringDrive, drive_passive_scan_active_ready,
    drive_passive_scan_recurring_ready,
};
#[cfg(target_arch = "riscv32")]
pub use first::{
    PassiveScanFirstControllerTimeWait, PassiveScanFirstDrive, PassiveScanFirstResume,
    drive_passive_scan_first_ready,
};
