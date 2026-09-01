//! HCI order composition for the ESP32-S31 passive scanner.
//!
//! The lower scanner runner owns LL state and hardware. This module only keeps
//! the accepted standard Enable command affine beside that runner until `RUN`,
//! then transfers the returned command authority into the active scan session.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerDeferredLegacyScanningStart, LeControllerEndpointMismatch,
    LeControllerResponsePending, LeControllerResponsePublication, LeLegacyAdvertisingReportEvent,
    LeLegacyAdvertisingReportEventError, LeLegacyAdvertisingReportPublication,
    LeLegacyScanningDuplicatePolicy,
};
use open_esp_radio_bluetooth_ll::scanning::{
    LegacyAdvertisingDuplicateFilter, LegacyAdvertisingReport, LegacyAdvertisingReportKind,
    LegacyAdvertisingReportParseError, LegacyPassiveScanParameters, LegacyPassiveScannerDisabled,
    LegacyScanDuplicatePolicy, LegacyScanInterval, LegacyScanTimingError, LegacyScanWindow,
};

use crate::{
    BluetoothControllerIdleResponsePending, BluetoothControllerPublishedTaskService,
    BluetoothPassiveScanActiveFault, BluetoothPassiveScanActiveSession,
    BluetoothPassiveScanActiveStep, BluetoothPassiveScanActiveWait,
    BluetoothPassiveScanEventCpuOwned, BluetoothPassiveScanFirstRunner,
    BluetoothPassiveScanFirstRunnerFailure, BluetoothPassiveScanFirstRunnerRetryCause,
    BluetoothPassiveScanFirstRunnerStep, BluetoothSchedulerRunInterruptStorage,
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
                    failure @ BluetoothPassiveScanFirstRunnerFailure::Retryable(_) => {
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
    BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn has_pending_event(&self) -> bool {
        self.pending_event.is_some()
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
