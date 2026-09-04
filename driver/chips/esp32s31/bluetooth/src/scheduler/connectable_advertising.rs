//! Common-timeline admission for one response-capable legacy advertisement.
//!
//! This stage owns no MMIO authority. It reserves the complete response window,
//! applies the overlap-resolved endpoints to the CPU-owned graph, and joins the
//! sole item to the independently proven empty software-list epoch.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError;
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerRandomAddress, BluetoothControllerSramAddress,
};

use super::BluetoothSchedulerEmptyListMergeError;
use crate::{
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerTimeSample,
    BluetoothSchedulerReservationError, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerTimingPolicy,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingEmptyListLinkPrepared,
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingEventImagePrepared,
        BluetoothLegacyConnectableAdvertisingPublicationPrepared,
    },
    scheduler_timeline::{
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
pub(crate) struct BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
    item: BluetoothLegacyConnectableAdvertisingPublicationPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

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
    pub(crate) fn prepare_publication(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
        BluetoothLegacyConnectableAdvertisingSchedulerPublicationPrepared {
            item: self.item.prepare_publication(),
            reservation: self.reservation,
        }
    }
}

impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    /// Admit the complete response-capable first window into the common timeline.
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc rejection returns the exact affine candidate"
        )
    )]
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc rejection returns the exact affine candidate"
        )
    )]
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc cancellation invariant retains every affine owner"
        )
    )]
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc cancellation invariant retains every affine owner"
        )
    )]
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc rejection retains the complete affine event"
        )
    )]
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "an identity mismatch retains the complete affine merge"
        )
    )]
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
mod tests {
    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertising::{AdvertisingInterval, LegacyAdvertisingData, PrimaryAdvertisingChannelMap},
        connectable_advertising::{
            LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
            LegacyConnectableAdvertisingSet, LegacyScanResponseData,
        },
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyConnectableAdvertisingMemoryGraphStorage,
        BluetoothNonScanningRxMemoryModelAddress, BluetoothNonScanningRxMemoryStorage,
        BluetoothPeripheralConnectionDefaultTxPowerDbm,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };

    use super::{
        BluetoothLegacyConnectableAdvertisingAdmissionObservation,
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothLegacyConnectableAdvertisingEventPreparationError,
        BluetoothLegacyConnectableAdvertisingEventPrepared,
        BluetoothLegacyConnectableAdvertisingSequenceObservation,
    };
    use crate::{
        BluetoothClockedResources, BluetoothControllerRuntimeResources,
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
        BluetoothLegacyAdvertisingDefaultTxPowerDbm,
        BluetoothLegacyAdvertisingRecurringTimingObservation,
        BluetoothLegacyAdvertisingTimingObservation,
        BluetoothLegacyConnectableAdvertisingRuntimeResources,
        BluetoothPeripheralConnectionRuntimeConfig, BluetoothPeripheralConnectionRuntimeResources,
        BluetoothRadioHardware, BluetoothSchedulerInitialized, BluetoothSchedulerInstant,
        BluetoothSchedulerReservationError, BluetoothStopped,
        connectable_advertising::{
            BluetoothLegacyConnectableAdvertisingEventCandidate,
            BluetoothLegacyConnectableAdvertisingSetPrepared, refine_portable_set,
        },
    };

    struct TestPlatform;

    fn scheduler<const CAPACITY: usize>() -> BluetoothSchedulerInitialized<TestPlatform, 1, CAPACITY>
    {
        let stopped =
            BluetoothStopped::from_hardware(TestPlatform, BluetoothRadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        clocked
            .initialize_controller_hal_with(|_, _| {})
            .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
    }

    fn definition(kind: LeDeviceAddressKind) -> BluetoothLegacyConnectableAdvertisingSetPrepared {
        refine_portable_set(LegacyConnectableAdvertisingSet::new(
            LegacyConnectableAdvertisement::new(
                LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 0xc6], kind),
                LegacyAdvertisingData::new_owned(&[2, 1, 6]).unwrap(),
                LeChannelSelectionAlgorithmTwoSupport::Unsupported,
            ),
            LegacyScanResponseData::new_owned(&[3, 3, 0xaa, 0xfe]).unwrap(),
            PrimaryAdvertisingChannelMap::new(true, false, false).unwrap(),
            AdvertisingInterval::new(32).unwrap(),
        ))
        .unwrap()
    }

    fn connectable_runtime(base: u32) -> BluetoothLegacyConnectableAdvertisingRuntimeResources {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyConnectableAdvertisingMemoryGraphStorage::new(),
        ));
        BluetoothLegacyConnectableAdvertisingRuntimeResources::claim_static_model(
            storage,
            BluetoothLegacyConnectableAdvertisingMemoryGraphModelAddress::new(base).unwrap(),
            BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(4),
        )
        .unwrap()
    }

    fn peripheral_runtime(base: u32) -> BluetoothPeripheralConnectionRuntimeResources {
        let graph = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let receive = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
            graph,
            BluetoothPeripheralConnectionMemoryGraphModelAddress::new(base).unwrap(),
            receive,
            BluetoothNonScanningRxMemoryModelAddress::new(base + 0x1000).unwrap(),
            BluetoothPeripheralConnectionRuntimeConfig::new(
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(4),
            ),
        )
        .unwrap()
    }

    fn candidate<const CAPACITY: usize>(
        scheduler: &BluetoothSchedulerInitialized<TestPlatform, 1, CAPACITY>,
        connectable: &mut BluetoothLegacyConnectableAdvertisingRuntimeResources,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        current_micros: u32,
    ) -> BluetoothLegacyConnectableAdvertisingEventCandidate {
        let prepared = match connectable.begin_event(definition, peripheral) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the disjoint idle owners must prepare"),
        };
        prepared
            .form_first_event_candidate(
                BluetoothLegacyAdvertisingTimingObservation {
                    current: BluetoothSchedulerInstant::from_image(current_micros),
                    radio_ready: BluetoothSchedulerInstant::from_image(current_micros),
                    epoch: BluetoothControllerSchedulerEpoch::new(
                        BluetoothControllerTimeSample::for_validation(100),
                        1_000,
                        scheduler.controller_time_scale(),
                    ),
                },
                scheduler.scheduler_config(),
            )
            .unwrap_or_else(|failure| {
                let _prepared = failure.into_prepared();
                panic!("the bounded response window must project")
            })
    }

    fn restore(
        cancelled: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled,
        connectable: &mut BluetoothLegacyConnectableAdvertisingRuntimeResources,
        peripheral: &mut BluetoothPeripheralConnectionRuntimeResources,
    ) {
        let _definition = connectable
            .restore_cancelled(cancelled, peripheral)
            .unwrap_or_else(|_| panic!("the exact graph and receive pool must restore"));
        assert!(connectable.event_is_idle());
        assert!(peripheral.allocation_is_idle());
    }

    fn prepare<const CAPACITY: usize>(
        task: &mut crate::BluetoothControllerPoweredTaskRuntime<'_, CAPACITY>,
        candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
    ) -> BluetoothLegacyConnectableAdvertisingEventPrepared {
        let start = candidate.raw_window().start();
        let admitted = match task.admit_legacy_connectable_advertising_first_event(
            candidate,
            BluetoothLegacyConnectableAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(20_000)),
            },
        ) {
            Ok(admitted) => admitted,
            Err(_) => panic!("the guarded admission sample must remain early"),
        };
        match task.prepare_legacy_connectable_advertising_event(
            admitted,
            BluetoothLegacyConnectableAdvertisingSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(10_000)),
            },
        ) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the independent sequence sample must remain early"),
        }
    }

    fn cancel_merge<const CAPACITY: usize>(
        task: &mut crate::BluetoothControllerPoweredTaskRuntime<'_, CAPACITY>,
        merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled {
        match task.cancel_legacy_connectable_advertising_empty_list_merge(merged) {
            Ok(cancelled) => cancelled,
            Err(_) => panic!("the originating list and receive pool must release"),
        }
    }

    #[test]
    fn overlapping_candidate_rejection_retains_both_events_and_releases_cleanly() {
        let mut scheduler = scheduler::<1>();
        let mut first_connectable = connectable_runtime(0x2f00_1000);
        let mut first_peripheral = peripheral_runtime(0x2f00_3000);
        let mut second_connectable = connectable_runtime(0x2f00_6000);
        let mut second_peripheral = peripheral_runtime(0x2f00_8000);
        let first = candidate(
            &scheduler,
            &mut first_connectable,
            &mut first_peripheral,
            definition(LeDeviceAddressKind::Public),
            10_000,
        );
        let second = candidate(
            &scheduler,
            &mut second_connectable,
            &mut second_peripheral,
            definition(LeDeviceAddressKind::Random),
            10_000,
        );
        let first_start = first.raw_window().start();
        let second_start = second.raw_window().start();
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let first = match task.admit_legacy_connectable_advertising_first_event(
            first,
            BluetoothLegacyConnectableAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    first_start.wrapping_sub(20_000),
                ),
            },
        ) {
            Ok(admitted) => admitted,
            Err(_) => panic!("the first response window must reserve the sole slot"),
        };
        let failure = match task.admit_legacy_connectable_advertising_first_event(
            second,
            BluetoothLegacyConnectableAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    second_start.wrapping_sub(20_000),
                ),
            },
        ) {
            Ok(_) => panic!("the overlapping event cannot enter the full timeline"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
                BluetoothSchedulerReservationError::TimelineFull
            )
        );
        let second_cancelled = failure
            .into_candidate()
            .cancel()
            .unwrap_or_else(|_| panic!("the second event retains its exact pool"));
        let first_cancelled = task
            .cancel_legacy_connectable_advertising_pre_sequence(first)
            .unwrap_or_else(|_| panic!("the first event retains its exact pool"));
        restore(
            second_cancelled,
            &mut second_connectable,
            &mut second_peripheral,
        );
        restore(
            first_cancelled,
            &mut first_connectable,
            &mut first_peripheral,
        );
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn rejected_sequence_sample_releases_timeline_and_all_graph_owners() {
        let mut scheduler = scheduler::<1>();
        let mut connectable = connectable_runtime(0x2f00_b000);
        let mut peripheral = peripheral_runtime(0x2f00_d000);
        let candidate = candidate(
            &scheduler,
            &mut connectable,
            &mut peripheral,
            definition(LeDeviceAddressKind::Public),
            10_000,
        );
        let start = candidate.raw_window().start();
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let admitted = match task.admit_legacy_connectable_advertising_first_event(
            candidate,
            BluetoothLegacyConnectableAdvertisingAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(start.wrapping_sub(20_000)),
            },
        ) {
            Ok(admitted) => admitted,
            Err(_) => panic!("the first guarded sample must remain early"),
        };
        let failure = match task.prepare_legacy_connectable_advertising_event(
            admitted,
            BluetoothLegacyConnectableAdvertisingSequenceObservation {
                sample: BluetoothControllerTimeSample::for_validation(start),
            },
        ) {
            Ok(_) => panic!("a sample at the event start must fail closed"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(
                crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired
            )
        );
        let cancelled = failure
            .into_candidate()
            .cancel()
            .unwrap_or_else(|_| panic!("the rejected event retains its exact pool"));
        restore(cancelled, &mut connectable, &mut peripheral);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn phase_locked_connectable_recurrence_reserves_and_cancels_losslessly() {
        let mut scheduler = scheduler::<1>();
        let mut connectable = connectable_runtime(0x2f00_e000);
        let mut peripheral = peripheral_runtime(0x2f01_0000);
        let definition = definition(LeDeviceAddressKind::Public);
        let first = candidate(
            &scheduler,
            &mut connectable,
            &mut peripheral,
            definition,
            10_000,
        );
        let previous_phase = first.phase();
        let cancelled = first
            .cancel()
            .unwrap_or_else(|_| panic!("the first candidate retains the exact receive pool"));
        restore(cancelled, &mut connectable, &mut peripheral);

        let prepared = connectable
            .begin_event(definition, &mut peripheral)
            .unwrap_or_else(|_| panic!("the restored runtime accepts the successor"));
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scheduler.controller_time_scale(),
        );
        let recurring = prepared
            .form_recurring_event_candidate(
                BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
                previous_phase,
                definition.set().interval().as_micros(),
                scheduler.scheduler_config(),
            )
            .unwrap_or_else(|failure| {
                let _prepared = failure.into_prepared();
                panic!("one selected-channel successor fits the retained epoch")
            });
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let admitted = task
            .admit_legacy_connectable_advertising_recurring_event(recurring)
            .unwrap_or_else(|failure| {
                let _candidate = failure.into_candidate();
                panic!("an empty timeline accepts the phase-locked successor")
            });
        let cancelled = task
            .cancel_legacy_connectable_advertising_pre_sequence(admitted)
            .unwrap_or_else(|_| panic!("recurring cancellation returns every affine owner"));
        restore(cancelled, &mut connectable, &mut peripheral);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn colliding_connectable_recurrence_keeps_both_role_graphs_and_nominal_phase() {
        let mut scheduler = scheduler::<2>();
        let first_definition = definition(LeDeviceAddressKind::Public);
        let second_definition = definition(LeDeviceAddressKind::Random);
        let mut first_connectable = connectable_runtime(0x2f02_0000);
        let mut first_peripheral = peripheral_runtime(0x2f02_2000);
        let mut second_connectable = connectable_runtime(0x2f02_5000);
        let mut second_peripheral = peripheral_runtime(0x2f02_7000);

        let first_initial = candidate(
            &scheduler,
            &mut first_connectable,
            &mut first_peripheral,
            first_definition,
            10_000,
        );
        let previous_phase = first_initial.phase();
        let first_cancelled = first_initial
            .cancel()
            .unwrap_or_else(|_| panic!("the first initial candidate remains CPU-owned"));
        restore(
            first_cancelled,
            &mut first_connectable,
            &mut first_peripheral,
        );

        let second_initial = candidate(
            &scheduler,
            &mut second_connectable,
            &mut second_peripheral,
            second_definition,
            10_000,
        );
        assert_eq!(second_initial.phase(), previous_phase);
        let second_cancelled = second_initial
            .cancel()
            .unwrap_or_else(|_| panic!("the second initial candidate remains CPU-owned"));
        restore(
            second_cancelled,
            &mut second_connectable,
            &mut second_peripheral,
        );

        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            scheduler.controller_time_scale(),
        );
        let start_offset_micros = first_definition.set().interval().as_micros();
        let first = first_connectable
            .begin_event(first_definition, &mut first_peripheral)
            .unwrap_or_else(|_| panic!("the first restored runtime accepts its successor"))
            .form_recurring_event_candidate(
                BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
                previous_phase,
                start_offset_micros,
                scheduler.scheduler_config(),
            )
            .unwrap_or_else(|failure| {
                let _prepared = failure.into_prepared();
                panic!("the first successor projects from its nominal phase")
            });
        let second = second_connectable
            .begin_event(second_definition, &mut second_peripheral)
            .unwrap_or_else(|_| panic!("the second restored runtime accepts its successor"))
            .form_recurring_event_candidate(
                BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
                previous_phase,
                start_offset_micros,
                scheduler.scheduler_config(),
            )
            .unwrap_or_else(|failure| {
                let _prepared = failure.into_prepared();
                panic!("the second successor projects from the same nominal phase")
            });
        let recurring_phase = first.phase();
        assert_eq!(second.phase(), recurring_phase);

        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let first = task
            .admit_legacy_connectable_advertising_recurring_event(first)
            .unwrap_or_else(|failure| {
                let _candidate = failure.into_candidate();
                panic!("the first recurring event reserves the empty timeline")
            });
        let failure = match task.admit_legacy_connectable_advertising_recurring_event(second) {
            Ok(_) => panic!("a phase-locked collision cannot be displaced"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(
                BluetoothSchedulerReservationError::RecurringOverlapUnsupported
            )
        );
        assert_eq!(failure.candidate.phase(), recurring_phase);

        let second_cancelled = failure
            .into_candidate()
            .cancel()
            .unwrap_or_else(|_| panic!("collision rejection retains the second graph"));
        let first_cancelled = task
            .cancel_legacy_connectable_advertising_pre_sequence(first)
            .unwrap_or_else(|_| panic!("the accepted reservation retains the first graph"));
        restore(
            first_cancelled,
            &mut first_connectable,
            &mut first_peripheral,
        );
        restore(
            second_cancelled,
            &mut second_connectable,
            &mut second_peripheral,
        );
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn occupied_list_rejects_second_item_without_losing_either_owner() {
        let mut scheduler = scheduler::<2>();
        let mut first_connectable = connectable_runtime(0x2f01_0000);
        let mut first_peripheral = peripheral_runtime(0x2f01_2000);
        let mut second_connectable = connectable_runtime(0x2f01_5000);
        let mut second_peripheral = peripheral_runtime(0x2f01_7000);
        let first = candidate(
            &scheduler,
            &mut first_connectable,
            &mut first_peripheral,
            definition(LeDeviceAddressKind::Random),
            10_000,
        );
        let second = candidate(
            &scheduler,
            &mut second_connectable,
            &mut second_peripheral,
            definition(LeDeviceAddressKind::Public),
            30_000,
        );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let first = prepare(&mut task, first);
        let second = prepare(&mut task, second);
        let first = match task.prepare_legacy_connectable_advertising_empty_list_merge(first) {
            Ok(merged) => merged,
            Err(_) => panic!("the pristine list must accept its first item"),
        };
        let failure = match task.prepare_legacy_connectable_advertising_empty_list_merge(second) {
            Ok(_) => panic!("the exclusive list must reject a second first item"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            super::BluetoothSchedulerEmptyListMergeError::ListNotEmpty
        );
        let second = failure.into_prepared();
        let first_cancelled = cancel_merge(&mut task, first);
        let second = match task.prepare_legacy_connectable_advertising_empty_list_merge(second) {
            Ok(merged) => merged,
            Err(_) => panic!("cancelling the first item must reopen the exact empty list"),
        };
        let second_cancelled = cancel_merge(&mut task, second);
        restore(
            first_cancelled,
            &mut first_connectable,
            &mut first_peripheral,
        );
        restore(
            second_cancelled,
            &mut second_connectable,
            &mut second_peripheral,
        );
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }
}
