//! Shared affine controller-time lifecycle for pre-publication role preparation.
//!
//! Role adapters retain their semantic phase and provide only a lossless
//! rollback function. This engine owns request, bounded recheck, explicit
//! cancellation, and abandoned-request drain. A controller becomes reusable
//! only after the exact cancelled request reports `Drained`.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerTimeAcquisitionError, BluetoothControllerTimeSample,
    controller::time::{
        BluetoothControllerTimeEventError, BluetoothControllerTimePendingCore,
        BluetoothControllerTimePendingCoreStep, BluetoothControllerTimePendingOrphanStep,
        BluetoothControllerTimePendingOwner, BluetoothControllerTimePendingOwnerStep,
        BluetoothControllerTimeRequest,
    },
};

/// Minimal controller-time operations required by the shared preparation engine.
pub(crate) trait BluetoothTimedPreparationController:
    BluetoothControllerTimePendingOwner
{
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeAcquisitionError>;
}

/// Role-specific result of returning one unpublished graph to its runtime.
///
/// This is deliberately not `Result`: restoration failure retains an affine
/// owner and is a sealed lifecycle outcome, not a value-level error.
#[derive(Debug)]
pub(crate) enum BluetoothTimedPreparationRollbackOutcome<R> {
    Restored,
    FailStop(R),
}

impl<R> BluetoothTimedPreparationRollbackOutcome<R> {
    fn into_failure(self) -> Option<R> {
        match self {
            Self::Restored => None,
            Self::FailStop(owner) => Some(owner),
        }
    }
}

type BluetoothTimedPreparationRollback<C, P, R> =
    fn(&mut C, P) -> BluetoothTimedPreparationRollbackOutcome<R>;

#[derive(Debug)]
struct BluetoothTimedPreparationOwner<C, P, R>
where
    C: BluetoothTimedPreparationController,
{
    controller: C,
    phase: Option<P>,
    rollback: BluetoothTimedPreparationRollback<C, P, R>,
    cancelled: Option<BluetoothTimedPreparationRollbackOutcome<R>>,
}

impl<C, P, R> BluetoothControllerTimePendingOwner for BluetoothTimedPreparationOwner<C, P, R>
where
    C: BluetoothTimedPreparationController,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        self.controller.recheck_owned_controller_time(request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        let result = self.controller.cancel_owned_controller_time(request);
        let Some(phase) = self.phase.take() else {
            return Err(BluetoothControllerTimeEventError::RequestMismatch);
        };
        self.cancelled = Some((self.rollback)(&mut self.controller, phase));
        result
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        self.controller.drain_orphan_controller_time()
    }
}

/// Closed permanent-fault class shared by every timed role preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothTimedPreparationFailStopCause {
    ControllerTime(BluetoothControllerTimeAcquisitionError),
    Rollback,
    PhaseOwnership,
}

/// Sealed controller and optional rollback owner after an unsafe transition.
#[must_use = "retain the complete fail-stop owner"]
#[derive(Debug)]
pub(crate) struct BluetoothTimedPreparationFailStop<C, R> {
    cause: BluetoothTimedPreparationFailStopCause,
    controller: C,
    rollback: Option<R>,
}

impl<C, R> BluetoothTimedPreparationFailStop<C, R> {
    pub(crate) const fn cause(&self) -> BluetoothTimedPreparationFailStopCause {
        self.cause
    }

    pub(crate) fn into_parts(self) -> (C, Option<R>) {
        (self.controller, self.rollback)
    }
}

/// One exact in-flight role preparation request.
#[must_use = "recheck or explicitly cancel the timed preparation"]
#[derive(Debug)]
pub(crate) struct BluetoothTimedPreparationPending<C, P, R>
where
    C: BluetoothTimedPreparationController,
{
    core: BluetoothControllerTimePendingCore<BluetoothTimedPreparationOwner<C, P, R>>,
}

/// Result of one bounded sample observation.
#[must_use = "retain Waiting, consume Ready, or retain FailStop"]
pub(crate) enum BluetoothTimedPreparationStep<C, P, R>
where
    C: BluetoothTimedPreparationController,
{
    Waiting(BluetoothTimedPreparationPending<C, P, R>),
    Ready {
        controller: C,
        phase: P,
        sample: BluetoothControllerTimeSample,
    },
    FailStop(BluetoothTimedPreparationFailStop<C, R>),
}

/// Controller whose role graph is restored while its abandoned request drains.
#[must_use = "drain the exact abandoned request before controller reuse"]
#[derive(Debug)]
pub(crate) struct BluetoothTimedPreparationCancellationPending<C>
where
    C: BluetoothTimedPreparationController,
{
    controller: C,
}

/// One bounded abandoned-request observation.
#[must_use = "retain Waiting, consume Recovered, or retain FailStop"]
pub(crate) enum BluetoothTimedPreparationCancellationStep<C, R>
where
    C: BluetoothTimedPreparationController,
{
    Waiting(BluetoothTimedPreparationCancellationPending<C>),
    Recovered(C),
    FailStop(BluetoothTimedPreparationFailStop<C, R>),
}

