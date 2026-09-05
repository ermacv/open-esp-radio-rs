//! Peripheral-connection scheduler preparation and completion.
//!
//! This module owns the connection-specific descriptor and memory transitions.
//! The parent scheduler retains protocol-neutral timeline, list-epoch and MMIO
//! publication primitives.

#[cfg(target_arch = "riscv32")]
use core::ops::ControlFlow;

#[cfg(target_arch = "riscv32")]
mod recurring;
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "peripheral_connection/recurring/transaction.rs"]
mod recurring_transaction_tests;
#[cfg(target_arch = "riscv32")]
pub use recurring::{
    BluetoothPeripheralConnectionRecurringCandidateError,
    BluetoothPeripheralConnectionRecurringEmptySchedulerMergeFailure,
    BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    BluetoothPeripheralConnectionRecurringEventCandidate,
    BluetoothPeripheralConnectionRecurringEventPreparationError,
    BluetoothPeripheralConnectionRecurringEventPreparationFailure,
    BluetoothPeripheralConnectionRecurringEventPrepared,
    BluetoothPeripheralConnectionRecurringPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use recurring::{
    BluetoothPeripheralConnectionRecurringSchedulerPublicationFailStop,
    BluetoothPeripheralConnectionRecurringSchedulerValidationFailure,
};

use super::BluetoothSchedulerEmptyListMergeError;
#[cfg(target_arch = "riscv32")]
use super::BluetoothSchedulerHeadPublicationError;

use crate::BluetoothControllerPoweredTaskRuntime;
#[cfg(target_arch = "riscv32")]
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionCompletedEvent,
    BluetoothPeripheralConnectionCompletionClassification,
    BluetoothPeripheralConnectionFirstEventPublicationRemainder,
    BluetoothPeripheralConnectionFirstEventRxPublished,
    BluetoothPeripheralConnectionPacketStartTiming, BluetoothPeripheralConnectionRecycledEvent,
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionFirstEventCandidate,
    BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared,
    BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared,
};
#[cfg(target_arch = "riscv32")]
use crate::peripheral_connection_completion::{
    BluetoothPeripheralConnectionCompletionRole, BluetoothPeripheralConnectionRecycleFailure,
    BluetoothPeripheralConnectionRecycleFailureCause, BluetoothPeripheralConnectionRecycleOutcome,
    BluetoothPeripheralConnectionRecycleReady,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::BluetoothSingleItemSchedulerSoftwareListRemovalReady;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler::timeline::{
    BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerWindowReservation,
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothControllerTimeSample, BluetoothSchedulerReservationError,
    BluetoothSchedulerSequenceAuthorizationError, BluetoothSchedulerSequenceReady,
    BluetoothSchedulerTimingPolicy,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPeripheralConnectionMemoryGraphPublicationError,
    BluetoothPeripheralConnectionMemoryGraphPublicationMismatch,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListIndex,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
};

/// Fresh initial-admission sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh connection admission observation must be consumed or retained"]
pub(crate) struct BluetoothPeripheralConnectionAdmissionObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh connection sequence observation must be consumed or retained"]
pub(crate) struct BluetoothPeripheralConnectionSequenceObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// First connection event after timeline admission and before sequence authorization.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted connection event must pass sequence authorization or be cancelled"]
pub(crate) struct BluetoothPeripheralConnectionFirstPreSequence {
    candidate: BluetoothPeripheralConnectionFirstEventCandidate,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
}

/// Why one CPU-owned connection candidate could not complete scheduler preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothPeripheralConnectionFirstEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    Descriptor,
}

/// Lossless failure before connection scheduler-list publication.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connection candidate must be retried, cancelled, or retained"]
pub(crate) struct BluetoothPeripheralConnectionFirstEventPreparationFailure {
    candidate: BluetoothPeripheralConnectionFirstEventCandidate,
    error: BluetoothPeripheralConnectionFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPeripheralConnectionFirstEventPreparationFailure {
    pub(crate) const fn error(&self) -> BluetoothPeripheralConnectionFirstEventPreparationError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> BluetoothPeripheralConnectionFirstEventCandidate {
        self.candidate
    }
}

/// Sequence-authorized connection image paired with its exact timeline slot.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared connection event must be merged, cancelled, or retained"]
pub(crate) struct BluetoothPeripheralConnectionEventPrepared {
    event: BluetoothPeripheralConnectionFirstEventDirectionFindingPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(test)]
impl BluetoothPeripheralConnectionEventPrepared {
    pub(crate) const fn requested_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.event.requested_window()
    }

