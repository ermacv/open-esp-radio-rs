//! Common-timeline admission for one response-capable legacy advertisement.
//!
//! This stage owns no MMIO authority. It reserves the complete response window,
//! applies the overlap-resolved endpoints to the CPU-owned graph, and joins the
//! sole item to the independently proven empty software-list epoch.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::le::advertising::connectable::LegacyConnectableAdvertisingPublicationPrepared;
use oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ControllerRandomAddress;

use crate::{
    ControllerTimeSample,
    le::advertising::connectable::{
        LegacyConnectableAdvertisingCancellationInvariant, LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingEmptyListLinkPrepared,
        LegacyConnectableAdvertisingEventCandidate, LegacyConnectableAdvertisingEventImagePrepared,
    },
    runtime_resources::ControllerPoweredTaskRuntime,
    scheduler::{
        SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerTimingPolicy,
        timeline::{
            SchedulerInitialAdmissionResolved, SchedulerRecurringReserved, SchedulerSequenceReady,
            SchedulerWindowReservation,
        },
    },
};

use oer_esp32s31_hal::types::BluetoothControllerSramAddress;

use super::SchedulerEmptyListMergeError;

/// Fresh initial-admission sample sealed by the controller-time worker.
#[must_use = "the fresh connectable-advertising admission sample must be consumed or retained"]
pub(crate) struct LegacyConnectableAdvertisingAdmissionObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[must_use = "the fresh connectable-advertising sequence sample must be consumed or retained"]
pub(crate) struct LegacyConnectableAdvertisingSequenceObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// First response-capable event admitted before its independent sequence gate.
#[must_use = "the admitted event must pass sequence authorization or be cancelled"]
pub(crate) struct LegacyConnectableAdvertisingPreSequence {
    candidate: LegacyConnectableAdvertisingEventCandidate,
    reservation: LegacyConnectableAdvertisingPreSequenceReservation,
}

enum LegacyConnectableAdvertisingPreSequenceReservation {
    Initial(SchedulerWindowReservation<SchedulerInitialAdmissionResolved>),
    Recurring(SchedulerWindowReservation<SchedulerRecurringReserved>),
}

/// Why one first response-capable event could not reach complete event fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    EventFields(LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
}

/// Lossless first-event admission or preparation rejection.
#[must_use = "retry, cancel, or retain the unchanged connectable-advertising candidate"]
pub(crate) struct LegacyConnectableAdvertisingEventPreparationFailure {
    candidate: LegacyConnectableAdvertisingEventCandidate,
    error: LegacyConnectableAdvertisingEventPreparationError,
}

impl LegacyConnectableAdvertisingEventPreparationFailure {
    pub(crate) const fn error(&self) -> LegacyConnectableAdvertisingEventPreparationError {
        self.error
    }

    pub(crate) fn into_candidate(self) -> LegacyConnectableAdvertisingEventCandidate {
        self.candidate
    }
}

