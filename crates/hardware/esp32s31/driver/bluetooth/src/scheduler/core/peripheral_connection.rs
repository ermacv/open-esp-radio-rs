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
    PeripheralConnectionRecurringCandidateError,
    PeripheralConnectionRecurringEmptySchedulerMergeFailure,
    PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    PeripheralConnectionRecurringEventCandidate,
    PeripheralConnectionRecurringEventPreparationError,
    PeripheralConnectionRecurringEventPreparationFailure,
    PeripheralConnectionRecurringEventPrepared, PeripheralConnectionRecurringPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use recurring::{
    PeripheralConnectionRecurringSchedulerPublicationFailStop,
    PeripheralConnectionRecurringSchedulerValidationFailure,
};

use super::SchedulerEmptyListMergeError;
#[cfg(target_arch = "riscv32")]
use super::SchedulerHeadPublicationError;
#[cfg(any(target_arch = "riscv32", test))]
use crate::le::peripheral::connection::{
    PeripheralConnectionFirstEventCandidate,
    PeripheralConnectionFirstEventDirectionFindingPrepared,
    PeripheralConnectionFirstEventSchedulerAdmissionPrepared,
};
#[cfg(target_arch = "riscv32")]
use crate::le::peripheral::{
    completion::{
        PeripheralConnectionCompletionRole, PeripheralConnectionRecycleFailure,
        PeripheralConnectionRecycleFailureCause, PeripheralConnectionRecycleOutcome,
        PeripheralConnectionRecycleReady,
    },
    connection::{
        PeripheralConnectionCompletedEvent, PeripheralConnectionCompletionClassification,
        PeripheralConnectionFirstEventPublicationRemainder,
        PeripheralConnectionFirstEventRxPublished, PeripheralConnectionPacketStartTiming,
        PeripheralConnectionRecycledEvent,
    },
};

use crate::runtime_resources::ControllerPoweredTaskRuntime;
#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady;
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    ControllerTimeSample,
    scheduler::{
        SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerSequenceReady,
        SchedulerTimingPolicy,
        timeline::{SchedulerInitialAdmissionResolved, SchedulerWindowReservation},
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    PeripheralConnectionMemoryGraphPublicationError,
    PeripheralConnectionMemoryGraphPublicationMismatch,
    PeripheralConnectionSchedulerItemCompletionStatus,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
};

use oer_esp32s31_hal::{
    bluetooth::BluetoothSchedulerHardwareListIndex, types::BluetoothControllerSramAddress,
};

/// Fresh initial-admission sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh connection admission observation must be consumed or retained"]
pub(crate) struct PeripheralConnectionAdmissionObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh connection sequence observation must be consumed or retained"]
pub(crate) struct PeripheralConnectionSequenceObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// First connection event after timeline admission and before sequence authorization.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted connection event must pass sequence authorization or be cancelled"]
pub(crate) struct PeripheralConnectionFirstPreSequence {
    candidate: PeripheralConnectionFirstEventCandidate,
    reservation: SchedulerWindowReservation<SchedulerInitialAdmissionResolved>,
}

/// Why one CPU-owned connection candidate could not complete scheduler preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum PeripheralConnectionFirstEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    Descriptor,
}

/// Lossless failure before connection scheduler-list publication.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connection candidate must be retried, cancelled, or retained"]
pub(crate) struct PeripheralConnectionFirstEventPreparationFailure {
    candidate: PeripheralConnectionFirstEventCandidate,
    error: PeripheralConnectionFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionFirstEventPreparationFailure {
    pub(crate) const fn error(&self) -> PeripheralConnectionFirstEventPreparationError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> PeripheralConnectionFirstEventCandidate {
        self.candidate
    }
}

/// Sequence-authorized connection image paired with its exact timeline slot.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared connection event must be merged, cancelled, or retained"]
pub(crate) struct PeripheralConnectionEventPrepared {
    event: PeripheralConnectionFirstEventDirectionFindingPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(test)]
impl PeripheralConnectionEventPrepared {
    pub(crate) const fn requested_window(&self) -> crate::scheduler::SchedulerRawWindow {
        self.event.requested_window()
    }

