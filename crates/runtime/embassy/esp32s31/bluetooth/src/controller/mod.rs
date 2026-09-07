//! Sole Embassy owner for the ESP32-S31 LE Controller command lifecycle.
//!
//! This actor composes the chip-owned idle command transaction, the bounded
//! DTM/advertising first-event runners and the active-session actors. It does
//! not interpret HCI commands or reproduce radio policy. Every awaited future borrows an owner
//! retained in the actor's affine state slot; cancellation therefore leaves
//! the exact lower transaction available to the next `run` call.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
mod recurring;

#[cfg(target_arch = "riscv32")]
pub use recurring::AdvertisingRecurringFailStop;

#[cfg(target_arch = "riscv32")]
use recurring::{
    CandidateMismatch as ConnectableRecurringCandidateMismatch,
    CommandCandidate as ConnectableRecurringCommandCandidate,
    CommandGraphPrepared as ConnectableRecurringCommandGraphPrepared,
    CommandMerged as ConnectableRecurringCommandMerged,
    CommandPrepared as ConnectableRecurringCommandPrepared,
    CommandWait as ConnectableRecurringCommandWait,
    GraphPreparedMismatch as ConnectableRecurringGraphPreparedMismatch,
    MergedMismatch as ConnectableRecurringMergedMismatch,
    PreparedMismatch as ConnectableRecurringPreparedMismatch,
    ResponseCandidate as ConnectableRecurringResponseCandidate,
    ResponseGraphPrepared as ConnectableRecurringResponseGraphPrepared,
    ResponseMerged as ConnectableRecurringResponseMerged,
    ResponsePrepared as ConnectableRecurringResponsePrepared,
    ResponseWait as ConnectableRecurringResponseWait,
    SequencePendingMismatch as ConnectableRecurringSequencePendingMismatch,
    SequencePendingPhase as ConnectableRecurringSequencePendingPhase,
    StopDrive as ConnectableRecurringStopDrive,
    StopDriveHandler as ConnectableRecurringStopDriveHandler,
};

use crate::session::dtm::DtmSessionRetry;

