//! Active legacy-advertising composition after the first scheduler `RUN`.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerIdleCommandTask, ControllerIdleResponsePending,
        ControllerIdleResponsePublication, ControllerPublishedTaskService,
        SchedulerRunInterruptStorage, boot::SingleItemSchedulerCompletionFaultOwner,
    },
    interrupt::SchedulerWakeCell,
    le::{
        advertising::{
            LegacyAdvertisingEventCompleted, legacy::completion::LegacyAdvertisingCompletionRole,
        },
        dtm::DtmPostUnlinkWakeCell,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
        completion::{
            SingleItemCompletion, SingleItemCompletionFault, SingleItemCompletionFaultCause,
            SingleItemCompletionStep, SingleItemCompletionWaitKind,
        },
        core::LegacyAdvertisingSchedulerRecycleStep,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciActiveLegacyAdvertisingCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerDeferredLegacyAdvertisingDisable, LeControllerResetBarrier,
    LeControllerResetCompletion, LeControllerResponsePending, LeControllerResponsePublication,
};

use oer_esp32s31_bluetooth_memory::LegacyAdvertisingEventCompletionStatuses;

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = oer_bluetooth_hci::LeControllerCommandReady<'runtime, ()>;
type CompletionRole = LegacyAdvertisingCompletionRole<'static>;
type SchedulerRunning = crate::scheduler::core::SingleItemSchedulerRunning<CompletionRole>;
type RemovalReady =
    crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady<CompletionRole>;

enum LegacyAdvertisingOrder<'runtime> {
    Ready(Order<'runtime>),
    Detached,
}

/// Accepted Enable response paired with the exact already-running graph.
#[must_use = "publish the response while retaining the running advertising owner"]
pub struct LegacyAdvertisingResponsePendingSession<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pending: ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    running: SchedulerRunning,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        pending: ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        running: SchedulerRunning,
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
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        self.pending.wait_response_capacity(controller).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        let Self { pending, running } = self;
        match pending.try_publish(controller) {
            ControllerIdleResponsePublication::Published(task) => {
                LegacyAdvertisingResponsePublication::Published(
                    LegacyAdvertisingActiveSession::from_running(task, running),
                )
            }
            ControllerIdleResponsePublication::Pending(pending) => {
                LegacyAdvertisingResponsePublication::Pending(Self { pending, running })
            }
            ControllerIdleResponsePublication::EndpointMismatch(pending) => {
                LegacyAdvertisingResponsePublication::EndpointMismatch(Self { pending, running })
            }
            ControllerIdleResponsePublication::Fault { pending, error } => {
                LegacyAdvertisingResponsePublication::Fault {
                    pending: Self { pending, running },
                    error,
                }
            }
        }
    }
}

/// Result of publishing the Success response for an already-running event.
#[must_use = "retain the active session or unchanged response transaction"]
pub enum LegacyAdvertisingResponsePublication<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyAdvertisingActiveSession<'runtime, S, SCHEDULER_CAPACITY>),
    Pending(LegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    EndpointMismatch(LegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        pending: LegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>,
        error: oer_bluetooth_hci::HciChannelError,
    },
}

struct LegacyAdvertisingActiveAxes<'runtime, S, const CAPACITY: usize> {
    task: Task<'runtime, S, CAPACITY>,
    order: LegacyAdvertisingOrder<'runtime>,
    scheduler_item_address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum LegacyAdvertisingActivePhase {
    Completion(SingleItemCompletion<CompletionRole>),
    RemovalReady(RemovalReady),
}

/// Exact HCI order and active advertising graph after Success publication.
#[must_use = "drive the advertising graph to its next CPU-owned event boundary"]
pub struct LegacyAdvertisingActiveSession<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    axes: LegacyAdvertisingActiveAxes<'runtime, S, SCHEDULER_CAPACITY>,
    phase: LegacyAdvertisingActivePhase,
}

/// Borrowed wait source for the exact active-advertising phase.
pub enum LegacyAdvertisingActiveWait<'a> {
    Scheduler(&'a SchedulerWakeCell),
    PostUnlink(&'a DtmPostUnlinkWakeCell),
}

/// One bounded active-advertising progression result.
#[must_use = "retain the active owner, CPU boundary, unrelated list, or fail-stop owner"]
pub enum LegacyAdvertisingActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    Waiting(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
}

/// One pending command response whose radio continuation remains independently active.
#[must_use = "publish the response while continuing the advertising radio graph"]
pub struct LegacyAdvertisingActiveResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    >,
}

