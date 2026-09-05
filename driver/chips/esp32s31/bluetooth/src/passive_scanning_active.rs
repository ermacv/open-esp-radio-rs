//! Completion and reclamation runner for one passive LE scan window.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::{
    LegacyAdvertisingReport, LegacyAdvertisingReportParseError, LegacyPassiveScanWindowInFlight,
    LegacyPassiveScannerEnabled, LegacyScanDuplicatePolicy, PrimaryScanChannel,
    parse_legacy_advertising_report,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeReceivedBatch, BluetoothPassiveScanSchedulerItemCompletionStatus,
};

use crate::controller::boot::BluetoothSingleItemSchedulerCompletionFaultOwner;
use crate::scheduler::core::BluetoothPassiveScanSchedulerRecycleStep;
use crate::single_item_completion::{
    BluetoothSingleItemCompletion, BluetoothSingleItemCompletionFault,
    BluetoothSingleItemCompletionFaultCause, BluetoothSingleItemCompletionStep,
    BluetoothSingleItemCompletionWaitKind,
};
use crate::{
    BluetoothControllerPublishedTaskService, BluetoothDtmPostUnlinkWakeCell,
    BluetoothPassiveScanFirstRunning, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerWakeCell,
};

type RemovalReady = crate::scheduler::core::BluetoothSingleItemSchedulerSoftwareListRemovalReady<
    BluetoothPassiveScanCompletionRole,
>;

struct BluetoothPassiveScanActiveAxes<'runtime, S, const CAPACITY: usize> {
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    window: LegacyPassiveScanWindowInFlight,
    phase: crate::BluetoothPassiveScanEventPhase,
}

enum BluetoothPassiveScanActivePhase {
    Completion(BluetoothSingleItemCompletion<BluetoothPassiveScanCompletionRole>),
    RemovalReady(RemovalReady),
}

pub(crate) struct BluetoothPassiveScanCompletionRole;

