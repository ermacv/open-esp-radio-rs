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
pub use recurring::EmbassyBluetoothLegacyConnectableAdvertisingRecurringFailStop;

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

use crate::EmbassyBluetoothDtmSessionRetry;

#[cfg(target_arch = "riscv32")]
use core::ops::ControlFlow;
#[cfg(target_arch = "riscv32")]
use embassy_futures::select::{Either, select};
#[cfg(target_arch = "riscv32")]
use embassy_sync::blocking_mutex::raw::RawMutex;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame, LeControllerCommandEndpoint,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothControllerIdleCommandIntake, BluetoothControllerIdleCommandMismatch,
    BluetoothControllerIdleCommandRoute, BluetoothControllerIdleCommandTask,
    BluetoothControllerIdleResetBarrier, BluetoothControllerIdleResetCompletion,
    BluetoothControllerIdleResponsePending, BluetoothControllerIdleResponsePublication,
    BluetoothControllerSchedulerCurrentError, BluetoothDtmActiveCommandMismatch,
    BluetoothDtmActiveSessionFault, BluetoothDtmFirstPreparationCleanup,
    BluetoothDtmFirstPreparationCleanupStep, BluetoothDtmFirstPreparationCompletion,
    BluetoothDtmFirstPreparationFailStop, BluetoothDtmFirstRunnerFailure,
    BluetoothDtmFirstRunnerRetry, BluetoothDtmOrderReady, BluetoothDtmResetCompletionReady,
    BluetoothDtmResetCompletionStart, BluetoothDtmResetResponsePending,
    BluetoothDtmResetResponsePublication, BluetoothDtmResetRestoreFailure,
    BluetoothDtmResetRestoreStep, BluetoothDtmResetStoppingFault, BluetoothDtmResetStoppingRunner,
    BluetoothDtmResetStoppingStep, BluetoothDtmResetStoppingWait, BluetoothDtmResponsePending,
    BluetoothLegacyAdvertisingActiveCommandIntake, BluetoothLegacyAdvertisingActiveCommandMismatch,
    BluetoothLegacyAdvertisingActiveCommandRoute, BluetoothLegacyAdvertisingActiveFault,
    BluetoothLegacyAdvertisingActivePendingFault, BluetoothLegacyAdvertisingActivePendingRadioStep,
    BluetoothLegacyAdvertisingActiveResponsePending,
    BluetoothLegacyAdvertisingActiveResponsePublication, BluetoothLegacyAdvertisingActiveSession,
    BluetoothLegacyAdvertisingActiveWait, BluetoothLegacyAdvertisingCpuOwnedCommandIntake,
    BluetoothLegacyAdvertisingCpuOwnedCommandMismatch,
    BluetoothLegacyAdvertisingCpuOwnedCommandRoute,
    BluetoothLegacyAdvertisingCpuOwnedResponsePending,
    BluetoothLegacyAdvertisingCpuOwnedResponsePublication,
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingDisableResponsePublication, BluetoothLegacyAdvertisingDisableRestore,
    BluetoothLegacyAdvertisingDisableRestoreStep, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothLegacyAdvertisingFirstRunnerFailure, BluetoothLegacyAdvertisingFirstRunnerRetry,
    BluetoothLegacyAdvertisingRecurringCommandIntake,
    BluetoothLegacyAdvertisingRecurringCommandMismatch,
    BluetoothLegacyAdvertisingRecurringCommandRoute, BluetoothLegacyAdvertisingRecurringFault,
    BluetoothLegacyAdvertisingRecurringOrderProgress,
    BluetoothLegacyAdvertisingRecurringOrderState,
    BluetoothLegacyAdvertisingRecurringResponsePublication,
    BluetoothLegacyAdvertisingRecurringRetry, BluetoothLegacyAdvertisingRecurringRunner,
    BluetoothLegacyAdvertisingRecurringStart, BluetoothLegacyAdvertisingRecurringStopBegin,
    BluetoothLegacyAdvertisingRecurringStopFault, BluetoothLegacyAdvertisingRecurringStopRestore,
    BluetoothLegacyAdvertisingRecurringStopRestoreStep, BluetoothLegacyAdvertisingResetCompletion,
    BluetoothLegacyAdvertisingResetCompletionReady, BluetoothLegacyAdvertisingResetResponsePending,
    BluetoothLegacyAdvertisingResetResponsePublication, BluetoothLegacyAdvertisingResetRestore,
    BluetoothLegacyAdvertisingResetRestoreStep, BluetoothLegacyAdvertisingResponsePendingSession,
    BluetoothLegacyAdvertisingResponsePublication, BluetoothLegacyAdvertisingStopping,
    BluetoothLegacyAdvertisingStoppingFault, BluetoothLegacyAdvertisingStoppingStep,
    BluetoothLegacyConnectableAdvertisingActivePendingFailStop,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingActiveResponsePublication,
    BluetoothLegacyConnectableAdvertisingActiveWait,
    BluetoothLegacyConnectableAdvertisingCommandIntake,
    BluetoothLegacyConnectableAdvertisingCommandMismatch,
    BluetoothLegacyConnectableAdvertisingCommandRoute,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailure,
    BluetoothLegacyConnectableAdvertisingFirstRunnerRetry,
    BluetoothLegacyConnectableAdvertisingHciActiveFailStop,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingHciActiveStep,
    BluetoothLegacyConnectableAdvertisingResponsePending,
    BluetoothLegacyConnectableAdvertisingResponsePublication,
    BluetoothLegacyConnectableAdvertisingStopping,
    BluetoothLegacyConnectableAdvertisingStoppingFailStop,
    BluetoothLegacyConnectableAdvertisingStoppingStep,
    BluetoothLegacyConnectablePeripheralFirstHciAxis,
    BluetoothLegacyConnectablePeripheralFirstHciFailStop,
    BluetoothLegacyConnectablePeripheralFirstHciResetFailStop,
    BluetoothLegacyConnectablePeripheralFirstHciResetOutcome,
    BluetoothLegacyConnectablePeripheralFirstHciResponsePublication,
    BluetoothLegacyConnectablePeripheralFirstHciRunning,
    BluetoothPassiveScanHciActiveCommandIntake, BluetoothPassiveScanHciActiveCommandMismatch,
    BluetoothPassiveScanHciActiveCommandRoute, BluetoothPassiveScanHciActiveFault,
    BluetoothPassiveScanHciActivePendingFault, BluetoothPassiveScanHciActivePendingRadioStep,
    BluetoothPassiveScanHciActiveResponsePending, BluetoothPassiveScanHciActiveResponsePublication,
    BluetoothPassiveScanHciActiveSession, BluetoothPassiveScanHciCommandIntake,
    BluetoothPassiveScanHciCommandMismatch, BluetoothPassiveScanHciCommandRoute,
    BluetoothPassiveScanHciCpuResponsePending, BluetoothPassiveScanHciCpuResponsePublication,
    BluetoothPassiveScanHciFirstRunnerFailure, BluetoothPassiveScanHciRecurringFailure,
    BluetoothPassiveScanHciRecurringRunner, BluetoothPassiveScanHciReportStep,
    BluetoothPassiveScanHciReportsComplete, BluetoothPassiveScanHciReportsPending,
    BluetoothPassiveScanHciResponsePendingSession, BluetoothPassiveScanHciResponsePublication,
    BluetoothPassiveScanHciStopping, BluetoothPassiveScanHciStoppingFault,
    BluetoothPassiveScanHciStoppingStep, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerRunInterruptStorage,
};