/// Complete CPU event fields paired with the exact common-timeline reservation.
#[must_use = "the prepared event must be merged, cancelled, or retained"]
pub(crate) struct LegacyConnectableAdvertisingEventPrepared {
    image: LegacyConnectableAdvertisingEventImagePrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

/// Lossless rejection while joining the connectable event to the empty list.
#[must_use = "the unchanged connectable event remains prepared and CPU-owned"]
pub(crate) struct LegacyConnectableAdvertisingEmptySchedulerMergeFailure {
    error: SchedulerEmptyListMergeError,
    prepared: LegacyConnectableAdvertisingEventPrepared,
}

impl LegacyConnectableAdvertisingEmptySchedulerMergeFailure {
    pub(crate) const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    pub(crate) fn into_prepared(self) -> LegacyConnectableAdvertisingEventPrepared {
        self.prepared
    }
}

/// First response-capable event joined to the exclusive empty scheduler list.
#[must_use = "the merged event must enter an atomic publication or be cancelled"]
pub(crate) struct LegacyConnectableAdvertisingEmptySchedulerMergePrepared {
    item: LegacyConnectableAdvertisingEmptyListLinkPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

/// Complete pre-MMIO owner after freezing the response-capable memory graph.
#[must_use = "the publication owner must enter the atomic MMIO suffix or be cancelled"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct LegacyConnectableAdvertisingSchedulerPublicationPrepared {
    item: LegacyConnectableAdvertisingPublicationPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl LegacyConnectableAdvertisingSchedulerPublicationPrepared {
    pub(crate) const fn random_address(&self) -> Option<ControllerRandomAddress> {
        self.item.random_address()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingPublicationPrepared,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.item, self.reservation)
    }
}

/// Failed empty-list cancellation retaining every owner at its exact stage.
#[must_use = "the retained list merge or ownership invariant must remain fail-stop owned"]
pub(crate) enum LegacyConnectableAdvertisingEmptySchedulerCancelFailure {
    ListIdentity {
        _merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    },
    Ownership {
        _invariant: LegacyConnectableAdvertisingCancellationInvariant,
    },
}

impl LegacyConnectableAdvertisingEmptySchedulerMergePrepared {
    pub(crate) const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Freeze the complete graph immediately before the atomic publication suffix.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_publication(
        self,
    ) -> LegacyConnectableAdvertisingSchedulerPublicationPrepared {
        LegacyConnectableAdvertisingSchedulerPublicationPrepared {
            item: self.item.prepare_publication(),
            reservation: self.reservation,
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "connectable admission and rollback retain the exact event graph and scheduler reservation without allocation"
)]
impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    /// Admit the complete response-capable first window into the common timeline.
    pub(crate) fn admit_legacy_connectable_advertising_first_event(
        &mut self,
        candidate: LegacyConnectableAdvertisingEventCandidate,
        admission: LegacyConnectableAdvertisingAdmissionObservation,
    ) -> Result<
        LegacyConnectableAdvertisingPreSequence,
        LegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let requested = candidate.raw_window();
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
            Ok(reservation) => Ok(LegacyConnectableAdvertisingPreSequence {
                candidate,
                reservation: LegacyConnectableAdvertisingPreSequenceReservation::Initial(
                    reservation,
                ),
            }),
            Err(error) => Err(LegacyConnectableAdvertisingEventPreparationFailure {
                candidate,
                error: LegacyConnectableAdvertisingEventPreparationError::Timeline(error),
            }),
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
        candidate: LegacyConnectableAdvertisingEventCandidate,
    ) -> Result<
        LegacyConnectableAdvertisingPreSequence,
        LegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let requested = candidate.raw_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(requested.start(), requested.end(), timing_policy)
        {
            Ok(reservation) => Ok(LegacyConnectableAdvertisingPreSequence {
                candidate,
                reservation: LegacyConnectableAdvertisingPreSequenceReservation::Recurring(
                    reservation,
                ),
            }),
            Err(error) => Err(LegacyConnectableAdvertisingEventPreparationFailure {
                candidate,
                error: LegacyConnectableAdvertisingEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the fresh sequence sample and encode accepted start/end fields.
    pub(crate) fn prepare_legacy_connectable_advertising_event(
        &mut self,
        admitted: LegacyConnectableAdvertisingPreSequence,
        sequence: LegacyConnectableAdvertisingSequenceObservation,
    ) -> Result<
        LegacyConnectableAdvertisingEventPrepared,
        LegacyConnectableAdvertisingEventPreparationFailure,
    > {
        let LegacyConnectableAdvertisingPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation {
            LegacyConnectableAdvertisingPreSequenceReservation::Initial(reservation) => {
                match reservation.authorize_sequence(sequence.sample) {
                    Ok(reservation) => reservation,
                    Err(failure) => {
                        let error = failure.error();
                        self.release_scheduler_reservation(failure.into_reservation());
                        return Err(LegacyConnectableAdvertisingEventPreparationFailure {
                            candidate,
                            error: LegacyConnectableAdvertisingEventPreparationError::Sequence(
                                error,
                            ),
                        });
                    }
                }
            }
            LegacyConnectableAdvertisingPreSequenceReservation::Recurring(reservation) => {
                match reservation.authorize_sequence(sequence.sample) {
                    Ok(reservation) => reservation,
                    Err(failure) => {
                        let error = failure.error();
                        self.release_scheduler_reservation(failure.into_reservation());
                        return Err(LegacyConnectableAdvertisingEventPreparationFailure {
                            candidate,
                            error: LegacyConnectableAdvertisingEventPreparationError::Sequence(
                                error,
                            ),
                        });
                    }
                }
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(
            resolved_window,
            reservation.timing_policy().sequence_lead_raw_delta(),
        ) {
            Ok(image) => Ok(LegacyConnectableAdvertisingEventPrepared { image, reservation }),
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(LegacyConnectableAdvertisingEventPreparationFailure {
                    candidate,
                    error: LegacyConnectableAdvertisingEventPreparationError::EventFields(error),
                })
            }
        }
    }

    /// Release an admitted event before its fresh sequence sample arrives.
    pub(crate) fn cancel_legacy_connectable_advertising_pre_sequence(
        &mut self,
        admitted: LegacyConnectableAdvertisingPreSequence,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        let LegacyConnectableAdvertisingPreSequence {
            candidate,
            reservation,
        } = admitted;
        match reservation {
            LegacyConnectableAdvertisingPreSequenceReservation::Initial(reservation) => {
                self.release_scheduler_reservation(reservation);
            }
            LegacyConnectableAdvertisingPreSequenceReservation::Recurring(reservation) => {
                self.release_scheduler_reservation(reservation)
            }
        }
        candidate.cancel()
    }

    /// Release complete event fields and their sequence-ready reservation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_legacy_connectable_advertising_event(
        &mut self,
        prepared: LegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    > {
        let LegacyConnectableAdvertisingEventPrepared { image, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        image.cancel()
    }

    /// Join one response-capable item to this epoch's exact empty list.
    pub(crate) fn prepare_legacy_connectable_advertising_empty_list_merge(
        &mut self,
        prepared: LegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        LegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    > {
        let LegacyConnectableAdvertisingEventPrepared { image, reservation } = prepared;
        let item = image.prepare_scheduler_bookkeeping();
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(LegacyConnectableAdvertisingEmptySchedulerMergeFailure {
                error,
                prepared: LegacyConnectableAdvertisingEventPrepared {
                    image: item.cancel(),
                    reservation,
                },
            });
        }
        Ok(LegacyConnectableAdvertisingEmptySchedulerMergePrepared {
            item: item.prepare_empty_list_link(),
            reservation,
        })
    }

    /// Cancel only through the same exclusive list and timeline owners.
    pub(crate) fn cancel_legacy_connectable_advertising_empty_list_merge(
        &mut self,
        merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(
                LegacyConnectableAdvertisingEmptySchedulerCancelFailure::ListIdentity {
                    _merged: merged,
                },
            );
        }
        let LegacyConnectableAdvertisingEmptySchedulerMergePrepared { item, reservation } = merged;
        self.release_scheduler_reservation(reservation);
        item.cancel().map_err(|invariant| {
            LegacyConnectableAdvertisingEmptySchedulerCancelFailure::Ownership {
                _invariant: invariant,
            }
        })
    }
}

#[cfg(test)]
mod tests;
