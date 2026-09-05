//! HCI order composition for the ESP32-S31 passive scanner.
//!
//! The lower scanner runner owns LL state and hardware. This module only keeps
//! the accepted standard Enable command affine beside that runner until `RUN`,
//! then transfers the returned command authority into the active scan session.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyScanningCommandRoute as HciActiveLegacyScanningCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerDeferredLegacyScanningDisable,
    LeControllerDeferredLegacyScanningStart, LeControllerEndpointMismatch,
    LeControllerResetBarrier, LeControllerResponsePending, LeControllerResponsePublication,
    LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError,
    LeLegacyAdvertisingReportPublication, LeLegacyScanningDuplicatePolicy,
};
use open_esp_radio_bluetooth_ll::scanning::{
    LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReport, LegacyAdvertisingReportKind,
    LegacyAdvertisingReportParseError, LegacyPassiveScanParameters, LegacyPassiveScannerDisabled,
    LegacyScanDuplicatePolicy, LegacyScanInterval, LegacyScanTimingError, LegacyScanWindow,
};

use crate::{
    BluetoothControllerIdleResetBarrier, BluetoothControllerIdleResponsePending,
    BluetoothControllerPublishedTaskService, BluetoothPassiveScanActiveFault,
    BluetoothPassiveScanActiveSession, BluetoothPassiveScanActiveStep,
    BluetoothPassiveScanActiveWait, BluetoothPassiveScanEventCpuOwned,
    BluetoothPassiveScanFirstRunner, BluetoothPassiveScanFirstRunnerFailure,
    BluetoothPassiveScanFirstRunnerRetryCause, BluetoothPassiveScanFirstRunnerStep,
    BluetoothSchedulerRunInterruptStorage,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

const LEGACY_SCAN_DUPLICATE_FILTER_CAPACITY: usize = 32;
type DuplicateFilter = LegacyAdvertisingDuplicateFilter<LEGACY_SCAN_DUPLICATE_FILTER_CAPACITY>;

/// HCI Enable retained beside the lower first-window runner.
#[must_use = "step the scanner until RUN or retain its exact failure"]
pub struct BluetoothPassiveScanHciFirstRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    runner: BluetoothPassiveScanFirstRunner<'runtime, S, CAPACITY>,
}

/// One finite HCI-composed first-window transition.
#[must_use = "retain a wait, running owner, or exact failure"]
pub enum BluetoothPassiveScanHciFirstRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>),
    Continue(BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>),
    Running(BluetoothPassiveScanHciFirstRunning<'runtime, S, CAPACITY>),
    Failed(BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
}

/// Accepted Enable and the exact scanner graph after scheduler `RUN`.
#[must_use = "publish Enable success while retaining the running scanner"]
pub struct BluetoothPassiveScanHciFirstRunning<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    running: crate::BluetoothPassiveScanFirstRunning<'runtime, S, CAPACITY>,
}

