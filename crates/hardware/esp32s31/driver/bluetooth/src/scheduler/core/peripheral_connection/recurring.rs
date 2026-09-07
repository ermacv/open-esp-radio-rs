//! Recurring peripheral-connection scheduler ownership and publication.

#![forbid(unsafe_op_in_unsafe_fn)]

use core::ops::ControlFlow;

mod transaction;

pub use transaction::{
    PeripheralConnectionRecurringCandidateError, PeripheralConnectionRecurringEventCandidate,
};

use crate::{
    ControllerSchedulerEpoch,
    le::peripheral::connection::{
        PeripheralConnectionCompletedEventRecurringRemainder,
        PeripheralConnectionRecurringTimingPolicy,
    },
    runtime_resources::ControllerPoweredTaskRuntime,
    scheduler::{
        SchedulerHeadPublicationError, SchedulerRawWindow, SchedulerReservationError,
        SchedulerSequenceAuthorizationError, SchedulerSequenceReady, SchedulerSoftwareConfig,
        SchedulerTimingPolicy,
        core::SchedulerEmptyListMergeError,
        timeline::{SchedulerRecurringReserved, SchedulerWindowReservation},
    },
};

use oer_bluetooth_ll::connection::{
    LePeripheralConnectionEventDelta, LePeripheralConnectionEventPrepared,
};

use oer_esp32s31_bluetooth_memory::{
    PeripheralConnectionMemoryGraphPublicationPrepared,
    PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
};

use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHead,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerRunInterruptsPrepared,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

use super::{PeripheralConnectionSchedulerCompleted, PeripheralConnectionSequenceObservation};

use transaction::{
    PeripheralConnectionRecurringCandidateFailure,
    PeripheralConnectionRecurringEventFieldsPrepared,
    PeripheralConnectionRecurringEventSchedulerHandoff, prepare_recurring_event_candidate,
};

impl PeripheralConnectionSchedulerCompleted {
    /// Form the provisional combined owner from the phase retained by this
    /// exact completed event.
    pub(crate) fn prepare_recurring_event_candidate(
        self,
        delta: LePeripheralConnectionEventDelta,
        epoch: ControllerSchedulerEpoch,
        scheduler_config: SchedulerSoftwareConfig,
        timing_policy: PeripheralConnectionRecurringTimingPolicy,
    ) -> ControlFlow<
        PeripheralConnectionRecurringCandidateFailure,
        PeripheralConnectionRecurringEventCandidate,
    > {
        prepare_recurring_event_candidate(self, delta, epoch, scheduler_config, timing_policy)
    }
}

/// Recurring connection event after exact common-timeline reservation.
#[must_use = "the recurring event must pass sequence authorization or be cancelled"]
pub struct PeripheralConnectionRecurringPreSequence {
    candidate: PeripheralConnectionRecurringEventCandidate,
    reservation: SchedulerWindowReservation<SchedulerRecurringReserved>,
}

/// Why one recurring connection event could not reach a sequence-ready image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionRecurringEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
}

/// Lossless recurring admission or sequence-preparation rejection.
#[must_use = "the unchanged recurring candidate must be retried, cancelled, or retained"]
pub struct PeripheralConnectionRecurringEventPreparationFailure {
    candidate: PeripheralConnectionRecurringEventCandidate,
    error: PeripheralConnectionRecurringEventPreparationError,
}

impl PeripheralConnectionRecurringEventPreparationFailure {
    pub const fn error(&self) -> PeripheralConnectionRecurringEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> PeripheralConnectionRecurringEventCandidate {
        self.candidate
    }
}

/// Sequence-authorized recurring image paired with its exact timeline slot.
#[must_use = "the recurring event must be merged, cancelled, or retained"]
pub struct PeripheralConnectionRecurringEventPrepared {
    event: PeripheralConnectionRecurringEventFieldsPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

impl PeripheralConnectionRecurringEventPrepared {
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn channel(&self) -> oer_bluetooth_ll::connection::LeDataChannelIndex {
        self.event.channel()
    }

