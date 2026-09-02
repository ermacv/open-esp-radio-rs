//! Peripheral-connection scheduler preparation and completion.
//!
//! This module owns the connection-specific descriptor and memory transitions.
//! The parent scheduler retains protocol-neutral timeline, list-epoch and MMIO
//! publication primitives.

use super::*;

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

/// Lossless rejection before any connection or scheduler MMIO publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the unchanged CPU-owned connection merge can be retried or cancelled"]
pub struct BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
    error: BluetoothSchedulerHeadPublicationError,
    merged: BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerHeadPublicationFailure {
    /// Exact reason the common scheduler head could not be prepared.
    pub const fn error(&self) -> BluetoothSchedulerHeadPublicationError {
        self.error
    }

    /// Recover the unchanged merge. No MMIO was performed.
    pub fn into_merged(self) -> BluetoothPeripheralConnectionEmptySchedulerMergePrepared {
        self.merged
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

/// First peripheral connection event admitted through the common RUN suffix.
#[cfg(target_arch = "riscv32")]
#[must_use = "the running connection event must advance through owned completion"]
pub struct BluetoothPeripheralConnectionSchedulerRunning {
    event: BluetoothPeripheralConnectionFirstEventRunning,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerRunning {
    pub(crate) fn new(
        event: BluetoothPeripheralConnectionFirstEventRxPublished,
        run: BluetoothSchedulerHardwareRunCommandPublished,
        reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) -> Self {
        Self {
            event: event.into_running(&run),
            run,
            reservation,
        }
    }

    /// Exact hardware-owned scheduler item.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    /// Hardware list containing the running connection item.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }

    /// Link Layer event counter, not advanced before completion.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Common-timeline reservation retained until completion or teardown.
    pub const fn reserved_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// First peripheral connection event after a fenced non-sentinel status read.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed connection graph must advance through unlink and recycle"]
pub struct BluetoothPeripheralConnectionSchedulerCompletionObserved {
    event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    pub const fn reserved_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// One bounded Controller-owned connection completion attempt.
#[must_use = "the connection graph and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSchedulerCompletionStep {
    DrainAlreadyActive(BluetoothPeripheralConnectionSchedulerRunning),
    SchedulerIdentityMismatch(BluetoothPeripheralConnectionSchedulerRunning),
    NoFinishedList(BluetoothPeripheralConnectionSchedulerRunning),
    UnrelatedList {
        drain:
            BluetoothSchedulerFinishedListDrainState<BluetoothPeripheralConnectionSchedulerRunning>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothPeripheralConnectionSchedulerRunning>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ),
}

/// One bounded continuation of a captured finished-list set while the
/// connection item is still running.
#[must_use = "the connection graph and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSchedulerRunningDrainStep {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPeripheralConnectionSchedulerRunning>,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPeripheralConnectionSchedulerRunning>,
    ),
    UnrelatedList {
        drain:
            BluetoothSchedulerFinishedListDrainState<BluetoothPeripheralConnectionSchedulerRunning>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothPeripheralConnectionSchedulerRunning>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ),
}

/// One bounded continuation after list zero already completed the connection item.
#[must_use = "the completed connection graph and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RepeatedConnectionList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

/// Completed connection graph after its exact hardware-list head became empty.
#[must_use = "the empty-head connection graph must advance through software-list removal"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
    event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    pub const fn reserved_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// Completed connection item removed from the source-owned software list.
#[must_use = "the unlinked connection graph must pass the finite removal gate"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
    event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }
}

/// Connection graph after the post-unlink return predicate became ready.
#[must_use = "the removal-ready connection graph must advance through recycle"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
    event: BluetoothPeripheralConnectionFirstEventCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.event.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }

    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    pub const fn status(&self) -> BluetoothPeripheralConnectionSchedulerItemCompletionStatus {
        self.event.status()
    }

    pub const fn reserved_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.reservation.window()
    }
}

/// Active connection owner after event-local memory and scheduler reclamation.
#[must_use = "the recycled connection must classify status before protocol advance"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerRecycled {
    event: BluetoothPeripheralConnectionRecycledEvent,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerRecycled {
    /// Link Layer counter retained without advancement after lower reclamation.
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

    /// Opaque hardware status awaiting reviewed connection-event classification.
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

    pub(crate) fn normalize_packet_start(
        self,
        normalize: impl FnOnce(
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPeripheralConnectionCapturedAnchorTime,
        ) -> BluetoothPeripheralConnectionPacketStartTiming,
    ) -> BluetoothPeripheralConnectionSchedulerPacketStartNormalized {
        BluetoothPeripheralConnectionSchedulerPacketStartNormalized {
            event: self.event.normalize_packet_start(normalize),
        }
    }
}

