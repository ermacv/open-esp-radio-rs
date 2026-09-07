//! Completion and reclamation runner for one passive LE scan window.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerPublishedTaskService, SchedulerRunInterruptStorage,
        boot::SingleItemSchedulerCompletionFaultOwner,
    },
    interrupt::SchedulerWakeCell,
    le::{dtm::DtmPostUnlinkWakeCell, scanning::PassiveScanFirstRunning},
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved,
        completion::{
            SingleItemCompletion, SingleItemCompletionFault, SingleItemCompletionFaultCause,
            SingleItemCompletionStep, SingleItemCompletionWaitKind,
        },
        core::PassiveScanSchedulerRecycleStep,
    },
};

use oer_bluetooth_ll::scanning::{
    LegacyAdvertisingReport, LegacyAdvertisingReportParseError, LegacyPassiveScanWindowInFlight,
    LegacyPassiveScannerEnabled, LegacyScanDuplicatePolicy, PrimaryScanChannel,
    parse_legacy_advertising_report,
};

use oer_esp32s31_bluetooth_memory::{LeReceivedBatch, PassiveScanSchedulerItemCompletionStatus};

type RemovalReady =
    crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady<PassiveScanCompletionRole>;

struct PassiveScanActiveAxes<'runtime, S, const CAPACITY: usize> {
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    window: LegacyPassiveScanWindowInFlight,
    phase: crate::le::scanning::PassiveScanEventPhase,
}

enum PassiveScanActivePhase {
    Completion(SingleItemCompletion<PassiveScanCompletionRole>),
    RemovalReady(RemovalReady),
}

pub(crate) struct PassiveScanCompletionRole;

impl crate::scheduler::core::SingleItemSchedulerRole for PassiveScanCompletionRole {
    type RunningItem = oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphRunning;
    type CompletionObservedItem =
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCompletionObserved;
    type Retained = crate::scheduler::timeline::SchedulerWindowReservation<
        crate::scheduler::timeline::SchedulerSequenceReady,
    >;

    fn running_item_address(
        item: &Self::RunningItem,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }

    fn observe_completion(
        item: Self::RunningItem,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    ) -> crate::scheduler::core::SingleItemRoleCompletionObservation<Self> {
        match item.observe_completion(observed) {
            oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => crate::scheduler::core::SingleItemRoleCompletionObservation::ListMismatch {
                running,
                observed,
            },
            oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCompletionObservation::StillInFlight(running) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::StillInFlight(running)
            }
            oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCompletionObservation::CompletionObserved(completed) => {
                crate::scheduler::core::SingleItemRoleCompletionObservation::CompletionObserved(completed)
            }
        }
    }

    fn completed_item_address(
        item: &Self::CompletionObservedItem,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        item.scheduler_item_address()
    }
}

/// One running scanner window and every owner needed to reclaim it.
#[must_use = "drive the scanner graph to its CPU-owned receive boundary"]
pub struct PassiveScanActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    axes: PassiveScanActiveAxes<'runtime, S, CAPACITY>,
    phase: PassiveScanActivePhase,
}