    pub(crate) const fn resolved_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.event.resolved_window()
    }
}

/// Lossless rejection while joining one detached connection item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connection event remains prepared and CPU-owned"]
pub(crate) struct BluetoothPeripheralConnectionEmptySchedulerMergeFailure {
    error: BluetoothSchedulerEmptyListMergeError,
    prepared: BluetoothPeripheralConnectionEventPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPeripheralConnectionEmptySchedulerMergeFailure {
    pub(crate) const fn error(&self) -> BluetoothSchedulerEmptyListMergeError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> BluetoothPeripheralConnectionEventPrepared {
        self.prepared
    }
}

/// Detached connection item joined to the source-owned empty scheduler list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the connection merge must be published or cancelled"]
pub struct BluetoothPeripheralConnectionEmptySchedulerMergePrepared {
    event: BluetoothPeripheralConnectionFirstEventSchedulerAdmissionPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPeripheralConnectionEmptySchedulerMergePrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    pub(crate) const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }
}

/// Lossless connection-head publication failure.
///
/// A head-validation rejection remains retryable because it precedes MMIO. An
/// RX proof mismatch follows an irreversible RX-list publication and therefore
/// seals every remaining affine owner without exposing a rollback operation.
#[cfg(target_arch = "riscv32")]
#[must_use = "retry only a pre-publication rejection; retain an RX mismatch fail-stop"]
pub struct BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
    ownership: BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership,
}

#[cfg(target_arch = "riscv32")]
#[allow(
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "each no-alloc variant retains a distinct complete affine publication phase"
)]
enum BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership {
    PrePublication {
        error: BluetoothSchedulerHeadPublicationError,
        merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    },
    FirstRxPublication {
        mismatch: BluetoothPeripheralConnectionMemoryGraphPublicationMismatch,
        _remainder: BluetoothPeripheralConnectionFirstEventPublicationRemainder,
        _reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
        _head: BluetoothSchedulerHardwareListHead,
    },
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
    /// Exact post-MMIO proof mismatch, when this failure is permanently sealed.
    pub const fn rx_publication_error(
        &self,
    ) -> Option<BluetoothPeripheralConnectionMemoryGraphPublicationError> {
        match &self.ownership {
            BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
                ..
            } => None,
            BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership::FirstRxPublication {
                mismatch,
                ..
            } => Some(mismatch.error()),
        }
    }

    /// Recover the unchanged merge only when no MMIO publication occurred.
    #[expect(
        clippy::result_large_err,
        reason = "the fail-stop error retains every post-publication affine owner"
    )]
    pub fn into_retryable_parts(
        self,
    ) -> Result<
        (
            BluetoothSchedulerHeadPublicationError,
            BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
        ),
        BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        match self.ownership {
            BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
                error,
                merged,
            } => Ok((error, merged)),
            ownership => Err(Self { ownership }),
        }
    }
}

/// Connection RX list and scheduler head made hardware-visible in one order.
#[cfg(target_arch = "riscv32")]
#[must_use = "the connection head must advance through the common RUN suffix"]
pub struct BluetoothPeripheralConnectionSchedulerHeadPublished {
    event: BluetoothPeripheralConnectionFirstEventRxPublished,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerHeadPublished {
    /// Exact selected event item retained by both hardware publications.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_head()
    }

    /// Hardware list containing the first connection item.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    /// Link Layer counter retained by the event before RUN publication.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothPeripheralConnectionFirstEventRxPublished,
        BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.event, self.publication, self.reservation)
    }
}

/// Active connection owner after event-local memory and scheduler reclamation.
#[must_use = "the recycled connection must classify peer activity before protocol advance"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerRecycled {
    event: BluetoothPeripheralConnectionRecycledEvent,
}

