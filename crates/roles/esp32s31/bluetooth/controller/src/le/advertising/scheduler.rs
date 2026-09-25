//! Legacy advertising preparation, publication and recycling over the hardware scheduler.

#[cfg(target_arch = "riscv32")]
use crate::le::advertising::legacy::{
    LegacyAdvertisingCompletionObservedEvent, LegacyAdvertisingRecurringEventCandidate,
};
use oer_esp32s31_bluetooth::runtime_resources::ControllerPoweredTaskRuntime;
#[allow(unused_imports)]
use oer_esp32s31_bluetooth::scheduler::core::*;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::scheduler::timeline::SchedulerRecurringReserved;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth::scheduler::timeline::{
    SchedulerInitialAdmissionResolved, SchedulerWindowReservation,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerSoftwareListRemovalReady,
};
#[cfg(any(target_arch = "riscv32", test))]
use {
    crate::le::advertising::LegacyAdvertisingFirstEventCandidate,
    oer_esp32s31_bluetooth::{
        ControllerTimeSample,
        scheduler::{
            SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerSequenceReady,
            SchedulerTimingPolicy,
        },
    },
};
use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

/// Fresh initial-admission sample sealed by the controller-time worker.
///
/// External code can carry this capability but cannot create one from an
/// integer timestamp. It is distinct from the later sequence-deadline sample.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh admission observation must be consumed or retained"]
pub struct LegacyAdvertisingAdmissionObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh sequence observation must be consumed or retained"]
pub struct LegacyAdvertisingSequenceObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// First advertising event after common timeline admission and before the
/// second sequence deadline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted event must pass sequence authorization or be retained"]
pub struct LegacyAdvertisingFirstPreSequence<'a> {
    candidate: LegacyAdvertisingFirstEventCandidate<'a>,
    reservation: SchedulerWindowReservation<SchedulerInitialAdmissionResolved>,
}

/// Recurring advertising event after exact timeline reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "authorize the recurring sequence deadline or retain the event"]
pub struct LegacyAdvertisingRecurringPreSequence<'a> {
    candidate: LegacyAdvertisingRecurringEventCandidate<'a>,
    reservation: SchedulerWindowReservation<SchedulerRecurringReserved>,
}

/// Why one recurring event could not reach a sequence-ready descriptor.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingRecurringEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    EventImage(oer_esp32s31_bluetooth_memory::LegacyAdvertisingMemoryGraphEventPrepareError),
}

/// Lossless recurring admission/preparation rejection.
#[cfg(target_arch = "riscv32")]
#[must_use = "retry, cancel, or retain the recurring event candidate"]
pub struct LegacyAdvertisingRecurringEventPreparationFailure<'a> {
    candidate: LegacyAdvertisingRecurringEventCandidate<'a>,
    error: LegacyAdvertisingRecurringEventPreparationError,
}

#[cfg(target_arch = "riscv32")]
impl<'a> LegacyAdvertisingRecurringEventPreparationFailure<'a> {
    pub const fn error(&self) -> LegacyAdvertisingRecurringEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> LegacyAdvertisingRecurringEventCandidate<'a> {
        self.candidate
    }
}

/// First advertising descriptor paired with its exact accepted timeline slot.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared event must be published, cancelled through its controller, or retained"]
pub struct LegacyAdvertisingEventPrepared<'a> {
    image: crate::le::advertising::legacy::LegacyAdvertisingEventImagePrepared<'a>,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyAdvertisingEventPrepared<'_> {
    pub const fn identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.image.identity()
    }

    pub fn pdu(&self) -> &[u8] {
        self.image.pdu()
    }

    /// Opaque nominal phase required by later advertising events.
    pub const fn phase(&self) -> crate::le::advertising::LegacyAdvertisingEventPhase {
        self.image.phase()
    }
}

/// Lossless rejection while joining one advertising item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged advertising event remains prepared and CPU-owned"]
pub struct LegacyAdvertisingEmptySchedulerMergeFailure<'a> {
    error: SchedulerEmptyListMergeError,
    prepared: LegacyAdvertisingEventPrepared<'a>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> LegacyAdvertisingEmptySchedulerMergeFailure<'a> {
    pub const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    pub fn into_prepared(self) -> LegacyAdvertisingEventPrepared<'a> {
        self.prepared
    }
}

