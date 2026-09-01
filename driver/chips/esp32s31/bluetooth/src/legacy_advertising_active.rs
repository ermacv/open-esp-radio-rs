//! Active legacy-advertising composition after the first scheduler `RUN`.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingEventCompletionStatuses;

use crate::{
    BluetoothControllerIdleCommandTask, BluetoothControllerIdleResponsePending,
    BluetoothControllerIdleResponsePublication, BluetoothControllerPublishedTaskService,
    BluetoothDtmPostUnlinkWakeCell, BluetoothLegacyAdvertisingEventCompleted,
    BluetoothLegacyAdvertisingPostUnlinkArmStep, BluetoothLegacyAdvertisingPostUnlinkAwaiting,
    BluetoothLegacyAdvertisingSchedulerCompletionObserved,
    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep,
    BluetoothLegacyAdvertisingSchedulerCompletionStep,
    BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved,
    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep,
    BluetoothLegacyAdvertisingSchedulerRecycleStep, BluetoothLegacyAdvertisingSchedulerRunning,
    BluetoothLegacyAdvertisingSchedulerRunningDrainStep,
    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady,
    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerFinishedListDrainPending,
    BluetoothSchedulerFinishedListDrainState, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerWakeBatch, BluetoothSchedulerWakeCell,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = open_esp_radio_bluetooth_hci::LeControllerCommandReady<'runtime, ()>;

/// Accepted Enable response paired with the exact already-running graph.
#[must_use = "publish the response while retaining the running advertising owner"]
pub struct BluetoothLegacyAdvertisingResponsePendingSession<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
    ) -> Self {
        Self { pending, running }
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
        self.pending.wait_response_capacity(controller).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> BluetoothLegacyAdvertisingResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        let Self { pending, running } = self;
        match pending.try_publish(controller) {
            BluetoothControllerIdleResponsePublication::Published(task) => {
                BluetoothLegacyAdvertisingResponsePublication::Published(
                    BluetoothLegacyAdvertisingActiveSession::from_running(task, running),
                )
            }
            BluetoothControllerIdleResponsePublication::Pending(pending) => {
                BluetoothLegacyAdvertisingResponsePublication::Pending(Self { pending, running })
            }
            BluetoothControllerIdleResponsePublication::EndpointMismatch(pending) => {
                BluetoothLegacyAdvertisingResponsePublication::EndpointMismatch(Self {
                    pending,
                    running,
                })
            }
            BluetoothControllerIdleResponsePublication::Fault { pending, error } => {
                BluetoothLegacyAdvertisingResponsePublication::Fault {
                    pending: Self { pending, running },
                    error,
                }
            }
        }
    }
}

/// Result of publishing the Success response for an already-running event.
#[must_use = "retain the active session or unchanged response transaction"]
pub enum BluetoothLegacyAdvertisingResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyAdvertisingActiveSession<'runtime, S, SCHEDULER_CAPACITY>),
    Pending(BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Fault {
        pending: BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>,
        error: open_esp_radio_bluetooth_hci::HciChannelError,
    },
}

