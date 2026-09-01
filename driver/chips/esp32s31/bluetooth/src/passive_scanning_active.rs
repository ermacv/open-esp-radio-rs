//! Completion and reclamation runner for one passive LE scan window.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::{
    LegacyAdvertisingReport, LegacyAdvertisingReportParseError, LegacyPassiveScanWindowInFlight,
    LegacyPassiveScannerEnabled, LegacyScanDuplicatePolicy, PrimaryScanChannel,
    parse_legacy_advertising_report,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanReceivedBatch, BluetoothPassiveScanSchedulerItemCompletionStatus,
};

use crate::{
    BluetoothControllerPublishedTaskService, BluetoothDtmPostUnlinkWakeCell,
    BluetoothPassiveScanFirstRunning, BluetoothPassiveScanPostUnlinkArmStep,
    BluetoothPassiveScanPostUnlinkAwaiting, BluetoothPassiveScanSchedulerCompletionObserved,
    BluetoothPassiveScanSchedulerCompletionObservedDrainStep,
    BluetoothPassiveScanSchedulerCompletionStep,
    BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved,
    BluetoothPassiveScanSchedulerHardwareHeadRetirementStep,
    BluetoothPassiveScanSchedulerRecycleStep, BluetoothPassiveScanSchedulerRunning,
    BluetoothPassiveScanSchedulerRunningDrainStep,
    BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
    BluetoothPassiveScanSoftwareListRemovalPublishedStep,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListDrainPending,
    BluetoothSchedulerFinishedListDrainState, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerWakeCell,
};

struct BluetoothPassiveScanActiveAxes<'runtime, S, const CAPACITY: usize> {
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    window: LegacyPassiveScanWindowInFlight,
    phase: crate::BluetoothPassiveScanEventPhase,
}