/// Recycled connection whose capture entered scheduler time and PHY calibration.
///
/// Hardware status and portable Link Layer continuation remain unclassified.
#[must_use = "the normalized connection must enter completion classification"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothPeripheralConnectionSchedulerPacketStartNormalized {
    event: BluetoothPeripheralConnectionPacketStartNormalizedEvent,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPeripheralConnectionSchedulerPacketStartNormalized {
    pub const fn event_counter(&self) -> u16 {
        self.event.event_counter()
    }

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

    pub const fn packet_start(&self) -> &BluetoothPeripheralConnectionPacketStartTiming {
        self.event.packet_start()
    }
}

/// One atomic attempt to release connection memory, timeline and list owners.
#[must_use = "every rejected branch retains the exact removal-ready connection"]
#[cfg(target_arch = "riscv32")]
#[allow(
    clippy::large_enum_variant,
    reason = "no-alloc failure branches retain the exact affine connection owner"
)]
pub enum BluetoothPeripheralConnectionSchedulerRecycleStep {
    SchedulerIdentityMismatch(BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady),
    FinishedListDrainStillActive(BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady),
    MemoryIdentityMismatch {
        ready: BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
        error: BluetoothPeripheralConnectionMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        ready: BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
        error: BluetoothLeRxError,
    },
    ReservationIdentityMismatch(BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady),
    Recycled(BluetoothPeripheralConnectionSchedulerRecycled),
}