#[cfg(target_arch = "riscv32")]
use crate::{
    EmbassyBluetoothDtmControllerTimeRecheck, EmbassyBluetoothDtmControllerTimeRecheckStatus,
    EmbassyBluetoothDtmFirstControllerTimeWait, EmbassyBluetoothDtmFirstDrive,
    EmbassyBluetoothDtmFirstResume, EmbassyBluetoothDtmSessionBoundary,
    EmbassyBluetoothDtmSessionTask, EmbassyBluetoothLegacyAdvertisingActiveDrive,
    EmbassyBluetoothLegacyAdvertisingDelaySource,
    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyAdvertisingFirstDrive, EmbassyBluetoothLegacyAdvertisingFirstResume,
    EmbassyBluetoothLegacyAdvertisingRecurringDrive,
    EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait,
    EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive,
    EmbassyBluetoothLegacyConnectableAdvertisingFirstResume,
    EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations,
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait,
    EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep,
    EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication,
    EmbassyBluetoothLegacyConnectablePeripheralFirstRetry,
    EmbassyBluetoothLegacyConnectablePeripheralFirstStoppingStep,
    EmbassyBluetoothPassiveScanActiveDrive, EmbassyBluetoothPassiveScanFirstControllerTimeWait,
    EmbassyBluetoothPassiveScanFirstDrive, EmbassyBluetoothPassiveScanFirstResume,
    EmbassyBluetoothPassiveScanRecurringDrive, EmbassyBluetoothRuntimeWakers,
    begin_legacy_connectable_peripheral_first_command_ready,
    begin_legacy_connectable_peripheral_first_response_pending,
    begin_legacy_connectable_peripheral_first_stopping, drive_dtm_first_ready,
    drive_legacy_advertising_active_ready, drive_legacy_advertising_first_ready,
    drive_legacy_advertising_recurring_ready, drive_legacy_connectable_advertising_active_ready,
    drive_legacy_connectable_advertising_first_ready,
    drive_legacy_connectable_advertising_initial_pending_ready_with,
    drive_legacy_connectable_advertising_pending_ready_with,
    drive_legacy_connectable_advertising_stopping_ready, drive_passive_scan_active_ready,
    drive_passive_scan_first_ready, drive_passive_scan_recurring_ready,
    finish_legacy_connectable_advertising_no_connection_stopping_with,
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

pub use dispatch::EmbassyBluetoothControllerCommandPhase;
#[cfg(target_arch = "riscv32")]
pub use owner::EmbassyBluetoothControllerCommandTask;
#[cfg(target_arch = "riscv32")]
pub use response::EmbassyBluetoothControllerCommandBoundary;
pub use response::{EmbassyBluetoothControllerIdleCompletion, EmbassyBluetoothControllerRetry};

#[cfg(any(target_arch = "riscv32", test))]
use dispatch::{
    ControllerCommandAction, ControllerCommandStimulus, reduce_controller_command_transition,
};
#[cfg(test)]
use owner::ControllerOwnerSlot;
#[cfg(target_arch = "riscv32")]
use owner::{
    EmbassyBluetoothControllerCommandState, EmbassyBluetoothUnownedFinishedListOwner,
    FirstCleanupReadiness, LegacyAdvertisingStopOrigin,
};

#[cfg(test)]
mod tests;