enum BluetoothPassiveScanActivePhase {
    RunningAwaitingWake(BluetoothPassiveScanSchedulerRunning),
    RunningReady {
        running: BluetoothPassiveScanSchedulerRunning,
        wake: crate::BluetoothSchedulerWakeBatch,
    },
    RunningDrain(BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerRunning>),
    CompletionDrain(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerCompletionObserved>,
    ),
    CompletionObserved(BluetoothPassiveScanSchedulerCompletionObserved),
    HardwareHeadEmpty(BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    PostUnlinkAwaiting(BluetoothPassiveScanPostUnlinkAwaiting),
    RemovalReady(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
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
    received: BluetoothPassiveScanReceivedBatch,
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

    pub const fn received(&self) -> &BluetoothPassiveScanReceivedBatch {
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
        BluetoothPassiveScanReceivedBatch,
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
    Completion(BluetoothPassiveScanSchedulerCompletionStep),
    RunningDrain(BluetoothPassiveScanSchedulerRunningDrainStep),
    CompletionDrain(BluetoothPassiveScanSchedulerCompletionObservedDrainStep),
    HardwareHeadRetirement(BluetoothPassiveScanSchedulerHardwareHeadRetirementStep),
    PostUnlinkArm(BluetoothPassiveScanPostUnlinkArmStep),
    PostUnlinkPublished(BluetoothPassiveScanSoftwareListRemovalPublishedStep),
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
            phase: BluetoothPassiveScanActivePhase::RunningAwaitingWake(running),
        }
    }

    pub fn radio_wait(&self) -> Option<BluetoothPassiveScanActiveWait<'_>> {
        match self.phase {
            BluetoothPassiveScanActivePhase::RunningAwaitingWake(_) => Some(
                BluetoothPassiveScanActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            BluetoothPassiveScanActivePhase::PostUnlinkAwaiting(_) => Some(
                BluetoothPassiveScanActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            _ => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(mut self) -> BluetoothPassiveScanActiveStep<'runtime, S, CAPACITY> {
        let scheduler_wake = if matches!(
            &self.phase,
            BluetoothPassiveScanActivePhase::RunningAwaitingWake(_)
        ) {
            let Some(wake) = self.axes.task.scheduler_wake().take() else {
                return BluetoothPassiveScanActiveStep::Waiting(self);
            };
            Some(wake)
        } else {
            None
        };
        match self.phase {
            BluetoothPassiveScanActivePhase::RunningAwaitingWake(running) => {
                self.phase = BluetoothPassiveScanActivePhase::RunningReady {
                    running,
                    wake: scheduler_wake
                        .expect("the running scanner phase consumed one scheduler wake"),
                };
                BluetoothPassiveScanActiveStep::Continue(self)
            }
            BluetoothPassiveScanActivePhase::RunningReady { running, wake } => {
                match self.axes.task.observe_passive_scan_completion(running, wake) {
                    BluetoothPassiveScanSchedulerCompletionStep::NoFinishedList(running) => {
                        self.phase =
                            BluetoothPassiveScanActivePhase::RunningAwaitingWake(running);
                        BluetoothPassiveScanActiveStep::Waiting(self)
                    }
                    BluetoothPassiveScanSchedulerCompletionStep::UnrelatedList {
                        drain,
                        observed,
                    } => {
                        self.phase = running_phase(drain);
                        BluetoothPassiveScanActiveStep::UnrelatedList {
                            session: self,
                            observed,
                        }
                    }
                    BluetoothPassiveScanSchedulerCompletionStep::StillInFlight(drain) => {
                        self.phase = running_phase(drain);
                        waiting_or_continue(self)
                    }
                    BluetoothPassiveScanSchedulerCompletionStep::CompletionObserved(drain) => {
                        self.phase = completed_phase(drain);
                        BluetoothPassiveScanActiveStep::Continue(self)
                    }
                    step @ BluetoothPassiveScanSchedulerCompletionStep::DrainAlreadyActive(_) => {
                        active_fault(
                            self.axes,
                            BluetoothPassiveScanActiveFaultCause::FinishedListDrainAlreadyActive,
                            BluetoothPassiveScanActiveFaultOwner::Completion(step),
                        )
                    }
                    step @ BluetoothPassiveScanSchedulerCompletionStep::SchedulerIdentityMismatch(
                        _,
                    ) => active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Completion(step),
                    ),
                }
            }
            BluetoothPassiveScanActivePhase::RunningDrain(pending) => match self
                .axes
                .task
                .continue_passive_scan_running_finished_list_drain(pending)
            {
                BluetoothPassiveScanSchedulerRunningDrainStep::UnrelatedList {
                    drain,
                    observed,
                } => {
                    self.phase = running_phase(drain);
                    BluetoothPassiveScanActiveStep::UnrelatedList {
                        session: self,
                        observed,
                    }
                }
                BluetoothPassiveScanSchedulerRunningDrainStep::StillInFlight(drain) => {
                    self.phase = running_phase(drain);
                    waiting_or_continue(self)
                }
                BluetoothPassiveScanSchedulerRunningDrainStep::CompletionObserved(drain) => {
                    self.phase = completed_phase(drain);
                    BluetoothPassiveScanActiveStep::Continue(self)
                }
                step @ BluetoothPassiveScanSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                    _,
                ) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                    BluetoothPassiveScanActiveFaultOwner::RunningDrain(step),
                ),
                step @ BluetoothPassiveScanSchedulerRunningDrainStep::DrainLost(_) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::FinishedListDrainLost,
                    BluetoothPassiveScanActiveFaultOwner::RunningDrain(step),
                ),
            },
            BluetoothPassiveScanActivePhase::CompletionDrain(pending) => match self
                .axes
                .task
                .continue_passive_scan_completed_finished_list_drain(pending)
            {
                BluetoothPassiveScanSchedulerCompletionObservedDrainStep::UnrelatedList {
                    drain,
                    observed,
                } => {
                    self.phase = completed_phase(drain);
                    BluetoothPassiveScanActiveStep::UnrelatedList {
                        session: self,
                        observed,
                    }
                }
                step @ BluetoothPassiveScanSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                    _,
                ) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                    BluetoothPassiveScanActiveFaultOwner::CompletionDrain(step),
                ),
                step @ BluetoothPassiveScanSchedulerCompletionObservedDrainStep::DrainLost(_) => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::FinishedListDrainLost,
                        BluetoothPassiveScanActiveFaultOwner::CompletionDrain(step),
                    )
                }
                step @ BluetoothPassiveScanSchedulerCompletionObservedDrainStep::RepeatedScannerList {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::RepeatedScannerList,
                    BluetoothPassiveScanActiveFaultOwner::CompletionDrain(step),
                ),
            },
            BluetoothPassiveScanActivePhase::CompletionObserved(completed) => match self
                .axes
                .task
                .observe_passive_scan_hardware_head_retirement(completed)
            {
                BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    observed,
                ) => {
                    self.phase = BluetoothPassiveScanActivePhase::HardwareHeadEmpty(observed);
                    BluetoothPassiveScanActiveStep::Continue(self)
                }
                step @ BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                    _,
                ) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                    BluetoothPassiveScanActiveFaultOwner::HardwareHeadRetirement(step),
                ),
                step @ BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                    _,
                ) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::FinishedListDrainStillActive,
                    BluetoothPassiveScanActiveFaultOwner::HardwareHeadRetirement(step),
                ),
                step @ BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::ExpectedHardwareHeadStillPublished,
                    BluetoothPassiveScanActiveFaultOwner::HardwareHeadRetirement(step),
                ),
                step @ BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::UnexpectedHardwareHeadChanged,
                    BluetoothPassiveScanActiveFaultOwner::HardwareHeadRetirement(step),
                ),
            },
            BluetoothPassiveScanActivePhase::HardwareHeadEmpty(observed) => match self
                .axes
                .task
                .unlink_and_arm_passive_scan_software_list_removal(observed)
            {
                BluetoothPassiveScanPostUnlinkArmStep::Armed(awaiting) => {
                    self.phase = BluetoothPassiveScanActivePhase::PostUnlinkAwaiting(awaiting);
                    BluetoothPassiveScanActiveStep::Continue(self)
                }
                step @ BluetoothPassiveScanPostUnlinkArmStep::MailboxBusy(_) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxBusy,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkArm(step),
                ),
                step @ BluetoothPassiveScanPostUnlinkArmStep::MailboxIdentityExhausted(_) => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxIdentityExhausted,
                        BluetoothPassiveScanActiveFaultOwner::PostUnlinkArm(step),
                    )
                }
                step @ BluetoothPassiveScanPostUnlinkArmStep::GenerationExhausted(_) => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxGenerationExhausted,
                        BluetoothPassiveScanActiveFaultOwner::PostUnlinkArm(step),
                    )
                }
                step @ BluetoothPassiveScanPostUnlinkArmStep::SchedulerIdentityMismatch(_) => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::PostUnlinkArm(step),
                    )
                }
                step @ BluetoothPassiveScanPostUnlinkArmStep::MailboxCommitMismatch(_) => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxCommitMismatch,
                        BluetoothPassiveScanActiveFaultOwner::PostUnlinkArm(step),
                    )
                }
            },
            BluetoothPassiveScanActivePhase::PostUnlinkAwaiting(awaiting) => match self
                .axes
                .task
                .consume_published_passive_scan_software_list_removal(awaiting)
            {
                BluetoothPassiveScanSoftwareListRemovalPublishedStep::NoSchedulerWork {
                    awaiting,
                    ..
                }
                | BluetoothPassiveScanSoftwareListRemovalPublishedStep::PublishedPending {
                    awaiting,
                } => {
                    self.phase = BluetoothPassiveScanActivePhase::PostUnlinkAwaiting(awaiting);
                    BluetoothPassiveScanActiveStep::Continue(self)
                }
                BluetoothPassiveScanSoftwareListRemovalPublishedStep::DirectPending {
                    awaiting,
                } => {
                    self.phase = BluetoothPassiveScanActivePhase::PostUnlinkAwaiting(awaiting);
                    BluetoothPassiveScanActiveStep::Waiting(self)
                }
                BluetoothPassiveScanSoftwareListRemovalPublishedStep::Ready { ready } => {
                    self.phase = BluetoothPassiveScanActivePhase::RemovalReady(ready);
                    BluetoothPassiveScanActiveStep::Continue(self)
                }
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                    _,
                ) => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkMailboxAffinityMismatch,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::Fault { .. } => {
                    active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::PrimaryInterruptFault,
                        BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                    )
                }
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkPendingRearmMismatch,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::RecheckUnavailable {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkRecheckUnavailable,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::PostUnlinkRecheckRearmMismatch,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
                step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                    ..
                }
                | step @ BluetoothPassiveScanSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                    ..
                } => active_fault(
                    self.axes,
                    BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                    BluetoothPassiveScanActiveFaultOwner::PostUnlinkPublished(step),
                ),
            },
            BluetoothPassiveScanActivePhase::RemovalReady(ready) => {
                match self.axes.task.recycle_passive_scan_completed(ready) {
                    BluetoothPassiveScanSchedulerRecycleStep::Recycled(recycled) => {
                        match self.axes.task.restore_passive_scan_recycled(recycled) {
                            Ok((received, status)) => {
                                let channel = self.axes.window.channel();
                                BluetoothPassiveScanActiveStep::CpuOwned(
                                    BluetoothPassiveScanEventCpuOwned {
                                        task: self.axes.task,
                                        scanner: self.axes.window.complete(),
                                        phase: self.axes.phase,
                                        channel,
                                        received,
                                        status,
                                    },
                                )
                            }
                            Err(failure) => active_fault(
                                self.axes,
                                BluetoothPassiveScanActiveFaultCause::RuntimeGraphMismatch,
                                BluetoothPassiveScanActiveFaultOwner::RuntimeRestore(failure),
                            ),
                        }
                    }
                    step @ BluetoothPassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch(
                        _,
                    ) => active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::FinishedListDrainStillActive(
                        _,
                    ) => active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::FinishedListDrainStillActive,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::MemoryIdentityMismatch {
                        ..
                    } => active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::MemoryIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothPassiveScanSchedulerRecycleStep::ReceiveInvalid { .. } => {
                        active_fault(
                            self.axes,
                            BluetoothPassiveScanActiveFaultCause::ReceiveInvalid,
                            BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                        )
                    }
                    step @ BluetoothPassiveScanSchedulerRecycleStep::ReservationIdentityMismatch(
                        _,
                    ) => active_fault(
                        self.axes,
                        BluetoothPassiveScanActiveFaultCause::ReservationIdentityMismatch,
                        BluetoothPassiveScanActiveFaultOwner::Recycle(step),
                    ),
                }
            }
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

fn waiting_or_continue<'runtime, S, const CAPACITY: usize>(
    session: BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
) -> BluetoothPassiveScanActiveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    if matches!(
        session.phase,
        BluetoothPassiveScanActivePhase::RunningAwaitingWake(_)
    ) {
        BluetoothPassiveScanActiveStep::Waiting(session)
    } else {
        BluetoothPassiveScanActiveStep::Continue(session)
    }
}

fn running_phase(
    drain: BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerRunning>,
) -> BluetoothPassiveScanActivePhase {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(running) => {
            BluetoothPassiveScanActivePhase::RunningAwaitingWake(running)
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothPassiveScanActivePhase::RunningDrain(pending)
        }
    }
}

fn completed_phase(
    drain: BluetoothSchedulerFinishedListDrainState<
        BluetoothPassiveScanSchedulerCompletionObserved,
    >,
) -> BluetoothPassiveScanActivePhase {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(completed) => {
            BluetoothPassiveScanActivePhase::CompletionObserved(completed)
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothPassiveScanActivePhase::CompletionDrain(pending)
        }
    }
}