#[cfg(target_arch = "riscv32")]
use crate::{
    notification::RuntimeNotifications,
    session::{
        advertising::{
            LegacyAdvertisingActiveDrive, LegacyAdvertisingDelaySource,
            LegacyAdvertisingFirstControllerTimeWait, LegacyAdvertisingFirstDrive,
            LegacyAdvertisingFirstResume, LegacyAdvertisingRecurringDrive,
            LegacyConnectableAdvertisingFirstControllerTimeWait,
            LegacyConnectableAdvertisingFirstDrive, LegacyConnectableAdvertisingFirstResume,
            LegacyConnectableAdvertisingReadyContinuations,
            LegacyConnectableAdvertisingRecurringCancellationWait,
            drive_legacy_advertising_active_ready, drive_legacy_advertising_first_ready,
            drive_legacy_advertising_recurring_ready,
            drive_legacy_connectable_advertising_active_ready,
            drive_legacy_connectable_advertising_first_ready,
            drive_legacy_connectable_advertising_initial_pending_ready_with,
            drive_legacy_connectable_advertising_pending_ready_with,
            drive_legacy_connectable_advertising_stopping_ready,
            finish_legacy_connectable_advertising_no_connection_stopping_with,
        },
        dtm::{
            DtmControllerTimeRecheck, DtmControllerTimeRecheckStatus, DtmFirstControllerTimeWait,
            DtmFirstDrive, DtmFirstResume, DtmSessionBoundary, DtmSessionTask,
            drive_dtm_first_ready,
        },
        peripheral::{
            LegacyConnectablePeripheralFirstControllerTimeWait,
            LegacyConnectablePeripheralFirstDrive, LegacyConnectablePeripheralFirstDriveStep,
            LegacyConnectablePeripheralFirstResponsePublication,
            LegacyConnectablePeripheralFirstStoppingStep, PeripheralFirstSessionRetry,
            begin_legacy_connectable_peripheral_first_command_ready,
            begin_legacy_connectable_peripheral_first_response_pending,
            begin_legacy_connectable_peripheral_first_stopping,
        },
        scan::{
            PassiveScanActiveDrive, PassiveScanFirstControllerTimeWait, PassiveScanFirstDrive,
            PassiveScanFirstResume, PassiveScanRecurringDrive, drive_passive_scan_active_ready,
            drive_passive_scan_first_ready, drive_passive_scan_recurring_ready,
        },
    },
};
#[cfg(target_arch = "riscv32")]
use core::ops::ControlFlow;
#[cfg(target_arch = "riscv32")]
use embassy_futures::select::{Either, select};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::{
    controller::{
        ControllerIdleCommandIntake, ControllerIdleCommandTask, ControllerIdleResetBarrier,
        ControllerIdleResetCompletion, ControllerIdleResponsePending,
        ControllerIdleResponsePublication, ControllerSchedulerCurrentError,
        SchedulerRunInterruptStorage,
    },
    le::{
        advertising::{
            LegacyAdvertisingActiveCommandIntake, LegacyAdvertisingActiveCommandMismatch,
            LegacyAdvertisingActiveCommandRoute, LegacyAdvertisingActiveFault,
            LegacyAdvertisingActivePendingFault, LegacyAdvertisingActivePendingRadioStep,
            LegacyAdvertisingActiveResponsePending, LegacyAdvertisingActiveResponsePublication,
            LegacyAdvertisingActiveSession, LegacyAdvertisingActiveWait,
            LegacyAdvertisingCpuOwnedCommandIntake, LegacyAdvertisingCpuOwnedCommandMismatch,
            LegacyAdvertisingCpuOwnedCommandRoute, LegacyAdvertisingCpuOwnedResponsePending,
            LegacyAdvertisingCpuOwnedResponsePublication, LegacyAdvertisingDisableResponsePending,
            LegacyAdvertisingDisableResponsePublication, LegacyAdvertisingDisableRestore,
            LegacyAdvertisingDisableRestoreStep, LegacyAdvertisingEventCpuOwned,
            LegacyAdvertisingFirstRunnerFailure, LegacyAdvertisingFirstRunnerRetry,
            LegacyAdvertisingRecurringCommandIntake, LegacyAdvertisingRecurringCommandMismatch,
            LegacyAdvertisingRecurringCommandRoute, LegacyAdvertisingRecurringFault,
            LegacyAdvertisingRecurringOrderProgress, LegacyAdvertisingRecurringOrderState,
            LegacyAdvertisingRecurringResponsePublication, LegacyAdvertisingRecurringRetry,
            LegacyAdvertisingRecurringRunner, LegacyAdvertisingRecurringStart,
            LegacyAdvertisingRecurringStopBegin, LegacyAdvertisingRecurringStopFault,
            LegacyAdvertisingRecurringStopRestore, LegacyAdvertisingRecurringStopRestoreStep,
            LegacyAdvertisingResetCompletion, LegacyAdvertisingResetCompletionReady,
            LegacyAdvertisingResetResponsePending, LegacyAdvertisingResetResponsePublication,
            LegacyAdvertisingResetRestore, LegacyAdvertisingResetRestoreStep,
            LegacyAdvertisingResponsePendingSession, LegacyAdvertisingResponsePublication,
            LegacyAdvertisingStopping, LegacyAdvertisingStoppingFault,
            LegacyAdvertisingStoppingStep, LegacyConnectableAdvertisingActivePendingFailStop,
            LegacyConnectableAdvertisingActiveResponsePending,
            LegacyConnectableAdvertisingActiveResponsePublication,
            LegacyConnectableAdvertisingActiveWait, LegacyConnectableAdvertisingCommandIntake,
            LegacyConnectableAdvertisingCommandMismatch, LegacyConnectableAdvertisingCommandRoute,
            LegacyConnectableAdvertisingFirstRunnerFailStop,
            LegacyConnectableAdvertisingFirstRunnerFailure,
            LegacyConnectableAdvertisingFirstRunnerRetry,
            LegacyConnectableAdvertisingHciActiveFailStop,
            LegacyConnectableAdvertisingHciActiveSession,
            LegacyConnectableAdvertisingHciActiveStep, LegacyConnectableAdvertisingResponsePending,
            LegacyConnectableAdvertisingResponsePublication, LegacyConnectableAdvertisingStopping,
            LegacyConnectableAdvertisingStoppingFailStop, LegacyConnectableAdvertisingStoppingStep,
        },
        dtm::{
            ControllerIdleCommandMismatch, ControllerIdleCommandRoute, DtmActiveCommandMismatch,
            DtmActiveSessionFault, DtmFirstPreparationCleanup, DtmFirstPreparationCleanupStep,
            DtmFirstPreparationCompletion, DtmFirstPreparationFailStop, DtmFirstRunnerFailure,
            DtmFirstRunnerRetry, DtmOrderReady, DtmResetCompletionReady, DtmResetCompletionStart,
            DtmResetResponsePending, DtmResetResponsePublication, DtmResetRestoreFailure,
            DtmResetRestoreStep, DtmResetStoppingFault, DtmResetStoppingRunner,
            DtmResetStoppingStep, DtmResetStoppingWait, DtmResponsePending,
        },
        peripheral::{
            LegacyConnectablePeripheralFirstHciAxis, LegacyConnectablePeripheralFirstHciFailStop,
            LegacyConnectablePeripheralFirstHciResetFailStop,
            LegacyConnectablePeripheralFirstHciResetOutcome,
            LegacyConnectablePeripheralFirstHciResponsePublication,
            LegacyConnectablePeripheralFirstHciRunning,
        },
        scanning::{
            PassiveScanHciActiveCommandIntake, PassiveScanHciActiveCommandMismatch,
            PassiveScanHciActiveCommandRoute, PassiveScanHciActiveFault,
            PassiveScanHciActivePendingFault, PassiveScanHciActivePendingRadioStep,
            PassiveScanHciActiveResponsePending, PassiveScanHciActiveResponsePublication,
            PassiveScanHciActiveSession, PassiveScanHciCommandIntake,
            PassiveScanHciCommandMismatch, PassiveScanHciCommandRoute,
            PassiveScanHciCpuResponsePending, PassiveScanHciCpuResponsePublication,
            PassiveScanHciFirstRunnerFailure, PassiveScanHciRecurringFailure,
            PassiveScanHciRecurringRunner, PassiveScanHciReportStep, PassiveScanHciReportsComplete,
            PassiveScanHciReportsPending, PassiveScanHciResponsePendingSession,
            PassiveScanHciResponsePublication, PassiveScanHciStopping, PassiveScanHciStoppingFault,
            PassiveScanHciStoppingStep,
        },
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    },
};