/// First advertising event joined to the source-owned empty scheduler list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the merged advertising event must be published or cancelled"]
pub struct LegacyAdvertisingEmptySchedulerMergePrepared<'a> {
    item: crate::le::advertising::legacy::LegacyAdvertisingEmptyListLinkPrepared<'a>,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl LegacyAdvertisingEmptySchedulerMergePrepared<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }
}

/// Lossless rejection before advertising scheduler-head MMIO publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the unchanged advertising merge remains CPU-owned"]
pub struct LegacyAdvertisingSchedulerHeadPublicationFailure<'a> {
    error: SchedulerHeadPublicationError,
    merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> LegacyAdvertisingSchedulerHeadPublicationFailure<'a> {
    pub const fn error(&self) -> SchedulerHeadPublicationError {
        self.error
    }

    pub fn into_merged(self) -> LegacyAdvertisingEmptySchedulerMergePrepared<'a> {
        self.merged
    }
}

/// Advertising graph whose first scheduler item is hardware-visible.
#[cfg(target_arch = "riscv32")]
#[must_use = "the published advertising head must advance through the RUN suffix"]
pub struct LegacyAdvertisingSchedulerHeadPublished<'a> {
    item: crate::le::advertising::legacy::LegacyAdvertisingHeadPublishedEvent<'a>,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    _reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> LegacyAdvertisingSchedulerHeadPublished<'a> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::le::advertising::legacy::LegacyAdvertisingHeadPublishedEvent<'a>,
        BluetoothSchedulerHardwareListHeadPublished,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.item, self.publication, self._reservation)
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the completed event must advance the LL owner exactly once"]
pub(crate) struct LegacyAdvertisingSchedulerRecycled<'a> {
    item: crate::le::advertising::legacy::LegacyAdvertisingRecycledEvent<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> LegacyAdvertisingSchedulerRecycled<'a> {
    /// Advance the exact LL event while retaining S31 diagnostic statuses.
    pub fn complete_event(
        self,
    ) -> crate::le::advertising::legacy::LegacyAdvertisingEventCompleted<'a> {
        self.item.complete_event()
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct LegacyAdvertisingSchedulerRecycleReady<'a> {
    item: LegacyAdvertisingCompletionObservedEvent<'a>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl LegacyAdvertisingSchedulerRecycleReady<'_> {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the advertising graph; success retains CPU ownership"]
pub(crate) enum LegacyAdvertisingSchedulerRecycleStep<'a> {
    SchedulerIdentityMismatch {
        _ready: LegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    FinishedListDrainStillActive {
        _ready: LegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    MemoryIdentityMismatch {
        _ready: LegacyAdvertisingSchedulerRecycleReady<'a>,
        _error: oer_esp32s31_bluetooth_memory::LegacyAdvertisingMemoryGraphRecycleError,
    },
    ReservationIdentityMismatch {
        _ready: LegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    Recycled(LegacyAdvertisingSchedulerRecycled<'a>),
}

/// Finite reason a first advertising event returned to pre-admission state.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingFirstEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    EventImage(oer_esp32s31_bluetooth_memory::LegacyAdvertisingMemoryGraphEventPrepareError),
}

/// Rejected first advertising event retaining the exact cancellable candidate.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the advertising candidate remains recoverable"]
pub struct LegacyAdvertisingFirstEventPreparationFailure<'a> {
    candidate: LegacyAdvertisingFirstEventCandidate<'a>,
    error: LegacyAdvertisingFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> LegacyAdvertisingFirstEventPreparationFailure<'a> {
    pub const fn error(&self) -> LegacyAdvertisingFirstEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> LegacyAdvertisingFirstEventCandidate<'a> {
        self.candidate
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for LegacyAdvertisingFirstEventPreparationFailure<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LegacyAdvertisingFirstEventPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
/// Role scheduler operations composed from the powered runtime's primitives.
pub(crate) trait LegacyAdvertisingScheduling<const SCHEDULER_CAPACITY: usize> {
    /// Admit one already projected first advertising event into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    fn admit_legacy_advertising_first_event<'a>(
        &mut self,
        candidate: LegacyAdvertisingFirstEventCandidate<'a>,
        admission: LegacyAdvertisingAdmissionObservation,
    ) -> Result<
        LegacyAdvertisingFirstPreSequence<'a>,
        LegacyAdvertisingFirstEventPreparationFailure<'a>,
    >;

    /// Authorize the second deadline and encode the overlap-resolved event image.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    fn prepare_legacy_advertising_first_event<'a>(
        &mut self,
        admitted: LegacyAdvertisingFirstPreSequence<'a>,
        sequence: LegacyAdvertisingSequenceObservation,
    ) -> Result<LegacyAdvertisingEventPrepared<'a>, LegacyAdvertisingFirstEventPreparationFailure<'a>>;

    /// Reserve one exact recurring advertising window without displacement.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    fn admit_legacy_advertising_recurring_event<'a>(
        &mut self,
        candidate: LegacyAdvertisingRecurringEventCandidate<'a>,
    ) -> Result<
        LegacyAdvertisingRecurringPreSequence<'a>,
        LegacyAdvertisingRecurringEventPreparationFailure<'a>,
    >;

    /// Authorize the recurring deadline and encode its complete event chain.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    fn prepare_legacy_advertising_recurring_event<'a>(
        &mut self,
        admitted: LegacyAdvertisingRecurringPreSequence<'a>,
        sequence: LegacyAdvertisingSequenceObservation,
    ) -> Result<
        LegacyAdvertisingEventPrepared<'a>,
        LegacyAdvertisingRecurringEventPreparationFailure<'a>,
    >;

    /// Release an unpublished first advertising event and restore both owners.
    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_legacy_advertising_first_event<'a>(
        &mut self,
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'a>;

    /// Release an admitted first event before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_legacy_advertising_first_pre_sequence<'a>(
        &mut self,
        admitted: LegacyAdvertisingFirstPreSequence<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'a>;

    /// Release an admitted recurring event before its sequence sample arrives.
    #[cfg(target_arch = "riscv32")]
    fn cancel_legacy_advertising_recurring_pre_sequence<'a>(
        &mut self,
        admitted: LegacyAdvertisingRecurringPreSequence<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingRecurringCancelled<'a>;

    /// Join one prepared advertising item to this epoch's empty scheduler list.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    fn prepare_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> Result<
        LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
        LegacyAdvertisingEmptySchedulerMergeFailure<'a>,
    >;

    /// Cancel a not-yet-published advertising merge through the same list epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "an identity rejection retains the complete advertising merge"
    )]
    fn cancel_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<LegacyAdvertisingEventPrepared<'a>, LegacyAdvertisingEmptySchedulerMergePrepared<'a>>;

    /// Publish one prepared advertising item through the common head edge.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection retains the complete advertising merge"
    )]
    fn publish_legacy_advertising_scheduler_head<'a>(
        &mut self,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        LegacyAdvertisingSchedulerHeadPublished<'a>,
        LegacyAdvertisingSchedulerHeadPublicationFailure<'a>,
    >;

    /// Release the advertising memory, timeline and source-list owners together.
    #[cfg(target_arch = "riscv32")]
    fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::legacy::completion::LegacyAdvertisingCompletionRole<'a>,
        >,
    ) -> LegacyAdvertisingSchedulerRecycleStep<'a>;

    /// Reclaim one response-capable advertising graph and classify its copied RX batch.
    #[cfg(target_arch = "riscv32")]
    fn recycle_legacy_connectable_advertising_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingCompletionRole,
        >,
    ) -> crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingRecycleStep;
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> LegacyAdvertisingScheduling<SCHEDULER_CAPACITY>
    for ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    #[cfg(any(target_arch = "riscv32", test))]
    fn admit_legacy_advertising_first_event<'a>(
        &mut self,
        candidate: LegacyAdvertisingFirstEventCandidate<'a>,
        admission: LegacyAdvertisingAdmissionObservation,
    ) -> Result<
        LegacyAdvertisingFirstPreSequence<'a>,
        LegacyAdvertisingFirstEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy = SchedulerTimingPolicy::from_scheduler_config(
            self.scheduler_config(),
            self.controller_time_scale(),
        );
        match self.scheduler_timeline_mut().reserve_initial_window(
            raw_window.start(),
            raw_window.end(),
            timing_policy,
            admission.sample,
        ) {
            Ok(reservation) => Ok(LegacyAdvertisingFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(LegacyAdvertisingFirstEventPreparationFailure {
                candidate,
                error: LegacyAdvertisingFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_legacy_advertising_first_event<'a>(
        &mut self,
        admitted: LegacyAdvertisingFirstPreSequence<'a>,
        sequence: LegacyAdvertisingSequenceObservation,
    ) -> Result<LegacyAdvertisingEventPrepared<'a>, LegacyAdvertisingFirstEventPreparationFailure<'a>>
    {
        let LegacyAdvertisingFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(LegacyAdvertisingFirstEventPreparationFailure {
                    candidate,
                    error: LegacyAdvertisingFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(resolved_window) {
            Ok(image) => Ok(LegacyAdvertisingEventPrepared { image, reservation }),
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(LegacyAdvertisingFirstEventPreparationFailure {
                    candidate,
                    error: LegacyAdvertisingFirstEventPreparationError::EventImage(error),
                })
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    fn admit_legacy_advertising_recurring_event<'a>(
        &mut self,
        candidate: LegacyAdvertisingRecurringEventCandidate<'a>,
    ) -> Result<
        LegacyAdvertisingRecurringPreSequence<'a>,
        LegacyAdvertisingRecurringEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy = SchedulerTimingPolicy::from_scheduler_config(
            self.scheduler_config(),
            self.controller_time_scale(),
        );
        match self.scheduler_timeline_mut().reserve_recurring_window(
            raw_window.start(),
            raw_window.end(),
            timing_policy,
        ) {
            Ok(reservation) => Ok(LegacyAdvertisingRecurringPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(LegacyAdvertisingRecurringEventPreparationFailure {
                candidate,
                error: LegacyAdvertisingRecurringEventPreparationError::Timeline(error),
            }),
        }
    }

    #[cfg(target_arch = "riscv32")]
    fn prepare_legacy_advertising_recurring_event<'a>(
        &mut self,
        admitted: LegacyAdvertisingRecurringPreSequence<'a>,
        sequence: LegacyAdvertisingSequenceObservation,
    ) -> Result<
        LegacyAdvertisingEventPrepared<'a>,
        LegacyAdvertisingRecurringEventPreparationFailure<'a>,
    > {
        let LegacyAdvertisingRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(LegacyAdvertisingRecurringEventPreparationFailure {
                    candidate,
                    error: LegacyAdvertisingRecurringEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(resolved_window) {
            Ok(prepared) => {
                let (image, _, _, _) = prepared.into_parts();
                Ok(LegacyAdvertisingEventPrepared { image, reservation })
            }
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(LegacyAdvertisingRecurringEventPreparationFailure {
                    candidate,
                    error: LegacyAdvertisingRecurringEventPreparationError::EventImage(error),
                })
            }
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_legacy_advertising_first_event<'a>(
        &mut self,
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'a> {
        let LegacyAdvertisingEventPrepared { image, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        image.cancel()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_legacy_advertising_first_pre_sequence<'a>(
        &mut self,
        admitted: LegacyAdvertisingFirstPreSequence<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'a> {
        let LegacyAdvertisingFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    #[cfg(target_arch = "riscv32")]
    fn cancel_legacy_advertising_recurring_pre_sequence<'a>(
        &mut self,
        admitted: LegacyAdvertisingRecurringPreSequence<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingRecurringCancelled<'a> {
        let LegacyAdvertisingRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn prepare_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> Result<
        LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
        LegacyAdvertisingEmptySchedulerMergeFailure<'a>,
    > {
        let LegacyAdvertisingEventPrepared { image, reservation } = prepared;
        let item = image.prepare_scheduler_bookkeeping();
        let address = item.scheduler_item_address();
        if let Err(error) = self.scheduler_list_mut().prepare_first_item(address) {
            return Err(LegacyAdvertisingEmptySchedulerMergeFailure {
                error,
                prepared: LegacyAdvertisingEventPrepared {
                    image: item.cancel(),
                    reservation,
                },
            });
        }
        Ok(LegacyAdvertisingEmptySchedulerMergePrepared {
            item: item.prepare_empty_list_link(),
            reservation,
        })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn cancel_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<LegacyAdvertisingEventPrepared<'a>, LegacyAdvertisingEmptySchedulerMergePrepared<'a>>
    {
        if !self
            .scheduler_list_mut()
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let LegacyAdvertisingEmptySchedulerMergePrepared { item, reservation } = merged;
        Ok(LegacyAdvertisingEventPrepared {
            image: item.cancel().cancel(),
            reservation,
        })
    }

    #[cfg(target_arch = "riscv32")]
    fn publish_legacy_advertising_scheduler_head<'a>(
        &mut self,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        LegacyAdvertisingSchedulerHeadPublished<'a>,
        LegacyAdvertisingSchedulerHeadPublicationFailure<'a>,
    > {
        let address = merged.scheduler_item_address();
        let publication =
            match self.publish_first_scheduler_item_head(address, merged.hardware_list_index()) {
                Ok(publication) => publication,
                Err(error) => {
                    return Err(LegacyAdvertisingSchedulerHeadPublicationFailure { error, merged });
                }
            };
        let LegacyAdvertisingEmptySchedulerMergePrepared { item, reservation } = merged;
        let item = item.into_head_published(&publication);
        Ok(LegacyAdvertisingSchedulerHeadPublished {
            item,
            publication,
            _reservation: reservation,
        })
    }

    #[cfg(target_arch = "riscv32")]
    fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::legacy::completion::LegacyAdvertisingCompletionRole<'a>,
        >,
    ) -> LegacyAdvertisingSchedulerRecycleStep<'a> {
        let (item, removal, reservation) = ready.into_parts();
        let ready = LegacyAdvertisingSchedulerRecycleReady {
            item,
            removal,
            reservation,
        };
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                .scheduler_list_mut()
                .retains_software_list_removal_ready_first_item(address)
        {
            return LegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.scheduler_finished_lists_mut().is_active() {
            return LegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive {
                _ready: ready,
            };
        }
        let LegacyAdvertisingSchedulerRecycleReady {
            item,
            removal,
            reservation,
        } = ready;
        let prepared = match item.prepare_recycle(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (item, removal) = failure.into_parts();
                return LegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch {
                    _ready: LegacyAdvertisingSchedulerRecycleReady {
                        item,
                        removal,
                        reservation,
                    },
                    _error: error,
                };
            }
        };
        let release = match self.scheduler_timeline_mut().prepare_release(reservation) {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (item, removal) = prepared.into_parts();
                return LegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch {
                    _ready: LegacyAdvertisingSchedulerRecycleReady {
                        item,
                        removal,
                        reservation,
                    },
                };
            }
        };
        let item = prepared.commit();
        release.commit();
        self.scheduler_list_mut().commit_recycled_first_item();
        LegacyAdvertisingSchedulerRecycleStep::Recycled(LegacyAdvertisingSchedulerRecycled { item })
    }

    #[cfg(target_arch = "riscv32")]
    fn recycle_legacy_connectable_advertising_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingCompletionRole,
        >,
    ) -> crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingRecycleStep
    {
        use crate::le::advertising::connectable::completion::{
            LegacyConnectableAdvertisingRecycleReady, LegacyConnectableAdvertisingRecycleStep,
        };

        let (item, removal, reservation) = ready.into_parts();
        let ready = LegacyConnectableAdvertisingRecycleReady::new(item, removal, reservation);
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                .scheduler_list_mut()
                .retains_software_list_removal_ready_first_item(address)
        {
            return LegacyConnectableAdvertisingRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.scheduler_finished_lists_mut().is_active() {
            return LegacyConnectableAdvertisingRecycleStep::FinishedListDrainStillActive {
                _ready: ready,
            };
        }

        #[cfg(feature = "dtm-diagnostics")]
        if let Some(nodes) = ready.receive_observations() {
            crate::le::advertising::diagnostics::record(nodes);
        }
        let (item, removal, reservation) = ready.into_parts();
        let (memory, remainder) = item.into_parts();
        let prepared = match memory.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_parts();
                return LegacyConnectableAdvertisingRecycleStep::MemoryIdentityMismatch {
                    _ready: LegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::LegacyConnectableAdvertisingCompletionObserved::new(
                            memory, remainder,
                        ),
                        removal,
                        reservation,
                    ),
                    _error: error,
                };
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_prepared().into_parts();
                return LegacyConnectableAdvertisingRecycleStep::ReceiveInvalid {
                    _ready: LegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::LegacyConnectableAdvertisingCompletionObserved::new(
                            memory, remainder,
                        ),
                        removal,
                        reservation,
                    ),
                    _error: error,
                };
            }
        };
        let release = match self.scheduler_timeline_mut().prepare_release(reservation) {
            Ok(release) => release,
            Err(failure) => {
                let reservation = failure.into_reservation();
                let (memory, removal) = extracted.into_prepared().into_parts();
                return LegacyConnectableAdvertisingRecycleStep::ReservationIdentityMismatch {
                    _ready: LegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::LegacyConnectableAdvertisingCompletionObserved::new(
                            memory, remainder,
                        ),
                        removal,
                        reservation,
                    ),
                };
            }
        };
        let recycled = extracted.commit();
        release.commit();
        self.scheduler_list_mut().commit_recycled_first_item();
        LegacyConnectableAdvertisingRecycleStep::Classified(remainder.classify_recycled(recycled))
    }
}

#[cfg(test)]
mod tests;
