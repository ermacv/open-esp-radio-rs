//! Active legacy-advertising composition after the first scheduler `RUN`.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciActiveLegacyAdvertisingCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerDeferredLegacyAdvertisingDisable, LeControllerResetBarrier,
    LeControllerResetCompletion, LeControllerResponsePending, LeControllerResponsePublication,
};
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingEventCompletionStatuses;

use crate::controller::boot::BluetoothSingleItemSchedulerCompletionFaultOwner;
use crate::legacy_advertising_completion::BluetoothLegacyAdvertisingCompletionRole;
use crate::scheduler::core::BluetoothLegacyAdvertisingSchedulerRecycleStep;
use crate::single_item_completion::{
    BluetoothSingleItemCompletion, BluetoothSingleItemCompletionFault,
    BluetoothSingleItemCompletionFaultCause, BluetoothSingleItemCompletionStep,
    BluetoothSingleItemCompletionWaitKind,
};
use crate::{
    BluetoothControllerIdleCommandTask, BluetoothControllerIdleResponsePending,
    BluetoothControllerIdleResponsePublication, BluetoothControllerPublishedTaskService,
    BluetoothDtmPostUnlinkWakeCell, BluetoothLegacyAdvertisingEventCompleted,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerWakeCell,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = open_esp_radio_bluetooth_hci::LeControllerCommandReady<'runtime, ()>;
type CompletionRole = BluetoothLegacyAdvertisingCompletionRole<'static>;
type SchedulerRunning = crate::scheduler::core::BluetoothSingleItemSchedulerRunning<CompletionRole>;
type RemovalReady =
    crate::scheduler::core::BluetoothSingleItemSchedulerSoftwareListRemovalReady<CompletionRole>;

enum BluetoothLegacyAdvertisingOrder<'runtime> {
    Ready(Order<'runtime>),
    Detached,
}

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
    running: SchedulerRunning,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        pending: BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
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
    order: BluetoothLegacyAdvertisingOrder<'runtime>,
    scheduler_item_address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum BluetoothLegacyAdvertisingActivePhase {
    Completion(BluetoothSingleItemCompletion<CompletionRole>),
    RemovalReady(RemovalReady),
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

/// One pending command response whose radio continuation remains independently active.
#[must_use = "publish the response while continuing the advertising radio graph"]
pub struct BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    >,
}

/// Response publication with the active radio owner preserved exactly once.
#[must_use = "retain the pending response or the returned command-ready session"]
pub enum BluetoothLegacyAdvertisingActiveResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One bounded radio transition while an HCI response remains pending.
#[must_use = "continue radio progress without losing the pending response"]
pub enum BluetoothLegacyAdvertisingActivePendingRadioStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Waiting(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    UnrelatedList {
        pending: BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingActivePendingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner preserving the exact pending response and radio fault.
#[must_use = "retain the failed radio and ordered response for shutdown diagnostics"]
pub struct BluetoothLegacyAdvertisingActivePendingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _fault: BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>,
    _response: LeControllerResponsePending<'runtime, ()>,
}

pub(crate) enum BluetoothLegacyAdvertisingStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Accepted active-role Disable or Reset driven through the current event completion.
#[must_use = "drive the current event to CPU ownership before restoring the runtime"]
pub struct BluetoothLegacyAdvertisingStopping<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    active: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    order: BluetoothLegacyAdvertisingStopOrder<'runtime>,
}

/// One bounded active advertising stop transition.
#[must_use = "retain the stop order through event completion and runtime restore"]
pub enum BluetoothLegacyAdvertisingStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Waiting(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    DisableRestore(BluetoothLegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
    ResetRestore(BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingStoppingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner retaining Disable/Reset order beside the exact lower fault.
#[must_use = "retain the failed stop transaction for shutdown diagnostics"]
pub struct BluetoothLegacyAdvertisingStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _fault: BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>,
    _order: BluetoothLegacyAdvertisingStopOrder<'runtime>,
}

/// Opaque owner for an impossible endpoint mismatch after active command intake.
#[must_use = "retain the complete command, radio continuation and order"]
pub struct BluetoothLegacyAdvertisingActiveCommandMismatch<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command accepted while an advertising event is in flight.
#[must_use = "publish, stop, or retain the exact mismatch owner"]
pub enum BluetoothLegacyAdvertisingActiveCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyAdvertisingActiveCommandMismatch<'runtime, 'command, S, CAPACITY>,
    ),
}

/// One non-blocking command intake while the current event remains hardware-owned.
#[must_use = "route a command or retain the exact active session"]
pub enum BluetoothLegacyAdvertisingActiveCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothLegacyAdvertisingActiveCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
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

struct BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    scheduler_item_address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
    completed: BluetoothLegacyAdvertisingEventCompleted<'static>,
}

