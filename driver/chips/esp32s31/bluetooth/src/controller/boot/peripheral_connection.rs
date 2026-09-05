//! Peripheral-connection controller preparation, completion, and recurrence.

use core::ops::ControlFlow;

use super::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerEpochRetained,
    BluetoothControllerSchedulerNowReady, controller_time_begin_error, controller_time_event_error,
};
use crate::controller::time::{
    BluetoothControllerTimeEventError, BluetoothControllerTimePendingCore,
    BluetoothControllerTimePendingCoreStep, BluetoothControllerTimePendingOrphanStep,
    BluetoothControllerTimePendingOwner, BluetoothControllerTimePendingOwnerStep,
    BluetoothControllerTimeRequest,
};
use crate::scheduler::core::BluetoothPeripheralConnectionSchedulerCompletionClassification;

/// Result of closing one recycled connection event against capture evidence.
#[must_use = "retain the unchanged retry owner or completed connection"]
pub enum BluetoothPeripheralConnectionCompletionStep {
    SchedulerEpochUnavailable(crate::BluetoothPeripheralConnectionSchedulerRecycled),
    Completed(crate::BluetoothPeripheralConnectionSchedulerCompleted),
}

/// Production attempt to enter recurring preparation from one exact completion.
#[must_use = "retain the prepared candidate or the exact retry owner"]
pub enum BluetoothPeripheralConnectionRecurringCandidateStep {
    Prepared(crate::BluetoothPeripheralConnectionRecurringEventCandidate),
    SchedulerEpochUnavailable(BluetoothPeripheralConnectionRecurringRetry),
    TimingPolicyUnavailable(BluetoothPeripheralConnectionRecurringRetry),
    Rejected {
        error: crate::BluetoothPeripheralConnectionRecurringCandidateError,
        retry: BluetoothPeripheralConnectionRecurringRetry,
    },
}

/// Exact completed connection and typed event distance restored before admission.
#[must_use = "retry recurrence or retain the exact completed connection"]
pub struct BluetoothPeripheralConnectionRecurringRetry {
    completed: crate::BluetoothPeripheralConnectionSchedulerCompleted,
    delta: open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
}

impl BluetoothPeripheralConnectionRecurringRetry {
    fn new(
        completed: crate::BluetoothPeripheralConnectionSchedulerCompleted,
        delta: open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) -> Self {
        Self { completed, delta }
    }

    fn from_cancelled(
        cancelled: (
            crate::BluetoothPeripheralConnectionSchedulerCompleted,
            open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
        ),
    ) -> Self {
        let (completed, delta) = cancelled;
        Self::new(completed, delta)
    }

    pub const fn completed(&self) -> &crate::BluetoothPeripheralConnectionSchedulerCompleted {
        &self.completed
    }

    pub const fn delta(
        &self,
    ) -> open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta {
        self.delta
    }

    pub fn into_parts(
        self,
    ) -> (
        crate::BluetoothPeripheralConnectionSchedulerCompleted,
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) {
        (self.completed, self.delta)
    }
}

/// Result after a fresh sequence sample closes recurring preparation.
#[must_use = "retain the task service and prepared or retryable recurring owner"]
pub enum BluetoothPeripheralConnectionRecurringSequenceCompletion<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    },
    EventRejected {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::BluetoothPeripheralConnectionRecurringEventPreparationFailure,
    },
    EmptyListRejected {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        failure: crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
    },
}

/// Finite reason a checked-out connection event returned to CPU ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionControllerPreparationError {
    TimingWindow,
    Event(crate::scheduler::core::BluetoothPeripheralConnectionFirstEventPreparationError),
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Permanent controller-time fault while preparing an accepted connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothPeripheralConnectionControllerPreparationFailStopCause {
    ControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    PhaseOwnership,
}

enum BluetoothPeripheralConnectionControllerPreparationPhase {
    Sequence {
        admitted: crate::scheduler::core::BluetoothPeripheralConnectionFirstPreSequence,
        packet: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedPdu,
    },
}

struct BluetoothPeripheralConnectionControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothPeripheralConnectionControllerPreparationPhase>,
}

/// One exact in-flight sequence-deadline observation for a connection event.
#[must_use = "recheck or explicitly cancel the exact connection time request"]
pub struct BluetoothPeripheralConnectionControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    core: BluetoothControllerTimePendingCore<
        BluetoothPeripheralConnectionControllerPreparationTimeOwner<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    >,
}