    pub const fn reserved_window(&self) -> SchedulerRawWindow {
        self.reservation.window()
    }
}

/// Lossless rejection while joining one recurring item to the empty list.
#[must_use = "the recurring event remains prepared and CPU-owned"]
pub struct PeripheralConnectionRecurringEmptySchedulerMergeFailure {
    error: SchedulerEmptyListMergeError,
    prepared: PeripheralConnectionRecurringEventPrepared,
}

impl PeripheralConnectionRecurringEmptySchedulerMergeFailure {
    pub const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    pub fn into_prepared(self) -> PeripheralConnectionRecurringEventPrepared {
        self.prepared
    }
}

/// Detached recurring item joined to the source-owned empty scheduler list.
#[must_use = "the recurring merge must be validated for publication or cancelled"]
pub struct PeripheralConnectionRecurringEmptySchedulerMergePrepared {
    event: PeripheralConnectionRecurringSchedulerAdmissionPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

/// Scheduler-owned graph plus opaque LL/phase rollback half after detachment.
#[must_use = "merge, restore, or retain the complete recurring admission owner"]
struct PeripheralConnectionRecurringSchedulerAdmissionPrepared {
    graph: PeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
    transaction: PeripheralConnectionRecurringSchedulerTransaction,
}

/// Private provisional LL/phase owner; only this module can commit it.
#[must_use = "restore the handoff or commit it after final scheduler validation"]
struct PeripheralConnectionRecurringSchedulerTransaction {
    handoff: PeripheralConnectionRecurringEventSchedulerHandoff,
}

impl PeripheralConnectionRecurringSchedulerTransaction {
    const fn event_counter(&self) -> u16 {
        self.handoff.provisional.event_counter()
    }

    const fn channel(&self) -> oer_bluetooth_ll::connection::LeDataChannelIndex {
        self.handoff.provisional.channel()
    }

    fn into_handoff(self) -> PeripheralConnectionRecurringEventSchedulerHandoff {
        self.handoff
    }

    fn commit(self) -> PeripheralConnectionRecurringSchedulerCommittedRemainder {
        let PeripheralConnectionRecurringEventSchedulerHandoff {
            remainder,
            provisional,
            original_phase: _,
            proposed_phase,
            delta: _,
        } = self.handoff;
        PeripheralConnectionRecurringSchedulerCommittedRemainder {
            event: provisional.commit(),
            remainder,
            phase: proposed_phase,
        }
    }
}

/// Private committed LL/phase owner retained across RX publication.
#[must_use = "rejoin the committed event with its exact RX-published graph"]
pub(super) struct PeripheralConnectionRecurringSchedulerCommittedRemainder {
    event: LePeripheralConnectionEventPrepared,
    remainder: PeripheralConnectionCompletedEventRecurringRemainder,
    phase: crate::le::peripheral::connection::PeripheralConnectionRecurringPhase,
}

impl PeripheralConnectionRecurringSchedulerCommittedRemainder {
    fn join_rx_publication(
        self,
        graph: oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphRxPublished,
    ) -> crate::le::peripheral::connection::PeripheralConnectionFirstEventRxPublished {
        self.remainder
            .join_recurring_rx_publication(graph, self.event, self.phase)
    }
}

impl PeripheralConnectionRecurringSchedulerAdmissionPrepared {
    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    const fn event_counter(&self) -> u16 {
        self.transaction.event_counter()
    }

    const fn channel(&self) -> oer_bluetooth_ll::connection::LeDataChannelIndex {
        self.transaction.channel()
    }

    fn restore_event_fields(self) -> PeripheralConnectionRecurringEventFieldsPrepared {
        PeripheralConnectionRecurringEventFieldsPrepared::from_scheduler_parts(
            self.graph.cancel(),
            self.transaction.into_handoff(),
        )
    }

    fn commit(self) -> PeripheralConnectionRecurringSchedulerPublicationPrepared {
        PeripheralConnectionRecurringSchedulerPublicationPrepared {
            graph: self.graph.prepare_publication(),
            remainder: self.transaction.commit(),
        }
    }
}

/// Scheduler-owned combined graph after the single LL/phase commit.
#[must_use = "publish RX memory and rejoin the committed recurring event"]
struct PeripheralConnectionRecurringSchedulerPublicationPrepared {
    graph: PeripheralConnectionMemoryGraphPublicationPrepared,
    remainder: PeripheralConnectionRecurringSchedulerCommittedRemainder,
}

impl PeripheralConnectionRecurringSchedulerPublicationPrepared {
    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    fn into_parts(
        self,
    ) -> (
        PeripheralConnectionMemoryGraphPublicationPrepared,
        PeripheralConnectionRecurringSchedulerCommittedRemainder,
    ) {
        (self.graph, self.remainder)
    }
}

impl PeripheralConnectionRecurringEmptySchedulerMergePrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn channel(&self) -> oer_bluetooth_ll::connection::LeDataChannelIndex {
        self.event.channel()
    }