/// One response retained at a CPU-owned boundary before the next event is scheduled.
#[must_use = "publish the response while retaining the complete advertising continuation"]
pub struct BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Result of publishing a command response at the between-event boundary.
#[must_use = "retain the pending response or the returned CPU-owned continuation"]
pub enum BluetoothLegacyAdvertisingCpuOwnedResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Accepted Disable after the hardware graph is CPU-owned but before runtime restore.
#[must_use = "restore the advertising graph before completing Disable"]
pub struct BluetoothLegacyAdvertisingDisableRestore<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    radio: BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    deferred: LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>,
}

/// Successful runtime restore with the exact Disable response still pending.
#[must_use = "publish Disable before returning the idle command owner"]
pub struct BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// One exact Disable runtime-restore attempt.
#[must_use = "publish the response or retain the unchanged restore owner"]
pub enum BluetoothLegacyAdvertisingDisableRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Rejected(BluetoothLegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
}

/// Result of publishing the exact successful Disable response.
#[must_use = "retain backpressure/fault or return the sole idle command owner"]
pub enum BluetoothLegacyAdvertisingDisableResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completed(BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Reset retained with the exact CPU-owned advertising graph.
#[must_use = "restore the graph and apply Reset only after quiescence"]
pub struct BluetoothLegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    barrier: LeControllerResetBarrier<
        'runtime,
        BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// CPU-owned advertising graph and opaque Reset order before runtime restore.
#[must_use = "restore the exact graph before applying Reset"]
pub struct BluetoothLegacyAdvertisingResetRestore<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    radio: BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    barrier: LeControllerResetBarrier<'runtime, ()>,
}

/// One advertising Reset graph-restore attempt.
#[must_use = "apply Reset or retain the unchanged restore owner"]
pub enum BluetoothLegacyAdvertisingResetRestoreStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    CompletionReady(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
    Rejected(BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY>),
}

/// Restored advertising runtime with Reset still undispatched.
#[must_use = "apply Reset once through the matching endpoint"]
pub struct BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    barrier: LeControllerResetBarrier<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of applying the retained Reset after advertising quiescence.
#[must_use = "publish Reset or retain the endpoint mismatch"]
pub enum BluetoothLegacyAdvertisingResetCompletion<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>),
}

/// Applied Reset response retaining the already-restored Controller task.
#[must_use = "publish Reset before returning the idle command owner"]
pub struct BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, Task<'runtime, S, CAPACITY>>,
}

/// Result of publishing Reset after advertising quiescence.
#[must_use = "retain backpressure/fault or return the sole idle command owner"]
pub enum BluetoothLegacyAdvertisingResetResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completed(BluetoothControllerIdleCommandTask<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Opaque owner for an impossible endpoint mismatch after CPU-boundary intake.
#[must_use = "retain the complete command, order and advertising continuation"]
pub struct BluetoothLegacyAdvertisingCpuOwnedCommandMismatch<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed command route at the safe boundary between advertising events.
#[must_use = "publish, disable, reset, or retain the exact mismatch owner"]
pub enum BluetoothLegacyAdvertisingCpuOwnedCommandRoute<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>),
    Disable(BluetoothLegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>),
    ResetBarrier(BluetoothLegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyAdvertisingCpuOwnedCommandMismatch<'runtime, 'command, S, CAPACITY>,
    ),
}