/// A failed semantic conversion or exact lower first-window owner.
#[must_use = "retry the lower edge or recover ordered hardware-failure response"]
pub enum BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Parameters {
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
        task: Task<'runtime, S, CAPACITY>,
        error: LegacyScanTimingError,
    },
    Lower {
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
        failure: BluetoothPassiveScanFirstRunnerFailure<'runtime, S, CAPACITY>,
    },
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Retry only a lower pre-`RUN` publication/start edge.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn retry(self) -> Result<BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>, Self> {
        match self {
            Self::Lower {
                command,
                failure: BluetoothPassiveScanFirstRunnerFailure::Retryable(retry),
            } => Ok(BluetoothPassiveScanHciFirstRunner {
                command,
                runner: retry.retry(),
            }),
            failure => Err(failure),
        }
    }

    /// Inspect a retryable lower cause without separating its owner.
    pub fn retry_cause(&self) -> Option<&BluetoothPassiveScanFirstRunnerRetryCause<S::Error>> {
        match self {
            Self::Lower {
                failure: BluetoothPassiveScanFirstRunnerFailure::Retryable(retry),
                ..
            } => Some(retry.cause()),
            _ => None,
        }
    }

    /// Convert a failure with recovered idle ownership into ordered Hardware Failure.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn into_hardware_failure_response(
        self,
    ) -> Result<BluetoothControllerIdleResponsePending<'runtime, S, CAPACITY>, Self> {
        let (command, task) = match self {
            Self::Parameters { command, task, .. } => (command, task),
            Self::Lower { command, failure } => {
                let task = match failure {
                    BluetoothPassiveScanFirstRunnerFailure::ColdBegin { failure, .. } => {
                        failure.into_parts().0
                    }
                    BluetoothPassiveScanFirstRunnerFailure::ColdRecheck { failure, .. } => {
                        failure.into_parts().0
                    }
                    BluetoothPassiveScanFirstRunnerFailure::WarmBegin { failure, .. } => {
                        failure.into_parts().0.into_task_service()
                    }
                    BluetoothPassiveScanFirstRunnerFailure::WarmRecheck { failure, .. } => {
                        failure.into_parts().0.into_task_service()
                    }
                    BluetoothPassiveScanFirstRunnerFailure::Recovered { task, .. } => task,
                    failure @ (BluetoothPassiveScanFirstRunnerFailure::PreparationFailStop {
                        ..
                    }
                    | BluetoothPassiveScanFirstRunnerFailure::PublicationFailStop(_)
                    | BluetoothPassiveScanFirstRunnerFailure::Retryable(_)) => {
                        return Err(Self::Lower { command, failure });
                    }
                };
                (command, task)
            }
        };
        Ok(BluetoothControllerIdleResponsePending::new(
            command
                .map_owner(|()| task)
                .into_hardware_failure_response(),
        ))
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains HCI order and the exact hardware owner"
    )]
    pub(crate) fn begin(
        task: Task<'runtime, S, CAPACITY>,
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    ) -> Result<Self, BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>> {
        let request = command.request();
        let interval = match LegacyScanInterval::new(request.parameters().interval_units_625_us()) {
            Ok(interval) => interval,
            Err(error) => {
                return Err(BluetoothPassiveScanHciFirstRunnerFailure::Parameters {
                    command,
                    task,
                    error,
                });
            }
        };
        let window = match LegacyScanWindow::new(request.parameters().window_units_625_us()) {
            Ok(window) => window,
            Err(error) => {
                return Err(BluetoothPassiveScanHciFirstRunnerFailure::Parameters {
                    command,
                    task,
                    error,
                });
            }
        };
        let parameters = match LegacyPassiveScanParameters::new(interval, window) {
            Ok(parameters) => parameters,
            Err(error) => {
                return Err(BluetoothPassiveScanHciFirstRunnerFailure::Parameters {
                    command,
                    task,
                    error,
                });
            }
        };
        let duplicate_policy = match request.duplicate_policy() {
            LeLegacyScanningDuplicatePolicy::ReportAll => LegacyScanDuplicatePolicy::ReportAll,
            LeLegacyScanningDuplicatePolicy::FilterDuplicates => {
                LegacyScanDuplicatePolicy::FilterDuplicates
            }
        };
        let scanner = LegacyPassiveScannerDisabled::new(parameters).enable(duplicate_policy);
        match BluetoothPassiveScanFirstRunner::begin(task, scanner) {
            Ok(runner) => Ok(Self { command, runner }),
            Err(failure) => {
                Err(BluetoothPassiveScanHciFirstRunnerFailure::Lower { command, failure })
            }
        }
    }

    /// Execute exactly one lower transition without releasing HCI order.
    pub fn step(self) -> BluetoothPassiveScanHciFirstRunnerStep<'runtime, S, CAPACITY> {
        let Self { command, runner } = self;
        match runner.step() {
            BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(runner) => {
                BluetoothPassiveScanHciFirstRunnerStep::WaitControllerTime(Self { command, runner })
            }
            BluetoothPassiveScanFirstRunnerStep::Continue(runner) => {
                BluetoothPassiveScanHciFirstRunnerStep::Continue(Self { command, runner })
            }
            BluetoothPassiveScanFirstRunnerStep::Running(running) => {
                BluetoothPassiveScanHciFirstRunnerStep::Running(
                    BluetoothPassiveScanHciFirstRunning { command, running },
                )
            }
            BluetoothPassiveScanFirstRunnerStep::Failed(failure) => {
                BluetoothPassiveScanHciFirstRunnerStep::Failed(
                    BluetoothPassiveScanHciFirstRunnerFailure::Lower { command, failure },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanHciFirstRunning<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Pair the exact success response with the already-running scanner.
    pub fn into_response_pending_session(
        self,
    ) -> BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY> {
        let active = BluetoothPassiveScanActiveSession::from_first_running(self.running);
        BluetoothPassiveScanHciResponsePendingSession {
            transaction: self.command.map_owner(|()| active).into_started_response(),
        }
    }
}

/// Accepted Enable response paired with the exact already-running scanner.
#[must_use = "publish the response while retaining the active scanner"]
pub struct BluetoothPassiveScanHciResponsePendingSession<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
    >,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothPassiveScanHciResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (active, order) = ready.into_parts();
                BluetoothPassiveScanHciResponsePublication::Published(
                    BluetoothPassiveScanHciActiveSession {
                        active,
                        order,
                        duplicate_filter: DuplicateFilter::new(),
                    },
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothPassiveScanHciResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothPassiveScanHciResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault { pending, error } => {
                BluetoothPassiveScanHciResponsePublication::Fault {
                    pending: Self {
                        transaction: pending,
                    },
                    error,
                }
            }
        }
    }
}