    pub const fn reserved_window(&self) -> SchedulerRawWindow {
        self.reservation.window()
    }
}

/// Lossless rejection of the final common-list/head validation.
#[must_use = "the unchanged recurring merge can be retried or cancelled"]
pub(crate) struct PeripheralConnectionRecurringSchedulerValidationFailure {
    error: SchedulerHeadPublicationError,
    merged: PeripheralConnectionRecurringEmptySchedulerMergePrepared,
}

impl PeripheralConnectionRecurringSchedulerValidationFailure {
    #[expect(
        dead_code,
        reason = "the active peripheral actor does not yet inspect recurring scheduler validation failures"
    )]
    pub(crate) const fn error(&self) -> SchedulerHeadPublicationError {
        self.error
    }

    #[expect(
        dead_code,
        reason = "the active peripheral actor does not yet recover rejected recurring scheduler merges"
    )]
    pub(crate) fn into_merged(self) -> PeripheralConnectionRecurringEmptySchedulerMergePrepared {
        self.merged
    }
}

/// Common-list identity and head encoding validated without publication.
#[must_use = "prepare RUN interrupts, then commit and publish, or recover the merge"]
pub(crate) struct PeripheralConnectionRecurringSchedulerValidated {
    merged: PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    head: BluetoothSchedulerHardwareListHead,
}

impl PeripheralConnectionRecurringSchedulerValidated {
    pub(crate) fn into_merged(self) -> PeripheralConnectionRecurringEmptySchedulerMergePrepared {
        self.merged
    }

    /// Consume the final fallible RUN prerequisite and atomically commit LL and phase.
    pub(crate) fn commit(
        self,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> PeripheralConnectionRecurringSchedulerCommitted {
        let PeripheralConnectionRecurringEmptySchedulerMergePrepared { event, reservation } =
            self.merged;
        let event = event.commit();
        PeripheralConnectionRecurringSchedulerCommitted {
            event,
            reservation,
            head: self.head,
            interrupts,
        }
    }
}

/// Committed recurring event whose remaining publication suffix is infallible.
#[must_use = "publish RX/head and the common scheduler RUN suffix"]
pub(crate) struct PeripheralConnectionRecurringSchedulerCommitted {
    event: PeripheralConnectionRecurringSchedulerPublicationPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
    head: BluetoothSchedulerHardwareListHead,
    interrupts: BluetoothSchedulerRunInterruptsPrepared,
}

/// Sealed recurring owner after RX-list MMIO could not rejoin its graph proof.
#[must_use = "retain every committed recurring publication owner"]
pub(crate) struct PeripheralConnectionRecurringSchedulerPublicationFailStop {
    mismatch: oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphPublicationMismatch,
    _remainder: PeripheralConnectionRecurringSchedulerCommittedRemainder,
    _reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
    _head: BluetoothSchedulerHardwareListHead,
    _interrupts: BluetoothSchedulerRunInterruptsPrepared,
}

impl PeripheralConnectionRecurringSchedulerPublicationFailStop {
    #[expect(
        dead_code,
        reason = "the active peripheral actor does not yet inspect recurring publication fail-stop diagnostics"
    )]
    pub(crate) const fn error(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphPublicationError {
        self.mismatch.error()
    }
}

impl PeripheralConnectionRecurringSchedulerCommitted {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    fn into_parts(
        self,
    ) -> (
        PeripheralConnectionRecurringSchedulerPublicationPrepared,
        SchedulerWindowReservation<SchedulerSequenceReady>,
        BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerRunInterruptsPrepared,
    ) {
        (self.event, self.reservation, self.head, self.interrupts)
    }
}

impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    /// Reserve one exact recurring connection window without displacement.
    pub(crate) fn admit_peripheral_connection_recurring_event(
        &mut self,
        candidate: PeripheralConnectionRecurringEventCandidate,
    ) -> ControlFlow<
        PeripheralConnectionRecurringEventPreparationFailure,
        PeripheralConnectionRecurringPreSequence,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(raw_window.start(), raw_window.end(), timing_policy)
        {
            Ok(reservation) => ControlFlow::Continue(PeripheralConnectionRecurringPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => {
                ControlFlow::Break(PeripheralConnectionRecurringEventPreparationFailure {
                    candidate,
                    error: PeripheralConnectionRecurringEventPreparationError::Timeline(error),
                })
            }
        }
    }

    /// Authorize the recurring deadline and encode its infallible event fields.
    pub(crate) fn prepare_peripheral_connection_recurring_event(
        &mut self,
        admitted: PeripheralConnectionRecurringPreSequence,
        sequence: PeripheralConnectionSequenceObservation,
    ) -> ControlFlow<
        PeripheralConnectionRecurringEventPreparationFailure,
        PeripheralConnectionRecurringEventPrepared,
    > {
        let PeripheralConnectionRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return ControlFlow::Break(PeripheralConnectionRecurringEventPreparationFailure {
                    candidate,
                    error: PeripheralConnectionRecurringEventPreparationError::Sequence(error),
                });
            }
        };
        ControlFlow::Continue(PeripheralConnectionRecurringEventPrepared {
            event: candidate.prepare_event_fields(),
            reservation,
        })
    }

    pub(crate) fn cancel_peripheral_connection_recurring_pre_sequence(
        &mut self,
        admitted: PeripheralConnectionRecurringPreSequence,
    ) -> (
        PeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let PeripheralConnectionRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    pub(crate) fn cancel_peripheral_connection_recurring_event(
        &mut self,
        prepared: PeripheralConnectionRecurringEventPrepared,
    ) -> (
        PeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let PeripheralConnectionRecurringEventPrepared { event, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        event.cancel()
    }

    pub(crate) fn prepare_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        prepared: PeripheralConnectionRecurringEventPrepared,
    ) -> ControlFlow<
        PeripheralConnectionRecurringEmptySchedulerMergeFailure,
        PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    > {
        let PeripheralConnectionRecurringEventPrepared { event, reservation } = prepared;
        let (graph, transaction) = event.into_scheduler_parts();
        let event = PeripheralConnectionRecurringSchedulerAdmissionPrepared {
            graph: graph.prepare_scheduler_admission(),
            transaction: PeripheralConnectionRecurringSchedulerTransaction {
                handoff: transaction,
            },
        };
        let address = event.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return ControlFlow::Break(PeripheralConnectionRecurringEmptySchedulerMergeFailure {
                error,
                prepared: PeripheralConnectionRecurringEventPrepared {
                    event: event.restore_event_fields(),
                    reservation,
                },
            });
        }
        ControlFlow::Continue(PeripheralConnectionRecurringEmptySchedulerMergePrepared {
            event,
            reservation,
        })
    }

    pub(crate) fn cancel_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        merged: PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> ControlFlow<
        PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        PeripheralConnectionRecurringEventPrepared,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return ControlFlow::Break(merged);
        }
        let PeripheralConnectionRecurringEmptySchedulerMergePrepared { event, reservation } =
            merged;
        ControlFlow::Continue(PeripheralConnectionRecurringEventPrepared {
            event: event.restore_event_fields(),
            reservation,
        })
    }

    /// Seal the exact common-list identity and encodable hardware head.
    pub(crate) fn validate_peripheral_connection_recurring_scheduler(
        &self,
        merged: PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> ControlFlow<
        PeripheralConnectionRecurringSchedulerValidationFailure,
        PeripheralConnectionRecurringSchedulerValidated,
    > {
        let address = merged.scheduler_item_address();
        match self.validate_first_scheduler_item_head(address) {
            Ok(head) => ControlFlow::Continue(PeripheralConnectionRecurringSchedulerValidated {
                merged,
                head,
            }),
            Err(error) => {
                ControlFlow::Break(PeripheralConnectionRecurringSchedulerValidationFailure {
                    error,
                    merged,
                })
            }
        }
    }

    /// Publish the already committed event through the RX/head suffix.
    ///
    /// A proof mismatch after RX-list MMIO seals the committed LL successor,
    /// its HAL publication and every remaining scheduler owner in one
    /// fail-stop value. It cannot be recovered as a retryable merge.
    #[allow(
        unsafe_code,
        reason = "the HAL publication consumes the unique task-side peripheral memory owner"
    )]
    pub(crate) fn publish_peripheral_connection_recurring_scheduler_head(
        &mut self,
        committed: PeripheralConnectionRecurringSchedulerCommitted,
    ) -> ControlFlow<
        PeripheralConnectionRecurringSchedulerPublicationFailStop,
        (
            super::PeripheralConnectionSchedulerHeadPublished,
            BluetoothSchedulerRunInterruptsPrepared,
        ),
    > {
        let address = committed.scheduler_item_address();
        let index = BluetoothSchedulerHardwareListIndex::ZERO;
        let (event, reservation, head, interrupts) = committed.into_parts();
        let (graph, remainder) = event.into_parts();
        let graph = match unsafe { self.task.publish_peripheral_connection_rx_memory(graph) } {
            Ok(graph) => graph,
            Err(mismatch) => {
                return ControlFlow::Break(
                    PeripheralConnectionRecurringSchedulerPublicationFailStop {
                        mismatch,
                        _remainder: remainder,
                        _reservation: reservation,
                        _head: head,
                        _interrupts: interrupts,
                    },
                );
            }
        };
        let event = remainder.join_rx_publication(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        ControlFlow::Continue((
            super::PeripheralConnectionSchedulerHeadPublished {
                event,
                publication,
                reservation,
            },
            interrupts,
        ))
    }
}