/// Completion outcome retaining either the unchanged retry owner or closed event.
#[must_use = "the unchanged retry owner or completed owner must be retained"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothPeripheralConnectionSchedulerCompletionClassification {
    NormalizationUnavailable(BluetoothPeripheralConnectionSchedulerRecycled),
    Completed(BluetoothPeripheralConnectionSchedulerCompleted),
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerRecycled {
    /// Link Layer counter retained without advancement after lower reclamation.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Opaque hardware status retained without teardown interpretation.
    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    /// Copied receive batch which no longer aliases controller SRAM.
    pub const fn received(
        &self,
    ) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch<
        { open_esp_radio_esp32s31_bluetooth_memory::BLUETOOTH_NON_SCANNING_RX_NODE_COUNT },
    > {
        self.event.received()
    }

    pub(crate) fn classify_completion(
        self,
        normalize: impl FnOnce(
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionCapturedAnchorTime,
        ) -> Option<BluetoothPeripheralConnectionPacketStartTiming>,
    ) -> BluetoothPeripheralConnectionSchedulerCompletionClassification {
        match self.event.classify_completion(normalize) {
            BluetoothPeripheralConnectionCompletionClassification::NormalizationUnavailable(
                event,
            ) => {
                BluetoothPeripheralConnectionSchedulerCompletionClassification::NormalizationUnavailable(
                    BluetoothPeripheralConnectionSchedulerRecycled { event },
                )
            }
            BluetoothPeripheralConnectionCompletionClassification::Completed(event) => {
                BluetoothPeripheralConnectionSchedulerCompletionClassification::Completed(
                    BluetoothPeripheralConnectionSchedulerCompleted { event },
                )
            }
        }
    }
}