impl crate::scheduler::core::BluetoothSingleItemSchedulerRole
    for BluetoothPassiveScanCompletionRole
{
    type RunningItem =
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphRunning;
    type CompletionObservedItem =
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCompletionObserved;
    type Retained = crate::scheduler::timeline::BluetoothSchedulerWindowReservation<
        crate::scheduler::timeline::BluetoothSchedulerSequenceReady,
    >;

    fn running_item_address(
        item: &Self::RunningItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation<Self> {
        match item.observe_completion(observed) {
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::ListMismatch {
                running,
                observed,
            },
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCompletionObservation::StillInFlight(running) => {
                crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::StillInFlight(running)
            }
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                crate::scheduler::core::BluetoothSingleItemRoleCompletionObservation::CompletionObserved(completed)
            }
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}

/// One running scanner window and every owner needed to reclaim it.
#[must_use = "drive the scanner graph to its CPU-owned receive boundary"]
pub struct BluetoothPassiveScanActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    axes: BluetoothPassiveScanActiveAxes<'runtime, S, CAPACITY>,
    phase: BluetoothPassiveScanActivePhase,
}

/// Borrowed wake source for the exact active scanner phase.
pub enum BluetoothPassiveScanActiveWait<'a> {
    Scheduler(&'a BluetoothSchedulerWakeCell),
    PostUnlink(&'a BluetoothDtmPostUnlinkWakeCell),
}

/// One bounded active-scanner transition.
#[must_use = "retain the active owner, CPU result, unrelated list, or fail-stop owner"]
pub enum BluetoothPassiveScanActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>),
    Waiting(BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(BluetoothPassiveScanEventCpuOwned<'runtime, S, CAPACITY>),
    Fault(BluetoothPassiveScanActiveFault<'runtime, S, CAPACITY>),
}

/// Copied receive results after the graph and timeline slot returned to idle.
#[must_use = "consume reports and retain or disable the portable scanner"]
pub struct BluetoothPassiveScanEventCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    scanner: LegacyPassiveScannerEnabled,
    phase: crate::BluetoothPassiveScanEventPhase,
    channel: PrimaryScanChannel,
    received: BluetoothLeReceivedBatch,
    status: BluetoothPassiveScanSchedulerItemCompletionStatus,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanEventCpuOwned<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn channel(&self) -> PrimaryScanChannel {
        self.channel
    }

    pub const fn phase(&self) -> crate::BluetoothPassiveScanEventPhase {
        self.phase
    }

    pub const fn received(&self) -> &BluetoothLeReceivedBatch {
        &self.received
    }

    pub const fn completion_status(&self) -> BluetoothPassiveScanSchedulerItemCompletionStatus {
        self.status
    }

    pub const fn duplicate_policy(&self) -> LegacyScanDuplicatePolicy {
        self.scanner.duplicate_policy()
    }

    /// Parse one hardware-copied PDU at the portable Link Layer boundary.
    pub fn report(
        &self,
        index: usize,
    ) -> Result<Option<LegacyAdvertisingReport>, LegacyAdvertisingReportParseError> {
        let Some(packet) = self.received.packet(index) else {
            return Ok(None);
        };
        parse_legacy_advertising_report(packet.as_bytes(), self.channel, packet.rssi_dbm())
            .map(Some)
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
        LegacyPassiveScannerEnabled,
        crate::BluetoothPassiveScanEventPhase,
        BluetoothLeReceivedBatch,
        BluetoothPassiveScanSchedulerItemCompletionStatus,
    ) {
        (
            self.task,
            self.scanner,
            self.phase,
            self.received,
            self.status,
        )
    }
}

/// Finite fail-closed classification for scanner completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanActiveFaultCause {
    FinishedListDrainAlreadyActive,
    SchedulerIdentityMismatch,
    FinishedListDrainLost,
    RepeatedScannerList,
    FinishedListDrainStillActive,
    ExpectedHardwareHeadStillPublished,
    UnexpectedHardwareHeadChanged,
    PostUnlinkMailboxBusy,
    PostUnlinkMailboxIdentityExhausted,
    PostUnlinkMailboxGenerationExhausted,
    PostUnlinkMailboxCommitMismatch,
    PostUnlinkMailboxAffinityMismatch,
    PrimaryInterruptFault,
    PostUnlinkNoSchedulerWorkRearmMismatch,
    PostUnlinkPendingRearmMismatch,
    PostUnlinkRecheckUnavailable,
    PostUnlinkRecheckRearmMismatch,
    MemoryIdentityMismatch,
    ReceiveInvalid,
    ReservationIdentityMismatch,
    RuntimeGraphMismatch,
}

#[allow(
    dead_code,
    reason = "the opaque fault owner intentionally retains every lower affine token"
)]
enum BluetoothPassiveScanActiveFaultOwner {
    Completion(
        BluetoothSingleItemCompletionFault<
            BluetoothSingleItemSchedulerCompletionFaultOwner<BluetoothPassiveScanCompletionRole>,
        >,
    ),
    Recycle(BluetoothPassiveScanSchedulerRecycleStep),
    RuntimeRestore(crate::passive_scanning::BluetoothPassiveScanRuntimeRestoreFailure),
}

/// Opaque fail-stop owner retaining the Controller, LL state and lower graph.
#[must_use = "retain the exact failed scanner owner for diagnostic shutdown"]
pub struct BluetoothPassiveScanActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothPassiveScanActiveFaultCause,
    _axes: BluetoothPassiveScanActiveAxes<'runtime, S, CAPACITY>,
    _owner: BluetoothPassiveScanActiveFaultOwner,
}