/// Borrowed wake source for the exact active scanner phase.
pub enum PassiveScanActiveWait<'a> {
    Scheduler(&'a SchedulerWakeCell),
    PostUnlink(&'a DtmPostUnlinkWakeCell),
}

/// One bounded active-scanner transition.
#[must_use = "retain the active owner, CPU result, unrelated list, or fail-stop owner"]
pub enum PassiveScanActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(PassiveScanActiveSession<'runtime, S, CAPACITY>),
    Waiting(PassiveScanActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: PassiveScanActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(PassiveScanEventCpuOwned<'runtime, S, CAPACITY>),
    Fault(PassiveScanActiveFault<'runtime, S, CAPACITY>),
}

/// Copied receive results after the graph and timeline slot returned to idle.
#[must_use = "consume reports and retain or disable the portable scanner"]
pub struct PassiveScanEventCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    scanner: LegacyPassiveScannerEnabled,
    phase: crate::le::scanning::PassiveScanEventPhase,
    channel: PrimaryScanChannel,
    received: LeReceivedBatch,
    status: PassiveScanSchedulerItemCompletionStatus,
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanEventCpuOwned<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn channel(&self) -> PrimaryScanChannel {
        self.channel
    }

    pub const fn phase(&self) -> crate::le::scanning::PassiveScanEventPhase {
        self.phase
    }

    pub const fn received(&self) -> &LeReceivedBatch {
        &self.received
    }

    pub const fn completion_status(&self) -> PassiveScanSchedulerItemCompletionStatus {
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
        ControllerPublishedTaskService<'runtime, S, CAPACITY>,
        LegacyPassiveScannerEnabled,
        crate::le::scanning::PassiveScanEventPhase,
        LeReceivedBatch,
        PassiveScanSchedulerItemCompletionStatus,
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
pub enum PassiveScanActiveFaultCause {
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
enum PassiveScanActiveFaultOwner {
    Completion(
        SingleItemCompletionFault<
            SingleItemSchedulerCompletionFaultOwner<PassiveScanCompletionRole>,
        >,
    ),
    Recycle(PassiveScanSchedulerRecycleStep),
    RuntimeRestore(crate::le::scanning::passive::PassiveScanRuntimeRestoreFailure),
}

/// Opaque fail-stop owner retaining the Controller, LL state and lower graph.
#[must_use = "retain the exact failed scanner owner for diagnostic shutdown"]
pub struct PassiveScanActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: PassiveScanActiveFaultCause,
    _axes: PassiveScanActiveAxes<'runtime, S, CAPACITY>,
    _owner: PassiveScanActiveFaultOwner,
}

impl<S, const CAPACITY: usize> PassiveScanActiveFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> PassiveScanActiveFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanActiveSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn from_first_running(first: PassiveScanFirstRunning<'runtime, S, CAPACITY>) -> Self {
        let (task, window, phase, running) = first.into_parts();
        Self {
            axes: PassiveScanActiveAxes {
                task,
                window,
                phase,
            },
            phase: PassiveScanActivePhase::Completion(SingleItemCompletion::new(running)),
        }
    }

    pub fn radio_wait(&self) -> Option<PassiveScanActiveWait<'_>> {
        let PassiveScanActivePhase::Completion(completion) = &self.phase else {
            return None;
        };
        match completion.wait_kind() {
            Some(SingleItemCompletionWaitKind::Scheduler) => Some(
                PassiveScanActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            Some(SingleItemCompletionWaitKind::PostUnlink) => Some(
                PassiveScanActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            None => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(self) -> PassiveScanActiveStep<'runtime, S, CAPACITY> {
        let Self { mut axes, phase } = self;
        match phase {
            PassiveScanActivePhase::Completion(completion) => {
                let step = completion.step(&mut axes.task);
                match step {
                    SingleItemCompletionStep::Continue(completion) => {
                        PassiveScanActiveStep::Continue(Self {
                            axes,
                            phase: PassiveScanActivePhase::Completion(completion),
                        })
                    }
                    SingleItemCompletionStep::Waiting(completion) => {
                        PassiveScanActiveStep::Waiting(Self {
                            axes,
                            phase: PassiveScanActivePhase::Completion(completion),
                        })
                    }
                    SingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => PassiveScanActiveStep::UnrelatedList {
                        session: Self {
                            axes,
                            phase: PassiveScanActivePhase::Completion(completion),
                        },
                        observed,
                    },
                    SingleItemCompletionStep::RemovalReady(ready) => {
                        PassiveScanActiveStep::Continue(Self {
                            axes,
                            phase: PassiveScanActivePhase::RemovalReady(ready),
                        })
                    }
                    SingleItemCompletionStep::Fault(fault) => {
                        let cause = passive_scan_fault_cause(fault.cause);
                        active_fault(axes, cause, PassiveScanActiveFaultOwner::Completion(fault))
                    }
                }
            }
            PassiveScanActivePhase::RemovalReady(ready) => {
                match axes.task.recycle_passive_scan_completed(ready) {
                    PassiveScanSchedulerRecycleStep::Recycled(recycled) => {
                        match axes.task.restore_passive_scan_recycled(recycled) {
                            Ok((received, status)) => {
                                let channel = axes.window.channel();
                                PassiveScanActiveStep::CpuOwned(PassiveScanEventCpuOwned {
                                    task: axes.task,
                                    scanner: axes.window.complete(),
                                    phase: axes.phase,
                                    channel,
                                    received,
                                    status,
                                })
                            }
                            Err(failure) => active_fault(
                                axes,
                                PassiveScanActiveFaultCause::RuntimeGraphMismatch,
                                PassiveScanActiveFaultOwner::RuntimeRestore(failure),
                            ),
                        }
                    }
                    step @ PassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch { .. } => {
                        active_fault(
                            axes,
                            PassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                            PassiveScanActiveFaultOwner::Recycle(step),
                        )
                    }
                    step @ PassiveScanSchedulerRecycleStep::FinishedListDrainStillActive {
                        ..
                    } => active_fault(
                        axes,
                        PassiveScanActiveFaultCause::FinishedListDrainStillActive,
                        PassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ PassiveScanSchedulerRecycleStep::MemoryIdentityMismatch { .. } => {
                        active_fault(
                            axes,
                            PassiveScanActiveFaultCause::MemoryIdentityMismatch,
                            PassiveScanActiveFaultOwner::Recycle(step),
                        )
                    }
                    step @ PassiveScanSchedulerRecycleStep::ReceiveInvalid { .. } => active_fault(
                        axes,
                        PassiveScanActiveFaultCause::ReceiveInvalid,
                        PassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ PassiveScanSchedulerRecycleStep::ReservationIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        PassiveScanActiveFaultCause::ReservationIdentityMismatch,
                        PassiveScanActiveFaultOwner::Recycle(step),
                    ),
                }
            }
        }
    }
}

fn passive_scan_fault_cause(cause: SingleItemCompletionFaultCause) -> PassiveScanActiveFaultCause {
    match cause {
        SingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            PassiveScanActiveFaultCause::FinishedListDrainAlreadyActive
        }
        SingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            PassiveScanActiveFaultCause::SchedulerIdentityMismatch
        }
        SingleItemCompletionFaultCause::FinishedListDrainLost => {
            PassiveScanActiveFaultCause::FinishedListDrainLost
        }
        SingleItemCompletionFaultCause::RepeatedRoleList => {
            PassiveScanActiveFaultCause::RepeatedScannerList
        }
        SingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            PassiveScanActiveFaultCause::FinishedListDrainStillActive
        }
        SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            PassiveScanActiveFaultCause::ExpectedHardwareHeadStillPublished
        }
        SingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            PassiveScanActiveFaultCause::UnexpectedHardwareHeadChanged
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            PassiveScanActiveFaultCause::PostUnlinkMailboxBusy
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            PassiveScanActiveFaultCause::PostUnlinkMailboxIdentityExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            PassiveScanActiveFaultCause::PostUnlinkMailboxGenerationExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            PassiveScanActiveFaultCause::PostUnlinkMailboxCommitMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            PassiveScanActiveFaultCause::PostUnlinkMailboxAffinityMismatch
        }
        SingleItemCompletionFaultCause::PrimaryInterruptFault => {
            PassiveScanActiveFaultCause::PrimaryInterruptFault
        }
        SingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            PassiveScanActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            PassiveScanActiveFaultCause::PostUnlinkPendingRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            PassiveScanActiveFaultCause::PostUnlinkRecheckUnavailable
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            PassiveScanActiveFaultCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn active_fault<'runtime, S, const CAPACITY: usize>(
    axes: PassiveScanActiveAxes<'runtime, S, CAPACITY>,
    cause: PassiveScanActiveFaultCause,
    owner: PassiveScanActiveFaultOwner,
) -> PassiveScanActiveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    PassiveScanActiveStep::Fault(PassiveScanActiveFault {
        cause,
        _axes: axes,
        _owner: owner,
    })
}
