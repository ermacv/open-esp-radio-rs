//! Scheduler configuration, time policy and affine hardware transactions.
//!
//! Portable policies remain available on hosts; hardware transactions and
//! timeline admission retain their target-or-test availability.

#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) mod completion;

pub(crate) mod config;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod core;
pub(crate) mod finished_lists;
pub(crate) mod insertion;
pub(crate) mod lock_modify;
pub(crate) mod time;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod timeline;

pub use config::SchedulerSoftwareConfig;

/// Stable task-side access to the shared interrupt owner for scheduler start.
///
/// This is deliberately separate from hard-handler dispatch. Implementations
/// must execute the finite dynamic interrupt preparation synchronously while
/// retaining the owner in the same stable slot. Task-side stop and removal
/// rechecks serialize with live interrupt service without moving that owner.
#[cfg(target_arch = "riscv32")]
pub trait SchedulerRunInterruptStorage {
    /// Monotonic platform time used by the retained quiescence deadline.
    fn monotonic_micros() -> u64;

    /// Serialize one finite common-stop step with the stable ISR register owner.
    /// Storage rejection must return the unchanged sequence.
    fn step_scheduler_stop(
        &self,
        controller: &mut oer_esp32s31_hal::bluetooth::ControllerHal<'_>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopStep,
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    >;

    /// Exact reason the stable owner could not prepare scheduler interrupts.
    type Error;

    /// Clear stale dynamic sources and enable the scheduler-run groups.
    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<oer_esp32s31_hal::bluetooth::BluetoothSchedulerRunInterruptsPrepared, Self::Error>;

    /// Recheck the complete scheduler software-list removal predicate while
    /// the stable interrupt-register owner remains in platform storage.
    ///
    /// Failure returns the unchanged affine empty-head proof.
    fn recheck_scheduler_software_list_removal(
        &self,
        controller: &mut oer_esp32s31_hal::bluetooth::ControllerHal<'_>,
        head: oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Result<
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalJoin,
        oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadEmptyObserved,
    >;
}

#[cfg(any(target_arch = "riscv32", test))]
pub use core::{ControllerTimeAcquisitionError, SchedulerEmptyListMergeError};
#[cfg(target_arch = "riscv32")]
pub use core::{SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState};
#[cfg(any(target_arch = "riscv32", test))]
pub use core::{SchedulerHeadPublicationError, SchedulerInitialized};
#[cfg(any(target_arch = "riscv32", test))]
pub use timeline::{
    SchedulerRawWindow, SchedulerReservationError, SchedulerReservationReleaseError,
    SchedulerReservationReleaseFailure, SchedulerSequenceAuthorizationError,
    SchedulerSequenceReady, SchedulerTimingPolicy, SchedulerWindowReservation,
};

pub use finished_lists::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};

pub use insertion::{
    SchedulerInsertionBeginOutcome, SchedulerInsertionBusyDecision, SchedulerInsertionEndPrelude,
    SchedulerInsertionFinalAction, SchedulerInsertionItemStatusGate,
    SchedulerInsertionLockModifyGate, SchedulerInsertionSleepDecision, SchedulerInsertionSleepGate,
};

pub use lock_modify::{
    BluetoothSchedulerLockModifyInterruptObservation, SchedulerLockModifyBeginError,
    SchedulerLockModifyEvent, SchedulerLockModifyEventCell, SchedulerLockModifyEventPublication,
    SchedulerLockModifyPublicationResult, SchedulerLockModifyWorker, SchedulerLockModifyWorkerStep,
};
