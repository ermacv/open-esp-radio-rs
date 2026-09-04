//! Peripheral-connection controller preparation, completion, and recurrence.

use super::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerEpochRetained,
    BluetoothControllerSchedulerNowReady, controller_time_begin_error, controller_time_event_error,
};
use crate::controller_time::{
    BluetoothControllerTimeEventError, BluetoothControllerTimePendingCore,
    BluetoothControllerTimePendingCoreStep, BluetoothControllerTimePendingOrphanStep,
    BluetoothControllerTimePendingOwner, BluetoothControllerTimePendingOwnerStep,
    BluetoothControllerTimeRequest,
};
use crate::scheduler::BluetoothPeripheralConnectionSchedulerCompletionClassification;

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
    ControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    TimingWindow,
    Event(crate::scheduler::BluetoothPeripheralConnectionFirstEventPreparationError),
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Terminal result of one source-ordered first connection preparation.
#[must_use = "publish the connection item or retain every returned owner"]
pub enum BluetoothPeripheralConnectionControllerPreparationOutcome {
    /// The sole allocation was already checked out; no input was consumed.
    RuntimeUnavailable {
        error: crate::BluetoothPeripheralConnectionRuntimeBeginError,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        packet_start: crate::BluetoothLe1MPacketStartTiming,
    },
    /// Preparation was rejected after consuming the packet-time input.
    Rejected {
        error: BluetoothPeripheralConnectionControllerPreparationError,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    },
    /// The selected item is joined to the CPU-owned common scheduler list.
    Prepared(crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared),
}

enum BluetoothPeripheralConnectionControllerPreparationPhase {
    Sequence(crate::scheduler::BluetoothPeripheralConnectionFirstPreSequence),
}

struct BluetoothPeripheralConnectionControllerPreparationTimeOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: Option<BluetoothPeripheralConnectionControllerPreparationPhase>,
    cancelled: Option<BluetoothPeripheralConnectionControllerPreparationOutcome>,
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

/// Terminal connection preparation with the exact task service retained.
#[must_use = "the task owner and connection outcome must be handled together"]
pub struct BluetoothPeripheralConnectionControllerPreparationTerminal<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    controller: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    outcome: BluetoothPeripheralConnectionControllerPreparationOutcome,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Recover the retained Controller and exact connection outcome.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPeripheralConnectionControllerPreparationOutcome,
    ) {
        (self.controller, self.outcome)
    }
}

