//! Recurring peripheral-connection scheduler ownership and publication.

#![forbid(unsafe_op_in_unsafe_fn)]

mod transaction;

pub use transaction::{
    BluetoothPeripheralConnectionRecurringCandidateError,
    BluetoothPeripheralConnectionRecurringEventCandidate,
};
use transaction::{
    BluetoothPeripheralConnectionRecurringCandidateFailure,
    BluetoothPeripheralConnectionRecurringEventFieldsPrepared,
    BluetoothPeripheralConnectionRecurringEventSchedulerHandoff, prepare_recurring_event_candidate,
};

use super::{
    BluetoothPeripheralConnectionSchedulerCompleted,
    BluetoothPeripheralConnectionSequenceObservation,
};
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
    BluetoothPeripheralConnectionRecurringTimingPolicy,
};
use crate::scheduler::BluetoothSchedulerEmptyListMergeError;
use crate::scheduler_timeline::{
    BluetoothSchedulerRecurringReserved, BluetoothSchedulerWindowReservation,
};
use crate::{
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerSchedulerEpoch,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerRawWindow,
    BluetoothSchedulerReservationError, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerSequenceReady, BluetoothSchedulerSoftwareConfig,
    BluetoothSchedulerTimingPolicy,
};
use open_esp_radio_bluetooth_ll::connection::{
    LePeripheralConnectionEventDelta, LePeripheralConnectionEventPrepared,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
    BluetoothPeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerRunInterruptsPrepared,
};

impl BluetoothPeripheralConnectionSchedulerCompleted {
    /// Form the provisional combined owner from the phase retained by this
    /// exact completed event.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure restores the completed event and phase"
    )]
    pub(crate) fn prepare_recurring_event_candidate(
        self,
        delta: LePeripheralConnectionEventDelta,
        epoch: BluetoothControllerSchedulerEpoch,
        scheduler_config: BluetoothSchedulerSoftwareConfig,
        timing_policy: BluetoothPeripheralConnectionRecurringTimingPolicy,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringEventCandidate,
        BluetoothPeripheralConnectionRecurringCandidateFailure,
    > {
        prepare_recurring_event_candidate(self, delta, epoch, scheduler_config, timing_policy)
    }
}

/// Recurring connection event after exact common-timeline reservation.
#[must_use = "the recurring event must pass sequence authorization or be cancelled"]
pub struct BluetoothPeripheralConnectionRecurringPreSequence {
    candidate: BluetoothPeripheralConnectionRecurringEventCandidate,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>,
}

/// Why one recurring connection event could not reach a sequence-ready image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPeripheralConnectionRecurringEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
}

/// Lossless recurring admission or sequence-preparation rejection.
#[must_use = "the unchanged recurring candidate must be retried, cancelled, or retained"]
pub struct BluetoothPeripheralConnectionRecurringEventPreparationFailure {
    candidate: BluetoothPeripheralConnectionRecurringEventCandidate,
    error: BluetoothPeripheralConnectionRecurringEventPreparationError,
}

impl BluetoothPeripheralConnectionRecurringEventPreparationFailure {
    pub const fn error(&self) -> BluetoothPeripheralConnectionRecurringEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> BluetoothPeripheralConnectionRecurringEventCandidate {
        self.candidate
    }
}

/// Sequence-authorized recurring image paired with its exact timeline slot.
#[must_use = "the recurring event must be merged, cancelled, or retained"]
pub struct BluetoothPeripheralConnectionRecurringEventPrepared {
    event: BluetoothPeripheralConnectionRecurringEventFieldsPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

impl BluetoothPeripheralConnectionRecurringEventPrepared {
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn channel(&self) -> open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex {
        self.event.channel()
    }