    pub(crate) const fn resolved_window(&self) -> crate::scheduler::SchedulerRawWindow {
        self.event.resolved_window()
    }
}

/// Lossless rejection while joining one detached connection item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged connection event remains prepared and CPU-owned"]
pub(crate) struct PeripheralConnectionEmptySchedulerMergeFailure {
    error: SchedulerEmptyListMergeError,
    prepared: PeripheralConnectionEventPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionEmptySchedulerMergeFailure {
    pub(crate) const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> PeripheralConnectionEventPrepared {
        self.prepared
    }
}

/// Detached connection item joined to the source-owned empty scheduler list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the connection merge must be published or cancelled"]
pub struct PeripheralConnectionEmptySchedulerMergePrepared {
    event: PeripheralConnectionFirstEventSchedulerAdmissionPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PeripheralConnectionEmptySchedulerMergePrepared {
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
pub struct PeripheralConnectionSchedulerHeadPublicationFailure {
    ownership: PeripheralConnectionSchedulerHeadPublicationFailureOwnership,
}

#[cfg(target_arch = "riscv32")]
#[allow(
    clippy::enum_variant_names,
    clippy::large_enum_variant,
    reason = "each no-alloc variant retains a distinct complete affine publication phase"
)]
enum PeripheralConnectionSchedulerHeadPublicationFailureOwnership {
    PrePublication {
        error: SchedulerHeadPublicationError,
        merged: PeripheralConnectionEmptySchedulerMergePrepared,
    },
    FirstRxPublication {
        mismatch: PeripheralConnectionMemoryGraphPublicationMismatch,
        _remainder: PeripheralConnectionFirstEventPublicationRemainder,
        _reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
        _head: BluetoothSchedulerHardwareListHead,
    },
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionSchedulerHeadPublicationFailure {
    /// Exact post-MMIO proof mismatch, when this failure is permanently sealed.
    pub const fn rx_publication_error(
        &self,
    ) -> Option<PeripheralConnectionMemoryGraphPublicationError> {
        match &self.ownership {
            PeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
                ..
            } => None,
            PeripheralConnectionSchedulerHeadPublicationFailureOwnership::FirstRxPublication {
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
            SchedulerHeadPublicationError,
            PeripheralConnectionEmptySchedulerMergePrepared,
        ),
        PeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        match self.ownership {
            PeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
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
pub struct PeripheralConnectionSchedulerHeadPublished {
    event: PeripheralConnectionFirstEventRxPublished,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionSchedulerHeadPublished {
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
        PeripheralConnectionFirstEventRxPublished,
        BluetoothSchedulerHardwareListHeadPublished,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.event, self.publication, self.reservation)
    }
}

/// Active connection owner after event-local memory and scheduler reclamation.
#[must_use = "the recycled connection must classify peer activity before protocol advance"]
#[cfg(target_arch = "riscv32")]
pub struct PeripheralConnectionSchedulerRecycled {
    event: PeripheralConnectionRecycledEvent,
}

/// Completion outcome retaining either the unchanged retry owner or closed event.
#[must_use = "the unchanged retry owner or completed owner must be retained"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum PeripheralConnectionSchedulerCompletionClassification {
    NormalizationUnavailable(PeripheralConnectionSchedulerRecycled),
    Completed(PeripheralConnectionSchedulerCompleted),
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionSchedulerRecycled {
    /// Link Layer counter retained without advancement after lower reclamation.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Opaque hardware status retained without teardown interpretation.
    pub const fn status(&self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    /// Copied receive batch which no longer aliases controller SRAM.
    pub const fn received(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::LeReceivedBatch<
        { oer_esp32s31_bluetooth_memory::BLUETOOTH_NON_SCANNING_RX_NODE_COUNT },
    > {
        self.event.received()
    }

    pub(crate) fn classify_completion(
        self,
        normalize: impl FnOnce(
            oer_esp32s31_bluetooth_memory::PeripheralConnectionCapturedAnchorTime,
        ) -> Option<PeripheralConnectionPacketStartTiming>,
    ) -> PeripheralConnectionSchedulerCompletionClassification {
        match self.event.classify_completion(normalize) {
            PeripheralConnectionCompletionClassification::NormalizationUnavailable(event) => {
                PeripheralConnectionSchedulerCompletionClassification::NormalizationUnavailable(
                    PeripheralConnectionSchedulerRecycled { event },
                )
            }
            PeripheralConnectionCompletionClassification::Completed(event) => {
                PeripheralConnectionSchedulerCompletionClassification::Completed(
                    PeripheralConnectionSchedulerCompleted { event },
                )
            }
        }
    }
}

/// Closed portable event retaining its active chip allocation and observations.
#[must_use = "the completed connection must enter recurrence or teardown"]
#[cfg(target_arch = "riscv32")]
pub struct PeripheralConnectionSchedulerCompleted {
    event: PeripheralConnectionCompletedEvent,
}

#[cfg(target_arch = "riscv32")]
impl PeripheralConnectionSchedulerCompleted {
    /// Portable completion record with the exactly-once advanced successor.
    pub const fn link_layer_completion(
        &self,
    ) -> &oer_bluetooth_ll::connection::LePeripheralConnectionEventCompleted {
        self.event.link_layer_completion()
    }

    pub const fn event_counter(&self) -> u16 {
        self.link_layer_completion().event_counter()
    }

    /// Opaque hardware completion status. It carries no teardown policy.
    pub const fn status(&self) -> PeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    pub const fn received(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::LeReceivedBatch<
        { oer_esp32s31_bluetooth_memory::BLUETOOTH_NON_SCANNING_RX_NODE_COUNT },
    > {
        self.event.received()
    }

    /// Normalized packet start when peer activity was captured.
    pub const fn packet_start(&self) -> Option<&PeripheralConnectionPacketStartTiming> {
        self.event.packet_start()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    /// Admit one causal first-connection window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the exact affine connection candidate"
    )]
    pub(crate) fn admit_peripheral_connection_first_event(
        &mut self,
        candidate: PeripheralConnectionFirstEventCandidate,
        admission: PeripheralConnectionAdmissionObservation,
    ) -> Result<
        PeripheralConnectionFirstPreSequence,
        PeripheralConnectionFirstEventPreparationFailure,
    > {
        let requested = candidate.requested_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(PeripheralConnectionFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(PeripheralConnectionFirstEventPreparationFailure {
                candidate,
                error: PeripheralConnectionFirstEventPreparationError::Timeline(error),
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
        admitted: PeripheralConnectionFirstPreSequence,
        sequence: PeripheralConnectionSequenceObservation,
        default_tx_power: oer_esp32s31_bluetooth_memory::PeripheralConnectionDefaultTxPowerDbm,
        direction_finding_workspace: oer_esp32s31_bluetooth_memory::DirectionFindingWorkspaceLink,
    ) -> Result<PeripheralConnectionEventPrepared, PeripheralConnectionFirstEventPreparationFailure>
    {
        let PeripheralConnectionFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(PeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: PeripheralConnectionFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_fields(resolved_window, default_tx_power) {
            Ok(event) => Ok(PeripheralConnectionEventPrepared {
                event: event.install_direction_finding_workspace(direction_finding_workspace),
                reservation,
            }),
            Err(candidate) => {
                self.release_scheduler_reservation(reservation);
                Err(PeripheralConnectionFirstEventPreparationFailure {
                    candidate,
                    error: PeripheralConnectionFirstEventPreparationError::Descriptor,
                })
            }
        }
    }

    /// Release one unpublished connection event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_event(
        &mut self,
        prepared: PeripheralConnectionEventPrepared,
    ) -> (
        crate::le::peripheral::PeripheralConnectionRuntimeAllocation,
        oer_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let PeripheralConnectionEventPrepared { event, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        event.cancel()
    }

    /// Release an admitted connection candidate before sequence authorization.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_peripheral_connection_first_pre_sequence(
        &mut self,
        admitted: PeripheralConnectionFirstPreSequence,
    ) -> (
        crate::le::peripheral::PeripheralConnectionRuntimeAllocation,
        oer_bluetooth_ll::connection::LePeripheralConnection,
    ) {
        let PeripheralConnectionFirstPreSequence {
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
        prepared: PeripheralConnectionEventPrepared,
    ) -> Result<
        PeripheralConnectionEmptySchedulerMergePrepared,
        PeripheralConnectionEmptySchedulerMergeFailure,
    > {
        let PeripheralConnectionEventPrepared { event, reservation } = prepared;
        let event = event.prepare_scheduler_admission();
        let address = event.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(PeripheralConnectionEmptySchedulerMergeFailure {
                error,
                prepared: PeripheralConnectionEventPrepared {
                    event: event.cancel(),
                    reservation,
                },
            });
        }
        Ok(PeripheralConnectionEmptySchedulerMergePrepared { event, reservation })
    }

    /// Restore an unpublished connection merge through the same scheduler epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc cancellation failure retains the complete affine merge"
    )]
    #[cfg_attr(
        all(target_arch = "riscv32", not(test)),
        expect(
            dead_code,
            reason = "host ownership tests exercise pre-publication merge recovery; the current first-event actor publishes the merge directly"
        )
    )]
    pub(crate) fn cancel_peripheral_connection_empty_list_merge(
        &mut self,
        merged: PeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<PeripheralConnectionEventPrepared, PeripheralConnectionEmptySchedulerMergePrepared>
    {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let PeripheralConnectionEmptySchedulerMergePrepared { event, reservation } = merged;
        Ok(PeripheralConnectionEventPrepared {
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
        merged: PeripheralConnectionEmptySchedulerMergePrepared,
    ) -> Result<
        PeripheralConnectionSchedulerHeadPublished,
        PeripheralConnectionSchedulerHeadPublicationFailure,
    > {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(PeripheralConnectionSchedulerHeadPublicationFailure {
                    ownership:
                        PeripheralConnectionSchedulerHeadPublicationFailureOwnership::PrePublication {
                            error,
                            merged,
                        },
                });
            }
        };
        let PeripheralConnectionEmptySchedulerMergePrepared { event, reservation } = merged;
        let (graph, remainder) = event.prepare_publication().into_parts();
        let graph = match unsafe { self.task.publish_peripheral_connection_rx_memory(graph) } {
            Ok(graph) => graph,
            Err(mismatch) => {
                return Err(PeripheralConnectionSchedulerHeadPublicationFailure {
                    ownership:
                        PeripheralConnectionSchedulerHeadPublicationFailureOwnership::FirstRxPublication {
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
        Ok(PeripheralConnectionSchedulerHeadPublished {
            event,
            publication,
            reservation,
        })
    }

    /// Copy RX results and release the connection event's three lower owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_peripheral_connection_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<PeripheralConnectionCompletionRole>,
    ) -> PeripheralConnectionRecycleOutcome {
        let (event, removal, reservation) = ready.into_parts();
        let ready = PeripheralConnectionRecycleReady::new(event, removal, reservation);
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return ControlFlow::Break(PeripheralConnectionRecycleFailure::new(
                PeripheralConnectionRecycleFailureCause::SchedulerIdentityMismatch,
                ready,
            ));
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return ControlFlow::Break(PeripheralConnectionRecycleFailure::new(
                PeripheralConnectionRecycleFailureCause::FinishedListDrainStillActive,
                ready,
            ));
        }
        let (event, removal, reservation) = ready.into_parts();
        let prepared = match event.prepare_recycle(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_parts();
                return ControlFlow::Break(PeripheralConnectionRecycleFailure::new(
                    PeripheralConnectionRecycleFailureCause::MemoryIdentityMismatch(error),
                    PeripheralConnectionRecycleReady::new(event, removal, reservation),
                ));
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_prepared().into_parts();
                return ControlFlow::Break(PeripheralConnectionRecycleFailure::new(
                    PeripheralConnectionRecycleFailureCause::ReceiveInvalid(error),
                    PeripheralConnectionRecycleReady::new(event, removal, reservation),
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
                return ControlFlow::Break(PeripheralConnectionRecycleFailure::new(
                    PeripheralConnectionRecycleFailureCause::ReservationIdentityMismatch,
                    PeripheralConnectionRecycleReady::new(event, removal, reservation),
                ));
            }
        };
        let event = extracted.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        ControlFlow::Continue(PeripheralConnectionSchedulerRecycled { event })
    }
}