/// Result of publishing scanner Enable success.
#[must_use = "retain the active scanner or unchanged response transaction"]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub enum BluetoothPassiveScanHciResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Pending(BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothPassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Active lower scanner paired with the sole next-command authority.
#[must_use = "drive radio progress and preserve HCI command authority"]
pub struct BluetoothPassiveScanHciActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    active: BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

/// One bounded active scanner transition with HCI order retained.
#[must_use = "retain radio progress, the completed report batch, or the fail-stop owner"]
pub enum BluetoothPassiveScanHciActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Waiting(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: crate::BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Fault(BluetoothPassiveScanHciActiveFault<'runtime, S, CAPACITY>),
}

struct BluetoothPassiveScanHciActiveRadio<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    active: BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
    duplicate_filter: DuplicateFilter,
}

/// One command response whose scanner window remains hardware-owned.
#[must_use = "publish the response while continuing scanner reclamation"]
pub struct BluetoothPassiveScanHciActiveResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothPassiveScanHciActiveRadio<'runtime, S, CAPACITY>,
    >,
}

/// Response publication with the active scanner returned exactly once.
#[must_use = "retain the response transaction or returned command-ready scanner"]
pub enum BluetoothPassiveScanHciActiveResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Pending(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One radio transition while an accepted command response is backpressured.
#[must_use = "continue scanner reclamation without losing the response"]
pub enum BluetoothPassiveScanHciActivePendingRadioStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Waiting(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    UnrelatedList {
        pending: BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        observed: crate::BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Fault(BluetoothPassiveScanHciActivePendingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner preserving the radio fault and pending response.
#[must_use = "retain the exact failed scanner and ordered response"]
pub struct BluetoothPassiveScanHciActivePendingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _fault: BluetoothPassiveScanActiveFault<'runtime, S, CAPACITY>,
    _response: LeControllerResponsePending<'runtime, ()>,
    _duplicate_filter: DuplicateFilter,
}

enum BluetoothPassiveScanHciStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyScanningDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Accepted Disable or Reset retained until the current window is quiescent.
#[must_use = "drive the current scanner window to CPU ownership"]
pub struct BluetoothPassiveScanHciStopping<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    radio: BluetoothPassiveScanHciActiveRadio<'runtime, S, CAPACITY>,
    order: BluetoothPassiveScanHciStopOrder<'runtime>,
}

/// One bounded transition toward a quiescent scanner.
#[must_use = "retain stop order until scanner ownership is fully reclaimed"]
#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc stop path retains the complete affine scanner fault"
)]
pub enum BluetoothPassiveScanHciStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>),
    Waiting(BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>,
        observed: crate::BluetoothSchedulerFinishedHardwareListObserved,
    },
    Disable(BluetoothControllerIdleResponsePending<'runtime, S, CAPACITY>),
    Reset(BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    Fault(BluetoothPassiveScanHciStoppingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner retaining the scanner fault and accepted stop command.
#[must_use = "retain the exact failed stop transaction for diagnostics"]
pub struct BluetoothPassiveScanHciStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _fault: BluetoothPassiveScanActiveFault<'runtime, S, CAPACITY>,
    _order: BluetoothPassiveScanHciStopOrder<'runtime>,
    _duplicate_filter: DuplicateFilter,
}

/// Opaque owner for an impossible endpoint mismatch during an active window.
#[must_use = "retain the command and complete active scanner owner"]
pub struct BluetoothPassiveScanHciActiveCommandMismatch<
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
        BluetoothPassiveScanHciActiveRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command consumed while one scan window is in flight.
#[must_use = "publish, quiesce, or retain the exact mismatch owner"]
pub enum BluetoothPassiveScanHciActiveCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothPassiveScanHciActiveCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking HCI intake while the scanner graph remains hardware-owned.
#[must_use = "route a command or retain the exact active session"]
pub enum BluetoothPassiveScanHciActiveCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothPassiveScanHciActiveCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Fail-stop scanner owner retaining HCI order and duplicate-filter history.
#[must_use = "retain the exact failed scanner owner for diagnostic shutdown"]
pub struct BluetoothPassiveScanHciActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _fault: BluetoothPassiveScanActiveFault<'runtime, S, CAPACITY>,
    _order: LeControllerCommandReady<'runtime, ()>,
    _duplicate_filter: DuplicateFilter,
}

/// CPU-owned receive batch being converted and published as standard HCI events.
#[must_use = "publish or explicitly retain every report before starting the next scan window"]
pub struct BluetoothPassiveScanHciReportsPending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completed: BluetoothPassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
    next_report: usize,
    pending_event: Option<LeLegacyAdvertisingReportEvent>,
}