/// One non-blocking command intake while no advertising event is published.
#[must_use = "route a command or retain the exact CPU-owned continuation"]
pub enum BluetoothLegacyAdvertisingCpuOwnedCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothLegacyAdvertisingCpuOwnedCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        completed: BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        completed: BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        completed: BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        completed: BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
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
        match &self.axes.order {
            BluetoothLegacyAdvertisingOrder::Ready(order) => order.accepts_endpoint(controller),
            BluetoothLegacyAdvertisingOrder::Detached => false,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        Order<'runtime>,
        open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
        BluetoothSchedulerHardwareListIndex,
        BluetoothLegacyAdvertisingEventCompleted<'static>,
    ) {
        (
            self.axes.task,
            match self.axes.order {
                BluetoothLegacyAdvertisingOrder::Ready(order) => order,
                BluetoothLegacyAdvertisingOrder::Detached => {
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
        scheduler_item_address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
        completed: BluetoothLegacyAdvertisingEventCompleted<'static>,
    ) -> Self {
        Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                task,
                order: BluetoothLegacyAdvertisingOrder::Ready(order),
                scheduler_item_address,
                hardware_list_index,
            },
            completed,
        }
    }

    fn into_radio(
        self,
    ) -> (
        BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
        Order<'runtime>,
    ) {
        let Self { axes, completed } = self;
        (
            BluetoothLegacyAdvertisingCpuOwnedRadio {
                task: axes.task,
                scheduler_item_address: axes.scheduler_item_address,
                hardware_list_index: axes.hardware_list_index,
                completed,
            },
            match axes.order {
                BluetoothLegacyAdvertisingOrder::Ready(order) => order,
                BluetoothLegacyAdvertisingOrder::Detached => {
                    unreachable!("a detached CPU owner cannot accept another command")
                }
            },
        )
    }

    fn from_radio(
        radio: BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY>,
        order: Order<'runtime>,
    ) -> Self {
        Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                task: radio.task,
                order: BluetoothLegacyAdvertisingOrder::Ready(order),
                scheduler_item_address: radio.scheduler_item_address,
                hardware_list_index: radio.hardware_list_index,
            },
            completed: radio.completed,
        }
    }

    fn into_detached_radio(self) -> BluetoothLegacyAdvertisingCpuOwnedRadio<'runtime, S, CAPACITY> {
        let Self { axes, completed } = self;
        match axes.order {
            BluetoothLegacyAdvertisingOrder::Detached => BluetoothLegacyAdvertisingCpuOwnedRadio {
                task: axes.task,
                scheduler_item_address: axes.scheduler_item_address,
                hardware_list_index: axes.hardware_list_index,
                completed,
            },
            BluetoothLegacyAdvertisingOrder::Ready(_) => {
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
    ) -> BluetoothLegacyAdvertisingCpuOwnedCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY>
    {
        let (radio, order) = self.into_radio();
        let ready = order.map_owner(|()| radio);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route = match controller
                    .route_active_legacy_advertising_classified_command(command)
                {
                    HciActiveLegacyAdvertisingCommandRoute::ResponsePending(transaction) => {
                        BluetoothLegacyAdvertisingCpuOwnedCommandRoute::ResponsePending(
                            BluetoothLegacyAdvertisingCpuOwnedResponsePending { transaction },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                        let (radio, deferred) = deferred.into_parts();
                        BluetoothLegacyAdvertisingCpuOwnedCommandRoute::Disable(
                            BluetoothLegacyAdvertisingDisableRestore { radio, deferred },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                        BluetoothLegacyAdvertisingCpuOwnedCommandRoute::ResetBarrier(
                            BluetoothLegacyAdvertisingCpuOwnedResetBarrier { barrier },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                        BluetoothLegacyAdvertisingCpuOwnedCommandRoute::EndpointMismatch(
                            BluetoothLegacyAdvertisingCpuOwnedCommandMismatch { _command: command },
                        )
                    }
                };
                BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Empty {
                    completed: Self::from_radio(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothLegacyAdvertisingCpuOwnedCommandIntake::EndpointMismatch {
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
                BluetoothLegacyAdvertisingCpuOwnedCommandIntake::Channel {
                    completed: Self::from_radio(radio, order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (radio, order) = ready.into_parts();
                BluetoothLegacyAdvertisingCpuOwnedCommandIntake::NonCommand {
                    completed: Self::from_radio(radio, order),
                    frame,
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingCpuOwnedResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> BluetoothLegacyAdvertisingCpuOwnedResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Published(
                    BluetoothLegacyAdvertisingEventCpuOwned::from_radio(radio, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothLegacyAdvertisingCpuOwnedResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothLegacyAdvertisingCpuOwnedResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<BluetoothLegacyAdvertisingActiveWait<'_>> {
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> BluetoothLegacyAdvertisingActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (active, order) = ready.into_parts();
                BluetoothLegacyAdvertisingActiveResponsePublication::Published(
                    active.attach_order(order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothLegacyAdvertisingActiveResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothLegacyAdvertisingActiveResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothLegacyAdvertisingActiveResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }

    pub fn step_radio(
        self,
    ) -> BluetoothLegacyAdvertisingActivePendingRadioStep<'runtime, S, CAPACITY> {
        let (active, response) = self.transaction.into_parts();
        match active.step_radio() {
            BluetoothLegacyAdvertisingActiveStep::Continue(active) => {
                BluetoothLegacyAdvertisingActivePendingRadioStep::Continue(Self {
                    transaction: response.map_owner(|()| active),
                })
            }
            BluetoothLegacyAdvertisingActiveStep::Waiting(active) => {
                BluetoothLegacyAdvertisingActivePendingRadioStep::Waiting(Self {
                    transaction: response.map_owner(|()| active),
                })
            }
            BluetoothLegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                BluetoothLegacyAdvertisingActivePendingRadioStep::UnrelatedList {
                    pending: Self {
                        transaction: response.map_owner(|()| session),
                    },
                    observed,
                }
            }
            BluetoothLegacyAdvertisingActiveStep::CpuOwned(completed) => {
                BluetoothLegacyAdvertisingActivePendingRadioStep::CpuOwned(
                    BluetoothLegacyAdvertisingCpuOwnedResponsePending {
                        transaction: response.map_owner(|()| completed.into_detached_radio()),
                    },
                )
            }
            BluetoothLegacyAdvertisingActiveStep::Fault(fault) => {
                BluetoothLegacyAdvertisingActivePendingRadioStep::Fault(
                    BluetoothLegacyAdvertisingActivePendingFault {
                        _fault: fault,
                        _response: response,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<BluetoothLegacyAdvertisingActiveWait<'_>> {
        self.active.radio_wait()
    }

    pub fn step(self) -> BluetoothLegacyAdvertisingStoppingStep<'runtime, S, CAPACITY> {
        let Self { active, order } = self;
        match active.step_radio() {
            BluetoothLegacyAdvertisingActiveStep::Continue(active) => {
                BluetoothLegacyAdvertisingStoppingStep::Continue(Self { active, order })
            }
            BluetoothLegacyAdvertisingActiveStep::Waiting(active) => {
                BluetoothLegacyAdvertisingStoppingStep::Waiting(Self { active, order })
            }
            BluetoothLegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                BluetoothLegacyAdvertisingStoppingStep::UnrelatedList {
                    stopping: Self {
                        active: session,
                        order,
                    },
                    observed,
                }
            }
            BluetoothLegacyAdvertisingActiveStep::CpuOwned(completed) => {
                let radio = completed.into_detached_radio();
                match order {
                    BluetoothLegacyAdvertisingStopOrder::Disable(deferred) => {
                        BluetoothLegacyAdvertisingStoppingStep::DisableRestore(
                            BluetoothLegacyAdvertisingDisableRestore { radio, deferred },
                        )
                    }
                    BluetoothLegacyAdvertisingStopOrder::Reset(barrier) => {
                        BluetoothLegacyAdvertisingStoppingStep::ResetRestore(
                            BluetoothLegacyAdvertisingResetRestore { radio, barrier },
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingActiveStep::Fault(fault) => {
                BluetoothLegacyAdvertisingStoppingStep::Fault(
                    BluetoothLegacyAdvertisingStoppingFault {
                        _fault: fault,
                        _order: order,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingDisableRestore<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Restore the exact graph and only then construct successful Disable.
    pub fn restore(
        mut self,
    ) -> BluetoothLegacyAdvertisingDisableRestoreStep<'runtime, S, CAPACITY> {
        match self
            .radio
            .task
            .restore_legacy_advertising_completed_disabled(self.radio.completed)
        {
            Ok(()) => BluetoothLegacyAdvertisingDisableRestoreStep::ResponsePending(
                BluetoothLegacyAdvertisingDisableResponsePending {
                    transaction: self
                        .deferred
                        .map_owner(|()| self.radio.task)
                        .into_stopped_response(),
                },
            ),
            Err(completed) => {
                self.radio.completed = completed;
                BluetoothLegacyAdvertisingDisableRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> BluetoothLegacyAdvertisingDisableResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                BluetoothLegacyAdvertisingDisableResponsePublication::Completed(
                    BluetoothControllerIdleCommandTask::from_ready(ready),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothLegacyAdvertisingDisableResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothLegacyAdvertisingDisableResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothLegacyAdvertisingDisableResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingCpuOwnedResetBarrier<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn begin_restore(self) -> BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY> {
        let (radio, barrier) = self.barrier.into_parts();
        BluetoothLegacyAdvertisingResetRestore { radio, barrier }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingResetRestore<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn restore(mut self) -> BluetoothLegacyAdvertisingResetRestoreStep<'runtime, S, CAPACITY> {
        match self
            .radio
            .task
            .restore_legacy_advertising_completed_disabled(self.radio.completed)
        {
            Ok(()) => BluetoothLegacyAdvertisingResetRestoreStep::CompletionReady(
                BluetoothLegacyAdvertisingResetCompletionReady {
                    barrier: self.barrier.map_owner(|()| self.radio.task),
                },
            ),
            Err(completed) => {
                self.radio.completed = completed;
                BluetoothLegacyAdvertisingResetRestoreStep::Rejected(self)
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> BluetoothLegacyAdvertisingResetCompletion<'runtime, S, CAPACITY> {
        match controller.complete_reset_after_quiescence(self.barrier) {
            LeControllerResetCompletion::ResponsePending(transaction) => {
                BluetoothLegacyAdvertisingResetCompletion::ResponsePending(
                    BluetoothLegacyAdvertisingResetResponsePending { transaction },
                )
            }
            LeControllerResetCompletion::EndpointMismatch(barrier) => {
                BluetoothLegacyAdvertisingResetCompletion::EndpointMismatch(Self { barrier })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingResetResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
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
    ) -> BluetoothLegacyAdvertisingResetResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                BluetoothLegacyAdvertisingResetResponsePublication::Completed(
                    BluetoothControllerIdleCommandTask::from_ready(ready),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothLegacyAdvertisingResetResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothLegacyAdvertisingResetResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothLegacyAdvertisingResetResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
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
    Completion(
        BluetoothSingleItemCompletionFault<
            BluetoothSingleItemSchedulerCompletionFaultOwner<CompletionRole>,
        >,
    ),
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
        running: SchedulerRunning,
    ) -> Self {
        let scheduler_item_address = running.scheduler_item_address();
        let hardware_list_index = running.hardware_list_index();
        let (task, order) = task.into_parts();
        Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                task,
                order: BluetoothLegacyAdvertisingOrder::Ready(order),
                scheduler_item_address,
                hardware_list_index,
            },
            phase: BluetoothLegacyAdvertisingActivePhase::Completion(
                BluetoothSingleItemCompletion::new(running),
            ),
        }
    }

    pub(crate) fn from_recurring_running(
        task: Task<'runtime, S, CAPACITY>,
        order: Order<'runtime>,
        running: SchedulerRunning,
    ) -> Self {
        Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: BluetoothLegacyAdvertisingOrder::Ready(order),
            },
            phase: BluetoothLegacyAdvertisingActivePhase::Completion(
                BluetoothSingleItemCompletion::new(running),
            ),
        }
    }

    pub(crate) fn from_recurring_response_pending(
        task: Task<'runtime, S, CAPACITY>,
        response: LeControllerResponsePending<'runtime, ()>,
        running: SchedulerRunning,
    ) -> BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY> {
        let active = Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: BluetoothLegacyAdvertisingOrder::Detached,
            },
            phase: BluetoothLegacyAdvertisingActivePhase::Completion(
                BluetoothSingleItemCompletion::new(running),
            ),
        };
        BluetoothLegacyAdvertisingActiveResponsePending {
            transaction: response.map_owner(|()| active),
        }
    }

    pub(crate) fn from_recurring_stopping(
        task: Task<'runtime, S, CAPACITY>,
        order: BluetoothLegacyAdvertisingStopOrder<'runtime>,
        running: SchedulerRunning,
    ) -> BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY> {
        let active = Self {
            axes: BluetoothLegacyAdvertisingActiveAxes {
                scheduler_item_address: running.scheduler_item_address(),
                hardware_list_index: running.hardware_list_index(),
                task,
                order: BluetoothLegacyAdvertisingOrder::Detached,
            },
            phase: BluetoothLegacyAdvertisingActivePhase::Completion(
                BluetoothSingleItemCompletion::new(running),
            ),
        };
        BluetoothLegacyAdvertisingStopping { active, order }
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
        match &self.axes.order {
            BluetoothLegacyAdvertisingOrder::Ready(order) => order.accepts_endpoint(controller),
            BluetoothLegacyAdvertisingOrder::Detached => false,
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
    ) -> Result<(), open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch> {
        match &self.axes.order {
            BluetoothLegacyAdvertisingOrder::Ready(order) => {
                controller.wait_command_available(order).await
            }
            BluetoothLegacyAdvertisingOrder::Detached => {
                Err(open_esp_radio_bluetooth_hci::LeControllerEndpointMismatch)
            }
        }
    }

    fn detach_order(mut self) -> (Self, Order<'runtime>) {
        let order = core::mem::replace(
            &mut self.axes.order,
            BluetoothLegacyAdvertisingOrder::Detached,
        );
        match order {
            BluetoothLegacyAdvertisingOrder::Ready(order) => (self, order),
            BluetoothLegacyAdvertisingOrder::Detached => {
                unreachable!("an already-detached active owner cannot accept another command")
            }
        }
    }

    fn attach_order(mut self, order: Order<'runtime>) -> Self {
        match self.axes.order {
            BluetoothLegacyAdvertisingOrder::Detached => {
                self.axes.order = BluetoothLegacyAdvertisingOrder::Ready(order);
                self
            }
            BluetoothLegacyAdvertisingOrder::Ready(_) => {
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
    ) -> BluetoothLegacyAdvertisingActiveCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY>
    {
        let (active, order) = self.detach_order();
        let ready = order.map_owner(|()| active);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route = match controller
                    .route_active_legacy_advertising_classified_command(command)
                {
                    HciActiveLegacyAdvertisingCommandRoute::ResponsePending(transaction) => {
                        BluetoothLegacyAdvertisingActiveCommandRoute::ResponsePending(
                            BluetoothLegacyAdvertisingActiveResponsePending { transaction },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::Disable(deferred) => {
                        let (active, deferred) = deferred.into_parts();
                        BluetoothLegacyAdvertisingActiveCommandRoute::Stopping(
                            BluetoothLegacyAdvertisingStopping {
                                active,
                                order: BluetoothLegacyAdvertisingStopOrder::Disable(deferred),
                            },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::ResetBarrier(barrier) => {
                        let (active, barrier) = barrier.into_parts();
                        BluetoothLegacyAdvertisingActiveCommandRoute::Stopping(
                            BluetoothLegacyAdvertisingStopping {
                                active,
                                order: BluetoothLegacyAdvertisingStopOrder::Reset(barrier),
                            },
                        )
                    }
                    HciActiveLegacyAdvertisingCommandRoute::EndpointMismatch(command) => {
                        BluetoothLegacyAdvertisingActiveCommandRoute::EndpointMismatch(
                            BluetoothLegacyAdvertisingActiveCommandMismatch { _command: command },
                        )
                    }
                };
                BluetoothLegacyAdvertisingActiveCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (active, order) = ready.into_parts();
                BluetoothLegacyAdvertisingActiveCommandIntake::Empty {
                    active: active.attach_order(order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (active, order) = ready.into_parts();
                BluetoothLegacyAdvertisingActiveCommandIntake::EndpointMismatch {
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
                BluetoothLegacyAdvertisingActiveCommandIntake::Channel {
                    active: active.attach_order(order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (active, order) = ready.into_parts();
                BluetoothLegacyAdvertisingActiveCommandIntake::NonCommand {
                    active: active.attach_order(order),
                    frame,
                }
            }
        }
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyAdvertisingActiveWait<'_>> {
        let BluetoothLegacyAdvertisingActivePhase::Completion(completion) = &self.phase else {
            return None;
        };
        match completion.wait_kind() {
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler) => Some(
                BluetoothLegacyAdvertisingActiveWait::Scheduler(self.axes.task.scheduler_wake()),
            ),
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink) => Some(
                BluetoothLegacyAdvertisingActiveWait::PostUnlink(self.axes.task.post_unlink_wake()),
            ),
            None => None,
        }
    }

    /// Advance one wake, completion, drain, unlink, mailbox or recycle edge.
    pub fn step_radio(self) -> BluetoothLegacyAdvertisingActiveStep<'runtime, S, CAPACITY> {
        let Self { mut axes, phase } = self;
        match phase {
            BluetoothLegacyAdvertisingActivePhase::Completion(completion) => {
                let step = completion.step(&mut axes.task);
                match step {
                    BluetoothSingleItemCompletionStep::Continue(completion) => {
                        BluetoothLegacyAdvertisingActiveStep::Continue(Self {
                            axes,
                            phase: BluetoothLegacyAdvertisingActivePhase::Completion(completion),
                        })
                    }
                    BluetoothSingleItemCompletionStep::Waiting(completion) => {
                        BluetoothLegacyAdvertisingActiveStep::Waiting(Self {
                            axes,
                            phase: BluetoothLegacyAdvertisingActivePhase::Completion(completion),
                        })
                    }
                    BluetoothSingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => BluetoothLegacyAdvertisingActiveStep::UnrelatedList {
                        session: Self {
                            axes,
                            phase: BluetoothLegacyAdvertisingActivePhase::Completion(completion),
                        },
                        observed,
                    },
                    BluetoothSingleItemCompletionStep::RemovalReady(ready) => {
                        BluetoothLegacyAdvertisingActiveStep::Continue(Self {
                            axes,
                            phase: BluetoothLegacyAdvertisingActivePhase::RemovalReady(ready),
                        })
                    }
                    BluetoothSingleItemCompletionStep::Fault(fault) => {
                        let cause = legacy_advertising_fault_cause(fault.cause);
                        active_fault(
                            axes,
                            cause,
                            BluetoothLegacyAdvertisingActiveFaultOwner::Completion(fault),
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingActivePhase::RemovalReady(ready) => {
                let step = axes.task.recycle_legacy_advertising_completed(ready);
                match step {
                    BluetoothLegacyAdvertisingSchedulerRecycleStep::Recycled(recycled) => {
                        BluetoothLegacyAdvertisingActiveStep::CpuOwned(
                            BluetoothLegacyAdvertisingEventCpuOwned {
                                axes,
                                completed: recycled.complete_event(),
                            },
                        )
                    }
                    step @ BluetoothLegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch { .. } => active_fault(
                        axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothLegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive { .. } => active_fault(
                        axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothLegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch { .. } => active_fault(
                        axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::MemoryIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(step),
                    ),
                    step @ BluetoothLegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch { .. } => active_fault(
                        axes,
                        BluetoothLegacyAdvertisingActiveFaultCause::ReservationIdentityMismatch,
                        BluetoothLegacyAdvertisingActiveFaultOwner::Recycle(step),
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

fn legacy_advertising_fault_cause(
    cause: BluetoothSingleItemCompletionFaultCause,
) -> BluetoothLegacyAdvertisingActiveFaultCause {
    match cause {
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainAlreadyActive
        }
        BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::SchedulerIdentityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost => {
            BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainLost
        }
        BluetoothSingleItemCompletionFaultCause::RepeatedRoleList => {
            BluetoothLegacyAdvertisingActiveFaultCause::RepeatedAdvertisingList
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            BluetoothLegacyAdvertisingActiveFaultCause::FinishedListDrainStillActive
        }
        BluetoothSingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            BluetoothLegacyAdvertisingActiveFaultCause::ExpectedHardwareHeadStillPublished
        }
        BluetoothSingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            BluetoothLegacyAdvertisingActiveFaultCause::UnexpectedHardwareHeadChanged
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxBusy
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxIdentityExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxGenerationExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxCommitMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkMailboxAffinityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PrimaryInterruptFault => {
            BluetoothLegacyAdvertisingActiveFaultCause::PrimaryInterruptFault
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkPendingRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkRecheckUnavailable
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            BluetoothLegacyAdvertisingActiveFaultCause::PostUnlinkRecheckRearmMismatch
        }
    }
}