struct BluetoothLegacyAdvertisingActiveAxes<'runtime, S, const CAPACITY: usize> {
    task: Task<'runtime, S, CAPACITY>,
    order: Order<'runtime>,
    scheduler_item_address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum BluetoothLegacyAdvertisingActivePhase {
    RunningAwaitingWake(BluetoothLegacyAdvertisingSchedulerRunning<'static>),
    RunningReady {
        running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
        wake: BluetoothSchedulerWakeBatch,
    },
    RunningDrain(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerRunning<'static>,
        >,
    ),
    CompletionDrain(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'static>,
        >,
    ),
    CompletionObserved(BluetoothLegacyAdvertisingSchedulerCompletionObserved<'static>),
    HardwareHeadEmpty(BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'static>),
    PostUnlinkAwaiting(BluetoothLegacyAdvertisingPostUnlinkAwaiting<'static>),
    RemovalReady(BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'static>),
}

/// Exact HCI order and active advertising graph after Success publication.
#[must_use = "drive the advertising graph to its next CPU-owned event boundary"]
pub struct BluetoothLegacyAdvertisingActiveSession<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    axes: BluetoothLegacyAdvertisingActiveAxes<'runtime, S, SCHEDULER_CAPACITY>,
    phase: BluetoothLegacyAdvertisingActivePhase,
}

/// Borrowed wait source for the exact active-advertising phase.
pub enum BluetoothLegacyAdvertisingActiveWait<'a> {
    Scheduler(&'a BluetoothSchedulerWakeCell),
    PostUnlink(&'a BluetoothDtmPostUnlinkWakeCell),
}

/// One bounded active-advertising progression result.
#[must_use = "retain the active owner, CPU boundary, unrelated list, or fail-stop owner"]
pub enum BluetoothLegacyAdvertisingActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    Waiting(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
}

/// Exact Controller, HCI order and completed event between advertising events.
#[must_use = "schedule the successor or stop at this CPU-owned boundary"]
pub struct BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    axes: BluetoothLegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    completed: BluetoothLegacyAdvertisingEventCompleted<'static>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn statuses(&self) -> BluetoothLegacyAdvertisingEventCompletionStatuses {
        self.completed.statuses()
    }

    pub const fn phase(&self) -> crate::BluetoothLegacyAdvertisingEventPhase {
        self.completed.phase()
    }

    pub const fn scheduler_item_address(
        &self,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        self.axes.scheduler_item_address
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.axes.hardware_list_index
    }

    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.axes.order.accepts_endpoint(controller)
    }
}

/// Finite fail-closed classification for active advertising progression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingActiveFaultCause {
    FinishedListDrainAlreadyActive,
    SchedulerIdentityMismatch,
    FinishedListDrainLost,
    RepeatedAdvertisingList,
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
    ReservationIdentityMismatch,
}

#[allow(
    dead_code,
    reason = "the opaque fault owner intentionally retains every lower affine token"
)]
enum BluetoothLegacyAdvertisingActiveFaultOwner {
    Completion(BluetoothLegacyAdvertisingSchedulerCompletionStep<'static>),
    RunningDrain(BluetoothLegacyAdvertisingSchedulerRunningDrainStep<'static>),
    CompletionDrain(BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep<'static>),
    HardwareHeadRetirement(BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep<'static>),
    PostUnlinkArm(BluetoothLegacyAdvertisingPostUnlinkArmStep<'static>),
    PostUnlinkPublished(BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep<'static>),
    Recycle(BluetoothLegacyAdvertisingSchedulerRecycleStep<'static>),
}

/// Opaque fail-stop owner retaining the Controller, HCI order and lower graph.
#[must_use = "retain the exact failed advertising owner for diagnostic shutdown"]
pub struct BluetoothLegacyAdvertisingActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingActiveFaultCause,
    _axes: BluetoothLegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    _owner: BluetoothLegacyAdvertisingActiveFaultOwner,
}

impl<S, const CAPACITY: usize> BluetoothLegacyAdvertisingActiveFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyAdvertisingActiveFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_running(
        task: BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>,
        running: BluetoothLegacyAdvertisingSchedulerRunning<'static>,
    ) -> Self {
        let scheduler_item_address = running.scheduler_item_address();
        let hardware_list_index = running.hardware_list_index();
        let (task, order) = task.into_parts();
        Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                task,
                order,
                scheduler_item_address,
                hardware_list_index,
            },
            phase: BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(running),
        }
    }

    pub const fn scheduler_item_address(
        &self,
    ) -> open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress {
        self.axes.scheduler_item_address
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.axes.hardware_list_index
    }

    pub const fn scheduler_wake(&self) -> &BluetoothSchedulerWakeCell {
        self.axes.task.scheduler_wake()
    }

    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &open_esp_radio_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        self.axes.order.accepts_endpoint(controller)
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyAdvertisingActiveWait<'_>> {
        match self.phase {
            BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(_) => Some(
                BluetoothLegacyAdvertisingActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            BluetoothLegacyAdvertisingActivePhase::PostUnlinkAwaiting(_) => Some(
                BluetoothLegacyAdvertisingActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            _ => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(mut self) -> BluetoothLegacyAdvertisingActiveStep<'runtime, S, CAPACITY> {
        let scheduler_wake = if matches!(
            &self.phase,
            BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(_)
        ) {
            let Some(wake) = self.axes.task.scheduler_wake().take() else {
                return BluetoothLegacyAdvertisingActiveStep::Waiting(self);
            };
            Some(wake)
        } else {
            None
        };
        match self.phase {
            BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(running) => {
                let wake = scheduler_wake
                    .expect("the running-awaiting-wake phase consumed one scheduler wake");
                self.phase = BluetoothLegacyAdvertisingActivePhase::RunningReady { running, wake };
                BluetoothLegacyAdvertisingActiveStep::Continue(self)
            }
            BluetoothLegacyAdvertisingActivePhase::RunningReady { running, wake } => {
                let step = self
                    .axes
                    .task
                    .observe_legacy_advertising_completion(running, wake);
                match step {
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::NoFinishedList(running) => {
                        self.phase =
                            BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(running);
                        BluetoothLegacyAdvertisingActiveStep::Waiting(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::UnrelatedList {
                        drain,
                        observed,
                    } => {
                        self.phase = running_phase(drain);
                        BluetoothLegacyAdvertisingActiveStep::UnrelatedList {
                            session: self,
                            observed,
                        }
                    }
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::StillInFlight(drain) => {
                        self.phase = running_phase(drain);
                        waiting_or_continue(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::CompletionObserved(
                        drain,
                    ) => {
                        self.phase = completed_phase(drain);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::DrainAlreadyActive(
                        running,
                    ) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainAlreadyActive,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Completion(
                            BluetoothLegacyAdvertisingSchedulerCompletionStep::DrainAlreadyActive(
                                running,
                            ),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerCompletionStep::SchedulerIdentityMismatch(
                        running,
                    ) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Completion(
                            BluetoothLegacyAdvertisingSchedulerCompletionStep::SchedulerIdentityMismatch(
                                running,
                            ),
                        ),
                    ),
                }
            }
            BluetoothLegacyAdvertisingActivePhase::RunningDrain(pending) => {
                let step = self
                    .axes
                    .task
                    .continue_legacy_advertising_running_finished_list_drain(pending);
                match step {
                    BluetoothLegacyAdvertisingSchedulerRunningDrainStep::UnrelatedList {
                        drain,
                        observed,
                    } => {
                        self.phase = running_phase(drain);
                        BluetoothLegacyAdvertisingActiveStep::UnrelatedList {
                            session: self,
                            observed,
                        }
                    }
                    BluetoothLegacyAdvertisingSchedulerRunningDrainStep::StillInFlight(drain) => {
                        self.phase = running_phase(drain);
                        waiting_or_continue(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerRunningDrainStep::CompletionObserved(
                        drain,
                    ) => {
                        self.phase = completed_phase(drain);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                        pending,
                    ) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::RunningDrain(
                            BluetoothLegacyAdvertisingSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                                pending,
                            ),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerRunningDrainStep::DrainLost(pending) => {
                        active_fault(
                            self.axes,
                            BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainLost,
                            BluetoothLegacyAdvertisingActiveFaultOwner::RunningDrain(
                                BluetoothLegacyAdvertisingSchedulerRunningDrainStep::DrainLost(
                                    pending,
                                ),
                            ),
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingActivePhase::CompletionDrain(pending) => {
                let step = self
                    .axes
                    .task
                    .continue_legacy_advertising_completed_finished_list_drain(pending);
                match step {
                    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::UnrelatedList {
                        drain,
                        observed,
                    } => {
                        self.phase = completed_phase(drain);
                        BluetoothLegacyAdvertisingActiveStep::UnrelatedList {
                            session: self,
                            observed,
                        }
                    }
                    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(pending) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::CompletionDrain(
                            BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(pending),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::DrainLost(pending) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainLost,
                        BluetoothLegacyAdvertisingActiveFaultOwner::CompletionDrain(
                            BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::DrainLost(pending),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::RepeatedAdvertisingList { drain, observed } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::RepeatedAdvertisingList,
                        BluetoothLegacyAdvertisingActiveFaultOwner::CompletionDrain(
                            BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::RepeatedAdvertisingList { drain, observed },
                        ),
                    ),
                }
            }
            BluetoothLegacyAdvertisingActivePhase::CompletionObserved(completed) => {
                let step = self
                    .axes
                    .task
                    .observe_legacy_advertising_hardware_head_retirement(completed);
                match step {
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::EmptyObserved(
                        observed,
                    ) => {
                        self.phase =
                            BluetoothLegacyAdvertisingActivePhase::HardwareHeadEmpty(observed);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::HardwareHeadRetirement(
                            BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive,
                        BluetoothLegacyAdvertisingActiveFaultOwner::HardwareHeadRetirement(
                            BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished { completed, observed } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::ExpectedHardwareHeadStillPublished,
                        BluetoothLegacyAdvertisingActiveFaultOwner::HardwareHeadRetirement(
                            BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished { completed, observed },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged { completed, observed } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::UnexpectedHardwareHeadChanged,
                        BluetoothLegacyAdvertisingActiveFaultOwner::HardwareHeadRetirement(
                            BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged { completed, observed },
                        ),
                    ),
                }
            }
            BluetoothLegacyAdvertisingActivePhase::HardwareHeadEmpty(observed) => {
                let step = self
                    .axes
                    .task
                    .unlink_and_arm_legacy_advertising_software_list_removal(observed);
                match step {
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::Armed(awaiting) => {
                        self.phase =
                            BluetoothLegacyAdvertisingActivePhase::PostUnlinkAwaiting(awaiting);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxBusy(observed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxBusy,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkArm(
                            BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxBusy(observed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxIdentityExhausted(observed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxIdentityExhausted,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkArm(
                            BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxIdentityExhausted(observed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::GenerationExhausted(observed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxGenerationExhausted,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkArm(
                            BluetoothLegacyAdvertisingPostUnlinkArmStep::GenerationExhausted(observed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::SchedulerIdentityMismatch(observed) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkArm(
                            BluetoothLegacyAdvertisingPostUnlinkArmStep::SchedulerIdentityMismatch(observed),
                        ),
                    ),
                    BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxCommitMismatch(unlinked) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxCommitMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkArm(
                            BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxCommitMismatch(unlinked),
                        ),
                    ),
                }
            }
            BluetoothLegacyAdvertisingActivePhase::PostUnlinkAwaiting(awaiting) => {
                let step = self
                    .axes
                    .task
                    .consume_published_legacy_advertising_software_list_removal(awaiting);
                match step {
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, .. }
                    | BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::PublishedPending { awaiting } => {
                        self.phase = BluetoothLegacyAdvertisingActivePhase::PostUnlinkAwaiting(awaiting);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::DirectPending { awaiting } => {
                        self.phase = BluetoothLegacyAdvertisingActivePhase::PostUnlinkAwaiting(awaiting);
                        BluetoothLegacyAdvertisingActiveStep::Waiting(self)
                    }
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Ready { ready } => {
                        self.phase = BluetoothLegacyAdvertisingActivePhase::RemovalReady(ready);
                        BluetoothLegacyAdvertisingActiveStep::Continue(self)
                    }
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxAffinityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Fault { unlinked, fault } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PrimaryInterruptFault,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Fault { unlinked, fault },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkPendingRearmMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkRecheckUnavailable,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkRecheckRearmMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { unlinked, event } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { unlinked, event },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { unlinked } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::PostUnlinkPublished(
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { unlinked },
                        ),
                    ),
                }
            }
            BluetoothLegacyAdvertisingActivePhase::RemovalReady(ready) => {
                let step = self.axes.task.recycle_legacy_advertising_completed(ready);
                match step {
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::Recycled(recycled) => {
                        BluetoothLegacyAdvertisingActiveStep::CpuOwned(
                            BluetoothLegacyAdvertisingEventCpuOwned {
                                axes: self.axes,
                                completed: recycled.complete_event(),
                            },
                        )
                    }
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch(ready) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(
                            BluetoothLegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch(ready),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive(ready) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(
                            BluetoothLegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive(ready),
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch { ready, error } => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::MemoryIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(
                            BluetoothLegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch { ready, error },
                        ),
                    ),
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch(ready) => active_fault(
                        self.axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::ReservationIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(
                            BluetoothLegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch(ready),
                        ),
                    ),
                }
            }
        }
    }
}

fn active_fault<'runtime, S, const CAPACITY: usize>(
    axes: BluetoothLegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    cause: BluetoothLegacyAdvertisingActiveFaultCause,
    owner: BluetoothLegacyAdvertisingActiveFaultOwner,
) -> BluetoothLegacyAdvertisingActiveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    BluetoothLegacyAdvertisingActiveStep::Fault(BluetoothLegacyAdvertisingActiveFault {
        cause,
        _axes: axes,
        _owner: owner,
    })
}

fn waiting_or_continue<'runtime, S, const CAPACITY: usize>(
    session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
) -> BluetoothLegacyAdvertisingActiveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    if matches!(
        session.phase,
        BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(_)
    ) {
        BluetoothLegacyAdvertisingActiveStep::Waiting(session)
    } else {
        BluetoothLegacyAdvertisingActiveStep::Continue(session)
    }
}

fn running_phase(
    drain: BluetoothSchedulerFinishedListDrainState<
        BluetoothLegacyAdvertisingSchedulerRunning<'static>,
    >,
) -> BluetoothLegacyAdvertisingActivePhase {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(running) => {
            BluetoothLegacyAdvertisingActivePhase::RunningAwaitingWake(running)
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothLegacyAdvertisingActivePhase::RunningDrain(pending)
        }
    }
}

fn completed_phase(
    drain: BluetoothSchedulerFinishedListDrainState<
        BluetoothLegacyAdvertisingSchedulerCompletionObserved<'static>,
    >,
) -> BluetoothLegacyAdvertisingActivePhase {
    match drain {
        BluetoothSchedulerFinishedListDrainState::Drained(completed) => {
            BluetoothLegacyAdvertisingActivePhase::CompletionObserved(completed)
        }
        BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
            BluetoothLegacyAdvertisingActivePhase::CompletionDrain(pending)
        }
    }
}
