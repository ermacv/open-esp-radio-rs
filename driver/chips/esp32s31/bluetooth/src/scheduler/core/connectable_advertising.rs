//! Common-timeline admission for one response-capable legacy advertisement.
//!
//! This stage owns no MMIO authority. It reserves the complete response window,
//! applies the overlap-resolved endpoints to the CPU-owned graph, and joins the
//! sole item to the independently proven empty software-list epoch.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError;
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::BluetoothControllerRandomAddress;
use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;

use super::BluetoothSchedulerEmptyListMergeError;
#[cfg(target_arch = "riscv32")]
use crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingPublicationPrepared;
use crate::{
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerTimeSample,
    BluetoothSchedulerReservationError, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerTimingPolicy,
    le::advertising::connectable::{
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared,
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingEventImagePrepared,
    },
    scheduler::timeline::{
        BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerRecurringReserved,
        BluetoothSchedulerSequenceReady, BluetoothSchedulerWindowReservation,
    },
};

/// Fresh initial-admission sample sealed by the controller-time worker.
#[must_use = "the fresh connectable-advertising admission sample must be consumed or retained"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingAdmissionObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[must_use = "the fresh connectable-advertising sequence sample must be consumed or retained"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingSequenceObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// First response-capable event admitted before its independent sequence gate.
#[must_use = "the admitted event must pass sequence authorization or be cancelled"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPreSequence {
    candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
    reservation: BluetoothLegacyConnectableAdvertisingPreSequenceReservation,
}

enum BluetoothLegacyConnectableAdvertisingPreSequenceReservation {
    Initial(BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>),
    Recurring(BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>),
}

/// Why one first response-capable event could not reach complete event fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    EventFields(BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
}

/// Lossless first-event admission or preparation rejection.
#[must_use = "retry, cancel, or retain the unchanged connectable-advertising candidate"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
    candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
    error: BluetoothLegacyConnectableAdvertisingEventPreparationError,
}

impl BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
    pub(crate) const fn error(&self) -> BluetoothLegacyConnectableAdvertisingEventPreparationError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> BluetoothLegacyConnectableAdvertisingEventCandidate {
        self.candidate
    }
}

/// Complete CPU event fields paired with the exact common-timeline reservation.
#[must_use = "the prepared event must be merged, cancelled, or retained"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEventPrepared {
    image: BluetoothLegacyConnectableAdvertisingEventImagePrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

/// Lossless rejection while joining the connectable event to the empty list.
#[must_use = "the unchanged connectable event remains prepared and CPU-owned"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure {
    error: BluetoothSchedulerEmptyListMergeError,
    prepared: BluetoothLegacyConnectableAdvertisingEventPrepared,
}

impl BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure {
    pub(crate) const fn error(&self) -> BluetoothSchedulerEmptyListMergeError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> BluetoothLegacyConnectableAdvertisingEventPrepared {
        self.prepared
    }
}

/// First response-capable event joined to the exclusive empty scheduler list.
#[must_use = "the merged event must enter an atomic publication or be cancelled"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared {
    item: BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

/// Complete pre-MMIO owner after freezing the response-capable memory graph.
#[must_use = "the publication owner must enter the atomic MMIO suffix or be cancelled"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
    item: BluetoothLegacyConnectableAdvertisingPublicationPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
    pub(crate) const fn random_address(&self) -> Option<BluetoothControllerRandomAddress> {
        self.item.random_address()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingPublicationPrepared,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.item, self.reservation)
    }
}

/// Failed empty-list cancellation retaining every owner at its exact stage.
#[must_use = "the retained list merge or ownership invariant must remain fail-stop owned"]
pub(crate) enum BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure {
    ListIdentity {
        _merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    },
    Ownership {
        _invariant: BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    },
}