    pub const fn reserved_window(&self) -> BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// Lossless rejection while joining one recurring item to the empty list.
#[must_use = "the recurring event remains prepared and CPU-owned"]
pub struct BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure {
    error: BluetoothSchedulerEmptyListMergeError,
    prepared: BluetoothPeripheralConnectionRecurringEventPrepared,
}

impl BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure {
    pub const fn error(&self) -> BluetoothSchedulerEmptyListMergeError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothPeripheralConnectionRecurringEventPrepared {
        self.prepared
    }
}

/// Detached recurring item joined to the source-owned empty scheduler list.
#[must_use = "the recurring merge must be validated for publication or cancelled"]
pub struct BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
    event: BluetoothPeripheralConnectionRecurringSchedulerAdmissionPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

/// Scheduler-owned graph plus opaque LL/phase rollback half after detachment.
#[must_use = "merge, restore, or retain the complete recurring admission owner"]
struct BluetoothPeripheralConnectionRecurringSchedulerAdmissionPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphRecurringSchedulerAdmissionPrepared,
    transaction: BluetoothPeripheralConnectionRecurringSchedulerTransaction,
}

/// Private provisional LL/phase owner; only this module can commit it.
#[must_use = "restore the handoff or commit it after final scheduler validation"]
struct BluetoothPeripheralConnectionRecurringSchedulerTransaction {
    handoff: BluetoothPeripheralConnectionRecurringEventSchedulerHandoff,
}

impl BluetoothPeripheralConnectionRecurringSchedulerTransaction {
    const fn event_counter(&self) -> u16 {
        self.handoff.provisional.event_counter()
    }

    const fn channel(&self) -> open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex {
        self.handoff.provisional.channel()
    }

    fn into_handoff(self) -> BluetoothPeripheralConnectionRecurringEventSchedulerHandoff {
        self.handoff
    }

    fn commit(self) -> BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder {
        let BluetoothPeripheralConnectionRecurringEventSchedulerHandoff {
            remainder,
            provisional,
            original_phase: _,
            proposed_phase,
            delta: _,
        } = self.handoff;
        BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder {
            event: provisional.commit(),
            remainder,
            phase: proposed_phase,
        }
    }
}

/// Private committed LL/phase owner retained across RX publication.
#[must_use = "rejoin the committed event with its exact RX-published graph"]
struct BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder {
    event: LePeripheralConnectionEventPrepared,
    remainder: BluetoothPeripheralConnectionCompletedEventRecurringRemainder,
    phase: crate::peripheral_connection::BluetoothPeripheralConnectionRecurringPhase,
}

impl BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder {
    fn join_rx_publication(
        self,
        graph: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionMemoryGraphRxPublished,
    ) -> crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventRxPublished {
        self.remainder
            .join_recurring_rx_publication(graph, self.event, self.phase)
    }
}

impl BluetoothPeripheralConnectionRecurringSchedulerAdmissionPrepared {
    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    const fn event_counter(&self) -> u16 {
        self.transaction.event_counter()
    }

    const fn channel(&self) -> open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex {
        self.transaction.channel()
    }

    fn restore_event_fields(self) -> BluetoothPeripheralConnectionRecurringEventFieldsPrepared {
        BluetoothPeripheralConnectionRecurringEventFieldsPrepared::from_scheduler_parts(
            self.graph.cancel(),
            self.transaction.into_handoff(),
        )
    }

    fn commit(self) -> BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared {
        BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared {
            graph: self.graph.prepare_publication(),
            remainder: self.transaction.commit(),
        }
    }
}

/// Scheduler-owned combined graph after the single LL/phase commit.
#[must_use = "publish RX memory and rejoin the committed recurring event"]
struct BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared {
    graph: BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
    remainder: BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder,
}

impl BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared {
    const fn scheduler_head(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionMemoryGraphPublicationPrepared,
        BluetoothPeripheralConnectionRecurringSchedulerCommittedRemainder,
    ) {
        (self.graph, self.remainder)
    }
}

impl BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn channel(&self) -> open_esp_radio_bluetooth_ll::connection::LeDataChannelIndex {
        self.event.channel()
    }