/// Closed portable event retaining its active chip allocation and observations.
#[must_use = "the completed connection must enter recurrence or teardown"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerCompleted {
    event: BluetoothPeripheralConnectionCompletedEvent,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerCompleted {
    /// Portable completion record with the exactly-once advanced successor.
    pub const fn link_layer_completion(
        &self,
    ) -> &open_esp_radio_bluetooth_ll::connection::LePeripheralConnectionEventCompleted {
        self.event.link_layer_completion()
    }

    pub const fn event_counter(&self) -> u16 {
        self.link_layer_completion().event_counter()
    }

    /// Opaque hardware completion status. It carries no teardown policy.
    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    pub const fn received(
        &self,
    ) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch<
        { open_esp_radio_esp32s31_bluetooth_memory::BLUETOOTH_NON_SCANNING_RX_NODE_COUNT },
    > {
        self.event.received()
    }

    /// Normalized packet start when peer activity was captured.
    pub const fn packet_start(&self) -> Option<&BluetoothPeripheralConnectionPacketStartTiming> {
        self.event.packet_start()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    /// Admit one causal first-connection window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact affine connection candidate"
    )]
    pub(crate) fn admit_peripheral_connection_first_event(
        &mut self,
        candidate: BluetoothPeripheralConnectionFirstEventCandidate,
        admission: BluetoothPeripheralConnectionAdmissionObservation,
    ) -> Result<
        BluetoothPeripheralConnectionFirstPreSequence,
        BluetoothPeripheralConnectionFirstEventPreparationFailure,
    > {
        let requested = candidate.requested_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(BluetoothPeripheralConnectionFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                candidate,
                error: BluetoothPeripheralConnectionFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the second deadline and encode only the resolved connection window.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact affine connection candidate"
    )]
    pub(crate) fn prepare_peripheral_connection_first_event(
        &mut self,
        admitted: BluetoothPeripheralConnectionFirstPreSequence,
        sequence: BluetoothPeripheralConnectionSequenceObservation,
        default_tx_power: open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionDefaultTxPowerDbm,
        direction_finding_workspace: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDirectionFindingWorkspaceLink,
    ) -> Result<
        BluetoothPeripheralConnectionEventPrepared,
        BluetoothPeripheralConnectionFirstEventPreparationFailure,
    > {
        let BluetoothPeripheralConnectionFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_fields(resolved_window, default_tx_power) {
            Ok(event) => Ok(BluetoothPeripheralConnectionEventPrepared {
                event: event.install_direction_finding_workspace(direction_finding_workspace),
                reservation,
            }),
            Err(candidate) => {
                self.release_scheduler_reservation(reservation);
                Err(BluetoothPeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothPeripheralConnectionFirstEventPreparationError::Descriptor,
                })
            }
        }
    }

    /// Release one unpublished connection event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_event(
        &mut self,
        prepared: BluetoothPeripheralConnectionEventPrepared,
    ) -> (
        crate::BluetoothPeripheralConnectionRuntimeAllocation,
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let BluetoothPeripheralConnectionEventPrepared { event, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        event.cancel()
    }

    /// Release an admitted connection candidate before sequence authorization.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_pre_sequence(
        &mut self,
        admitted: BluetoothPeripheralConnectionFirstPreSequence,
    ) -> (
        crate::BluetoothPeripheralConnectionRuntimeAllocation,
        open_esp_radio_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let BluetoothPeripheralConnectionFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    /// Join the selected connection item to this epoch's empty scheduler list.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure retains the complete affine connection event"
    )]
    pub(crate) fn prepare_peripheral_connection_empty_list_merge(
        &mut self,
        prepared: BluetoothPeripheralConnectionEventPrepared,
    ) -> Result<
        BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
        BluetoothPeripheralConnectionEmptySchedulerMergeFailure,
    > {
        let BluetoothPeripheralConnectionEventPrepared { event, reservation } = prepared;
        let event = event.prepare_scheduler_admission();
        let address = event.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothPeripheralConnectionEmptySchedulerMergeFailure {
                error,
                prepared: BluetoothPeripheralConnectionEventPrepared {
                    event: event.cancel(),
                    reservation,
                },
            });
        }
        Ok(BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation })
    }

    /// Restore an unpublished connection merge through the same scheduler epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc cancellation failure retains the complete affine merge"
    )]
    pub(crate) fn cancel_peripheral_connection_empty_list_merge(
        &mut self,
        merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionEventPrepared,
        BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation } =
            merged;
        Ok(BluetoothPeripheralConnectionEventPrepared {
            event: event.cancel(),
            reservation,
        })
    }

    /// Publish selector-two RX memory and the exact connection scheduler head.
    ///
    /// Common-list identity is validated before the first irreversible MMIO.
    /// An RX publication proof mismatch after that boundary is returned as a
    /// sealed fail-stop retaining every affine owner; only a validated join
    /// may continue to scheduler-head publication.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        clippy::result_large_err,
        reason = "the powered task owner and exact connection graph retain every PAC publication prerequisite"
    )]
    pub(crate) fn publish_peripheral_connection_scheduler_head(
        &mut self,
        merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPeripheralConnectionSchedulerHeadPublished,
        BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
                    ownership:
                        BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
                            error,
                            merged,
                        },
                });
            }
        };
        let BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation } =
            merged;
        let (graph, remainder) = event.prepare_publication().into_parts();
        let graph = match unsafe { self.task.publish_peripheral_connection_rx_memory(graph) } {
            Ok(graph) => graph,
            Err(mismatch) => {
                return Err(BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
                    ownership:
                        BluetoothPeripheralConnectionSchedulerHeadPublicationFailureOwnership::FirstRxPublication {
                            mismatch,
                            _remainder: remainder,
                            _reservation: reservation,
                            _head: head,
                        },
                });
            }
        };
        let event = remainder.join_rx_publication(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(BluetoothPeripheralConnectionSchedulerHeadPublished {
            event,
            publication,
            reservation,
        })
    }

    /// Copy RX results and release the connection event's three lower owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_peripheral_connection_completed(
        &mut self,
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            BluetoothPeripheralConnectionCompletionRole,
        >,
    ) -> BluetoothPeripheralConnectionRecycleOutcome {
        let (event, removal, reservation) = ready.into_parts();
        let ready = BluetoothPeripheralConnectionRecycleReady::new(event, removal, reservation);
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return ControlFlow::Break(BluetoothPeripheralConnectionRecycleFailure::new(
                BluetoothPeripheralConnectionRecycleFailureCause::SchedulerIdentityMismatch,
                ready,
            ));
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return ControlFlow::Break(BluetoothPeripheralConnectionRecycleFailure::new(
                BluetoothPeripheralConnectionRecycleFailureCause::FinishedListDrainStillActive,
                ready,
            ));
        }
        let (event, removal, reservation) = ready.into_parts();
        let prepared = match event.prepare_recycle(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_parts();
                return ControlFlow::Break(BluetoothPeripheralConnectionRecycleFailure::new(
                    BluetoothPeripheralConnectionRecycleFailureCause::MemoryIdentityMismatch(error),
                    BluetoothPeripheralConnectionRecycleReady::new(event, removal, reservation),
                ));
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_prepared().into_parts();
                return ControlFlow::Break(BluetoothPeripheralConnectionRecycleFailure::new(
                    BluetoothPeripheralConnectionRecycleFailureCause::ReceiveInvalid(error),
                    BluetoothPeripheralConnectionRecycleReady::new(event, removal, reservation),
                ));
            }
        };
        let release = match self
            .runtime
            .scheduler_timeline_mut()
            .prepare_release(reservation)
        {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (event, removal) = extracted.into_prepared().into_parts();
                return ControlFlow::Break(BluetoothPeripheralConnectionRecycleFailure::new(
                    BluetoothPeripheralConnectionRecycleFailureCause::ReservationIdentityMismatch,
                    BluetoothPeripheralConnectionRecycleReady::new(event, removal, reservation),
                ));
            }
        };
        let event = extracted.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        ControlFlow::Continue(BluetoothPeripheralConnectionSchedulerRecycled { event })
    }
}