/// Result of one bounded report parsing/filtering/publication transition.
#[must_use = "retain pending reports, the completed batch, or an exact diagnostic"]
pub enum BluetoothPassiveScanHciReportStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Masked(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Pending {
        reports: BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
    IgnoredMalformed {
        reports: BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: LegacyAdvertisingReportParseError,
    },
    EncodingFault {
        reports: BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: LeLegacyAdvertisingReportEventError,
    },
    EndpointMismatch(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Complete(BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
}

/// CPU-owned scanner after every report from one receive batch was handled.
#[must_use = "start the next interval-preserving window or stop the scanner"]
pub struct BluetoothPassiveScanHciReportsComplete<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completed: BluetoothPassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

struct BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completed: BluetoothPassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    duplicate_filter: DuplicateFilter,
}

/// One ordinary command response retained at the safe between-window boundary.
#[must_use = "publish the response before beginning the next scan window"]
pub struct BluetoothPassiveScanHciCpuResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Publication result for a response between passive scan windows.
#[must_use = "retain response backpressure or the returned complete scanner"]
pub enum BluetoothPassiveScanHciCpuResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
    Pending(BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Opaque owner for an impossible classified-command endpoint mismatch.
#[must_use = "retain the command, completed scanner and exact HCI order"]
pub struct BluetoothPassiveScanHciCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed command route between fully reclaimed passive scan windows.
#[must_use = "publish, disable, reset, or retain the exact mismatch owner"]
pub enum BluetoothPassiveScanHciCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Disable(BluetoothControllerIdleResponsePending<'runtime, S, CAPACITY>),
    Reset(BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    EndpointMismatch(BluetoothPassiveScanHciCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking HCI intake at the safe between-window boundary.
#[must_use = "route a command or retain the exact complete scanner"]
pub enum BluetoothPassiveScanHciCommandIntake<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothPassiveScanHciCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        completed: BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        completed: BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        completed: BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        completed: BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// HCI order and duplicate history retained through recurring-window preparation.
#[must_use = "step the recurring scanner until RUN or retain its exact failure"]
pub struct BluetoothPassiveScanHciRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothPassiveScanFirstRunner<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

/// One finite recurring-window transition.
#[must_use = "retain a wait, running owner, or exact failure"]
pub enum BluetoothPassiveScanHciRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Continue(BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Running(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Failed(BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
}

/// Exact lower recurring-start failure plus all session-wide HCI policy.
#[must_use = "retry the retained lower edge or keep the complete fail-stop owner"]
pub struct BluetoothPassiveScanHciRecurringFailure<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    failure: BluetoothPassiveScanFirstRunnerFailure<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<BluetoothPassiveScanActiveWait<'_>> {
        self.active.radio_wait()
    }

    /// Wait for Host command readiness while borrowing the active scanner.
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_command_available(&self.order).await
    }

    /// Consume and route at most one command without advancing the radio graph.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
    ) -> BluetoothPassiveScanHciActiveCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let Self {
            active,
            order,
            duplicate_filter,
        } = self;
        let ready = order.map_owner(|()| BluetoothPassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        });
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_scanning_classified_command(command) {
                        HciActiveLegacyScanningCommandRoute::ResponsePending(transaction) => {
                            BluetoothPassiveScanHciActiveCommandRoute::ResponsePending(
                                BluetoothPassiveScanHciActiveResponsePending { transaction },
                            )
                        }
                        HciActiveLegacyScanningCommandRoute::Disable(deferred) => {
                            let (radio, deferred) = deferred.into_parts();
                            BluetoothPassiveScanHciActiveCommandRoute::Stopping(
                                BluetoothPassiveScanHciStopping {
                                    radio,
                                    order: BluetoothPassiveScanHciStopOrder::Disable(deferred),
                                },
                            )
                        }
                        HciActiveLegacyScanningCommandRoute::ResetBarrier(barrier) => {
                            let (radio, barrier) = barrier.into_parts();
                            BluetoothPassiveScanHciActiveCommandRoute::Stopping(
                                BluetoothPassiveScanHciStopping {
                                    radio,
                                    order: BluetoothPassiveScanHciStopOrder::Reset(barrier),
                                },
                            )
                        }
                        HciActiveLegacyScanningCommandRoute::EndpointMismatch(command) => {
                            BluetoothPassiveScanHciActiveCommandRoute::EndpointMismatch(
                                BluetoothPassiveScanHciActiveCommandMismatch { _command: command },
                            )
                        }
                    };
                BluetoothPassiveScanHciActiveCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciActiveCommandIntake::Empty {
                    active: Self {
                        active: radio.active,
                        order,
                        duplicate_filter: radio.duplicate_filter,
                    },
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciActiveCommandIntake::EndpointMismatch {
                    active: Self {
                        active: radio.active,
                        order,
                        duplicate_filter: radio.duplicate_filter,
                    },
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciActiveCommandIntake::Channel {
                    active: Self {
                        active: radio.active,
                        order,
                        duplicate_filter: radio.duplicate_filter,
                    },
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciActiveCommandIntake::NonCommand {
                    active: Self {
                        active: radio.active,
                        order,
                        duplicate_filter: radio.duplicate_filter,
                    },
                    frame,
                }
            }
        }
    }

    /// Advance the lower radio graph while preserving HCI order and scan policy.
    pub fn step_radio(self) -> BluetoothPassiveScanHciActiveStep<'runtime, S, CAPACITY> {
        let Self {
            active,
            order,
            duplicate_filter,
        } = self;
        match active.step_radio() {
            BluetoothPassiveScanActiveStep::Continue(active) => {
                BluetoothPassiveScanHciActiveStep::Continue(Self {
                    active,
                    order,
                    duplicate_filter,
                })
            }
            BluetoothPassiveScanActiveStep::Waiting(active) => {
                BluetoothPassiveScanHciActiveStep::Waiting(Self {
                    active,
                    order,
                    duplicate_filter,
                })
            }
            BluetoothPassiveScanActiveStep::UnrelatedList { session, observed } => {
                BluetoothPassiveScanHciActiveStep::UnrelatedList {
                    session: Self {
                        active: session,
                        order,
                        duplicate_filter,
                    },
                    observed,
                }
            }
            BluetoothPassiveScanActiveStep::CpuOwned(completed) => {
                BluetoothPassiveScanHciActiveStep::CpuOwned(BluetoothPassiveScanHciReportsPending {
                    completed,
                    order,
                    duplicate_filter,
                    next_report: 0,
                    pending_event: None,
                })
            }
            BluetoothPassiveScanActiveStep::Fault(fault) => {
                BluetoothPassiveScanHciActiveStep::Fault(BluetoothPassiveScanHciActiveFault {
                    _fault: fault,
                    _order: order,
                    _duplicate_filter: duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<BluetoothPassiveScanActiveWait<'_>> {
        self.transaction.owner().active.radio_wait()
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothPassiveScanHciActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciActiveResponsePublication::Published(
                    BluetoothPassiveScanHciActiveSession {
                        active: radio.active,
                        order,
                        duplicate_filter: radio.duplicate_filter,
                    },
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothPassiveScanHciActiveResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothPassiveScanHciActiveResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothPassiveScanHciActiveResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }

    pub fn step_radio(
        self,
    ) -> BluetoothPassiveScanHciActivePendingRadioStep<'runtime, S, CAPACITY> {
        let (radio, response) = self.transaction.into_parts();
        let BluetoothPassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        } = radio;
        match active.step_radio() {
            BluetoothPassiveScanActiveStep::Continue(active) => {
                BluetoothPassiveScanHciActivePendingRadioStep::Continue(Self {
                    transaction: response.map_owner(|()| BluetoothPassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    }),
                })
            }
            BluetoothPassiveScanActiveStep::Waiting(active) => {
                BluetoothPassiveScanHciActivePendingRadioStep::Waiting(Self {
                    transaction: response.map_owner(|()| BluetoothPassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    }),
                })
            }
            BluetoothPassiveScanActiveStep::UnrelatedList { session, observed } => {
                BluetoothPassiveScanHciActivePendingRadioStep::UnrelatedList {
                    pending: Self {
                        transaction: response.map_owner(|()| BluetoothPassiveScanHciActiveRadio {
                            active: session,
                            duplicate_filter,
                        }),
                    },
                    observed,
                }
            }
            BluetoothPassiveScanActiveStep::CpuOwned(completed) => {
                BluetoothPassiveScanHciActivePendingRadioStep::CpuOwned(
                    BluetoothPassiveScanHciCpuResponsePending {
                        transaction: response.map_owner(|()| {
                            BluetoothPassiveScanHciCpuOwnedRadio {
                                completed,
                                duplicate_filter,
                            }
                        }),
                    },
                )
            }
            BluetoothPassiveScanActiveStep::Fault(fault) => {
                BluetoothPassiveScanHciActivePendingRadioStep::Fault(
                    BluetoothPassiveScanHciActivePendingFault {
                        _fault: fault,
                        _response: response,
                        _duplicate_filter: duplicate_filter,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanHciStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<BluetoothPassiveScanActiveWait<'_>> {
        self.radio.active.radio_wait()
    }

    pub fn step(self) -> BluetoothPassiveScanHciStoppingStep<'runtime, S, CAPACITY> {
        let Self { radio, order } = self;
        let BluetoothPassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        } = radio;
        match active.step_radio() {
            BluetoothPassiveScanActiveStep::Continue(active) => {
                BluetoothPassiveScanHciStoppingStep::Continue(Self {
                    radio: BluetoothPassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    },
                    order,
                })
            }
            BluetoothPassiveScanActiveStep::Waiting(active) => {
                BluetoothPassiveScanHciStoppingStep::Waiting(Self {
                    radio: BluetoothPassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    },
                    order,
                })
            }
            BluetoothPassiveScanActiveStep::UnrelatedList { session, observed } => {
                BluetoothPassiveScanHciStoppingStep::UnrelatedList {
                    stopping: Self {
                        radio: BluetoothPassiveScanHciActiveRadio {
                            active: session,
                            duplicate_filter,
                        },
                        order,
                    },
                    observed,
                }
            }
            BluetoothPassiveScanActiveStep::CpuOwned(completed) => {
                let radio = BluetoothPassiveScanHciCpuOwnedRadio {
                    completed,
                    duplicate_filter,
                };
                match order {
                    BluetoothPassiveScanHciStopOrder::Disable(deferred) => {
                        BluetoothPassiveScanHciStoppingStep::Disable(stop_scanner(radio, deferred))
                    }
                    BluetoothPassiveScanHciStopOrder::Reset(barrier) => {
                        BluetoothPassiveScanHciStoppingStep::Reset(reset_scanner(radio, barrier))
                    }
                }
            }
            BluetoothPassiveScanActiveStep::Fault(fault) => {
                BluetoothPassiveScanHciStoppingStep::Fault(BluetoothPassiveScanHciStoppingFault {
                    _fault: fault,
                    _order: order,
                    _duplicate_filter: duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn has_pending_event(&self) -> bool {
        self.pending_event.is_some()
    }

    /// Wait for a Host command without consuming this completed receive batch.
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_command_available(&self.order).await
    }

    /// Drop unpublished observations after hardware is already quiescent.
    ///
    /// This edge exists so Disable or Reset cannot be held hostage by a full
    /// unsolicited-event queue. The retained scanner and HCI order remain
    /// exact; only Host reports from the reclaimed batch are abandoned.
    pub fn discard_remaining(
        self,
    ) -> BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY> {
        BluetoothPassiveScanHciReportsComplete {
            completed: self.completed,
            order: self.order,
            duplicate_filter: self.duplicate_filter,
        }
    }

    pub async fn wait_report_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        if !self.order.accepts_endpoint(controller) {
            return Err(LeControllerEndpointMismatch);
        }
        controller.wait_legacy_advertising_report_capacity().await;
        Ok(())
    }

    /// Parse, filter and attempt to publish at most one report.
    pub fn step<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        mut self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothPassiveScanHciReportStep<'runtime, S, CAPACITY> {
        if !self.order.accepts_endpoint(controller) {
            return BluetoothPassiveScanHciReportStep::EndpointMismatch(self);
        }

        if self.pending_event.is_none() {
            if self.next_report >= self.completed.received().len() {
                return BluetoothPassiveScanHciReportStep::Complete(
                    BluetoothPassiveScanHciReportsComplete {
                        completed: self.completed,
                        order: self.order,
                        duplicate_filter: self.duplicate_filter,
                    },
                );
            }
            let report_index = self.next_report;
            self.next_report += 1;
            let report = match self.completed.report(report_index) {
                Ok(Some(report)) => report,
                Ok(None) => unreachable!("the copied receive-batch length bounds every index"),
                Err(error) => {
                    return BluetoothPassiveScanHciReportStep::IgnoredMalformed {
                        reports: self,
                        error,
                    };
                }
            };
            if self.completed.duplicate_policy() == LegacyScanDuplicatePolicy::FilterDuplicates
                && !self.duplicate_filter.accept(report)
            {
                return BluetoothPassiveScanHciReportStep::Masked(self);
            }
            match hci_report_event(report) {
                Ok(event) => self.pending_event = Some(event),
                Err(error) => {
                    return BluetoothPassiveScanHciReportStep::EncodingFault {
                        reports: self,
                        error,
                    };
                }
            }
        }

        let event = self
            .pending_event
            .as_ref()
            .expect("a parsed, accepted report retains its HCI event");
        match controller.try_publish_legacy_advertising_report(event) {
            Ok(LeLegacyAdvertisingReportPublication::Published) => {
                self.pending_event = None;
                BluetoothPassiveScanHciReportStep::Published(self)
            }
            Ok(LeLegacyAdvertisingReportPublication::Masked) => {
                self.pending_event = None;
                BluetoothPassiveScanHciReportStep::Masked(self)
            }
            Err(error) => BluetoothPassiveScanHciReportStep::Pending {
                reports: self,
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciReportsComplete<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn into_radio_and_order(
        self,
    ) -> (
        BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
        LeControllerCommandReady<'runtime, ()>,
    ) {
        (
            BluetoothPassiveScanHciCpuOwnedRadio {
                completed: self.completed,
                duplicate_filter: self.duplicate_filter,
            },
            self.order,
        )
    }

    fn from_radio_and_order(
        radio: BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
        order: LeControllerCommandReady<'runtime, ()>,
    ) -> Self {
        Self {
            completed: radio.completed,
            order,
            duplicate_filter: radio.duplicate_filter,
        }
    }

    /// Consume and classify at most one command before another window starts.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
    ) -> BluetoothPassiveScanHciCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let (radio, order) = self.into_radio_and_order();
        let ready = order.map_owner(|()| radio);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route = match controller
                    .route_active_legacy_scanning_classified_command(command)
                {
                    HciActiveLegacyScanningCommandRoute::ResponsePending(transaction) => {
                        BluetoothPassiveScanHciCommandRoute::ResponsePending(
                            BluetoothPassiveScanHciCpuResponsePending { transaction },
                        )
                    }
                    HciActiveLegacyScanningCommandRoute::Disable(deferred) => {
                        let (radio, deferred) = deferred.into_parts();
                        BluetoothPassiveScanHciCommandRoute::Disable(stop_scanner(radio, deferred))
                    }
                    HciActiveLegacyScanningCommandRoute::ResetBarrier(barrier) => {
                        let (radio, barrier) = barrier.into_parts();
                        BluetoothPassiveScanHciCommandRoute::Reset(reset_scanner(radio, barrier))
                    }
                    HciActiveLegacyScanningCommandRoute::EndpointMismatch(command) => {
                        BluetoothPassiveScanHciCommandRoute::EndpointMismatch(
                            BluetoothPassiveScanHciCommandMismatch { _command: command },
                        )
                    }
                };
                BluetoothPassiveScanHciCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciCommandIntake::Empty {
                    completed: Self::from_radio_and_order(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciCommandIntake::EndpointMismatch {
                    completed: Self::from_radio_and_order(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciCommandIntake::Channel {
                    completed: Self::from_radio_and_order(radio, order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciCommandIntake::NonCommand {
                    completed: Self::from_radio_and_order(radio, order),
                    frame,
                }
            }
        }
    }

    /// Begin the next interval-preserving receive window after report handling.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure retains every scanner and HCI owner"
    )]
    pub fn begin_recurring(
        self,
    ) -> Result<
        BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>,
        BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>,
    > {
        let Self {
            completed,
            order,
            duplicate_filter,
        } = self;
        let (task, scanner, phase, _received, _status) = completed.into_parts();
        match BluetoothPassiveScanFirstRunner::begin_recurring(task, scanner, phase) {
            Ok(runner) => Ok(BluetoothPassiveScanHciRecurringRunner {
                runner,
                order,
                duplicate_filter,
            }),
            Err(failure) => Err(BluetoothPassiveScanHciRecurringFailure {
                failure,
                order,
                duplicate_filter,
            }),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothPassiveScanHciCpuResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                BluetoothPassiveScanHciCpuResponsePublication::Published(
                    BluetoothPassiveScanHciReportsComplete::from_radio_and_order(radio, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothPassiveScanHciCpuResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothPassiveScanHciCpuResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothPassiveScanHciCpuResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

fn stop_scanner<'runtime, S, const CAPACITY: usize>(
    radio: BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    deferred: LeControllerDeferredLegacyScanningDisable<'runtime, ()>,
) -> BluetoothControllerIdleResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (task, scanner, _phase, _received, _status) = radio.completed.into_parts();
    let _disabled = scanner.disable();
    BluetoothControllerIdleResponsePending::new(
        deferred.map_owner(|()| task).into_stopped_response(),
    )
}

fn reset_scanner<'runtime, S, const CAPACITY: usize>(
    radio: BluetoothPassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    barrier: LeControllerResetBarrier<'runtime, ()>,
) -> BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (task, scanner, _phase, _received, _status) = radio.completed.into_parts();
    let _disabled = scanner.disable();
    BluetoothControllerIdleResetBarrier::new(barrier.map_owner(|()| task))
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn step(self) -> BluetoothPassiveScanHciRecurringRunnerStep<'runtime, S, CAPACITY> {
        let Self {
            runner,
            order,
            duplicate_filter,
        } = self;
        match runner.step() {
            BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(runner) => {
                BluetoothPassiveScanHciRecurringRunnerStep::WaitControllerTime(Self {
                    runner,
                    order,
                    duplicate_filter,
                })
            }
            BluetoothPassiveScanFirstRunnerStep::Continue(runner) => {
                BluetoothPassiveScanHciRecurringRunnerStep::Continue(Self {
                    runner,
                    order,
                    duplicate_filter,
                })
            }
            BluetoothPassiveScanFirstRunnerStep::Running(running) => {
                BluetoothPassiveScanHciRecurringRunnerStep::Running(
                    BluetoothPassiveScanHciActiveSession {
                        active: BluetoothPassiveScanActiveSession::from_first_running(running),
                        order,
                        duplicate_filter,
                    },
                )
            }
            BluetoothPassiveScanFirstRunnerStep::Failed(failure) => {
                BluetoothPassiveScanHciRecurringRunnerStep::Failed(
                    BluetoothPassiveScanHciRecurringFailure {
                        failure,
                        order,
                        duplicate_filter,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn retry(
        self,
    ) -> Result<BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>, Self> {
        let Self {
            failure,
            order,
            duplicate_filter,
        } = self;
        match failure {
            BluetoothPassiveScanFirstRunnerFailure::Retryable(retry) => {
                Ok(BluetoothPassiveScanHciRecurringRunner {
                    runner: retry.retry(),
                    order,
                    duplicate_filter,
                })
            }
            failure => Err(Self {
                failure,
                order,
                duplicate_filter,
            }),
        }
    }

    pub fn retry_cause(&self) -> Option<&BluetoothPassiveScanFirstRunnerRetryCause<S::Error>> {
        match &self.failure {
            BluetoothPassiveScanFirstRunnerFailure::Retryable(retry) => Some(retry.cause()),
            _ => None,
        }
    }
}

fn hci_report_event(
    report: LegacyAdvertisingReport,
) -> Result<LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError> {
    use open_esp_radio_bluetooth_hci::bt_hci::param::{AddrKind, BdAddr, LeAdvEventKind};
    use open_esp_radio_bluetooth_ll::LeDeviceAddressKind;

    let event_kind = match report.kind() {
        LegacyAdvertisingReportKind::ConnectableUndirected => LeAdvEventKind::AdvInd,
        LegacyAdvertisingReportKind::ConnectableDirected => LeAdvEventKind::AdvDirectInd,
        LegacyAdvertisingReportKind::NonconnectableUndirected => LeAdvEventKind::AdvNonconnInd,
        LegacyAdvertisingReportKind::ScanResponse => LeAdvEventKind::ScanRsp,
        LegacyAdvertisingReportKind::ScannableUndirected => LeAdvEventKind::AdvScanInd,
    };
    let address_kind = match report.advertiser().kind() {
        LeDeviceAddressKind::Public => AddrKind::PUBLIC,
        LeDeviceAddressKind::Random => AddrKind::RANDOM,
    };
    LeLegacyAdvertisingReportEvent::new(
        event_kind,
        address_kind,
        BdAddr::new(report.advertiser().wire_bytes()),
        report.data(),
        report.rssi_dbm(),
    )
}
