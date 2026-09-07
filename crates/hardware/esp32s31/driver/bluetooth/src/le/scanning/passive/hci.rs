//! HCI order composition for the ESP32-S31 passive scanner.
//!
//! The lower scanner runner owns LL state and hardware. This module only keeps
//! the accepted standard Enable command affine beside that runner until `RUN`,
//! then transfers the returned command authority into the active scan session.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerIdleResetBarrier, ControllerIdleResponsePending, ControllerPublishedTaskService,
        SchedulerRunInterruptStorage,
    },
    le::scanning::{
        PassiveScanActiveFault, PassiveScanActiveSession, PassiveScanActiveStep,
        PassiveScanActiveWait, PassiveScanEventCpuOwned, PassiveScanFirstRunner,
        PassiveScanFirstRunnerFailure, PassiveScanFirstRunnerRetryCause,
        PassiveScanFirstRunnerStep,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyScanningCommandRoute as HciActiveLegacyScanningCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerDeferredLegacyScanningDisable,
    LeControllerDeferredLegacyScanningStart, LeControllerEndpointMismatch,
    LeControllerResetBarrier, LeControllerResponsePending, LeControllerResponsePublication,
    LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError,
    LeLegacyAdvertisingReportPublication, LeLegacyScanningDuplicatePolicy,
};

use oer_bluetooth_ll::scanning::{
    LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReport, LegacyAdvertisingReportKind,
    LegacyAdvertisingReportParseError, LegacyPassiveScanParameters, LegacyPassiveScannerDisabled,
    LegacyScanDuplicatePolicy, LegacyScanInterval, LegacyScanTimingError, LegacyScanWindow,
};

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;

const LEGACY_SCAN_DUPLICATE_FILTER_CAPACITY: usize = 32;
type DuplicateFilter = LegacyAdvertisingDuplicateFilter<LEGACY_SCAN_DUPLICATE_FILTER_CAPACITY>;

/// HCI Enable retained beside the lower first-window runner.
#[must_use = "step the scanner until RUN or retain its exact failure"]
pub struct PassiveScanHciFirstRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    runner: PassiveScanFirstRunner<'runtime, S, CAPACITY>,
}