mod dispatch;
#[cfg(any(target_arch = "riscv32", test))]
mod owner;
#[cfg(target_arch = "riscv32")]
mod reset;
mod response;

pub(super) mod modem_timer;
#[cfg(any(test, target_arch = "riscv32"))]
pub(super) mod time_recheck;

pub use dispatch::ControllerCommandPhase;
#[cfg(target_arch = "riscv32")]
pub use owner::ControllerCommandTask;
#[cfg(target_arch = "riscv32")]
pub use response::ControllerCommandBoundary;

pub use response::{ControllerIdleCompletion, ControllerRetry};

#[cfg(any(target_arch = "riscv32", test))]
use dispatch::{
    ControllerCommandAction, ControllerCommandStimulus, reduce_controller_command_transition,
};
#[cfg(test)]
use owner::ControllerOwnerSlot;
#[cfg(target_arch = "riscv32")]
use owner::{
    ControllerCommandState, FirstCleanupReadiness, LegacyAdvertisingStopOrigin,
    UnownedFinishedListOwner,
};

#[cfg(test)]
mod tests;

#[cfg(target_arch = "riscv32")]
pub use modem_timer::{ModemTimerDriveStep, ModemTimerDriver};

pub use modem_timer::ModemTimerWakers;
#[cfg(target_arch = "riscv32")]
pub use time_recheck::{
    DtmAbsoluteRecheck, DtmAbsoluteRecheckWait, DtmRecheckDeadline, DtmRecheckPeriod,
    DtmRecheckPeriodError, DtmRecheckScheduleState, DtmRecheckStartError,
};
