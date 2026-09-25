//! Direct Test Mode first-event, active, stopping, and session boundaries.

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod stopping;
pub(crate) mod task;

#[cfg(target_arch = "riscv32")]
pub use active::DtmActiveWait;
#[cfg(any(target_arch = "riscv32", test))]
pub use active::{
    DtmActiveCommandSignal, DtmActivePendingSignal, DtmActiveRadioSignal, DtmActiveWaitError,
};
#[cfg(target_arch = "riscv32")]
pub use first::{DtmFirstControllerTimeWait, DtmFirstDrive, DtmFirstResume, drive_dtm_first_ready};
#[cfg(target_arch = "riscv32")]
pub use stopping::DtmShutdownWait;
#[cfg(target_arch = "riscv32")]
pub use stopping::DtmTestEndResponseWait;
#[cfg(any(target_arch = "riscv32", test))]
pub use stopping::DtmTestEndResponseWaitError;
#[cfg(any(target_arch = "riscv32", test))]
pub use stopping::{DtmStoppingSignal, DtmTestEndResponseSignal};

#[cfg(target_arch = "riscv32")]
pub use task::DtmSessionBoundary;

#[cfg(target_arch = "riscv32")]
pub use task::DtmSessionTask;
pub use task::{DtmControllerTimeRecheck, DtmControllerTimeRecheckStatus, DtmSessionRetry};
