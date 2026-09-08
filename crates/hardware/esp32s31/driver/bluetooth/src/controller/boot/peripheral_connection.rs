//! Peripheral-connection controller preparation, completion, and recurrence.

use core::ops::ControlFlow;

use crate::{
    controller::time::{
        ControllerTimeEventError, ControllerTimePendingCore, ControllerTimePendingCoreStep,
        ControllerTimePendingOrphanStep, ControllerTimePendingOwner,
        ControllerTimePendingOwnerStep, ControllerTimeRequest,
    },
    scheduler::core::PeripheralConnectionSchedulerCompletionClassification,
};

use super::{
    ControllerPublishedTaskService, ControllerSchedulerEpochRetained, ControllerSchedulerNowReady,
    controller_time_begin_error, controller_time_event_error,
};

/// Result of closing one recycled connection event against capture evidence.
#[must_use = "retain the unchanged retry owner or completed connection"]
pub enum PeripheralConnectionCompletionStep {
    SchedulerEpochUnavailable(crate::scheduler::PeripheralConnectionSchedulerRecycled),
    Completed(crate::scheduler::PeripheralConnectionSchedulerCompleted),
}

/// Production attempt to enter recurring preparation from one exact completion.
#[must_use = "retain the prepared candidate or the exact retry owner"]
pub enum PeripheralConnectionRecurringCandidateStep {
    Prepared(crate::scheduler::PeripheralConnectionRecurringEventCandidate),
    SchedulerEpochUnavailable(PeripheralConnectionRecurringRetry),
    TimingPolicyUnavailable(PeripheralConnectionRecurringRetry),
    Rejected {
        error: crate::scheduler::PeripheralConnectionRecurringCandidateError,
        retry: PeripheralConnectionRecurringRetry,
    },
}

/// Exact completed connection and typed event distance restored before admission.
#[must_use = "retry recurrence or retain the exact completed connection"]
pub struct PeripheralConnectionRecurringRetry {
    completed: crate::scheduler::PeripheralConnectionSchedulerCompleted,
    delta: oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
}

impl PeripheralConnectionRecurringRetry {
    fn new(
        completed: crate::scheduler::PeripheralConnectionSchedulerCompleted,
        delta: oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) -> Self {
        Self { completed, delta }
    }

    fn from_cancelled(
        cancelled: (
            crate::scheduler::PeripheralConnectionSchedulerCompleted,
            oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
        ),
    ) -> Self {
        let (completed, delta) = cancelled;
        Self::new(completed, delta)
    }

    pub const fn completed(&self) -> &crate::scheduler::PeripheralConnectionSchedulerCompleted {
        &self.completed
    }

    pub const fn delta(&self) -> oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta {
        self.delta
    }

    pub fn into_parts(
        self,
    ) -> (
        crate::scheduler::PeripheralConnectionSchedulerCompleted,
        oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) {
        (self.completed, self.delta)
    }
}

/// Result after a fresh sequence sample closes recurring preparation.
#[must_use = "retain the task service and prepared or retryable recurring owner"]
pub enum PeripheralConnectionRecurringSequenceCompletion<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    },
    EventRejected {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::scheduler::PeripheralConnectionRecurringEventPreparationFailure,
    },
    EmptyListRejected {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergeFailure,
    },
}

/// Finite reason a checked-out connection event returned to CPU ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionControllerPreparationError {
    TimingWindow,
    Event(crate::scheduler::core::PeripheralConnectionFirstEventPreparationError),
    EmptyList(crate::scheduler::SchedulerEmptyListMergeError),
}

/// Permanent controller-time fault while preparing an accepted connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralConnectionControllerPreparationFailStopCause {
    ControllerTime(crate::scheduler::ControllerTimeAcquisitionError),
    PhaseOwnership,
}

enum PeripheralConnectionControllerPreparationPhase {
    Sequence {
        admitted: crate::scheduler::core::PeripheralConnectionFirstPreSequence,
        packet: oer_esp32s31_bluetooth_memory::LeReceivedPdu,
    },
}

struct PeripheralConnectionControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<PeripheralConnectionControllerPreparationPhase>,
}

/// One exact in-flight sequence-deadline observation for a connection event.
#[must_use = "recheck or explicitly cancel the exact connection time request"]
pub struct PeripheralConnectionControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    core: ControllerTimePendingCore<
        PeripheralConnectionControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