impl BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Freeze the complete graph immediately before the atomic publication suffix.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_publication(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
        BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
            item: self.item.prepare_publication(),
            reservation: self.reservation,
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "connectable admission and rollback retain the exact event graph and scheduler reservation without allocation"
)]
impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    /// Admit the complete response-capable first window into the common timeline.
    pub(crate) fn admit_legacy_connectable_advertising_first_event(
        &mut self,
        candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
        admission: BluetoothLegacyConnectableAdvertisingAdmissionObservation,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPreSequence,
        BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let requested = candidate.raw_window();
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
            Ok(reservation) => Ok(BluetoothLegacyConnectableAdvertisingPreSequence {
                candidate,
                reservation: BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Initial(
                    reservation,
                ),
            }),
            Err(error) => Err(
                BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
                        error,
                    ),
                },
            ),
        }
    }

    /// Reserve one exact phase-locked response-capable successor.
    ///
    /// Recurrence never enters the initial overlap-displacement path: changing
    /// this start would corrupt the portable interval phase. A collision is a
    /// finite retry retaining the complete candidate.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn admit_legacy_connectable_advertising_recurring_event(
        &mut self,
        candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingPreSequence,
        BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let requested = candidate.raw_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(requested.start(), requested.end(), timing_policy)
        {
            Ok(reservation) => Ok(BluetoothLegacyConnectableAdvertisingPreSequence {
                candidate,
                reservation: BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Recurring(
                    reservation,
                ),
            }),
            Err(error) => Err(
                BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
                        error,
                    ),
                },
            ),
        }
    }

    /// Authorize the fresh sequence sample and encode accepted start/end fields.
    pub(crate) fn prepare_legacy_connectable_advertising_event(
        &mut self,
        admitted: BluetoothLegacyConnectableAdvertisingPreSequence,
        sequence: BluetoothLegacyConnectableAdvertisingSequenceObservation,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingEventPrepared,
        BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let BluetoothLegacyConnectableAdvertisingPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation {
            BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Initial(reservation) => {
                match reservation.authorize_sequence(sequence.sample) {
                    Ok(reservation) => reservation,
                    Err(failure) => {
                        let error = failure.error();
                        self.release_scheduler_reservation(failure.into_reservation());
                        return Err(BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
                            candidate,
                            error:
                                BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(
                                    error,
                                ),
                        });
                    }
                }
            }
            BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Recurring(reservation) => {
                match reservation.authorize_sequence(sequence.sample) {
                    Ok(reservation) => reservation,
                    Err(failure) => {
                        let error = failure.error();
                        self.release_scheduler_reservation(failure.into_reservation());
                        return Err(BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
                        candidate,
                        error:
                            BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(
                                error,
                            ),
                    });
                    }
                }
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(resolved_window) {
            Ok(image) => {
                Ok(BluetoothLegacyConnectableAdvertisingEventPrepared { image, reservation })
            }
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(
                    BluetoothLegacyConnectableAdvertisingEventPreparationFailure {
                        candidate,
                        error:
                            BluetoothLegacyConnectableAdvertisingEventPreparationError::EventFields(
                                error,
                            ),
                    },
                )
            }
        }
    }

    /// Release an admitted event before its fresh sequence sample arrives.
    pub(crate) fn cancel_legacy_connectable_advertising_pre_sequence(
        &mut self,
        admitted: BluetoothLegacyConnectableAdvertisingPreSequence,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        let BluetoothLegacyConnectableAdvertisingPreSequence {
            candidate,
            reservation,
        } = admitted;
        match reservation {
            BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Initial(reservation) => {
                self.release_scheduler_reservation(reservation);
            }
            BluetoothLegacyConnectableAdvertisingPreSequenceReservation::Recurring(reservation) => {
                self.release_scheduler_reservation(reservation)
            }
        }
        candidate.cancel()
    }

    /// Release complete event fields and their sequence-ready reservation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_legacy_connectable_advertising_event(
        &mut self,
        prepared: BluetoothLegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        let BluetoothLegacyConnectableAdvertisingEventPrepared { image, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        image.cancel()
    }

    /// Join one response-capable item to this epoch's exact empty list.
    pub(crate) fn prepare_legacy_connectable_advertising_empty_list_merge(
        &mut self,
        prepared: BluetoothLegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    > {
        let BluetoothLegacyConnectableAdvertisingEventPrepared { image, reservation } = prepared;
        let item = image.prepare_scheduler_bookkeeping();
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(
                BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure {
                    error,
                    prepared: BluetoothLegacyConnectableAdvertisingEventPrepared {
                        image: item.cancel(),
                        reservation,
                    },
                },
            );
        }
        Ok(
            BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared {
                item: item.prepare_empty_list_link(),
                reservation,
            },
        )
    }

    /// Cancel only through the same exclusive list and timeline owners.
    pub(crate) fn cancel_legacy_connectable_advertising_empty_list_merge(
        &mut self,
        merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(
                BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure::ListIdentity {
                    _merged: merged,
                },
            );
        }
        let BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared { item, reservation } =
            merged;
        self.release_scheduler_reservation(reservation);
        item.cancel().map_err(|invariant| {
            BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure::Ownership {
                _invariant: invariant,
            }
        })
    }
}

#[cfg(test)]
mod tests;