/// One finite HCI-composed first-window transition.
#[must_use = "retain a wait, running owner, or exact failure"]
pub enum PassiveScanHciFirstRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(PassiveScanHciFirstRunner<'runtime, S, CAPACITY>),
    Continue(PassiveScanHciFirstRunner<'runtime, S, CAPACITY>),
    Running(PassiveScanHciFirstRunning<'runtime, S, CAPACITY>),
    Failed(PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
}

/// Accepted Enable and the exact scanner graph after scheduler `RUN`.
#[must_use = "publish Enable success while retaining the running scanner"]
pub struct PassiveScanHciFirstRunning<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    running: crate::le::scanning::PassiveScanFirstRunning<'runtime, S, CAPACITY>,
}

/// A failed semantic conversion or exact lower first-window owner.
#[must_use = "retry the lower edge or recover ordered hardware-failure response"]
pub enum PassiveScanHciFirstRunnerFailure<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Parameters {
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
        task: Task<'runtime, S, CAPACITY>,
        error: LegacyScanTimingError,
    },
    Lower {
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
        failure: PassiveScanFirstRunnerFailure<'runtime, S, CAPACITY>,
    },
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Retry only a lower pre-`RUN` publication/start edge.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn retry(self) -> Result<PassiveScanHciFirstRunner<'runtime, S, CAPACITY>, Self> {
        match self {
            Self::Lower {
                command,
                failure: PassiveScanFirstRunnerFailure::Retryable(retry),
            } => Ok(PassiveScanHciFirstRunner {
                command,
                runner: retry.retry(),
            }),
            failure => Err(failure),
        }
    }

    /// Inspect a retryable lower cause without separating its owner.
    pub fn retry_cause(&self) -> Option<&PassiveScanFirstRunnerRetryCause<S::Error>> {
        match self {
            Self::Lower {
                failure: PassiveScanFirstRunnerFailure::Retryable(retry),
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
    ) -> Result<ControllerIdleResponsePending<'runtime, S, CAPACITY>, Self> {
        let (command, task) = match self {
            Self::Parameters { command, task, .. } => (command, task),
            Self::Lower { command, failure } => {
                let task = match failure {
                    PassiveScanFirstRunnerFailure::ColdBegin { failure, .. } => {
                        failure.into_parts().0
                    }
                    PassiveScanFirstRunnerFailure::ColdRecheck { failure, .. } => {
                        failure.into_parts().0
                    }
                    PassiveScanFirstRunnerFailure::WarmBegin { failure, .. } => {
                        failure.into_parts().0.into_task_service()
                    }
                    PassiveScanFirstRunnerFailure::WarmRecheck { failure, .. } => {
                        failure.into_parts().0.into_task_service()
                    }
                    PassiveScanFirstRunnerFailure::Recovered { task, .. } => task,
                    failure @ (PassiveScanFirstRunnerFailure::PreparationFailStop { .. }
                    | PassiveScanFirstRunnerFailure::PublicationFailStop(_)
                    | PassiveScanFirstRunnerFailure::Retryable(_)) => {
                        return Err(Self::Lower { command, failure });
                    }
                };
                (command, task)
            }
        };
        Ok(ControllerIdleResponsePending::new(
            command
                .map_owner(|()| task)
                .into_hardware_failure_response(),
        ))
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciFirstRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains HCI order and the exact hardware owner"
    )]
    pub(crate) fn begin(
        task: Task<'runtime, S, CAPACITY>,
        command: LeControllerDeferredLegacyScanningStart<'runtime, ()>,
    ) -> Result<Self, PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>> {
        let request = command.request();
        let interval = match LegacyScanInterval::new(request.parameters().interval_units_625_us()) {
            Ok(interval) => interval,
            Err(error) => {
                return Err(PassiveScanHciFirstRunnerFailure::Parameters {
                    command,
                    task,
                    error,
                });
            }
        };
        let window = match LegacyScanWindow::new(request.parameters().window_units_625_us()) {
            Ok(window) => window,
            Err(error) => {
                return Err(PassiveScanHciFirstRunnerFailure::Parameters {
                    command,
                    task,
                    error,
                });
            }
        };
        let parameters = match LegacyPassiveScanParameters::new(interval, window) {
            Ok(parameters) => parameters,
            Err(error) => {
                return Err(PassiveScanHciFirstRunnerFailure::Parameters {
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
        match PassiveScanFirstRunner::begin(task, scanner) {
            Ok(runner) => Ok(Self { command, runner }),
            Err(failure) => Err(PassiveScanHciFirstRunnerFailure::Lower { command, failure }),
        }
    }

    /// Execute exactly one lower transition without releasing HCI order.
    pub fn step(self) -> PassiveScanHciFirstRunnerStep<'runtime, S, CAPACITY> {
        let Self { command, runner } = self;
        match runner.step() {
            PassiveScanFirstRunnerStep::WaitControllerTime(runner) => {
                PassiveScanHciFirstRunnerStep::WaitControllerTime(Self { command, runner })
            }
            PassiveScanFirstRunnerStep::Continue(runner) => {
                PassiveScanHciFirstRunnerStep::Continue(Self { command, runner })
            }
            PassiveScanFirstRunnerStep::Running(running) => {
                PassiveScanHciFirstRunnerStep::Running(PassiveScanHciFirstRunning {
                    command,
                    running,
                })
            }
            PassiveScanFirstRunnerStep::Failed(failure) => {
                PassiveScanHciFirstRunnerStep::Failed(PassiveScanHciFirstRunnerFailure::Lower {
                    command,
                    failure,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciFirstRunning<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Pair the exact success response with the already-running scanner.
    pub fn into_response_pending_session(
        self,
    ) -> PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY> {
        let active = PassiveScanActiveSession::from_first_running(self.running);
        PassiveScanHciResponsePendingSession {
            transaction: self.command.map_owner(|()| active).into_started_response(),
        }
    }
}

/// Accepted Enable response paired with the exact already-running scanner.
#[must_use = "publish the response while retaining the active scanner"]
pub struct PassiveScanHciResponsePendingSession<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction:
        LeControllerResponsePending<'runtime, PassiveScanActiveSession<'runtime, S, CAPACITY>>,
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    ) -> PassiveScanHciResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (active, order) = ready.into_parts();
                PassiveScanHciResponsePublication::Published(PassiveScanHciActiveSession {
                    active,
                    order,
                    duplicate_filter: DuplicateFilter::new(),
                })
            }
            LeControllerResponsePublication::Pending(transaction) => {
                PassiveScanHciResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                PassiveScanHciResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault { pending, error } => {
                PassiveScanHciResponsePublication::Fault {
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
pub enum PassiveScanHciResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Pending(PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    EndpointMismatch(PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>),
    Fault {
        pending: PassiveScanHciResponsePendingSession<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Active lower scanner paired with the sole next-command authority.
#[must_use = "drive radio progress and preserve HCI command authority"]
pub struct PassiveScanHciActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    active: PassiveScanActiveSession<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

/// One bounded active scanner transition with HCI order retained.
#[must_use = "retain radio progress, the completed report batch, or the fail-stop owner"]
pub enum PassiveScanHciActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Waiting(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Fault(PassiveScanHciActiveFault<'runtime, S, CAPACITY>),
}

struct PassiveScanHciActiveRadio<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    active: PassiveScanActiveSession<'runtime, S, CAPACITY>,
    duplicate_filter: DuplicateFilter,
}

/// One command response whose scanner window remains hardware-owned.
#[must_use = "publish the response while continuing scanner reclamation"]
pub struct PassiveScanHciActiveResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction:
        LeControllerResponsePending<'runtime, PassiveScanHciActiveRadio<'runtime, S, CAPACITY>>,
}

/// Response publication with the active scanner returned exactly once.
#[must_use = "retain the response transaction or returned command-ready scanner"]
pub enum PassiveScanHciActiveResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Pending(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// One radio transition while an accepted command response is backpressured.
#[must_use = "continue scanner reclamation without losing the response"]
pub enum PassiveScanHciActivePendingRadioStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Waiting(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    UnrelatedList {
        pending: PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>,
        observed: crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved,
    },
    CpuOwned(PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Fault(PassiveScanHciActivePendingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner preserving the radio fault and pending response.
#[must_use = "retain the exact failed scanner and ordered response"]
pub struct PassiveScanHciActivePendingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _fault: PassiveScanActiveFault<'runtime, S, CAPACITY>,
    _response: LeControllerResponsePending<'runtime, ()>,
    _duplicate_filter: DuplicateFilter,
}

enum PassiveScanHciStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyScanningDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Accepted Disable or Reset retained until the current window is quiescent.
#[must_use = "drive the current scanner window to CPU ownership"]
pub struct PassiveScanHciStopping<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    radio: PassiveScanHciActiveRadio<'runtime, S, CAPACITY>,
    order: PassiveScanHciStopOrder<'runtime>,
}

/// One bounded transition toward a quiescent scanner.
#[must_use = "retain stop order until scanner ownership is fully reclaimed"]
#[expect(
    clippy::large_enum_variant,
    reason = "the no-alloc stop path retains the complete affine scanner fault"
)]
pub enum PassiveScanHciStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(PassiveScanHciStopping<'runtime, S, CAPACITY>),
    Waiting(PassiveScanHciStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: PassiveScanHciStopping<'runtime, S, CAPACITY>,
        observed: crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved,
    },
    Disable(ControllerIdleResponsePending<'runtime, S, CAPACITY>),
    Reset(ControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    Fault(PassiveScanHciStoppingFault<'runtime, S, CAPACITY>),
}

/// Fail-stop owner retaining the scanner fault and accepted stop command.
#[must_use = "retain the exact failed stop transaction for diagnostics"]
pub struct PassiveScanHciStoppingFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _fault: PassiveScanActiveFault<'runtime, S, CAPACITY>,
    _order: PassiveScanHciStopOrder<'runtime>,
    _duplicate_filter: DuplicateFilter,
}

/// Opaque owner for an impossible endpoint mismatch during an active window.
#[must_use = "retain the command and complete active scanner owner"]
pub struct PassiveScanHciActiveCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        PassiveScanHciActiveRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed route for a command consumed while one scan window is in flight.
#[must_use = "publish, quiesce, or retain the exact mismatch owner"]
pub enum PassiveScanHciActiveCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(PassiveScanHciStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(PassiveScanHciActiveCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking HCI intake while the scanner graph remains hardware-owned.
#[must_use = "route a command or retain the exact active session"]
pub enum PassiveScanHciActiveCommandIntake<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: PassiveScanHciActiveCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// Fail-stop scanner owner retaining HCI order and duplicate-filter history.
#[must_use = "retain the exact failed scanner owner for diagnostic shutdown"]
pub struct PassiveScanHciActiveFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _fault: PassiveScanActiveFault<'runtime, S, CAPACITY>,
    _order: LeControllerCommandReady<'runtime, ()>,
    _duplicate_filter: DuplicateFilter,
}

/// CPU-owned receive batch being converted and published as standard HCI events.
#[must_use = "publish or explicitly retain every report before starting the next scan window"]
pub struct PassiveScanHciReportsPending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completed: PassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
    next_report: usize,
    pending_event: Option<LeLegacyAdvertisingReportEvent>,
}

/// Result of one bounded report parsing/filtering/publication transition.
#[must_use = "retain pending reports, the completed batch, or an exact diagnostic"]
pub enum PassiveScanHciReportStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Masked(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Pending {
        reports: PassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
    IgnoredMalformed {
        reports: PassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: LegacyAdvertisingReportParseError,
    },
    EncodingFault {
        reports: PassiveScanHciReportsPending<'runtime, S, CAPACITY>,
        error: LeLegacyAdvertisingReportEventError,
    },
    EndpointMismatch(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    Complete(PassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
}

/// CPU-owned scanner after every report from one receive batch was handled.
#[must_use = "start the next interval-preserving window or stop the scanner"]
pub struct PassiveScanHciReportsComplete<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completed: PassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

struct PassiveScanHciCpuOwnedRadio<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completed: PassiveScanEventCpuOwned<'runtime, S, CAPACITY>,
    duplicate_filter: DuplicateFilter,
}

/// One ordinary command response retained at the safe between-window boundary.
#[must_use = "publish the response before beginning the next scan window"]
pub struct PassiveScanHciCpuResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction:
        LeControllerResponsePending<'runtime, PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>>,
}

/// Publication result for a response between passive scan windows.
#[must_use = "retain response backpressure or the returned complete scanner"]
pub enum PassiveScanHciCpuResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(PassiveScanHciReportsComplete<'runtime, S, CAPACITY>),
    Pending(PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Opaque owner for an impossible classified-command endpoint mismatch.
#[must_use = "retain the command, completed scanner and exact HCI order"]
pub struct PassiveScanHciCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    >,
}

/// Typed command route between fully reclaimed passive scan windows.
#[must_use = "publish, disable, reset, or retain the exact mismatch owner"]
pub enum PassiveScanHciCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>),
    Disable(ControllerIdleResponsePending<'runtime, S, CAPACITY>),
    Reset(ControllerIdleResetBarrier<'runtime, S, CAPACITY>),
    EndpointMismatch(PassiveScanHciCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking HCI intake at the safe between-window boundary.
#[must_use = "route a command or retain the exact complete scanner"]
pub enum PassiveScanHciCommandIntake<'runtime, 'command, 'buffer, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: PassiveScanHciCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        completed: PassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        completed: PassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        completed: PassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        completed: PassiveScanHciReportsComplete<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

/// HCI order and duplicate history retained through recurring-window preparation.
#[must_use = "step the recurring scanner until RUN or retain its exact failure"]
pub struct PassiveScanHciRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: PassiveScanFirstRunner<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

/// One finite recurring-window transition.
#[must_use = "retain a wait, running owner, or exact failure"]
pub enum PassiveScanHciRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Continue(PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Running(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Failed(PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
}

/// Exact lower recurring-start failure plus all session-wide HCI policy.
#[must_use = "retry the retained lower edge or keep the complete fail-stop owner"]
pub struct PassiveScanHciRecurringFailure<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    failure: PassiveScanFirstRunnerFailure<'runtime, S, CAPACITY>,
    order: LeControllerCommandReady<'runtime, ()>,
    duplicate_filter: DuplicateFilter,
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciActiveSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<PassiveScanActiveWait<'_>> {
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
    ) -> PassiveScanHciActiveCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let Self {
            active,
            order,
            duplicate_filter,
        } = self;
        let ready = order.map_owner(|()| PassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        });
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_scanning_classified_command(command) {
                        HciActiveLegacyScanningCommandRoute::ResponsePending(transaction) => {
                            PassiveScanHciActiveCommandRoute::ResponsePending(
                                PassiveScanHciActiveResponsePending { transaction },
                            )
                        }
                        HciActiveLegacyScanningCommandRoute::Disable(deferred) => {
                            let (radio, deferred) = deferred.into_parts();
                            PassiveScanHciActiveCommandRoute::Stopping(PassiveScanHciStopping {
                                radio,
                                order: PassiveScanHciStopOrder::Disable(deferred),
                            })
                        }
                        HciActiveLegacyScanningCommandRoute::ResetBarrier(barrier) => {
                            let (radio, barrier) = barrier.into_parts();
                            PassiveScanHciActiveCommandRoute::Stopping(PassiveScanHciStopping {
                                radio,
                                order: PassiveScanHciStopOrder::Reset(barrier),
                            })
                        }
                        HciActiveLegacyScanningCommandRoute::EndpointMismatch(command) => {
                            PassiveScanHciActiveCommandRoute::EndpointMismatch(
                                PassiveScanHciActiveCommandMismatch { _command: command },
                            )
                        }
                    };
                PassiveScanHciActiveCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciActiveCommandIntake::Empty {
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
                PassiveScanHciActiveCommandIntake::EndpointMismatch {
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
                PassiveScanHciActiveCommandIntake::Channel {
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
                PassiveScanHciActiveCommandIntake::NonCommand {
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
    pub fn step_radio(self) -> PassiveScanHciActiveStep<'runtime, S, CAPACITY> {
        let Self {
            active,
            order,
            duplicate_filter,
        } = self;
        match active.step_radio() {
            PassiveScanActiveStep::Continue(active) => PassiveScanHciActiveStep::Continue(Self {
                active,
                order,
                duplicate_filter,
            }),
            PassiveScanActiveStep::Waiting(active) => PassiveScanHciActiveStep::Waiting(Self {
                active,
                order,
                duplicate_filter,
            }),
            PassiveScanActiveStep::UnrelatedList { session, observed } => {
                PassiveScanHciActiveStep::UnrelatedList {
                    session: Self {
                        active: session,
                        order,
                        duplicate_filter,
                    },
                    observed,
                }
            }
            PassiveScanActiveStep::CpuOwned(completed) => {
                PassiveScanHciActiveStep::CpuOwned(PassiveScanHciReportsPending {
                    completed,
                    order,
                    duplicate_filter,
                    next_report: 0,
                    pending_event: None,
                })
            }
            PassiveScanActiveStep::Fault(fault) => {
                PassiveScanHciActiveStep::Fault(PassiveScanHciActiveFault {
                    _fault: fault,
                    _order: order,
                    _duplicate_filter: duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciActiveResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<PassiveScanActiveWait<'_>> {
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
    ) -> PassiveScanHciActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciActiveResponsePublication::Published(PassiveScanHciActiveSession {
                    active: radio.active,
                    order,
                    duplicate_filter: radio.duplicate_filter,
                })
            }
            LeControllerResponsePublication::Pending(transaction) => {
                PassiveScanHciActiveResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                PassiveScanHciActiveResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => PassiveScanHciActiveResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }

    pub fn step_radio(self) -> PassiveScanHciActivePendingRadioStep<'runtime, S, CAPACITY> {
        let (radio, response) = self.transaction.into_parts();
        let PassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        } = radio;
        match active.step_radio() {
            PassiveScanActiveStep::Continue(active) => {
                PassiveScanHciActivePendingRadioStep::Continue(Self {
                    transaction: response.map_owner(|()| PassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    }),
                })
            }
            PassiveScanActiveStep::Waiting(active) => {
                PassiveScanHciActivePendingRadioStep::Waiting(Self {
                    transaction: response.map_owner(|()| PassiveScanHciActiveRadio {
                        active,
                        duplicate_filter,
                    }),
                })
            }
            PassiveScanActiveStep::UnrelatedList { session, observed } => {
                PassiveScanHciActivePendingRadioStep::UnrelatedList {
                    pending: Self {
                        transaction: response.map_owner(|()| PassiveScanHciActiveRadio {
                            active: session,
                            duplicate_filter,
                        }),
                    },
                    observed,
                }
            }
            PassiveScanActiveStep::CpuOwned(completed) => {
                PassiveScanHciActivePendingRadioStep::CpuOwned(PassiveScanHciCpuResponsePending {
                    transaction: response.map_owner(|()| PassiveScanHciCpuOwnedRadio {
                        completed,
                        duplicate_filter,
                    }),
                })
            }
            PassiveScanActiveStep::Fault(fault) => {
                PassiveScanHciActivePendingRadioStep::Fault(PassiveScanHciActivePendingFault {
                    _fault: fault,
                    _response: response,
                    _duplicate_filter: duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn radio_wait(&self) -> Option<PassiveScanActiveWait<'_>> {
        self.radio.active.radio_wait()
    }

    pub fn step(self) -> PassiveScanHciStoppingStep<'runtime, S, CAPACITY> {
        let Self { radio, order } = self;
        let PassiveScanHciActiveRadio {
            active,
            duplicate_filter,
        } = radio;
        match active.step_radio() {
            PassiveScanActiveStep::Continue(active) => PassiveScanHciStoppingStep::Continue(Self {
                radio: PassiveScanHciActiveRadio {
                    active,
                    duplicate_filter,
                },
                order,
            }),
            PassiveScanActiveStep::Waiting(active) => PassiveScanHciStoppingStep::Waiting(Self {
                radio: PassiveScanHciActiveRadio {
                    active,
                    duplicate_filter,
                },
                order,
            }),
            PassiveScanActiveStep::UnrelatedList { session, observed } => {
                PassiveScanHciStoppingStep::UnrelatedList {
                    stopping: Self {
                        radio: PassiveScanHciActiveRadio {
                            active: session,
                            duplicate_filter,
                        },
                        order,
                    },
                    observed,
                }
            }
            PassiveScanActiveStep::CpuOwned(completed) => {
                let radio = PassiveScanHciCpuOwnedRadio {
                    completed,
                    duplicate_filter,
                };
                match order {
                    PassiveScanHciStopOrder::Disable(deferred) => {
                        PassiveScanHciStoppingStep::Disable(stop_scanner(radio, deferred))
                    }
                    PassiveScanHciStopOrder::Reset(barrier) => {
                        PassiveScanHciStoppingStep::Reset(reset_scanner(radio, barrier))
                    }
                }
            }
            PassiveScanActiveStep::Fault(fault) => {
                PassiveScanHciStoppingStep::Fault(PassiveScanHciStoppingFault {
                    _fault: fault,
                    _order: order,
                    _duplicate_filter: duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciReportsPending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    pub fn discard_remaining(self) -> PassiveScanHciReportsComplete<'runtime, S, CAPACITY> {
        PassiveScanHciReportsComplete {
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
    ) -> PassiveScanHciReportStep<'runtime, S, CAPACITY> {
        if !self.order.accepts_endpoint(controller) {
            return PassiveScanHciReportStep::EndpointMismatch(self);
        }

        if self.pending_event.is_none() {
            if self.next_report >= self.completed.received().len() {
                return PassiveScanHciReportStep::Complete(PassiveScanHciReportsComplete {
                    completed: self.completed,
                    order: self.order,
                    duplicate_filter: self.duplicate_filter,
                });
            }
            let report_index = self.next_report;
            self.next_report += 1;
            let report = match self.completed.report(report_index) {
                Ok(Some(report)) => report,
                Ok(None) => unreachable!("the copied receive-batch length bounds every index"),
                Err(error) => {
                    return PassiveScanHciReportStep::IgnoredMalformed {
                        reports: self,
                        error,
                    };
                }
            };
            if self.completed.duplicate_policy() == LegacyScanDuplicatePolicy::FilterDuplicates
                && !self.duplicate_filter.accept(report)
            {
                return PassiveScanHciReportStep::Masked(self);
            }
            match hci_report_event(report) {
                Ok(event) => self.pending_event = Some(event),
                Err(error) => {
                    return PassiveScanHciReportStep::EncodingFault {
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
                PassiveScanHciReportStep::Published(self)
            }
            Ok(LeLegacyAdvertisingReportPublication::Masked) => {
                self.pending_event = None;
                PassiveScanHciReportStep::Masked(self)
            }
            Err(error) => PassiveScanHciReportStep::Pending {
                reports: self,
                error,
            },
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciReportsComplete<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn into_radio_and_order(
        self,
    ) -> (
        PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
        LeControllerCommandReady<'runtime, ()>,
    ) {
        (
            PassiveScanHciCpuOwnedRadio {
                completed: self.completed,
                duplicate_filter: self.duplicate_filter,
            },
            self.order,
        )
    }

    fn from_radio_and_order(
        radio: PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
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
    ) -> PassiveScanHciCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        let (radio, order) = self.into_radio_and_order();
        let ready = order.map_owner(|()| radio);
        match controller.try_receive_classified_command_with_buffer(ready, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_scanning_classified_command(command) {
                        HciActiveLegacyScanningCommandRoute::ResponsePending(transaction) => {
                            PassiveScanHciCommandRoute::ResponsePending(
                                PassiveScanHciCpuResponsePending { transaction },
                            )
                        }
                        HciActiveLegacyScanningCommandRoute::Disable(deferred) => {
                            let (radio, deferred) = deferred.into_parts();
                            PassiveScanHciCommandRoute::Disable(stop_scanner(radio, deferred))
                        }
                        HciActiveLegacyScanningCommandRoute::ResetBarrier(barrier) => {
                            let (radio, barrier) = barrier.into_parts();
                            PassiveScanHciCommandRoute::Reset(reset_scanner(radio, barrier))
                        }
                        HciActiveLegacyScanningCommandRoute::EndpointMismatch(command) => {
                            PassiveScanHciCommandRoute::EndpointMismatch(
                                PassiveScanHciCommandMismatch { _command: command },
                            )
                        }
                    };
                PassiveScanHciCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciCommandIntake::Empty {
                    completed: Self::from_radio_and_order(radio, order),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciCommandIntake::EndpointMismatch {
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
                PassiveScanHciCommandIntake::Channel {
                    completed: Self::from_radio_and_order(radio, order),
                    buffer,
                    error,
                }
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciCommandIntake::NonCommand {
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
        PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>,
        PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>,
    > {
        let Self {
            completed,
            order,
            duplicate_filter,
        } = self;
        let (task, scanner, phase, _received, _status) = completed.into_parts();
        match PassiveScanFirstRunner::begin_recurring(task, scanner, phase) {
            Ok(runner) => Ok(PassiveScanHciRecurringRunner {
                runner,
                order,
                duplicate_filter,
            }),
            Err(failure) => Err(PassiveScanHciRecurringFailure {
                failure,
                order,
                duplicate_filter,
            }),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciCpuResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    ) -> PassiveScanHciCpuResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ready) => {
                let (radio, order) = ready.into_parts();
                PassiveScanHciCpuResponsePublication::Published(
                    PassiveScanHciReportsComplete::from_radio_and_order(radio, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                PassiveScanHciCpuResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                PassiveScanHciCpuResponsePublication::EndpointMismatch(Self { transaction })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => PassiveScanHciCpuResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }
}

fn stop_scanner<'runtime, S, const CAPACITY: usize>(
    radio: PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    deferred: LeControllerDeferredLegacyScanningDisable<'runtime, ()>,
) -> ControllerIdleResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (task, scanner, _phase, _received, _status) = radio.completed.into_parts();
    let _disabled = scanner.disable();
    ControllerIdleResponsePending::new(deferred.map_owner(|()| task).into_stopped_response())
}

fn reset_scanner<'runtime, S, const CAPACITY: usize>(
    radio: PassiveScanHciCpuOwnedRadio<'runtime, S, CAPACITY>,
    barrier: LeControllerResetBarrier<'runtime, ()>,
) -> ControllerIdleResetBarrier<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let (task, scanner, _phase, _received, _status) = radio.completed.into_parts();
    let _disabled = scanner.disable();
    ControllerIdleResetBarrier::new(barrier.map_owner(|()| task))
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn step(self) -> PassiveScanHciRecurringRunnerStep<'runtime, S, CAPACITY> {
        let Self {
            runner,
            order,
            duplicate_filter,
        } = self;
        match runner.step() {
            PassiveScanFirstRunnerStep::WaitControllerTime(runner) => {
                PassiveScanHciRecurringRunnerStep::WaitControllerTime(Self {
                    runner,
                    order,
                    duplicate_filter,
                })
            }
            PassiveScanFirstRunnerStep::Continue(runner) => {
                PassiveScanHciRecurringRunnerStep::Continue(Self {
                    runner,
                    order,
                    duplicate_filter,
                })
            }
            PassiveScanFirstRunnerStep::Running(running) => {
                PassiveScanHciRecurringRunnerStep::Running(PassiveScanHciActiveSession {
                    active: PassiveScanActiveSession::from_first_running(running),
                    order,
                    duplicate_filter,
                })
            }
            PassiveScanFirstRunnerStep::Failed(failure) => {
                PassiveScanHciRecurringRunnerStep::Failed(PassiveScanHciRecurringFailure {
                    failure,
                    order,
                    duplicate_filter,
                })
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn retry(self) -> Result<PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>, Self> {
        let Self {
            failure,
            order,
            duplicate_filter,
        } = self;
        match failure {
            PassiveScanFirstRunnerFailure::Retryable(retry) => Ok(PassiveScanHciRecurringRunner {
                runner: retry.retry(),
                order,
                duplicate_filter,
            }),
            failure => Err(Self {
                failure,
                order,
                duplicate_filter,
            }),
        }
    }

    pub fn retry_cause(&self) -> Option<&PassiveScanFirstRunnerRetryCause<S::Error>> {
        match &self.failure {
            PassiveScanFirstRunnerFailure::Retryable(retry) => Some(retry.cause()),
            _ => None,
        }
    }
}

fn hci_report_event(
    report: LegacyAdvertisingReport,
) -> Result<LeLegacyAdvertisingReportEvent, LeLegacyAdvertisingReportEventError> {
    use oer_bluetooth_hci::bt_hci::param::{AddrKind, BdAddr, LeAdvEventKind};

    use oer_bluetooth_ll::LeDeviceAddressKind;

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