const fn event_error(
    error: BluetoothControllerTimeEventError,
) -> BluetoothControllerTimeAcquisitionError {
    match error {
        BluetoothControllerTimeEventError::RequestMismatch => {
            BluetoothControllerTimeAcquisitionError::RequestMismatch
        }
        BluetoothControllerTimeEventError::OwnershipLost => {
            BluetoothControllerTimeAcquisitionError::OwnershipLost
        }
        BluetoothControllerTimeEventError::Faulted => {
            BluetoothControllerTimeAcquisitionError::Faulted
        }
    }
}

impl<C, P, R> BluetoothTimedPreparationPending<C, P, R>
where
    C: BluetoothTimedPreparationController,
{
    pub(crate) fn begin(
        mut controller: C,
        phase: P,
        rollback: BluetoothTimedPreparationRollback<C, P, R>,
    ) -> Result<Self, BluetoothTimedPreparationFailStop<C, R>> {
        let request = match controller.request_timed_preparation_sample() {
            Ok(request) => request,
            Err(error) => {
                let rollback = rollback(&mut controller, phase).into_failure();
                return Err(BluetoothTimedPreparationFailStop {
                    cause: BluetoothTimedPreparationFailStopCause::ControllerTime(error),
                    controller,
                    rollback,
                });
            }
        };
        Ok(Self {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothTimedPreparationOwner {
                    controller,
                    phase: Some(phase),
                    rollback,
                    cancelled: None,
                },
                request,
            ),
        })
    }

    pub(crate) fn recheck(self) -> BluetoothTimedPreparationStep<C, P, R> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothTimedPreparationStep::Waiting(Self { core });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let Some(phase) = owner.phase.take() else {
                    return BluetoothTimedPreparationStep::FailStop(
                        BluetoothTimedPreparationFailStop {
                            cause: BluetoothTimedPreparationFailStopCause::PhaseOwnership,
                            controller: owner.controller,
                            rollback: None,
                        },
                    );
                };
                let rollback = (owner.rollback)(&mut owner.controller, phase).into_failure();
                return BluetoothTimedPreparationStep::FailStop(
                    BluetoothTimedPreparationFailStop {
                        cause: BluetoothTimedPreparationFailStopCause::ControllerTime(event_error(
                            error,
                        )),
                        controller: owner.controller,
                        rollback,
                    },
                );
            }
        };
        let Some(phase) = owner.phase.take() else {
            return BluetoothTimedPreparationStep::FailStop(BluetoothTimedPreparationFailStop {
                cause: BluetoothTimedPreparationFailStopCause::PhaseOwnership,
                controller: owner.controller,
                rollback: None,
            });
        };
        BluetoothTimedPreparationStep::Ready {
            controller: owner.controller,
            phase,
            sample,
        }
    }

    pub(crate) fn cancel(
        self,
    ) -> Result<
        BluetoothTimedPreparationCancellationPending<C>,
        BluetoothTimedPreparationFailStop<C, R>,
    > {
        match self.core.cancel() {
            Ok(mut owner) => match owner.cancelled.take() {
                Some(BluetoothTimedPreparationRollbackOutcome::Restored) => {
                    Ok(BluetoothTimedPreparationCancellationPending {
                        controller: owner.controller,
                    })
                }
                Some(BluetoothTimedPreparationRollbackOutcome::FailStop(rollback)) => {
                    Err(BluetoothTimedPreparationFailStop {
                        cause: BluetoothTimedPreparationFailStopCause::Rollback,
                        controller: owner.controller,
                        rollback: Some(rollback),
                    })
                }
                None => Err(BluetoothTimedPreparationFailStop {
                    cause: BluetoothTimedPreparationFailStopCause::PhaseOwnership,
                    controller: owner.controller,
                    rollback: None,
                }),
            },
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                Err(BluetoothTimedPreparationFailStop {
                    cause: BluetoothTimedPreparationFailStopCause::ControllerTime(event_error(
                        error,
                    )),
                    controller: owner.controller,
                    rollback: owner
                        .cancelled
                        .take()
                        .and_then(BluetoothTimedPreparationRollbackOutcome::into_failure),
                })
            }
        }
    }
}

impl<C> BluetoothTimedPreparationCancellationPending<C>
where
    C: BluetoothTimedPreparationController,
{
    pub(crate) fn recheck<R>(mut self) -> BluetoothTimedPreparationCancellationStep<C, R> {
        match self.controller.drain_orphan_controller_time() {
            Ok(BluetoothControllerTimePendingOrphanStep::Waiting) => {
                BluetoothTimedPreparationCancellationStep::Waiting(self)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Drained) => {
                BluetoothTimedPreparationCancellationStep::Recovered(self.controller)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Idle) => {
                BluetoothTimedPreparationCancellationStep::FailStop(
                    BluetoothTimedPreparationFailStop {
                        cause: BluetoothTimedPreparationFailStopCause::PhaseOwnership,
                        controller: self.controller,
                        rollback: None,
                    },
                )
            }
            Err(error) => BluetoothTimedPreparationCancellationStep::FailStop(
                BluetoothTimedPreparationFailStop {
                    cause: BluetoothTimedPreparationFailStopCause::ControllerTime(event_error(
                        error,
                    )),
                    controller: self.controller,
                    rollback: None,
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests;