    pub const fn reserved_window(&self) -> BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// Lossless rejection of the final common-list/head validation.
#[must_use = "the unchanged recurring merge can be retried or cancelled"]
pub(crate) struct BluetoothPeripheralConnectionRecurringSchedulerValidationFailure {
    error: BluetoothSchedulerHeadPublicationError,
    merged: BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
}

impl BluetoothPeripheralConnectionRecurringSchedulerValidationFailure {
    pub(crate) const fn error(&self) -> BluetoothSchedulerHeadPublicationError {
        self.error
    }

    pub(crate) fn into_merged(
        self,
    ) -> BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
        self.merged
    }
}

/// Common-list identity and head encoding validated without publication.
#[must_use = "prepare RUN interrupts, then commit and publish, or recover the merge"]
pub(crate) struct BluetoothPeripheralConnectionRecurringSchedulerValidated {
    merged: BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    head: BluetoothSchedulerHardwareListHead,
}

impl BluetoothPeripheralConnectionRecurringSchedulerValidated {
    pub(crate) fn into_merged(
        self,
    ) -> BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
        self.merged
    }

    /// Consume the final fallible RUN prerequisite and atomically commit LL and phase.
    pub(crate) fn commit(
        self,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothPeripheralConnectionRecurringSchedulerCommitted {
        let BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
            event,
            reservation,
        } = self.merged;
        let event = event.commit();
        BluetoothPeripheralConnectionRecurringSchedulerCommitted {
            event,
            reservation,
            head: self.head,
            interrupts,
        }
    }
}

/// Committed recurring event whose remaining publication suffix is infallible.
#[must_use = "publish RX/head and the common scheduler RUN suffix"]
pub(crate) struct BluetoothPeripheralConnectionRecurringSchedulerCommitted {
    event: BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    head: BluetoothSchedulerHardwareListHead,
    interrupts: BluetoothSchedulerRunInterruptsPrepared,
}

impl BluetoothPeripheralConnectionRecurringSchedulerCommitted {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionRecurringSchedulerPublicationPrepared,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
        BluetoothSchedulerHardwareListHead,
        BluetoothSchedulerRunInterruptsPrepared,
    ) {
        (self.event, self.reservation, self.head, self.interrupts)
    }
}

impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    /// Reserve one exact recurring connection window without displacement.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring candidate"
    )]
    pub(crate) fn admit_peripheral_connection_recurring_event(
        &mut self,
        candidate: BluetoothPeripheralConnectionRecurringEventCandidate,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringPreSequence,
        BluetoothPeripheralConnectionRecurringEventPreparationFailure,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(raw_window.start(), raw_window.end(), timing_policy)
        {
            Ok(reservation) => Ok(BluetoothPeripheralConnectionRecurringPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(
                BluetoothPeripheralConnectionRecurringEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionRecurringEventPreparationError::Timeline(
                        error,
                    ),
                },
            ),
        }
    }

    /// Authorize the recurring deadline and encode its infallible event fields.
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring candidate"
    )]
    pub(crate) fn prepare_peripheral_connection_recurring_event(
        &mut self,
        admitted: BluetoothPeripheralConnectionRecurringPreSequence,
        sequence: BluetoothPeripheralConnectionSequenceObservation,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringEventPrepared,
        BluetoothPeripheralConnectionRecurringEventPreparationFailure,
    > {
        let BluetoothPeripheralConnectionRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation =
            match reservation.authorize_sequence(sequence.sample) {
                Ok(reservation) => reservation,
                Err(failure) => {
                    let error = failure.error();
                    self.release_scheduler_reservation(failure.into_reservation());
                    return Err(BluetoothPeripheralConnectionRecurringEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionRecurringEventPreparationError::Sequence(
                        error,
                    ),
                });
                }
            };
        Ok(BluetoothPeripheralConnectionRecurringEventPrepared {
            event: candidate.prepare_event_fields(),
            reservation,
        })
    }

    pub(crate) fn cancel_peripheral_connection_recurring_pre_sequence(
        &mut self,
        admitted: BluetoothPeripheralConnectionRecurringPreSequence,
    ) -> (
        BluetoothPeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let BluetoothPeripheralConnectionRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    pub(crate) fn cancel_peripheral_connection_recurring_event(
        &mut self,
        prepared: BluetoothPeripheralConnectionRecurringEventPrepared,
    ) -> (
        BluetoothPeripheralConnectionSchedulerCompleted,
        LePeripheralConnectionEventDelta,
    ) {
        let BluetoothPeripheralConnectionRecurringEventPrepared { event, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        event.cancel()
    }

    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    pub(crate) fn prepare_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        prepared: BluetoothPeripheralConnectionRecurringEventPrepared,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
        BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
    > {
        let BluetoothPeripheralConnectionRecurringEventPrepared { event, reservation } = prepared;
        let (graph, transaction) = event.into_scheduler_parts();
        let event = BluetoothPeripheralConnectionRecurringSchedulerAdmissionPrepared {
            graph: graph.prepare_scheduler_admission(),
            transaction: BluetoothPeripheralConnectionRecurringSchedulerTransaction {
                handoff: transaction,
            },
        };
        let address = event.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(
                BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure {
                    error,
                    prepared: BluetoothPeripheralConnectionRecurringEventPrepared {
                        event: event.restore_event_fields(),
                        reservation,
                    },
                },
            );
        }
        Ok(
            BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
                event,
                reservation,
            },
        )
    }

    #[allow(
        clippy::result_large_err,
        reason = "an identity rejection retains the complete recurring merge"
    )]
    pub(crate) fn cancel_peripheral_connection_recurring_empty_list_merge(
        &mut self,
        merged: BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringEventPrepared,
        BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared {
            event,
            reservation,
        } = merged;
        Ok(BluetoothPeripheralConnectionRecurringEventPrepared {
            event: event.restore_event_fields(),
            reservation,
        })
    }

    /// Seal the exact common-list identity and encodable hardware head.
    #[allow(
        clippy::result_large_err,
        reason = "validation failure retains the complete recurring merge"
    )]
    pub(crate) fn validate_peripheral_connection_recurring_scheduler(
        &self,
        merged: BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionRecurringSchedulerValidated,
        BluetoothPeripheralConnectionRecurringSchedulerValidationFailure,
    > {
        let address = merged.scheduler_item_address();
        match self.validate_first_scheduler_item_head(address) {
            Ok(head) => {
                Ok(BluetoothPeripheralConnectionRecurringSchedulerValidated { merged, head })
            }
            Err(error) => Err(
                BluetoothPeripheralConnectionRecurringSchedulerValidationFailure { error, merged },
            ),
        }
    }

    /// Publish the already committed event through the infallible RX/head suffix.
    #[allow(
        unsafe_code,
        reason = "sealed list/head validation and the committed graph retain every PAC prerequisite"
    )]
    pub(crate) fn publish_peripheral_connection_recurring_scheduler_head(
        &mut self,
        committed: BluetoothPeripheralConnectionRecurringSchedulerCommitted,
    ) -> (
        super::BluetoothPeripheralConnectionSchedulerHeadPublished,
        BluetoothSchedulerRunInterruptsPrepared,
    ) {
        let address = committed.scheduler_item_address();
        let index = BluetoothSchedulerHardwareListIndex::ZERO;
        let (event, reservation, head, interrupts) = committed.into_parts();
        let (graph, remainder) = event.into_parts();
        let graph = unsafe { self.task.publish_peripheral_connection_rx_memory(graph) };
        let event = remainder.join_rx_publication(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        (
            super::BluetoothPeripheralConnectionSchedulerHeadPublished {
                event,
                publication,
                reservation,
            },
            interrupts,
        )
    }
}