/// Result of one bounded connection sequence-time observation.
#[must_use = "retain Pending or consume the terminal task and connection result"]
pub enum BluetoothPeripheralConnectionControllerPreparationStep<
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
    fn terminal(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothPeripheralConnectionControllerPreparationOutcome,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
            BluetoothPeripheralConnectionControllerPreparationTerminal {
                controller: BluetoothControllerSchedulerEpochRetained { controller },
                outcome,
            },
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
                let phase = owner
                    .phase
                    .take()
                    .expect("failed connection time recheck retains its exact phase");
                let outcome = owner
                    .controller
                    .cancel_peripheral_connection_preparation_phase(
                        phase,
                        controller_time_event_error(error),
                    );
                return Self::terminal(owner.controller, outcome);
            }
        };
        let phase = owner
            .phase
            .take()
            .expect("completed connection time request retains its exact phase");
        let mut controller = owner.controller;
        match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted) => {
                let default_tx_power = controller
                    .peripheral_connection_resources
                    .default_tx_power_dbm();
                let direction_finding_workspace = controller.direction_finding_workspace;
                let prepared = match controller
                    .runtime
                    .prepare_peripheral_connection_first_event(
                        admitted,
                        crate::scheduler::BluetoothPeripheralConnectionSequenceObservation {
                            sample,
                        },
                        default_tx_power,
                        direction_finding_workspace,
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = failure.into_candidate().cancel();
                        controller.restore_peripheral_connection_allocation(allocation);
                        return Self::terminal(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error:
                                    BluetoothPeripheralConnectionControllerPreparationError::Event(
                                        error,
                                    ),
                                connection,
                            },
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_peripheral_connection_empty_list_merge(prepared)
                {
                    Ok(merged) => Self::terminal(
                        controller,
                        BluetoothPeripheralConnectionControllerPreparationOutcome::Prepared(merged),
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let (allocation, connection) = controller
                            .runtime
                            .cancel_peripheral_connection_first_event(failure.into_prepared());
                        controller.restore_peripheral_connection_allocation(allocation);
                        Self::terminal(
                            controller,
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error: BluetoothPeripheralConnectionControllerPreparationError::EmptyList(
                                    error,
                                ),
                                connection,
                            },
                        )
                    }
                }
            }
        }
    }

    /// Cancel the unpublished sequence request and restore the connection allocation.
    pub fn cancel(
        self,
    ) -> BluetoothPeripheralConnectionControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>
    {
        let mut owner = match self.core.cancel() {
            Ok(owner) => owner,
            Err(failure) => failure.into_parts().0,
        };
        let outcome = owner
            .cancelled
            .take()
            .expect("explicit connection time cancellation records its restored outcome");
        BluetoothPeripheralConnectionControllerPreparationTerminal {
            controller: BluetoothControllerSchedulerEpochRetained {
                controller: owner.controller,
            },
            outcome,
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
        let result = BluetoothControllerTimePendingOwner::cancel_owned_controller_time(
            &mut self.controller,
            request,
        );
        let error = match result {
            Ok(()) => crate::BluetoothControllerTimeAcquisitionError::Cancelled,
            Err(error) => controller_time_event_error(error),
        };
        let phase = self
            .phase
            .take()
            .expect("private connection time owner retains one exact preparation phase");
        self.cancelled = Some(
            self.controller
                .cancel_peripheral_connection_preparation_phase(phase, error),
        );
        result
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
    fn restore_peripheral_connection_allocation(
        &mut self,
        allocation: crate::BluetoothPeripheralConnectionRuntimeAllocation,
    ) {
        if self
            .peripheral_connection_resources
            .restore_idle(allocation)
            .is_err()
        {
            panic!("a connection phase returned an allocation to a different runtime");
        }
    }

    fn cancel_peripheral_connection_preparation_phase(
        &mut self,
        phase: BluetoothPeripheralConnectionControllerPreparationPhase,
        error: crate::BluetoothControllerTimeAcquisitionError,
    ) -> BluetoothPeripheralConnectionControllerPreparationOutcome {
        let (allocation, connection) = match phase {
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted) => self
                .runtime
                .cancel_peripheral_connection_first_pre_sequence(admitted),
        };
        self.restore_peripheral_connection_allocation(allocation);
        BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
            error: BluetoothPeripheralConnectionControllerPreparationError::ControllerTime(error),
            connection,
        }
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
                let outcome = self.cancel_peripheral_connection_preparation_phase(
                    phase,
                    controller_time_begin_error(error),
                );
                return Err(BluetoothPeripheralConnectionControllerPreparationTerminal {
                    controller: BluetoothControllerSchedulerEpochRetained { controller: self },
                    outcome,
                });
            }
        };
        Ok(BluetoothPeripheralConnectionControllerPreparationPending {
            core: BluetoothControllerTimePendingCore::new(
                BluetoothPeripheralConnectionControllerPreparationTimeOwner {
                    controller: self,
                    phase: Some(phase),
                    cancelled: None,
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
            Ok(candidate) => {
                BluetoothPeripheralConnectionRecurringCandidateStep::Prepared(candidate)
            }
            Err(failure) => {
                let error = failure.error();
                let (completed, delta) = failure.into_retry_parts();
                let retry = BluetoothPeripheralConnectionRecurringRetry::new(completed, delta);
                BluetoothPeripheralConnectionRecurringCandidateStep::Rejected { error, retry }
            }
        }
    }

    /// Reserve one provisional recurrence before acquiring its sequence sample.
    #[allow(
        clippy::result_large_err,
        reason = "timeline rejection retains the complete recurring candidate"
    )]
    pub fn admit_peripheral_connection_recurring_candidate(
        &mut self,
        candidate: crate::BluetoothPeripheralConnectionRecurringEventCandidate,
    ) -> Result<
        crate::BluetoothPeripheralConnectionRecurringPreSequence,
        crate::BluetoothPeripheralConnectionRecurringEventPreparationFailure,
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
    #[allow(
        clippy::result_large_err,
        reason = "list rejection retains the complete recurring event and reservation"
    )]
    pub fn prepare_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        prepared: crate::BluetoothPeripheralConnectionRecurringEventPrepared,
    ) -> Result<
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
    > {
        self.runtime
            .prepare_peripheral_connection_recurring_empty_list_merge(prepared)
    }

    /// Undo an unpublished empty-list merge while preserving its reservation.
    #[allow(
        clippy::result_large_err,
        reason = "identity mismatch retains the complete recurring merge"
    )]
    pub fn cancel_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        merged: crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> Result<
        crate::BluetoothPeripheralConnectionRecurringEventPrepared,
        crate::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    > {
        self.runtime
            .cancel_peripheral_connection_recurring_empty_list_merge(merged)
    }

    /// Publish selector-two RX memory and the first connection scheduler head.
    #[allow(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete no-alloc connection graph"
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

    /// Cancel an unpublished connection merge and restore its sole allocation.
    ///
    /// A scheduler-identity mismatch returns the unchanged merge, because this
    /// Controller cannot safely restore an item owned by another list epoch.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity rejection retains the complete connection merge"
    )]
    pub fn cancel_peripheral_connection_scheduler_merge(
        &mut self,
        merged: crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        crate::BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    > {
        let prepared = self
            .runtime
            .cancel_peripheral_connection_empty_list_merge(merged)?;
        let (allocation, connection) = self
            .runtime
            .cancel_peripheral_connection_first_event(prepared);
        self.restore_peripheral_connection_allocation(allocation);
        Ok(connection)
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
                crate::scheduler::BluetoothPeripheralConnectionSequenceObservation { sample },
            ) {
            Ok(prepared) => prepared,
            Err(failure) => {
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
            Ok(merged) => BluetoothPeripheralConnectionRecurringSequenceCompletion::Prepared {
                task: controller,
                merged,
            },
            Err(failure) => {
                BluetoothPeripheralConnectionRecurringSequenceCompletion::EmptyListRejected {
                    task: controller,
                    failure,
                }
            }
        }
    }

    /// Begin the first peripheral connection event from its causal packet time.
    ///
    /// The current sample authorizes timeline admission. A distinct later
    /// request authorizes descriptor sequencing after overlap resolution. Every
    /// rejection restores the exact connection allocation before returning the
    /// retained Controller epoch.
    pub fn begin_peripheral_connection_first_event(
        self,
        connection: open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
        packet_start: crate::BluetoothLe1MPacketStartTiming,
    ) -> BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        let Self {
            mut controller,
            epoch,
            sample,
        } = self;
        let allocation = match controller.peripheral_connection_resources.begin_event() {
            Ok(allocation) => allocation,
            Err(error) => {
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::RuntimeUnavailable {
                                error,
                                connection,
                                packet_start,
                            },
                    },
                );
            }
        };
        let prepared = allocation.prepare_first_event(connection, packet_start);
        let candidate = match prepared
            .project_scheduler_window(epoch, controller.runtime.scheduler_config())
        {
            Ok(candidate) => candidate,
            Err(prepared) => {
                let (allocation, connection) = prepared.cancel();
                controller.restore_peripheral_connection_allocation(allocation);
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error: BluetoothPeripheralConnectionControllerPreparationError::TimingWindow,
                                connection,
                            },
                    },
                );
            }
        };
        let admitted = match controller.runtime.admit_peripheral_connection_first_event(
            candidate,
            crate::scheduler::BluetoothPeripheralConnectionAdmissionObservation { sample },
        ) {
            Ok(admitted) => admitted,
            Err(failure) => {
                let error = failure.error();
                let (allocation, connection) = failure.into_candidate().cancel();
                controller.restore_peripheral_connection_allocation(allocation);
                return BluetoothPeripheralConnectionControllerPreparationStep::Terminal(
                    BluetoothPeripheralConnectionControllerPreparationTerminal {
                        controller: BluetoothControllerSchedulerEpochRetained { controller },
                        outcome:
                            BluetoothPeripheralConnectionControllerPreparationOutcome::Rejected {
                                error:
                                    BluetoothPeripheralConnectionControllerPreparationError::Event(
                                        error,
                                    ),
                                connection,
                            },
                    },
                );
            }
        };
        match controller.begin_peripheral_connection_preparation_time(
            BluetoothPeripheralConnectionControllerPreparationPhase::Sequence(admitted),
        ) {
            Ok(pending) => BluetoothPeripheralConnectionControllerPreparationStep::Pending(pending),
            Err(terminal) => {
                BluetoothPeripheralConnectionControllerPreparationStep::Terminal(terminal)
            }
        }
    }
}