/// Sequence-authorized first event plus its causal accepted packet.
#[must_use = "publish the first event or retain both the merge and causal packet"]
pub(crate) struct BluetoothPeripheralConnectionControllerPrepared {
    pub(crate) merged: crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    pub(crate) packet: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedPdu,
}

/// Sealed Controller and accepted owner after a permanent preparation fault.
#[must_use = "the faulted Controller and accepted connection owner must remain sealed"]
pub(crate) struct BluetoothPeripheralConnectionControllerPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    cause: BluetoothPeripheralConnectionControllerPreparationFailStopCause,
    _controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    _accepted: Option<crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) const fn cause(
        &self,
    ) -> BluetoothPeripheralConnectionControllerPreparationFailStopCause {
        self.cause
    }
}

/// Terminal connection preparation with the exact task service retained.
#[must_use = "the task owner and connection outcome must be handled together"]
pub(crate) enum BluetoothPeripheralConnectionControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Prepared {
        controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        prepared: BluetoothPeripheralConnectionControllerPrepared,
    },
    Recovered {
        controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPeripheralConnectionControllerPreparationError,
        accepted: crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest,
    },
    FailStop(
        BluetoothPeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

/// Result of one bounded connection sequence-time observation.
#[must_use = "retain Pending or consume the terminal task and connection result"]
pub(crate) enum BluetoothPeripheralConnectionControllerPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Pending(
        BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Terminal(
        BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn prepared(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        prepared: BluetoothPeripheralConnectionControllerPrepared,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
            BluetoothPeripheralConnectionControllerPreparationTerminal::Prepared {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                prepared,
            },
        )
    }

    fn recovered(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPeripheralConnectionControllerPreparationError,
        accepted: crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
            BluetoothPeripheralConnectionControllerPreparationTerminal::Recovered {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                error,
                accepted,
            },
        )
    }

    fn fail_stop(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: BluetoothPeripheralConnectionControllerPreparationFailStopCause,
        accepted: Option<
            crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest,
        >,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
            BluetoothPeripheralConnectionControllerPreparationTerminal::FailStop(
                BluetoothPeripheralConnectionControllerPreparationFailStop {
                    cause,
                    _controller: BluetoothControllerSchedulerEpochRetained { controller },
                    _accepted: accepted,
                },
            ),
        )
    }

    /// Perform one bounded observation of the connection sequence deadline.
    pub fn recheck(
        self,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        let (mut owner, sample) = match self.core.recheck() {
            Ok(BluetoothControllerTimePendingCoreStep::Waiting(core)) => {
                return BluetoothPeripheralConnectionControllerPreparationStep::Pending(Self {
                    core,
                });
            }
            Ok(BluetoothControllerTimePendingCoreStep::Ready { owner, sample }) => (owner, sample),
            Err(failure) => {
                let (mut owner, error) = failure.into_parts();
                let accepted = owner.phase.take().map(|phase| {
                    owner
                        .controller
                        .cancel_peripheral_connection_preparation_phase(phase)
                });
                return Self::fail_stop(
                    owner.controller,
                    BluetoothPeripheralConnectionControllerPreparationFailStopCause::ControllerTime(
                        controller_time_event_error(error),
                    ),
                    accepted,
                );
            }
        };
        let Some(phase) = owner.phase.take() else {
            return Self::fail_stop(
                owner.controller,
                BluetoothPeripheralConnectionControllerPreparationFailStopCause::PhaseOwnership,
                None,
            );
        };
        let mut controller = owner.controller;
        match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence {
                admitted,
                packet,
            } => {
                let default_tx_power = controller
                    .peripheral_connection_resources
                    .default_tx_power_dbm();
                let direction_finding_workspace = controller.direction_finding_workspace;
                let prepared = match controller
                    .runtime
                    .prepare_peripheral_connection_first_event(
                        admitted,
                        crate::scheduler::core::BluetoothPeripheralConnectionSequenceObservation {
                            sample,
                        },
                        default_tx_power,
                        direction_finding_workspace,
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = failure.into_candidate().cancel();
                        return Self::recovered(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationError::Event(error),
                            crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest::new(
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
                        BluetoothPeripheralConnectionControllerPrepared { merged, packet },
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = controller
                            .runtime
                            .cancel_peripheral_connection_first_event(failure.into_prepared());
                        Self::recovered(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationError::EmptyList(
                                error,
                            ),
                            crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest::new(
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

impl<'runtime, S, const SCHEDULER_CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothPeripheralConnectionControllerPreparationTimeOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        BluetoothControllerTimePendingOwner::recheck_owned_controller_time(
            &mut self.controller,
            request,
        )
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        BluetoothControllerTimePendingOwner::cancel_owned_controller_time(
            &mut self.controller,
            request,
        )
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        BluetoothControllerTimePendingOwner::drain_orphan_controller_time(&mut self.controller)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    fn cancel_peripheral_connection_preparation_phase(
        &mut self,
        phase: BluetoothPeripheralConnectionControllerPreparationPhase,
    ) -> crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest {
        let (allocation, connection, packet) = match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence {
                admitted,
                packet,
            } => {
                let (allocation, connection) = self
                    .runtime
                    .cancel_peripheral_connection_first_pre_sequence(admitted);
                (allocation, connection, packet)
            }
        };
        crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest::new(
            allocation, connection, packet,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the Controller and restored connection allocation"
    )]
    fn begin_peripheral_connection_preparation_time(
        mut self,
        phase: BluetoothPeripheralConnectionControllerPreparationPhase,
    ) -> Result<
        BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let request = match self.runtime.request_controller_time() {
            Ok(request) => request,
            Err(error) => {
                let accepted = self.cancel_peripheral_connection_preparation_phase(phase);
                return Err(BluetoothPeripheralConnectionControllerPreparationTerminal::FailStop(
                    BluetoothPeripheralConnectionControllerPreparationFailStop {
                        cause: BluetoothPeripheralConnectionControllerPreparationFailStopCause::ControllerTime(
                            controller_time_begin_error(error),
                        ),
                        _controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                        _accepted: Some(accepted),
                    },
                ));
            }
        };
        Ok(BluetoothPeripheralConnectionControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothPeripheralConnectionControllerPreparationTimeOwner {
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
        prepared: crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventFieldsPrepared,
    ) -> crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared
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
        recycled: crate::BluetoothPeripheralConnectionSchedulerRecycled,
    ) -> BluetoothPeripheralConnectionCompletionStep {
        let epoch = *self.scheduler_epoch;
        match recycled.classify_completion(|captured| {
            epoch.map(|epoch| {
                self.ble_phy_timing
                    .complete_le_1m_peripheral_connection_packet_start(epoch, captured)
            })
        }) {
            BluetoothPeripheralConnectionSchedulerCompletionClassification::NormalizationUnavailable(
                recycled,
            ) => BluetoothPeripheralConnectionCompletionStep::SchedulerEpochUnavailable(
                recycled,
            ),
            BluetoothPeripheralConnectionSchedulerCompletionClassification::Completed(
                completed,
            ) => BluetoothPeripheralConnectionCompletionStep::Completed(completed),
        }
    }

    /// Build one provisional recurrence from the real completed connection owner.
    ///
    /// The default runtime remains fail-closed because neither main-XTAL
    /// selection nor PHY initialization proves a worst-case local SCA. A board
    /// may opt in through its connection runtime config with an explicit ppm
    /// bound; that same config explicitly selects the reviewed software-WW path.
    pub fn prepare_peripheral_connection_recurring_candidate(
        &mut self,
        completed: crate::BluetoothPeripheralConnectionSchedulerCompleted,
        delta: open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
    ) -> BluetoothPeripheralConnectionRecurringCandidateStep {
        let Some(epoch) = *self.scheduler_epoch else {
            return BluetoothPeripheralConnectionRecurringCandidateStep::SchedulerEpochUnavailable(
                BluetoothPeripheralConnectionRecurringRetry::new(completed, delta),
            );
        };
        let Some(timing_policy) = self
            .peripheral_connection_resources
            .config()
            .recurring_timing_policy()
        else {
            return BluetoothPeripheralConnectionRecurringCandidateStep::TimingPolicyUnavailable(
                BluetoothPeripheralConnectionRecurringRetry::new(completed, delta),
            );
        };
        match completed.prepare_recurring_event_candidate(
            delta,
            epoch,
            self.runtime.scheduler_config(),
            timing_policy,
        ) {
            ControlFlow::Continue(candidate) => {
                BluetoothPeripheralConnectionRecurringCandidateStep::Prepared(candidate)
            }
            ControlFlow::Break(failure) => {
                let error = failure.error();
                let (completed, delta) = failure.into_retry_parts();
                let retry = BluetoothPeripheralConnectionRecurringRetry::new(completed, delta);
                BluetoothPeripheralConnectionRecurringCandidateStep::Rejected { error, retry }
            }
        }
    }

    /// Reserve one provisional recurrence before acquiring its sequence sample.
    pub fn admit_peripheral_connection_recurring_candidate(
        &mut self,
        candidate: crate::BluetoothPeripheralConnectionRecurringEventCandidate,
    ) -> ControlFlow<
        crate::BluetoothPeripheralConnectionRecurringEventPreparationFailure,
        crate::BluetoothPeripheralConnectionRecurringPreSequence,
    > {
        self.runtime
            .admit_peripheral_connection_recurring_event(candidate)
    }

    /// Cancel one candidate which owns no scheduler reservation yet.
    pub fn cancel_peripheral_connection_recurring_candidate(
        &mut self,
        candidate: crate::BluetoothPeripheralConnectionRecurringEventCandidate,
    ) -> BluetoothPeripheralConnectionRecurringRetry {
        BluetoothPeripheralConnectionRecurringRetry::from_cancelled(candidate.cancel())
    }

    /// Release a recurring reservation before sequence authorization.
    pub fn cancel_peripheral_connection_recurring_pre_sequence(
        &mut self,
        admitted: crate::BluetoothPeripheralConnectionRecurringPreSequence,
    ) -> BluetoothPeripheralConnectionRecurringRetry {
        let cancelled = self
            .runtime
            .cancel_peripheral_connection_recurring_pre_sequence(admitted);
        BluetoothPeripheralConnectionRecurringRetry::from_cancelled(cancelled)
    }

    /// Release a sequence-authorized recurring event and its timeline slot.
    pub fn cancel_peripheral_connection_recurring_event(
        &mut self,
        prepared: crate::BluetoothPeripheralConnectionRecurringEventPrepared,
    ) -> BluetoothPeripheralConnectionRecurringRetry {
        let cancelled = self
            .runtime
            .cancel_peripheral_connection_recurring_event(prepared);
        BluetoothPeripheralConnectionRecurringRetry::from_cancelled(cancelled)
    }

    /// Retry the infallible detach plus empty-list identity merge.
    pub fn prepare_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        prepared: crate::BluetoothPeripheralConnectionRecurringEventPrepared,
    ) -> ControlFlow<
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    > {
        self.runtime
            .prepare_peripheral_connection_recurring_empty_list_merge(prepared)
    }

    /// Undo an unpublished empty-list merge while preserving its reservation.
    pub fn cancel_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        merged: crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> ControlFlow<
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
        crate::BluetoothPeripheralConnectionRecurringEventPrepared,
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
        merged: crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
        crate::BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        self.runtime
            .publish_peripheral_connection_scheduler_head(merged)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Apply this fresh sequence sample to one reserved connection recurrence.
    pub fn finish_peripheral_connection_recurring_event(
        self,
        admitted: crate::BluetoothPeripheralConnectionRecurringPreSequence,
    ) -> BluetoothPeripheralConnectionRecurringSequenceCompletion<'runtime, S, SCHEDULER_CAPACITY>
    {
        let Self {
            mut controller,
            sample,
            ..
        } = self;
        let prepared = match controller
            .runtime
            .prepare_peripheral_connection_recurring_event(
                admitted,
                crate::scheduler::core::BluetoothPeripheralConnectionSequenceObservation { sample },
            ) {
            ControlFlow::Continue(prepared) => prepared,
            ControlFlow::Break(failure) => {
                return BluetoothPeripheralConnectionRecurringSequenceCompletion::EventRejected {
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
                BluetoothPeripheralConnectionRecurringSequenceCompletion::Prepared {
                    task: controller,
                    merged,
                }
            }
            ControlFlow::Break(failure) => {
                BluetoothPeripheralConnectionRecurringSequenceCompletion::EmptyListRejected {
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
        accepted: crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest,
        packet_start: crate::BluetoothLe1MPacketStartTiming,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
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
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal::Recovered {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        error: BluetoothPeripheralConnectionControllerPreparationError::TimingWindow,
                        accepted: crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest::new(
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
            crate::scheduler::core::BluetoothPeripheralConnectionAdmissionObservation { sample },
        ) {
            Ok(admitted) => admitted,
            Err(failure) => {
                let error = failure.error();
                let (allocation, connection) = failure.into_candidate().cancel();
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal::Recovered {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        error: BluetoothPeripheralConnectionControllerPreparationError::Event(
                            error,
                        ),
                        accepted: crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedRequest::new(
                            allocation,
                            connection,
                            packet,
                        ),
                    },
                );
            }
        };
        match controller.begin_peripheral_connection_preparation_time(
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence { admitted, packet },
        ) {
            Ok(pending) => BluetoothPeripheralConnectionControllerPreparationStep::Pending(pending),
            Err(terminal) => {
                BluetoothPeripheralConnectionControllerPreparationStep::Terminal(terminal)
            }
        }
    }
}