impl<S, const CAPACITY: usize> BluetoothPassiveScanActiveFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothPassiveScanActiveFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn from_first_running(
        first: BluetoothPassiveScanFirstRunning<'runtime, S, CAPACITY>,
    ) -> Self {
        let (task, window, phase, running) = first.into_parts();
        Self {
            axes: BluetoothPassiveScanActiveAxes {
                task,
                window,
                phase,
            },
            phase: BluetoothPassiveScanActivePhase::Completion(BluetoothSingleItemCompletion::new(
                running,
            )),
        }
    }

    pub fn radio_wait(&self) -> Option<BluetoothPassiveScanActiveWait<'_>> {
        let BluetoothPassiveScanActivePhase::Completion(completion) = &self.phase else {
            return None;
        };
        match completion.wait_kind() {
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler) => Some(
                BluetoothPassiveScanActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink) => Some(
                BluetoothPassiveScanActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            None => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(self) -> BluetoothPassiveScanActiveStep<'runtime, S, CAPACITY> {
        let Self { mut axes, phase } = self;
        match phase {
            BluetoothPassiveScanActivePhase::Completion(completion) => {
                let step = completion.step(&mut axes.task);
                match step {
                    BluetoothSingleItemCompletionStep::Continue(completion) => {
                        BluetoothPassiveScanActiveStep::Continue(Self {
                            axes,
                            phase: BluetoothPassiveScanActivePhase::Completion(completion),
                        })
                    }
                    BluetoothSingleItemCompletionStep::Waiting(completion) => {
                        BluetoothPassiveScanActiveStep::Waiting(Self {
                            axes,
                            phase: BluetoothPassiveScanActivePhase::Completion(completion),
                        })
                    }
                    BluetoothSingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => BluetoothPassiveScanActiveStep::UnrelatedList {
                        session: Self {
                            axes,
                            phase: BluetoothPassiveScanActivePhase::Completion(completion),
                        },
                        observed,
                    },
                    BluetoothSingleItemCompletionStep::RemovalReady(ready) => {
                        BluetoothPassiveScanActiveStep::Continue(Self {
                            axes,
                            phase: BluetoothPassiveScanActivePhase::RemovalReady(ready),
                        })
                    }
                    BluetoothSingleItemCompletionStep::Fault(fault) => {
                        let cause = passive_scan_fault_cause(fault.cause);
                        active_fault(
                            axes,
                            cause,
                            BluetoothPassiveScanActiveFaultOwner::Completion(fault),
                        )
                    }
                }
            }
            BluetoothPassiveScanActivePhase::RemovalReady(ready) => {
                match axes.task.recycle_passive_scan_completed(ready) {
                    BluetoothPassiveScanSchedulerRecycleStep::Recycled(recycled) => {
                        match axes.task.restore_passive_scan_recycled(recycled) {
                            Ok((received, status)) => {
                                let channel = axes.window.channel();
                                BluetoothPassiveScanActiveStep::CpuOwned(
                                    BluetoothPassiveScanEventCpuOwned {
                                        task: axes.task,
                                        scanner: axes.window.complete(),
                                        phase: axes.phase,
                                        channel,
                                        received,
                                        status,
                                    },
                                )
                            }
                            Err(failure) => active_fault(
                                axes,
                                BluetoothPassiveScanActiveFaultCause::RuntimeGraphMismatch,
                                BluetoothPassiveScanActiveFaultOwner::RuntimeRestore(failure),
                            ),
                        }
                    }
                    step @ BluetoothPassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::FinishedListDrainStillActive {
                        ..
                    } => active_fault(
                        axes,
                        BluetoothPassiveScanActiveFaultCause::FinishedListDrainStillActive,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::MemoryIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        BluetoothPassiveScanActiveFaultCause::MemoryIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::ReceiveInvalid { .. } => {
                        active_fault(
                            axes,
                            BluetoothPassiveScanActiveFaultCause::ReceiveInvalid,
                            BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                        )
                    }
                    step @ BluetoothPassiveScanSchedulerRecycleStep::ReservationIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        BluetoothPassiveScanActiveFaultCause::ReservationIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                }
            }
        }
    }
}

fn passive_scan_fault_cause(
    cause: BluetoothSingleItemCompletionFaultCause,
) -> BluetoothPassiveScanActiveFaultCause {
    match cause {
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            BluetoothPassiveScanActiveFaultCause::FinishedListDrainAlreadyActive
        }
        BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost => {
            BluetoothPassiveScanActiveFaultCause::FinishedListDrainLost
        }
        BluetoothSingleItemCompletionFaultCause::RepeatedRoleList => {
            BluetoothPassiveScanActiveFaultCause::RepeatedScannerList
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            BluetoothPassiveScanActiveFaultCause::FinishedListDrainStillActive
        }
        BluetoothSingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            BluetoothPassiveScanActiveFaultCause::ExpectedHardwareHeadStillPublished
        }
        BluetoothSingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            BluetoothPassiveScanActiveFaultCause::UnexpectedHardwareHeadChanged
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxBusy
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxIdentityExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxGenerationExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxCommitMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxAffinityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PrimaryInterruptFault => {
            BluetoothPassiveScanActiveFaultCause::PrimaryInterruptFault
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkPendingRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkRecheckUnavailable
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            BluetoothPassiveScanActiveFaultCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn active_fault<'runtime, S, const CAPACITY: usize>(
    axes: BluetoothPassiveScanActiveAxes<'runtime, S, CAPACITY>,
    cause: BluetoothPassiveScanActiveFaultCause,
    owner: BluetoothPassiveScanActiveFaultOwner,
) -> BluetoothPassiveScanActiveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    BluetoothPassiveScanActiveStep::Fault(BluetoothPassiveScanActiveFault {
        cause,
        _axes: axes,
        _owner: owner,
    })
}
