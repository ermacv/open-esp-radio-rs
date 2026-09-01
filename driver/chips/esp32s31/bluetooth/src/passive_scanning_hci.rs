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
    LeControllerResponsePending, LeControllerResponsePublication, LeLegacyScanningDuplicatePolicy,
};
use open_esp_radio_bluetooth_ll::scanning::{
    LegacyPassiveScanParameters, LegacyPassiveScannerDisabled, LegacyScanDuplicatePolicy,
    LegacyScanInterval, LegacyScanTimingError, LegacyScanWindow,
};

use crate::{
    BluetoothControllerIdleResponsePending, BluetoothControllerPublishedTaskService,
    BluetoothPassiveScanActiveSession, BluetoothPassiveScanFirstRunner,
    BluetoothPassiveScanFirstRunnerFailure, BluetoothPassiveScanFirstRunnerRetryCause,
    BluetoothPassiveScanFirstRunnerStep, BluetoothSchedulerRunInterruptStorage,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

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
                    BluetoothPassiveScanHciActiveSession { active, order },
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
}

impl<'runtime, S, const CAPACITY: usize> BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Recover the disjoint lower radio and opaque HCI-order owners for a
    /// higher executor composition which preserves both.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanActiveSession<'runtime, S, CAPACITY>,
        LeControllerCommandReady<'runtime, ()>,
    ) {
        (self.active, self.order)
    }
}