#[must_use = "the unlinked or removal-ready connection graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin {
    SchedulerIdentityMismatch {
        unlinked: BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    Pending(BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
    Ready(BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady),
}

#[must_use = "the unlinked or removal-ready connection graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck {
    SchedulerIdentityMismatch(BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
    StorageUnavailable(BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
    Pending(BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
    Ready(BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady),
}

/// Result of removing the sole connection item from the source-owned list.
#[must_use = "identity mismatch retains the empty-head graph"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep {
    SchedulerIdentityMismatch(BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved),
    Unlinked(BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked),
}

/// One bounded post-completion hardware-head retirement attempt.
#[must_use = "the connection completion owner must enter fail-stop handling or advance"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep {
    SchedulerIdentityMismatch(BluetoothPeripheralConnectionSchedulerCompletionObserved),
    FinishedListDrainStillActive(BluetoothPeripheralConnectionSchedulerCompletionObserved),
    ExpectedHeadStillPublished {
        completed: BluetoothPeripheralConnectionSchedulerCompletionObserved,
        observed: BluetoothSchedulerHardwareListHead,
    },
    UnexpectedHeadChanged {
        completed: BluetoothPeripheralConnectionSchedulerCompletionObserved,
        observed: BluetoothSchedulerHardwareListHead,
    },
    EmptyObserved(BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved),
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
    /// The remaining RX/head suffix is therefore infallible and ordered.
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
                return Err(
                    BluetoothPeripheralConnectionSchedulerHeadPublicationFailure { error, merged },
                );
            }
        };
        let BluetoothPeripheralConnectionEmptySchedulerMergePrepared { event, reservation } =
            merged;
        let (graph, remainder) = event.prepare_publication().into_parts();
        let graph = unsafe { self.task.publish_peripheral_connection_rx_memory(graph) };
        let event = remainder.join_rx_publication(graph);
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(BluetoothPeripheralConnectionSchedulerHeadPublished {
            event,
            publication,
            reservation,
        })
    }

    /// Perform one fresh, bounded first-connection completion observation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_peripheral_connection_completion(
        &mut self,
        running: BluetoothPeripheralConnectionSchedulerRunning,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothPeripheralConnectionSchedulerCompletionStep {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::SchedulerIdentityMismatch(
                running,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::DrainAlreadyActive(
                running,
            );
        }

        if self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
            .is_err()
        {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::DrainAlreadyActive(
                running,
            );
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerCompletionStep::NoFinishedList(running);
        };

        let BluetoothPeripheralConnectionSchedulerRunning {
            event,
            run,
            reservation,
        } = running;
        match event.observe_completion(observed) {
            BluetoothPeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running: event,
                observed,
            } => BluetoothPeripheralConnectionSchedulerCompletionStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPeripheralConnectionSchedulerRunning {
                        event,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPeripheralConnectionFirstEventCompletionObservation::StillInFlight(event) => {
                BluetoothPeripheralConnectionSchedulerCompletionStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerRunning {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                event,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPeripheralConnectionSchedulerCompletionStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerCompletionObserved {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured finished-list set while the connection is running.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_peripheral_connection_running_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerRunning,
        >,
    ) -> BluetoothPeripheralConnectionSchedulerRunningDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::DrainLost(pending);
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerRunningDrainStep::DrainLost(pending);
        };
        let BluetoothPeripheralConnectionSchedulerRunning {
            event,
            run,
            reservation,
        } = pending.into_owner();
        match event.observe_completion(observed) {
            BluetoothPeripheralConnectionFirstEventCompletionObservation::ListMismatch {
                running: event,
                observed,
            } => BluetoothPeripheralConnectionSchedulerRunningDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPeripheralConnectionSchedulerRunning {
                        event,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPeripheralConnectionFirstEventCompletionObservation::StillInFlight(event) => {
                BluetoothPeripheralConnectionSchedulerRunningDrainStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerRunning {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPeripheralConnectionFirstEventCompletionObservation::CompletionObserved(
                event,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPeripheralConnectionSchedulerRunningDrainStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPeripheralConnectionSchedulerCompletionObserved {
                            event,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue one captured set after list zero completed the connection item.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_peripheral_connection_completed_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ) -> BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::RepeatedConnectionList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        } else {
            BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        }
    }

    /// Observe the post-picker hardware-head retirement barrier for a connection.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_peripheral_connection_hardware_head_retirement(
        &mut self,
        completed: BluetoothPeripheralConnectionSchedulerCompletionObserved,
    ) -> BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
        }

        let BluetoothPeripheralConnectionSchedulerCompletionObserved {
            event,
            run,
            reservation,
        } = completed;
        match self
            .task
            .observe_scheduler_hardware_list_head_retirement(run)
        {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: BluetoothPeripheralConnectionSchedulerCompletionObserved {
                    event,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: BluetoothPeripheralConnectionSchedulerCompletionObserved {
                    event,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                assert_eq!(
                    head.completed_head().address(),
                    Some(address),
                    "the retired hardware head must retain the exact connection item"
                );
                self._scheduler_list
                    .retain_hardware_head_empty_first_item(address);
                BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
                        event,
                        head,
                        reservation,
                    },
                )
            }
        }
    }

    /// Remove the exact empty-head connection item from the source-owned list.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_peripheral_connection_software_list(
        &mut self,
        observed: BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed);
        }
        let BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved {
            event,
            head,
            reservation,
        } = observed;
        BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                event,
                head,
                reservation,
            },
        )
    }

    /// Join one serviced primary scheduler event to the already-unlinked connection.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_peripheral_connection_software_list_removal(
        &mut self,
        unlinked: BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Pending(
                    unlinked,
                );
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
            event,
            head,
            reservation,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Recheck one unlinked connection without requiring another interrupt edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_peripheral_connection_software_list_removal(
        &mut self,
        storage: &impl crate::BluetoothSchedulerRunInterruptStorage,
        unlinked: BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
    ) -> BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked);
        }
        let BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
            event,
            head,
            reservation,
        } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Pending(
                    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked {
                        event,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Ready(
                    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Copy RX results and release the connection event's three lower owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_peripheral_connection_completed(
        &mut self,
        ready: BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
    ) -> BluetoothPeripheralConnectionSchedulerRecycleStep {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothPeripheralConnectionSchedulerRecycleStep::SchedulerIdentityMismatch(
                ready,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPeripheralConnectionSchedulerRecycleStep::FinishedListDrainStillActive(
                ready,
            );
        }
        let BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
            event,
            removal,
            reservation,
        } = ready;
        let prepared = match event.prepare_recycle(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_parts();
                return BluetoothPeripheralConnectionSchedulerRecycleStep::MemoryIdentityMismatch {
                    ready: BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                    error,
                };
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (event, removal) = failure.into_prepared().into_parts();
                return BluetoothPeripheralConnectionSchedulerRecycleStep::ReceiveInvalid {
                    ready: BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                    error,
                };
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
                return BluetoothPeripheralConnectionSchedulerRecycleStep::ReservationIdentityMismatch(
                    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady {
                        event,
                        removal,
                        reservation,
                    },
                );
            }
        };
        let event = extracted.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        BluetoothPeripheralConnectionSchedulerRecycleStep::Recycled(
            BluetoothPeripheralConnectionSchedulerRecycled { event },
        )
    }
}