/// Sequence-authorized first event plus its causal accepted packet.
#[must_use = "publish the first event or retain both the merge and causal packet"]
pub(crate) struct PeripheralConnectionControllerPrepared {
    pub(crate) merged: crate::scheduler::PeripheralConnectionEmptySchedulerMergePrepared,
    pub(crate) packet: oer_esp32s31_bluetooth_memory::LeReceivedPdu,
}

/// Sealed Controller and accepted owner after a permanent preparation fault.
#[must_use = "the faulted Controller and accepted connection owner must remain sealed"]
pub(crate) struct PeripheralConnectionControllerPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    cause: PeripheralConnectionControllerPreparationFailStopCause,
    _controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    _accepted: Option<crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn cause(&self) -> PeripheralConnectionControllerPreparationFailStopCause {
        self.cause
    }
}

/// Terminal connection preparation with the exact task service retained.
#[must_use = "the task owner and connection outcome must be handled together"]
pub(crate) enum PeripheralConnectionControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        prepared: PeripheralConnectionControllerPrepared,
    },
    Recovered {
        controller: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        error: PeripheralConnectionControllerPreparationError,
        accepted: crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest,
    },
    FailStop(PeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of one bounded connection sequence-time observation.
#[must_use = "retain Pending or consume the terminal task and connection result"]
pub(crate) enum PeripheralConnectionControllerPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Pending(PeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>),
    Terminal(PeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn prepared(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        prepared: PeripheralConnectionControllerPrepared,
    ) -> PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        PeripheralConnectionControllerPreparationStep::Terminal(
            PeripheralConnectionControllerPreparationTerminal::Prepared {
                controller: ControllerSchedulerEpochRetained { controller },
                prepared,
            },
        )
    }

    fn recovered(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: PeripheralConnectionControllerPreparationError,
        accepted: crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest,
    ) -> PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        PeripheralConnectionControllerPreparationStep::Terminal(
            PeripheralConnectionControllerPreparationTerminal::Recovered {
                controller: ControllerSchedulerEpochRetained { controller },
                error,
                accepted,
            },
        )
    }

    fn fail_stop(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: PeripheralConnectionControllerPreparationFailStopCause,
        accepted: Option<crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest>,
    ) -> PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        PeripheralConnectionControllerPreparationStep::Terminal(
            PeripheralConnectionControllerPreparationTerminal::FailStop(
                PeripheralConnectionControllerPreparationFailStop {
                    cause,
                    _controller: ControllerSchedulerEpochRetained { controller },
                    _accepted: accepted,
                },
            ),
        )
    }

    /// Perform one bounded observation of the connection sequence deadline.
    pub fn recheck(
        self,
    ) -> PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(ControllerTimePendingCoreStep::Waiting(core)) => {
                return PeripheralConnectionControllerPreparationStep::Pending(Self { core });
            }
            Ok(ControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let accepted = owner.phase.take().map(|phase| {
                    owner
                        .controller
                        .cancel_peripheral_connection_preparation_phase(phase)
                });
                return Self::fail_stop(
                    owner.controller,
                    PeripheralConnectionControllerPreparationFailStopCause::ControllerTime(
                        controller_time_event_error(error),
                    ),
                    accepted,
                );
            }
        };
        let Some(phase) = owner.phase.take() else {
            return Self::fail_stop(
                owner.controller,
                PeripheralConnectionControllerPreparationFailStopCause::PhaseOwnership,
                None,
            );
        };
        let mut controller = owner.controller;
        match phase {
            PeripheralConnectionControllerPreparationPhase::Sequence { admitted, packet } => {
                let default_tx_power = controller
                    .peripheral_connection_resources
                    .default_tx_power_dbm();
                let direction_finding_workspace = controller.direction_finding_workspace;
                let prepared = match controller
                    .runtime
                    .prepare_peripheral_connection_first_event(
                        admitted,
                        crate::scheduler::core::PeripheralConnectionSequenceObservation { sample },
                        default_tx_power,
                        direction_finding_workspace,
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = failure.into_candidate().cancel();
                        return Self::recovered(
                            controller,
                            PeripheralConnectionControllerPreparationError::Event(error),
                            crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest::new(
                                allocation,
                                connection,
                                packet,
                            ),
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_peripheral_connection_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::prepared(
                        controller,
                        PeripheralConnectionControllerPrepared { merged, packet },
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = controller
                            .runtime
                            .cancel_peripheral_connection_first_event(failure.into_prepared());
                        Self::recovered(
                            controller,
                            PeripheralConnectionControllerPreparationError::EmptyList(
                                error,
                            ),
                            crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest::new(
                                allocation,
                                connection,
                                packet,
                            ),
                        )
                    }
                }
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize> ControllerTimePendingOwner
    for PeripheralConnectionControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        ControllerTimePendingOwner::recheck_owned_controller_time(&mut self.controller, request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        ControllerTimePendingOwner::cancel_owned_controller_time(&mut self.controller, request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        ControllerTimePendingOwner::drain_orphan_controller_time(&mut self.controller)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn cancel_peripheral_connection_preparation_phase(
        &mut self,
        phase: PeripheralConnectionControllerPreparationPhase,
    ) -> crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest {
        let (allocation, connection, packet) = match phase {
            PeripheralConnectionControllerPreparationPhase::Sequence { admitted, packet } => {
                let (allocation, connection) = self
                    .runtime
                    .cancel_peripheral_connection_first_pre_sequence(admitted);
                (allocation, connection, packet)
            }
        };
        crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest::new(
            allocation, connection, packet,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the Controller and restored connection allocation"
    )]
    fn begin_peripheral_connection_preparation_time(
        mut self,
        phase: PeripheralConnectionControllerPreparationPhase,
    ) -> Result<
        PeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        PeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let accepted = self.cancel_peripheral_connection_preparation_phase(phase);
                return Err(PeripheralConnectionControllerPreparationTerminal::FailStop(
                    PeripheralConnectionControllerPreparationFailStop {
                        cause:
                            PeripheralConnectionControllerPreparationFailStopCause::ControllerTime(
                                controller_time_begin_error(error),
                            ),
                        _controller: ControllerSchedulerEpochRetained { controller: self },
                        _accepted: Some(accepted),
                    },
                ));
            }
        };
        Ok(PeripheralConnectionControllerPreparationPending {
            core: ControllerTimePendingCore::new(
                PeripheralConnectionControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                },
                request,
            ),
        })
    }

    /// Join this powered epoch's global DF workspace to one connection image.
    #[allow(
        dead_code,
        reason = "the next peripheral scheduler-publication transition consumes this state"
    )]
    pub(crate) fn install_peripheral_connection_direction_finding_workspace(
        &self,
        prepared: crate::le::peripheral::connection::PeripheralConnectionFirstEventFieldsPrepared,
    ) -> crate::le::peripheral::connection::PeripheralConnectionFirstEventDirectionFindingPrepared
    {
        prepared.install_direction_finding_workspace(self.direction_finding_workspace)
    }

    /// Close one recycled LE 1M connection event against its capture evidence.
    ///
    /// An absent capture closes the event as missed without requiring an epoch.
    /// An available capture is normalized and closes the event as observed. This
    /// pure transition neither samples current time nor interprets hardware
    /// status, and it does not schedule recurrence.
    pub fn complete_peripheral_connection_event(
        &mut self,
        recycled: crate::scheduler::PeripheralConnectionSchedulerRecycled,
    ) -> PeripheralConnectionCompletionStep {
        let epoch = *self.scheduler_epoch;
        match recycled.classify_completion(|captured| {
            epoch.map(|epoch| {
                self.ble_phy_timing
                    .complete_le_1m_peripheral_connection_packet_start(epoch, captured)
            })
        }) {
            PeripheralConnectionSchedulerCompletionClassification::NormalizationUnavailable(
                recycled,
            ) => PeripheralConnectionCompletionStep::SchedulerEpochUnavailable(recycled),
            PeripheralConnectionSchedulerCompletionClassification::Completed(completed) => {
                PeripheralConnectionCompletionStep::Completed(completed)
            }
        }
    }

    pub(crate) fn process_peripheral_control(
        &mut self,
        completed: &mut crate::scheduler::PeripheralConnectionSchedulerCompleted,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
    ) -> Result<(), oer_bluetooth_ll::control::LePeripheralControlError> {
        completed.process_control(
            control,
            self.peripheral_connection_resources
                .config()
                .version_information(),
        )
    }

    /// Build one provisional recurrence from the real completed connection owner.
    ///
    /// The default runtime remains fail-closed because neither main-XTAL
    /// selection nor PHY initialization proves a worst-case local SCA. A board
    /// may opt in through its connection runtime config with an explicit ppm
    /// bound; that same config explicitly selects the reviewed software-WW path.
    pub fn prepare_peripheral_connection_recurring_candidate(
        &mut self,
        completed: crate::scheduler::PeripheralConnectionSchedulerCompleted,
        delta: oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) -> PeripheralConnectionRecurringCandidateStep {
        let Some(epoch) = *self.scheduler_epoch else {
            return PeripheralConnectionRecurringCandidateStep::SchedulerEpochUnavailable(
                PeripheralConnectionRecurringRetry::new(completed, delta),
            );
        };
        let Some(timing_policy) = self
            .peripheral_connection_resources
            .config()
            .recurring_timing_policy()
        else {
            return PeripheralConnectionRecurringCandidateStep::TimingPolicyUnavailable(
                PeripheralConnectionRecurringRetry::new(completed, delta),
            );
        };
        match completed.prepare_recurring_event_candidate(
            delta,
            epoch,
            self.runtime.scheduler_config(),
            timing_policy,
        ) {
            ControlFlow::Continue(candidate) => {
                PeripheralConnectionRecurringCandidateStep::Prepared(candidate)
            }
            ControlFlow::Break(failure) => {
                let error = failure.error();
                let (completed, delta) = failure.into_retry_parts();
                let retry = PeripheralConnectionRecurringRetry::new(completed, delta);
                PeripheralConnectionRecurringCandidateStep::Rejected { error, retry }
            }
        }
    }

    /// Reserve one provisional recurrence before acquiring its sequence sample.
    pub fn admit_peripheral_connection_recurring_candidate(
        &mut self,
        candidate: crate::scheduler::PeripheralConnectionRecurringEventCandidate,
    ) -> ControlFlow<
        crate::scheduler::PeripheralConnectionRecurringEventPreparationFailure,
        crate::scheduler::PeripheralConnectionRecurringPreSequence,
    > {
        self.runtime
            .admit_peripheral_connection_recurring_event(candidate)
    }

    /// Cancel one candidate which owns no scheduler reservation yet.
    pub fn cancel_peripheral_connection_recurring_candidate(
        &mut self,
        candidate: crate::scheduler::PeripheralConnectionRecurringEventCandidate,
    ) -> PeripheralConnectionRecurringRetry {
        PeripheralConnectionRecurringRetry::from_cancelled(candidate.cancel())
    }

    /// Release a recurring reservation before sequence authorization.
    pub fn cancel_peripheral_connection_recurring_pre_sequence(
        &mut self,
        admitted: crate::scheduler::PeripheralConnectionRecurringPreSequence,
    ) -> PeripheralConnectionRecurringRetry {
        let cancelled = self
            .runtime
            .cancel_peripheral_connection_recurring_pre_sequence(admitted);
        PeripheralConnectionRecurringRetry::from_cancelled(cancelled)
    }

    /// Release a sequence-authorized recurring event and its timeline slot.
    pub fn cancel_peripheral_connection_recurring_event(
        &mut self,
        prepared: crate::scheduler::PeripheralConnectionRecurringEventPrepared,
    ) -> PeripheralConnectionRecurringRetry {
        let cancelled = self
            .runtime
            .cancel_peripheral_connection_recurring_event(prepared);
        PeripheralConnectionRecurringRetry::from_cancelled(cancelled)
    }

    /// Retry the infallible detach plus empty-list identity merge.
    pub fn prepare_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        prepared: crate::scheduler::PeripheralConnectionRecurringEventPrepared,
    ) -> ControlFlow<
        crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergeFailure,
        crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    > {
        self.runtime
            .prepare_peripheral_connection_recurring_empty_list_merge(prepared)
    }

    /// Undo an unpublished empty-list merge while preserving its reservation.
    pub fn cancel_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        merged: crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> ControlFlow<
        crate::scheduler::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        crate::scheduler::PeripheralConnectionRecurringEventPrepared,
    > {
        self.runtime
            .cancel_peripheral_connection_recurring_empty_list_merge(merged)
    }

    /// Publish selector-two RX memory and the first connection scheduler head.
    ///
    /// A head-validation rejection retains a retryable merge. An RX proof
    /// mismatch follows irreversible MMIO and is sealed in the returned
    /// failure without a rollback operation.
    #[allow(
        clippy::result_large_err,
        reason = "each rejection retains the complete retryable or fail-stop connection ownership"
    )]
    pub fn publish_peripheral_connection_scheduler_head(
        &mut self,
        merged: crate::scheduler::PeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        crate::scheduler::PeripheralConnectionSchedulerHeadPublished,
        crate::scheduler::PeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        self.runtime
            .publish_peripheral_connection_scheduler_head(merged)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Apply this fresh sequence sample to one reserved connection recurrence.
    pub fn finish_peripheral_connection_recurring_event(
        self,
        admitted: crate::scheduler::PeripheralConnectionRecurringPreSequence,
    ) -> PeripheralConnectionRecurringSequenceCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            mut controller,
            sample,
            ..
        } = self;
        let prepared = match controller
            .runtime
            .prepare_peripheral_connection_recurring_event(
                admitted,
                crate::scheduler::core::PeripheralConnectionSequenceObservation { sample },
            ) {
            ControlFlow::Continue(prepared) => prepared,
            ControlFlow::Break(failure) => {
                return PeripheralConnectionRecurringSequenceCompletion::EventRejected {
                    task: controller,
                    failure,
                };
            }
        };
        match controller
            .runtime
            .prepare_peripheral_connection_recurring_empty_list_merge(prepared)
        {
            ControlFlow::Continue(merged) => {
                PeripheralConnectionRecurringSequenceCompletion::Prepared {
                    task: controller,
                    merged,
                }
            }
            ControlFlow::Break(failure) => {
                PeripheralConnectionRecurringSequenceCompletion::EmptyListRejected {
                    task: controller,
                    failure,
                }
            }
        }
    }

    /// Begin the first peripheral connection event from its accepted request.
    ///
    /// The current sample authorizes timeline admission. A distinct later
    /// request authorizes descriptor sequencing after overlap resolution. Every
    /// rejection returns the exact accepted request. The allocation is never
    /// checked out a second time: connectable advertising already transferred
    /// it with the causal `CONNECT_IND` packet.
    pub(crate) fn begin_peripheral_connection_first_event(
        self,
        accepted: crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest,
        packet_start: crate::le::peripheral::Le1MPacketStartTiming,
    ) -> PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            mut controller,
            epoch,
            sample,
        } = self;
        let (prepared, packet) = accepted.into_first_event_parts(packet_start);
        let candidate = match prepared
            .project_scheduler_window(epoch, controller.runtime.scheduler_config())
        {
            Ok(candidate) => candidate,
            Err(prepared) => {
                let (allocation, connection) = prepared.cancel();
                return PeripheralConnectionControllerPreparationStep::Terminal(
                    PeripheralConnectionControllerPreparationTerminal::Recovered {
                        controller: ControllerSchedulerEpochRetained { controller },
                        error: PeripheralConnectionControllerPreparationError::TimingWindow,
                        accepted: crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest::new(
                            allocation,
                            connection,
                            packet,
                        ),
                    },
                );
            }
        };
        let admitted = match controller.runtime.admit_peripheral_connection_first_event(
            candidate,
            crate::scheduler::core::PeripheralConnectionAdmissionObservation { sample },
        ) {
            Ok(admitted) => admitted,
            Err(failure) => {
                let error = failure.error();
                let (allocation, connection) = failure.into_candidate().cancel();
                return PeripheralConnectionControllerPreparationStep::Terminal(
                    PeripheralConnectionControllerPreparationTerminal::Recovered {
                        controller: ControllerSchedulerEpochRetained { controller },
                        error: PeripheralConnectionControllerPreparationError::Event(
                            error,
                        ),
                        accepted: crate::le::peripheral::connection::PeripheralConnectionAcceptedRequest::new(
                            allocation,
                            connection,
                            packet,
                        ),
                    },
                );
            }
        };
        match controller.begin_peripheral_connection_preparation_time(
            PeripheralConnectionControllerPreparationPhase::Sequence { admitted, packet },
        ) {
            Ok(pending) => PeripheralConnectionControllerPreparationStep::Pending(pending),
            Err(terminal) => PeripheralConnectionControllerPreparationStep::Terminal(terminal),
        }
    }
}