/// Response publication with the active radio owner preserved exactly once.
#[must_use = "retain the pending response or the returned command-ready session"]
pub enum LegacyAdvertisingActiveResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    Pending(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One bounded radio transition while an HCI response remains pending.
#[must_use = "continue radio progress without losing the pending response"]
pub enum LegacyAdvertisingActivePendingRadioStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Waiting(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    UnrelatedList {
        pending: LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingActivePendingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner preserving the exact pending response and radio fault.
#[must_use = "retain the failed radio and ordered response for shutdown diagnostics"]
pub struct LegacyAdvertisingActivePendingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _fault: LegacyAdvertisingActiveFault<'runtime, S, CAPACITY>,
    _response: LeControllerResponsePending<'runtime, ()>,
}

pub(crate) enum LegacyAdvertisingStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Accepted active-role Disable or Reset driven through the current event completion.
#[must_use = "drive the current event to CPU ownership before restoring the runtime"]
pub struct LegacyAdvertisingStopping<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    active: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    order: LegacyAdvertisingStopOrder<'runtime>,
}

/// One bounded active advertising stop transition.
#[must_use = "retain the stop order through event completion and runtime restore"]
pub enum LegacyAdvertisingStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Waiting(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: LegacyAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    DisableRestore(LegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
    ResetRestore(LegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingStoppingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner retaining Disable/Reset order beside the exact lower fault.
#[must_use = "retain the failed stop transaction for shutdown diagnostics"]
pub struct LegacyAdvertisingStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _fault: LegacyAdvertisingActiveFault<'runtime, S, CAPACITY>,
    _order: LegacyAdvertisingStopOrder<'runtime>,
}

/// Opaque owner for an impossible endpoint mismatch after active command intake.
#[must_use = "retain the complete command, radio continuation and order"]
pub struct LegacyAdvertisingActiveCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command accepted while an advertising event is in flight.
#[must_use = "publish, stop, or retain the exact mismatch owner"]
pub enum LegacyAdvertisingActiveCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingActiveCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking command intake while the current event remains hardware-owned.
#[must_use = "route a command or retain the exact active session"]
pub enum LegacyAdvertisingActiveCommandIntake<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: LegacyAdvertisingActiveCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Exact Controller, HCI order and completed event between advertising events.
#[must_use = "schedule the successor or stop at this CPU-owned boundary"]
pub struct LegacyAdvertisingEventCpuOwned<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    axes: LegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    completed: LegacyAdvertisingEventCompleted<'static>,
}

struct LegacyAdvertisingCpuOwnedRadio<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    scheduler_item_address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
    completed: LegacyAdvertisingEventCompleted<'static>,
}

/// One response retained at a CPU-owned boundary before the next event is scheduled.
#[must_use = "publish the response while retaining the complete advertising continuation"]
pub struct LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Result of publishing a command response at the between-event boundary.
#[must_use = "retain the pending response or the returned CPU-owned continuation"]
pub enum LegacyAdvertisingCpuOwnedResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    Pending(LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Accepted Disable after the hardware graph is CPU-owned but before runtime restore.
#[must_use = "restore the advertising graph before completing Disable"]
pub struct LegacyAdvertisingDisableRestore<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    radio: LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    deferred: LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>,
}

/// Successful runtime restore with the exact Disable response still pending.
#[must_use = "publish Disable before returning the idle command owner"]
pub struct LegacyAdvertisingDisableResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// One exact Disable runtime-restore attempt.
#[must_use = "publish the response or retain the unchanged restore owner"]
pub enum LegacyAdvertisingDisableRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Rejected(LegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
}

/// Result of publishing the exact successful Disable response.
#[must_use = "retain backpressure/fault or return the sole idle command owner"]
pub enum LegacyAdvertisingDisableResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completed(ControllerIdleCommandTask<'runtime, S, CAPACITY>),
    Pending(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Reset retained with the exact CPU-owned advertising graph.
#[must_use = "restore the graph and apply Reset only after quiescence"]
pub struct LegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    barrier:
        LeControllerResetBarrier<'runtime, LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>>,
}

/// CPU-owned advertising graph and opaque Reset order before runtime restore.
#[must_use = "restore the exact graph before applying Reset"]
pub struct LegacyAdvertisingResetRestore<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    radio: LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    barrier: LeControllerResetBarrier<'runtime, ()>,
}

/// One advertising Reset graph-restore attempt.
#[must_use = "apply Reset or retain the unchanged restore owner"]
pub enum LegacyAdvertisingResetRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    CompletionReady(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(LegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
}

/// Restored advertising runtime with Reset still undispatched.
#[must_use = "apply Reset once through the matching endpoint"]
pub struct LegacyAdvertisingResetCompletionReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    barrier: LeControllerResetBarrier<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of applying the retained Reset after advertising quiescence.
#[must_use = "publish Reset or retain the endpoint mismatch"]
pub enum LegacyAdvertisingResetCompletion<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
}

/// Applied Reset response retaining the already-restored Controller task.
#[must_use = "publish Reset before returning the idle command owner"]
pub struct LegacyAdvertisingResetResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of publishing Reset after advertising quiescence.
#[must_use = "retain backpressure/fault or return the sole idle command owner"]
pub enum LegacyAdvertisingResetResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completed(ControllerIdleCommandTask<'runtime, S, CAPACITY>),
    Pending(LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Opaque owner for an impossible endpoint mismatch after CPU-boundary intake.
#[must_use = "retain the complete command, order and advertising continuation"]
pub struct LegacyAdvertisingCpuOwnedCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed command route at the safe boundary between advertising events.
#[must_use = "publish, disable, reset, or retain the exact mismatch owner"]
pub enum LegacyAdvertisingCpuOwnedCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Disable(LegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
    ResetBarrier(LegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyAdvertisingCpuOwnedCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking command intake while no advertising event is published.
#[must_use = "route a command or retain the exact CPU-owned continuation"]
pub enum LegacyAdvertisingCpuOwnedCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: LegacyAdvertisingCpuOwnedCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        completed: LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        completed: LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        completed: LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        completed: LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn statuses(&self) -> LegacyAdvertisingEventCompletionStatuses {
        self.completed.statuses()
    }

    pub const fn phase(&self) -> crate::le::advertising::LegacyAdvertisingEventPhase {
        self.completed.phase()
    }

    pub const fn scheduler_item_address(
        &self,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
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
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        match &self.axes.order {
            LegacyAdvertisingOrder::Ready(order) => order.accepts_endpoint(controller),
            LegacyAdvertisingOrder::Detached => false,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        Order<'runtime>,
        oer_esp32s31_hal::types::BluetoothControllerSramAddress,
        BluetoothSchedulerHardwareListIndex,
        LegacyAdvertisingEventCompleted<'static>,
    ) {
        (
            self.axes.task,
            match self.axes.order {
                LegacyAdvertisingOrder::Ready(order) => order,
                LegacyAdvertisingOrder::Detached => {
                    unreachable!("a detached CPU owner cannot schedule another event")
                }
            },
            self.axes.scheduler_item_address,
            self.axes.hardware_list_index,
            self.completed,
        )
    }

    pub(crate) fn from_parts(
        task: Task<'runtime, S, CAPACITY>,
        order: Order<'runtime>,
        scheduler_item_address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
        completed: LegacyAdvertisingEventCompleted<'static>,
    ) -> Self {
        Self {
            axes: LegacyAdvertisingActiveAxes {
                task,
                order: LegacyAdvertisingOrder::Ready(order),
                scheduler_item_address,
                hardware_list_index,
            },
            completed,
        }
    }

    fn into_radio(
        self,
    ) -> (
        LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
        Order<'runtime>,
    ) {
        let Self { axes, completed } = self;
        (
            LegacyAdvertisingCpuOwnedRadio {
                task: axes.task,
                scheduler_item_address: axes.scheduler_item_address,
                hardware_list_index: axes.hardware_list_index,
                completed,
            },
            match axes.order {
                LegacyAdvertisingOrder::Ready(order) => order,
                LegacyAdvertisingOrder::Detached => {
                    unreachable!("a detached CPU owner cannot accept another command")
                }
            },
        )
    }

    fn from_radio(
        radio: LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
        order: Order<'runtime>,
    ) -> Self {
        Self {
            axes: LegacyAdvertisingActiveAxes {
                task: radio.task,
                order: LegacyAdvertisingOrder::Ready(order),
                scheduler_item_address: radio.scheduler_item_address,
                hardware_list_index: radio.hardware_list_index,
            },
            completed: radio.completed,
        }
    }

    fn into_detached_radio(self) -> LegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY> {
        let Self { axes, completed } = self;
        match axes.order {
            LegacyAdvertisingOrder::Detached => LegacyAdvertisingCpuOwnedRadio {
                task: axes.task,
                scheduler_item_address: axes.scheduler_item_address,
                hardware_list_index: axes.hardware_list_index,
                completed,
            },
            LegacyAdvertisingOrder::Ready(_) => {
                unreachable!("a command-ready CPU owner cannot join detached HCI order")
            }
        }
    }

    /// Consume and route at most one command before scheduling the successor event.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        buffer: &'buffer mut [u8],
    ) -> LegacyAdvertisingCpuOwnedCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let (radio, order) = self.into_radio();
        let ready = order.map_owner(|()| radio);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_advertising_classified_command(command) {
                        HciActiveLegacyAdvertisingCommandRoute::ResponsePending(transaction) => {
                            LegacyAdvertisingCpuOwnedCommandRoute::ResponsePending(
                                LegacyAdvertisingCpuOwnedResponsePending { transaction },
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                            let (radio, deferred) = deferred.into_parts();
                            LegacyAdvertisingCpuOwnedCommandRoute::Disable(
                                LegacyAdvertisingDisableRestore { radio, deferred },
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                            LegacyAdvertisingCpuOwnedCommandRoute::ResetBarrier(
                                LegacyAdvertisingCpuOwnedResetBarrier { barrier },
                            )
                        }
                        HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                            LegacyAdvertisingCpuOwnedCommandRoute::EndpointMismatch(
                                LegacyAdvertisingCpuOwnedCommandMismatch { _command: command },
                            )
                        }
                    };
                LegacyAdvertisingCpuOwnedCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                LegacyAdvertisingCpuOwnedCommandIntake::Empty {
                    completed: Self::from_radio(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                LegacyAdvertisingCpuOwnedCommandIntake::EndpointMismatch {
                    completed: Self::from_radio(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (radio, order) = ready.into_parts();
                LegacyAdvertisingCpuOwnedCommandIntake::Channel {
                    completed: Self::from_radio(radio, order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (radio, order) = ready.into_parts();
                LegacyAdvertisingCpuOwnedCommandIntake::NonCommand {
                    completed: Self::from_radio(radio, order),
                    frame,
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingCpuOwnedResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                LegacyAdvertisingCpuOwnedResponsePublication::Published(
                    LegacyAdvertisingEventCpuOwned::from_radio(radio, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                LegacyAdvertisingCpuOwnedResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                LegacyAdvertisingCpuOwnedResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => LegacyAdvertisingCpuOwnedResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<LegacyAdvertisingActiveWait<'_>> {
        self.transaction.owner().radio_wait()
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (active, order) = ready.into_parts();
                LegacyAdvertisingActiveResponsePublication::Published(active.attach_order(order))
            }
            LeControllerResponsePublication::Pending(transaction) => {
                LegacyAdvertisingActiveResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                LegacyAdvertisingActiveResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => LegacyAdvertisingActiveResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }

    pub fn step_radio(self) -> LegacyAdvertisingActivePendingRadioStep<'runtime, S, CAPACITY> {
        let (active, response) = self.transaction.into_parts();
        match active.step_radio() {
            LegacyAdvertisingActiveStep::Continue(active) => {
                LegacyAdvertisingActivePendingRadioStep::Continue(Self {
                    transaction: response.map_owner(|()| active),
                })
            }
            LegacyAdvertisingActiveStep::Waiting(active) => {
                LegacyAdvertisingActivePendingRadioStep::Waiting(Self {
                    transaction: response.map_owner(|()| active),
                })
            }
            LegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                LegacyAdvertisingActivePendingRadioStep::UnrelatedList {
                    pending: Self {
                        transaction: response.map_owner(|()| session),
                    },
                    observed,
                }
            }
            LegacyAdvertisingActiveStep::CpuOwned(completed) => {
                LegacyAdvertisingActivePendingRadioStep::CpuOwned(
                    LegacyAdvertisingCpuOwnedResponsePending {
                        transaction: response.map_owner(|()| completed.into_detached_radio()),
                    },
                )
            }
            LegacyAdvertisingActiveStep::Fault(fault) => {
                LegacyAdvertisingActivePendingRadioStep::Fault(
                    LegacyAdvertisingActivePendingFault {
                        _fault: fault,
                        _response: response,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<LegacyAdvertisingActiveWait<'_>> {
        self.active.radio_wait()
    }

    pub fn step(self) -> LegacyAdvertisingStoppingStep<'runtime, S, CAPACITY> {
        let Self { active, order } = self;
        match active.step_radio() {
            LegacyAdvertisingActiveStep::Continue(active) => {
                LegacyAdvertisingStoppingStep::Continue(Self { active, order })
            }
            LegacyAdvertisingActiveStep::Waiting(active) => {
                LegacyAdvertisingStoppingStep::Waiting(Self { active, order })
            }
            LegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                LegacyAdvertisingStoppingStep::UnrelatedList {
                    stopping: Self {
                        active: session,
                        order,
                    },
                    observed,
                }
            }
            LegacyAdvertisingActiveStep::CpuOwned(completed) => {
                let radio = completed.into_detached_radio();
                match order {
                    LegacyAdvertisingStopOrder::Disable(deferred) => {
                        LegacyAdvertisingStoppingStep::DisableRestore(
                            LegacyAdvertisingDisableRestore { radio, deferred },
                        )
                    }
                    LegacyAdvertisingStopOrder::Reset(barrier) => {
                        LegacyAdvertisingStoppingStep::ResetRestore(LegacyAdvertisingResetRestore {
                            radio,
                            barrier,
                        })
                    }
                }
            }
            LegacyAdvertisingActiveStep::Fault(fault) => {
                LegacyAdvertisingStoppingStep::Fault(LegacyAdvertisingStoppingFault {
                    _fault: fault,
                    _order: order,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Restore the exact graph and only then construct successful Disable.
    pub fn restore(mut self) -> LegacyAdvertisingDisableRestoreStep<'runtime, S, CAPACITY> {
        match self
            .radio
            .task
            .restore_legacy_advertising_completed_disabled(self.radio.completed)
        {
            Ok(()) => LegacyAdvertisingDisableRestoreStep::ResponsePending(
                LegacyAdvertisingDisableResponsePending {
                    transaction: self
                        .deferred
                        .map_owner(|()| self.radio.task)
                        .into_stopped_response(),
                },
            ),
            Err(completed) => {
                self.radio.completed = completed;
                LegacyAdvertisingDisableRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn from_cancelled(
        task: Task<'runtime, S, CAPACITY>,
        deferred: LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>,
    ) -> Self {
        Self {
            transaction: deferred.map_owner(|()| task).into_stopped_response(),
        }
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingDisableResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                LegacyAdvertisingDisableResponsePublication::Completed(
                    ControllerIdleCommandTask::from_ready(ready),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                LegacyAdvertisingDisableResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                LegacyAdvertisingDisableResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => LegacyAdvertisingDisableResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn begin_restore(self) -> LegacyAdvertisingResetRestore<'runtime, S, CAPACITY> {
        let (radio, barrier) = self.barrier.into_parts();
        LegacyAdvertisingResetRestore { radio, barrier }
    }
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingResetRestore<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn restore(mut self) -> LegacyAdvertisingResetRestoreStep<'runtime, S, CAPACITY> {
        match self
            .radio
            .task
            .restore_legacy_advertising_completed_disabled(self.radio.completed)
        {
            Ok(()) => LegacyAdvertisingResetRestoreStep::CompletionReady(
                LegacyAdvertisingResetCompletionReady {
                    barrier: self.barrier.map_owner(|()| self.radio.task),
                },
            ),
            Err(completed) => {
                self.radio.completed = completed;
                LegacyAdvertisingResetRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn from_cancelled(
        task: Task<'runtime, S, CAPACITY>,
        barrier: LeControllerResetBarrier<'runtime, ()>,
    ) -> Self {
        Self {
            barrier: barrier.map_owner(|()| task),
        }
    }

    pub fn complete<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingResetCompletion<'runtime, S, CAPACITY> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            LeControllerResetCompletion::ResponsePending(transaction) => {
                LegacyAdvertisingResetCompletion::ResponsePending(
                    LegacyAdvertisingResetResponsePending { transaction },
                )
            }
            LeControllerResetCompletion::EndpointMismatch(barrier) => {
                LegacyAdvertisingResetCompletion::EndpointMismatch(Self { barrier })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> LegacyAdvertisingResetResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                LegacyAdvertisingResetResponsePublication::Completed(
                    ControllerIdleCommandTask::from_ready(ready),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                LegacyAdvertisingResetResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                LegacyAdvertisingResetResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => LegacyAdvertisingResetResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

/// Finite fail-closed classification for active advertising progression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingActiveFaultCause {
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
enum LegacyAdvertisingActiveFaultOwner {
    Completion(SingleItemCompletionFault<SingleItemSchedulerCompletionFaultOwner<CompletionRole>>),
    Recycle(LegacyAdvertisingSchedulerRecycleStep<'static>),
}

/// Opaque fail-stop owner retaining the Controller, HCI order and lower graph.
#[must_use = "retain the exact failed advertising owner for diagnostic shutdown"]
pub struct LegacyAdvertisingActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyAdvertisingActiveFaultCause,
    _axes: LegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    _owner: LegacyAdvertisingActiveFaultOwner,
}

impl<S, const CAPACITY: usize> LegacyAdvertisingActiveFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyAdvertisingActiveFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize> LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_running(
        task: ControllerIdleCommandTask<'runtime, S, CAPACITY>,
        running: SchedulerRunning,
    ) -> Self {
        let scheduler_item_address = running.scheduler_item_address();
        let hardware_list_index = running.hardware_list_index();
        let (task, order) = task.into_parts();
        Self {
            axes: LegacyAdvertisingActiveAxes {
                task,
                order: LegacyAdvertisingOrder::Ready(order),
                scheduler_item_address,
                hardware_list_index,
            },
            phase: LegacyAdvertisingActivePhase::Completion(SingleItemCompletion::new(running)),
        }
    }

    pub(crate) fn from_recurring_running(
        task: Task<'runtime, S, CAPACITY>,
        order: Order<'runtime>,
        running: SchedulerRunning,
    ) -> Self {
        Self {
            axes: LegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: LegacyAdvertisingOrder::Ready(order),
            },
            phase: LegacyAdvertisingActivePhase::Completion(SingleItemCompletion::new(running)),
        }
    }

    pub(crate) fn from_recurring_response_pending(
        task: Task<'runtime, S, CAPACITY>,
        response: LeControllerResponsePending<'runtime, ()>,
        running: SchedulerRunning,
    ) -> LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY> {
        let active = Self {
            axes: LegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: LegacyAdvertisingOrder::Detached,
            },
            phase: LegacyAdvertisingActivePhase::Completion(SingleItemCompletion::new(running)),
        };
        LegacyAdvertisingActiveResponsePending {
            transaction: response.map_owner(|()| active),
        }
    }

    pub(crate) fn from_recurring_stopping(
        task: Task<'runtime, S, CAPACITY>,
        order: LegacyAdvertisingStopOrder<'runtime>,
        running: SchedulerRunning,
    ) -> LegacyAdvertisingStopping<'runtime, S, CAPACITY> {
        let active = Self {
            axes: LegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: LegacyAdvertisingOrder::Detached,
            },
            phase: LegacyAdvertisingActivePhase::Completion(SingleItemCompletion::new(running)),
        };
        LegacyAdvertisingStopping { active, order }
    }

    pub const fn scheduler_item_address(
        &self,
    ) -> oer_esp32s31_hal::types::BluetoothControllerSramAddress {
        self.axes.scheduler_item_address
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.axes.hardware_list_index
    }

    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.axes.task.scheduler_wake()
    }

    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &oer_bluetooth_hci::LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> bool {
        match &self.axes.order {
            LegacyAdvertisingOrder::Ready(order) => order.accepts_endpoint(controller),
            LegacyAdvertisingOrder::Detached => false,
        }
    }

    /// Wait for Host command readiness while borrowing the complete active owner.
    pub async fn wait_command_available<
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) -> Result<(), oer_bluetooth_hci::LeControllerEndpointMismatch> {
        match &self.axes.order {
            LegacyAdvertisingOrder::Ready(order) => controller.wait_command_available(order).await,
            LegacyAdvertisingOrder::Detached => {
                Err(oer_bluetooth_hci::LeControllerEndpointMismatch)
            }
        }
    }

    fn detach_order(mut self) -> (Self, Order<'runtime>) {
        let order = core::mem::replace(&mut self.axes.order, LegacyAdvertisingOrder::Detached);
        match order {
            LegacyAdvertisingOrder::Ready(order) => (self, order),
            LegacyAdvertisingOrder::Detached => {
                unreachable!("an already-detached active owner cannot accept another command")
            }
        }
    }

    fn attach_order(mut self, order: Order<'runtime>) -> Self {
        match self.axes.order {
            LegacyAdvertisingOrder::Detached => {
                self.axes.order = LegacyAdvertisingOrder::Ready(order);
                self
            }
            LegacyAdvertisingOrder::Ready(_) => {
                unreachable!("an active owner cannot acquire a second command authority")
            }
        }
    }

    /// Consume and route at most one command without advancing the radio graph.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const HOST_TO_CONTROLLER_DEPTH: usize,
        const CONTROLLER_TO_HOST_DEPTH: usize,
        const PACKET_CAPACITY: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<
            'command,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        buffer: &'buffer mut [u8],
    ) -> LegacyAdvertisingActiveCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let (active, order) = self.detach_order();
        let ready = order.map_owner(|()| active);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route = match controller
                    .route_active_legacy_advertising_classified_command(command)
                {
                    HciActiveLegacyAdvertisingCommandRoute::ResponsePending(transaction) => {
                        LegacyAdvertisingActiveCommandRoute::ResponsePending(
                            LegacyAdvertisingActiveResponsePending { transaction },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                        let (active, deferred) = deferred.into_parts();
                        LegacyAdvertisingActiveCommandRoute::Stopping(LegacyAdvertisingStopping {
                            active,
                            order: LegacyAdvertisingStopOrder::Disable(deferred),
                        })
                    }
                    HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                        let (active, barrier) = barrier.into_parts();
                        LegacyAdvertisingActiveCommandRoute::Stopping(LegacyAdvertisingStopping {
                            active,
                            order: LegacyAdvertisingStopOrder::Reset(barrier),
                        })
                    }
                    HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                        LegacyAdvertisingActiveCommandRoute::EndpointMismatch(
                            LegacyAdvertisingActiveCommandMismatch { _command: command },
                        )
                    }
                };
                LegacyAdvertisingActiveCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (active, order) = ready.into_parts();
                LegacyAdvertisingActiveCommandIntake::Empty {
                    active: active.attach_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (active, order) = ready.into_parts();
                LegacyAdvertisingActiveCommandIntake::EndpointMismatch {
                    active: active.attach_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (active, order) = ready.into_parts();
                LegacyAdvertisingActiveCommandIntake::Channel {
                    active: active.attach_order(order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (active, order) = ready.into_parts();
                LegacyAdvertisingActiveCommandIntake::NonCommand {
                    active: active.attach_order(order),
                    frame,
                }
            }
        }
    }

    pub fn radio_wait(&self) -> Option<LegacyAdvertisingActiveWait<'_>> {
        let LegacyAdvertisingActivePhase::Completion(completion) = &self.phase else {
            return None;
        };
        match completion.wait_kind() {
            Some(SingleItemCompletionWaitKind::Scheduler) => Some(
                LegacyAdvertisingActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            Some(SingleItemCompletionWaitKind::PostUnlink) => Some(
                LegacyAdvertisingActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            None => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(self) -> LegacyAdvertisingActiveStep<'runtime, S, CAPACITY> {
        let Self { mut axes, phase } = self;
        match phase {
            LegacyAdvertisingActivePhase::Completion(completion) => {
                let step = completion.step(&mut axes.task);
                match step {
                    SingleItemCompletionStep::Continue(completion) => {
                        LegacyAdvertisingActiveStep::Continue(Self {
                            axes,
                            phase: LegacyAdvertisingActivePhase::Completion(completion),
                        })
                    }
                    SingleItemCompletionStep::Waiting(completion) => {
                        LegacyAdvertisingActiveStep::Waiting(Self {
                            axes,
                            phase: LegacyAdvertisingActivePhase::Completion(completion),
                        })
                    }
                    SingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => LegacyAdvertisingActiveStep::UnrelatedList {
                        session: Self {
                            axes,
                            phase: LegacyAdvertisingActivePhase::Completion(completion),
                        },
                        observed,
                    },
                    SingleItemCompletionStep::RemovalReady(ready) => {
                        LegacyAdvertisingActiveStep::Continue(Self {
                            axes,
                            phase: LegacyAdvertisingActivePhase::RemovalReady(ready),
                        })
                    }
                    SingleItemCompletionStep::Fault(fault) => {
                        let cause = legacy_advertising_fault_cause(fault.cause);
                        active_fault(
                            axes,
                            cause,
                            LegacyAdvertisingActiveFaultOwner::Completion(fault),
                        )
                    }
                }
            }
            LegacyAdvertisingActivePhase::RemovalReady(ready) => {
                let step = axes.task.recycle_legacy_advertising_completed(ready);
                match step {
                    LegacyAdvertisingSchedulerRecycleStep::Recycled(recycled) => {
                        LegacyAdvertisingActiveStep::CpuOwned(LegacyAdvertisingEventCpuOwned {
                            axes,
                            completed: recycled.complete_event(),
                        })
                    }
                    step @ LegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        LegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        LegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step
                    @ LegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive {
                        ..
                    } => active_fault(
                        axes,
                        LegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive,
                        LegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step @ LegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        LegacyAdvertisingActiveFaultCause::MemoryIdentityMismatch,
                        LegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step @ LegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch {
                        ..
                    } => active_fault(
                        axes,
                        LegacyAdvertisingActiveFaultCause::ReservationIdentityMismatch,
                        LegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                }
            }
        }
    }
}

fn active_fault<'runtime, S, const CAPACITY: usize>(
    axes: LegacyAdvertisingActiveAxes<'runtime, S, CAPACITY>,
    cause: LegacyAdvertisingActiveFaultCause,
    owner: LegacyAdvertisingActiveFaultOwner,
) -> LegacyAdvertisingActiveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    LegacyAdvertisingActiveStep::Fault(LegacyAdvertisingActiveFault {
        cause,
        _axes: axes,
        _owner: owner,
    })
}

fn legacy_advertising_fault_cause(
    cause: SingleItemCompletionFaultCause,
) -> LegacyAdvertisingActiveFaultCause {
    match cause {
        SingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            LegacyAdvertisingActiveFaultCause::FinishedListDrainAlreadyActive
        }
        SingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            LegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch
        }
        SingleItemCompletionFaultCause::FinishedListDrainLost => {
            LegacyAdvertisingActiveFaultCause::FinishedListDrainLost
        }
        SingleItemCompletionFaultCause::RepeatedRoleList => {
            LegacyAdvertisingActiveFaultCause::RepeatedAdvertisingList
        }
        SingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            LegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive
        }
        SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            LegacyAdvertisingActiveFaultCause::ExpectedHardwareHeadStillPublished
        }
        SingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            LegacyAdvertisingActiveFaultCause::UnexpectedHardwareHeadChanged
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkMailboxBusy
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkMailboxIdentityExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkMailboxGenerationExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkMailboxCommitMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkMailboxAffinityMismatch
        }
        SingleItemCompletionFaultCause::PrimaryInterruptFault => {
            LegacyAdvertisingActiveFaultCause::PrimaryInterruptFault
        }
        SingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkPendingRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkRecheckUnavailable
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            LegacyAdvertisingActiveFaultCause::PostUnlinkRecheckRearmMismatch
        }
    }
}
