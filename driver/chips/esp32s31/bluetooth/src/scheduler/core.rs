//! Fact-bounded scheduler initialization after the controller HAL component.

#[cfg(any(target_arch = "riscv32", test))]
mod connectable_advertising;
#[cfg(target_arch = "riscv32")]
pub(crate) use connectable_advertising::{
    BluetoothLegacyConnectableAdvertisingAdmissionObservation,
    BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    BluetoothLegacyConnectableAdvertisingEventPreparationError,
    BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    BluetoothLegacyConnectableAdvertisingEventPrepared,
    BluetoothLegacyConnectableAdvertisingPreSequence,
    BluetoothLegacyConnectableAdvertisingSequenceObservation,
};
mod dtm;
#[cfg(target_arch = "riscv32")]
mod single_item;
pub use dtm::{
    BluetoothDtmControllerEventPreparationError, BluetoothDtmEmptySchedulerMergePrepared,
    BluetoothDtmInitialSchedulerItemPhase, BluetoothDtmRecurringSchedulerItemPhase,
    BluetoothDtmSchedulerHeadPublicationFailure, BluetoothDtmSchedulerHeadPublished,
    BluetoothDtmSchedulerRunning,
};
#[cfg(target_arch = "riscv32")]
pub use dtm::{
    BluetoothDtmControllerRxPreparationFailure,
    BluetoothDtmControllerRxRecurringPreparationFailure,
    BluetoothDtmControllerTxPreparationFailure,
    BluetoothDtmControllerTxRecurringPreparationFailure, BluetoothDtmSchedulerCompletionObserved,
    BluetoothDtmSchedulerCompletionObservedDrainStep, BluetoothDtmSchedulerCompletionStep,
    BluetoothDtmSchedulerHardwareHeadEmptyObserved,
    BluetoothDtmSchedulerHardwareHeadRetirementStep, BluetoothDtmSchedulerRecycleStep,
    BluetoothDtmSchedulerRunningDrainStep, BluetoothDtmSchedulerRxSuccessRecycleStep,
    BluetoothDtmSchedulerSoftwareListRemovalReady, BluetoothDtmSchedulerSoftwareListUnlinkStep,
    BluetoothDtmSchedulerSoftwareListUnlinked,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use dtm::{
    BluetoothDtmFirstPreparationCompletionClass, classify_dtm_first_preparation_completion,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use dtm::{
    BluetoothDtmReceiverFirstPreSequence, BluetoothDtmReceiverFirstStaged,
    BluetoothDtmReceiverRecurringPreSequence, BluetoothDtmSchedulerSoftwareListRemovalJoin,
    BluetoothDtmSchedulerSoftwareListRemovalRecheck, BluetoothDtmTransmitterFirstPreSequence,
    BluetoothDtmTransmitterFirstStaged, BluetoothDtmTransmitterRecurringPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use single_item::*;

#[cfg(any(target_arch = "riscv32", test))]
mod peripheral_connection;
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::BluetoothPeripheralConnectionFirstPreSequence;
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::BluetoothPeripheralConnectionSchedulerCompletionClassification;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use peripheral_connection::{
    BluetoothPeripheralConnectionAdmissionObservation,
    BluetoothPeripheralConnectionSequenceObservation,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use peripheral_connection::{
    BluetoothPeripheralConnectionEmptySchedulerMergePrepared,
    BluetoothPeripheralConnectionFirstEventPreparationError,
};
#[cfg(target_arch = "riscv32")]
pub use peripheral_connection::{
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
pub(crate) use peripheral_connection::{
    BluetoothPeripheralConnectionRecurringSchedulerPublicationFailStop,
    BluetoothPeripheralConnectionRecurringSchedulerValidationFailure,
};
#[cfg(target_arch = "riscv32")]
pub use peripheral_connection::{
    BluetoothPeripheralConnectionSchedulerCompleted,
    BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    BluetoothPeripheralConnectionSchedulerHeadPublished,
    BluetoothPeripheralConnectionSchedulerRecycled,
};

use crate::BluetoothSchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
use crate::le::advertising::legacy::{
    BluetoothLegacyAdvertisingCompletionObservedEvent,
    BluetoothLegacyAdvertisingRecurringEventCandidate,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::timeline::BluetoothSchedulerRecurringReserved;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler::timeline::{
    BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerWindowReservation,
};
use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerModemTimerRuntime,
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerRuntimeResources,
    controller::hal::BluetoothControllerHalInitialized,
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothControllerTimeSample, BluetoothLegacyAdvertisingFirstEventCandidate,
    BluetoothSchedulerReservationError, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerSequenceReady, BluetoothSchedulerTimingPolicy,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeReceivedBatch, BluetoothLeRxError, BluetoothPassiveScanMemoryGraphCommandPublished,
    BluetoothPassiveScanMemoryGraphCompletionObserved,
    BluetoothPassiveScanMemoryGraphPublicationError,
    BluetoothPassiveScanMemoryGraphPublicationMismatch,
    BluetoothPassiveScanMemoryGraphRecycleError, BluetoothPassiveScanMemoryGraphRecycled,
    BluetoothPassiveScanSchedulerItemCompletionStatus,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothPassiveScanMemoryGraphCpuOwned, BluetoothPassiveScanMemoryGraphEventPrepared,
    BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared, BluetoothPassiveScanPrimaryChannel,
    BluetoothPassiveScanSchedulerWindow, BluetoothPassiveScanStartSelection,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerLatchedTime, BluetoothControllerSramAddress,
    BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareListsCleared,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

#[cfg(any(target_arch = "riscv32", test))]
fn retain_matching_single_item_identity<Identity: Copy + Eq, Owner>(
    expected: Identity,
    observed: Identity,
    owner: Owner,
) -> Result<Owner, (Identity, Owner)> {
    if observed == expected {
        Ok(owner)
    } else {
        Err((expected, owner))
    }
}

/// Fresh initial-admission sample sealed by the controller-time worker.
///
/// External code can carry this capability but cannot create one from an
/// integer timestamp. It is distinct from the later sequence-deadline sample.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh admission observation must be consumed or retained"]
pub struct BluetoothLegacyAdvertisingAdmissionObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh sequence observation must be consumed or retained"]
pub struct BluetoothLegacyAdvertisingSequenceObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// First advertising event after common timeline admission and before the
/// second sequence deadline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted event must pass sequence authorization or be retained"]
pub struct BluetoothLegacyAdvertisingFirstPreSequence<'a> {
    candidate: BluetoothLegacyAdvertisingFirstEventCandidate<'a>,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
}

/// Recurring advertising event after exact timeline reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "authorize the recurring sequence deadline or retain the event"]
pub struct BluetoothLegacyAdvertisingRecurringPreSequence<'a> {
    candidate: BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>,
}

/// Why one recurring event could not reach a sequence-ready descriptor.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    EventImage(
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
    ),
}

/// Lossless recurring admission/preparation rejection.
#[cfg(target_arch = "riscv32")]
#[must_use = "retry, cancel, or retain the recurring event candidate"]
pub struct BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'a> {
    candidate: BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
    error: BluetoothLegacyAdvertisingRecurringEventPreparationError,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingRecurringEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> BluetoothLegacyAdvertisingRecurringEventCandidate<'a> {
        self.candidate
    }
}

/// First advertising descriptor paired with its exact accepted timeline slot.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared event must be published, cancelled through its controller, or retained"]
pub struct BluetoothLegacyAdvertisingEventPrepared<'a> {
    image: crate::le::advertising::legacy::BluetoothLegacyAdvertisingEventImagePrepared<'a>,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyAdvertisingEventPrepared<'_> {
    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.image.identity()
    }

    pub fn pdu(&self) -> &[u8] {
        self.image.pdu()
    }

    /// Opaque nominal phase required by later advertising events.
    pub const fn phase(&self) -> crate::BluetoothLegacyAdvertisingEventPhase {
        self.image.phase()
    }
}

/// Lossless rejection while joining one advertising item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged advertising event remains prepared and CPU-owned"]
pub struct BluetoothLegacyAdvertisingEmptySchedulerMergeFailure<'a> {
    error: BluetoothSchedulerEmptyListMergeError,
    prepared: BluetoothLegacyAdvertisingEventPrepared<'a>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingEmptySchedulerMergeFailure<'a> {
    pub const fn error(&self) -> BluetoothSchedulerEmptyListMergeError {
        self.error
    }

    pub fn into_prepared(self) -> BluetoothLegacyAdvertisingEventPrepared<'a> {
        self.prepared
    }
}

/// First advertising event joined to the source-owned empty scheduler list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the merged advertising event must be published or cancelled"]
pub struct BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a> {
    item: crate::le::advertising::legacy::BluetoothLegacyAdvertisingEmptyListLinkPrepared<'a>,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }
}

/// Fresh initial-admission sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner admission observation must be consumed or retained"]
pub struct BluetoothPassiveScanAdmissionObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner sequence observation must be consumed or retained"]
pub struct BluetoothPassiveScanSequenceObservation {
    pub(crate) sample: BluetoothControllerTimeSample,
}

/// CPU-owned scanner graph with a requested window not yet admitted to the
/// common scheduler timeline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the scanner candidate must enter common scheduling or be returned"]
pub struct BluetoothPassiveScanFirstEventCandidate {
    graph: BluetoothPassiveScanMemoryGraphCpuOwned,
    channel: BluetoothPassiveScanPrimaryChannel,
    requested_window: crate::BluetoothSchedulerRawWindow,
    controller_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPassiveScanFirstEventCandidate {
    pub(crate) const fn new(
        graph: BluetoothPassiveScanMemoryGraphCpuOwned,
        channel: BluetoothPassiveScanPrimaryChannel,
        requested_window: crate::BluetoothSchedulerRawWindow,
        controller_time: BluetoothControllerLatchedTime,
    ) -> Self {
        Self {
            graph,
            channel,
            requested_window,
            controller_time,
        }
    }

    pub const fn requested_window(&self) -> crate::BluetoothSchedulerRawWindow {
        self.requested_window
    }

    pub fn cancel(self) -> BluetoothPassiveScanMemoryGraphCpuOwned {
        self.graph
    }

    fn prepare_resolved_event(
        self,
        resolved_window: crate::BluetoothSchedulerRawWindow,
    ) -> BluetoothPassiveScanMemoryGraphEventPrepared {
        let window = BluetoothPassiveScanSchedulerWindow::from_controller_ticks(
            resolved_window.start(),
            resolved_window.end(),
        )
        .expect("a timeline reservation retains a non-empty forward window");
        let selection = if resolved_window.start() == self.requested_window.start() {
            BluetoothPassiveScanStartSelection::Requested
        } else {
            BluetoothPassiveScanStartSelection::EarliestAvailable
        };
        self.graph
            .prepare_first_event(self.channel, window, selection, self.controller_time)
    }
}

/// First scanner event after common timeline admission and before the second
/// sequence deadline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted scanner event must pass sequence authorization or be cancelled"]
pub struct BluetoothPassiveScanFirstPreSequence {
    candidate: BluetoothPassiveScanFirstEventCandidate,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
}

/// Scanner event image paired with the exact timeline interval encoded into it.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared scanner event must be merged, cancelled, or retained"]
pub struct BluetoothPassiveScanEventPrepared {
    graph: BluetoothPassiveScanMemoryGraphEventPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPassiveScanEventPrepared {
    pub const fn channel(&self) -> BluetoothPassiveScanPrimaryChannel {
        self.graph.channel()
    }

    pub const fn window(&self) -> BluetoothPassiveScanSchedulerWindow {
        self.graph.window()
    }
}

/// Finite scanner preparation rejection before any MMIO publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum BluetoothPassiveScanFirstEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
}

/// Lossless first-scanner-event preparation rejection.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged scanner graph must be retried, cancelled, or retained"]
pub struct BluetoothPassiveScanFirstEventPreparationFailure {
    candidate: BluetoothPassiveScanFirstEventCandidate,
    error: BluetoothPassiveScanFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPassiveScanFirstEventPreparationFailure {
    pub const fn error(&self) -> BluetoothPassiveScanFirstEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> BluetoothPassiveScanFirstEventCandidate {
        self.candidate
    }
}

/// Lossless rejection while joining one detached scanner item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged detached scanner event remains CPU-owned"]
pub struct BluetoothPassiveScanEmptySchedulerMergeFailure {
    error: BluetoothSchedulerEmptyListMergeError,
    prepared: BluetoothPassiveScanEventPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPassiveScanEmptySchedulerMergeFailure {
    /// Exact reason the exclusive scheduler epoch rejected the scanner item.
    pub const fn error(&self) -> BluetoothSchedulerEmptyListMergeError {
        self.error
    }

    /// Recover the unchanged detached scanner graph.
    pub fn into_prepared(self) -> BluetoothPassiveScanEventPrepared {
        self.prepared
    }
}

/// Detached scanner item joined to the source-owned empty scheduler list.
///
/// No scanner register, RX-list head, scheduler head or RUN command is visible
/// to hardware in this state. Cancellation restores both the common list epoch
/// and the scanner's private three-item free chain.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the scanner merge must be published through the same scheduler or cancelled"]
pub struct BluetoothPassiveScanEmptySchedulerMergePrepared {
    graph: BluetoothPassiveScanMemoryGraphSchedulerAdmissionPrepared,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothPassiveScanEmptySchedulerMergePrepared {
    /// Exact detached item selected as the common scheduler head.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    /// Hardware list assigned to the first standalone passive scanner.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        BluetoothSchedulerHardwareListIndex::ZERO
    }
}

#[cfg(target_arch = "riscv32")]
enum BluetoothPassiveScanSchedulerHeadPublicationFailureOwner {
    PrePublication(BluetoothPassiveScanEmptySchedulerMergePrepared),
    RxPublication {
        _mismatch: BluetoothPassiveScanMemoryGraphPublicationMismatch,
        _reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
        _head: BluetoothSchedulerHardwareListHead,
    },
}

/// Lossless scanner-head publication failure.
///
/// A scheduler-head validation error is retryable because it occurs before
/// MMIO. An RX publication mismatch follows the first MMIO write and is sealed
/// fail-stop while retaining the graph, HAL publication, reservation and
/// validated scheduler head.
#[cfg(target_arch = "riscv32")]
#[must_use = "inspect whether the exact retained owner is retryable or fail-stop"]
pub struct BluetoothPassiveScanSchedulerHeadPublicationFailure {
    head_error: Option<BluetoothSchedulerHeadPublicationError>,
    rx_publication_error: Option<BluetoothPassiveScanMemoryGraphPublicationError>,
    owner: BluetoothPassiveScanSchedulerHeadPublicationFailureOwner,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerHeadPublicationFailure {
    /// Exact reason the common scheduler head could not be prepared.
    pub const fn head_error(&self) -> Option<BluetoothSchedulerHeadPublicationError> {
        self.head_error
    }

    /// Return the typed RX publication mismatch after the first MMIO write.
    pub const fn rx_publication_error(
        &self,
    ) -> Option<BluetoothPassiveScanMemoryGraphPublicationError> {
        self.rx_publication_error
    }

    /// Recover the unchanged merge only for a pre-MMIO validation failure.
    pub fn into_retryable_merged(
        self,
    ) -> Result<BluetoothPassiveScanEmptySchedulerMergePrepared, Self> {
        match self {
            Self {
                owner:
                    BluetoothPassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(merged),
                ..
            } => Ok(merged),
            failure @ Self {
                owner:
                    BluetoothPassiveScanSchedulerHeadPublicationFailureOwner::RxPublication { .. },
                ..
            } => Err(failure),
        }
    }
}

/// Scanner graph whose RX list, command and scheduler head are hardware-visible.
///
/// The publication transaction validates the common list identity before its
/// first irreversible write, then publishes RX memory, the restricted scanner
/// command and the exact scheduler head in that order. Dynamic interrupts and
/// RUN remain absent.
#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner head must advance through the common RUN suffix"]
pub struct BluetoothPassiveScanSchedulerHeadPublished {
    graph: BluetoothPassiveScanMemoryGraphCommandPublished,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerHeadPublished {
    /// Exact scanner item retained by the graph and hardware head token.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_head()
    }

    /// Hardware list containing the first scanner item.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothPassiveScanMemoryGraphCommandPublished,
        BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.graph, self.publication, self.reservation)
    }
}

/// CPU-owned scanner graph and copied receive results.
#[cfg(target_arch = "riscv32")]
#[must_use = "return the graph and received packets to the scanner role owner"]
pub(crate) struct BluetoothPassiveScanSchedulerRecycled {
    graph: BluetoothPassiveScanMemoryGraphRecycled,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerRecycled {
    pub fn into_parts(
        self,
    ) -> (
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphCpuOwned,
        BluetoothLeReceivedBatch,
        BluetoothPassiveScanSchedulerItemCompletionStatus,
    ) {
        self.graph.into_parts()
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothPassiveScanSchedulerRecycleReady {
    graph: BluetoothPassiveScanMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerRecycleReady {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the scanner graph; success returns CPU ownership"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc role-tail outcomes retain the exact scanner graph on every branch"
)]
pub(crate) enum BluetoothPassiveScanSchedulerRecycleStep {
    SchedulerIdentityMismatch {
        _ready: BluetoothPassiveScanSchedulerRecycleReady,
    },
    FinishedListDrainStillActive {
        _ready: BluetoothPassiveScanSchedulerRecycleReady,
    },
    MemoryIdentityMismatch {
        _ready: BluetoothPassiveScanSchedulerRecycleReady,
        _error: BluetoothPassiveScanMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        _ready: BluetoothPassiveScanSchedulerRecycleReady,
        _error: BluetoothLeRxError,
    },
    ReservationIdentityMismatch {
        _ready: BluetoothPassiveScanSchedulerRecycleReady,
    },
    Recycled(BluetoothPassiveScanSchedulerRecycled),
}

/// Lossless rejection before advertising scheduler-head MMIO publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the unchanged advertising merge remains CPU-owned"]
pub struct BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure<'a> {
    error: BluetoothSchedulerHeadPublicationError,
    merged: BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure<'a> {
    pub const fn error(&self) -> BluetoothSchedulerHeadPublicationError {
        self.error
    }

    pub fn into_merged(self) -> BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a> {
        self.merged
    }
}

/// Advertising graph whose first scheduler item is hardware-visible.
#[cfg(target_arch = "riscv32")]
#[must_use = "the published advertising head must advance through the RUN suffix"]
pub struct BluetoothLegacyAdvertisingSchedulerHeadPublished<'a> {
    item: crate::le::advertising::legacy::BluetoothLegacyAdvertisingHeadPublishedEvent<'a>,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    _reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingSchedulerHeadPublished<'a> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::le::advertising::legacy::BluetoothLegacyAdvertisingHeadPublishedEvent<'a>,
        BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.item, self.publication, self._reservation)
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the completed event must advance the LL owner exactly once"]
pub(crate) struct BluetoothLegacyAdvertisingSchedulerRecycled<'a> {
    item: crate::le::advertising::legacy::BluetoothLegacyAdvertisingRecycledEvent<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingSchedulerRecycled<'a> {
    /// Advance the exact LL event while retaining S31 diagnostic statuses.
    pub fn complete_event(
        self,
    ) -> crate::le::advertising::legacy::BluetoothLegacyAdvertisingEventCompleted<'a> {
        self.item.complete_event()
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothLegacyAdvertisingSchedulerRecycleReady<'a> {
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingSchedulerRecycleReady<'_> {
    const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the advertising graph; success retains CPU ownership"]
pub(crate) enum BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
    SchedulerIdentityMismatch {
        _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    FinishedListDrainStillActive {
        _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    MemoryIdentityMismatch {
        _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady<'a>,
        _error: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphRecycleError,
    },
    ReservationIdentityMismatch {
        _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady<'a>,
    },
    Recycled(BluetoothLegacyAdvertisingSchedulerRecycled<'a>),
}

/// Finite reason a first advertising event returned to pre-admission state.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingFirstEventPreparationError {
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    EventImage(
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphEventPrepareError,
    ),
}

/// Rejected first advertising event retaining the exact cancellable candidate.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the advertising candidate remains recoverable"]
pub struct BluetoothLegacyAdvertisingFirstEventPreparationFailure<'a> {
    candidate: BluetoothLegacyAdvertisingFirstEventCandidate<'a>,
    error: BluetoothLegacyAdvertisingFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<'a> BluetoothLegacyAdvertisingFirstEventPreparationFailure<'a> {
    pub const fn error(&self) -> BluetoothLegacyAdvertisingFirstEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> BluetoothLegacyAdvertisingFirstEventCandidate<'a> {
        self.candidate
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl core::fmt::Debug for BluetoothLegacyAdvertisingFirstEventPreparationFailure<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothLegacyAdvertisingFirstEventPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Exclusive empty scheduler-list epoch owned by the source controller.
///
/// The PAC proof establishes that no hardware-list head remains published.
/// This owner adds the independently constructed source-owned software list,
/// which starts empty and cannot be aliased through a vendor container.
pub(crate) struct BluetoothSchedulerExclusiveListEpoch {
    _hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    state: BluetoothSchedulerExclusiveListState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothSchedulerExclusiveListState {
    Empty,
    FirstItemPrepared {
        address: BluetoothControllerSramAddress,
    },
    FirstItemHeadPublished {
        address: BluetoothControllerSramAddress,
    },
    FirstItemRunning {
        address: BluetoothControllerSramAddress,
    },
    FirstItemCompletionObserved {
        address: BluetoothControllerSramAddress,
    },
    FirstItemHardwareHeadEmptyObserved {
        address: BluetoothControllerSramAddress,
    },
    FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
        address: BluetoothControllerSramAddress,
    },
    FirstItemSoftwareListRemovalReady {
        address: BluetoothControllerSramAddress,
    },
}

impl BluetoothSchedulerExclusiveListEpoch {
    const fn new(hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared) -> Self {
        Self {
            _hardware_lists_cleared: hardware_lists_cleared,
            state: BluetoothSchedulerExclusiveListState::Empty,
        }
    }

    fn prepare_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> Result<(), BluetoothSchedulerEmptyListMergeError> {
        if self.state != BluetoothSchedulerExclusiveListState::Empty {
            return Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty);
        }
        self.state = BluetoothSchedulerExclusiveListState::FirstItemPrepared { address };
        Ok(())
    }

    fn cancel_first_item(&mut self, address: BluetoothControllerSramAddress) -> bool {
        if self.state != (BluetoothSchedulerExclusiveListState::FirstItemPrepared { address }) {
            return false;
        }
        self.state = BluetoothSchedulerExclusiveListState::Empty;
        true
    }

    fn can_publish_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        matches!(
            self.state,
            BluetoothSchedulerExclusiveListState::FirstItemPrepared {
                address: prepared
            } if prepared == address
        )
    }

    fn retain_published_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert!(
            self.can_publish_first_item(address),
            "only the merge-selected first item can become the hardware head"
        );
        self.state = BluetoothSchedulerExclusiveListState::FirstItemHeadPublished { address };
    }

    pub(crate) fn retain_running_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert_eq!(
            self.state,
            BluetoothSchedulerExclusiveListState::FirstItemHeadPublished { address },
            "only the published first item can enter the running scheduler phase"
        );
        self.state = BluetoothSchedulerExclusiveListState::FirstItemRunning { address };
    }

    fn retains_running_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        self.state == BluetoothSchedulerExclusiveListState::FirstItemRunning { address }
    }

    fn retain_completion_observed_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_running_first_item(address) {
            return false;
        }
        self.state = BluetoothSchedulerExclusiveListState::FirstItemCompletionObserved { address };
        true
    }

    fn retains_completion_observed_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == BluetoothSchedulerExclusiveListState::FirstItemCompletionObserved { address }
    }

    fn retain_hardware_head_empty_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_completion_observed_first_item(address) {
            return false;
        }
        self.state =
            BluetoothSchedulerExclusiveListState::FirstItemHardwareHeadEmptyObserved { address };
        true
    }

    fn retains_hardware_head_empty_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state
            == BluetoothSchedulerExclusiveListState::FirstItemHardwareHeadEmptyObserved { address }
    }

    fn unlink_software_list_first_item(&mut self, address: BluetoothControllerSramAddress) -> bool {
        if !self.retains_hardware_head_empty_first_item(address) {
            return false;
        }
        self.state =
            BluetoothSchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
                address,
            };
        true
    }

    fn retains_unlinked_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        self.state
            == BluetoothSchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
                address,
            }
    }

    fn retain_software_list_removal_ready_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_unlinked_first_item(address) {
            return false;
        }
        self.state =
            BluetoothSchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address };
        true
    }

    fn retains_software_list_removal_ready_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state
            == BluetoothSchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address }
    }

    fn commit_recycled_first_item(&mut self) {
        self.state = BluetoothSchedulerExclusiveListState::Empty;
    }
}

/// Why a first scheduler item could not consume the exclusive empty-list epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerEmptyListMergeError {
    /// Another item already consumed this scheduler epoch's empty-list proof.
    ListNotEmpty,
}

/// Why one private DTM controller-time phase could not complete.
///
/// Post-enable timing, recurring-RX current, admission and sequence samples are
/// acquired by the Controller and never cross the public DTM preparation
/// boundary. This finite error retains only the logical acquisition outcome;
/// the role-specific preparation failure continues to own every retry resource.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerTimeAcquisitionError {
    /// Another request or abandoned request still owns the latch worker.
    Busy,
    /// The logical worker and lower sticky latch owner disagreed at begin.
    OwnershipCollision,
    /// The non-repeating request generation space was exhausted.
    GenerationExhausted,
    /// A recheck or cancellation named a different logical request.
    RequestMismatch,
    /// The lower sticky latch owner disappeared before completion.
    OwnershipLost,
    /// An earlier ownership disagreement stopped the latch worker.
    Faulted,
    /// The caller explicitly abandoned this phase before completion.
    Cancelled,
}

/// Why a prepared first-item merge could not publish its scheduler head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerHeadPublicationError {
    /// The merge belongs to another scheduler epoch or list identity.
    SchedulerIdentityMismatch,
    /// The selected address aliases the reserved empty hardware-head image.
    EncodesEmptyHead,
}

impl From<BluetoothSchedulerHardwareListHeadError> for BluetoothSchedulerHeadPublicationError {
    fn from(error: BluetoothSchedulerHardwareListHeadError) -> Self {
        match error {
            BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead => Self::EncodesEmptyHead,
        }
    }
}

/// Affine proof that one specific captured finished-list set remains pending.
///
/// This token is created only by a bounded drain step which consumed one list
/// and observed that the same capture retained another list. It cannot be
/// constructed, copied or detached from the graph owner it protects.
#[must_use = "the retained finished-list capture must be continued or preserved"]
#[cfg(any(test, target_arch = "riscv32"))]
pub struct BluetoothSchedulerFinishedListDrainPending<Owner> {
    owner: Owner,
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> BluetoothSchedulerFinishedListDrainPending<Owner> {
    const fn new(owner: Owner) -> Self {
        Self { owner }
    }

    const fn owner(&self) -> &Owner {
        &self.owner
    }

    fn into_owner(self) -> Owner {
        self.owner
    }
}

/// Resulting ownership state after exactly one captured list was consumed.
///
/// `Drained` contains the ordinary graph owner only when the captured set is
/// exhausted. `Pending` retains both that owner and the provenance required to
/// consume the next list from the same capture.
#[must_use = "the graph and any pending finished-list capture must be retained"]
#[cfg(any(test, target_arch = "riscv32"))]
pub enum BluetoothSchedulerFinishedListDrainState<Owner> {
    /// The captured set is exhausted; no continuation is permitted.
    Drained(Owner),
    /// The same captured set retains another list.
    Pending(BluetoothSchedulerFinishedListDrainPending<Owner>),
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> BluetoothSchedulerFinishedListDrainState<Owner> {
    fn from_worker_step(owner: Owner, more: bool) -> Self {
        if more {
            Self::Pending(BluetoothSchedulerFinishedListDrainPending::new(owner))
        } else {
            Self::Drained(owner)
        }
    }
}

/// Hardware and source-owned software state after scheduler initialization.
///
/// This transition replaces the complete reviewed scheduler-init function:
/// all sixteen hardware list heads are removed, the scheduler policy is
/// retained without copying the vendor structure ABI, and one pristine static
/// Rust runtime replaces the vendor event object and generic broker nodes.
/// Typed event cells and workers make numeric broker source identifiers and an
/// intrusive callback list unnecessary.
///
/// The bounded software timeline is retained in the runtime owner, while
/// scheduler-item hardware publication, remaining hardware initialization and
/// stable ISR publication are still missing. This state therefore exposes no
/// PHY, BTBB, IRQ, Controller or Link-Layer readiness. HCI remains outside the
/// hardware boot chain until stable interrupt-owner publication completes.
/// Dropping this state is fail-stop because no verified rollback exists after
/// scheduler MMIO mutation.
#[must_use = "the initialized scheduler retains every powered Bluetooth owner"]
pub struct BluetoothSchedulerInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    task: BluetoothTaskResources,
    _interrupts: Option<BluetoothInterruptBankOwner>,
    _platform: BluetoothTeardownPendingPlatform<P>,
    time_scale: BluetoothControllerTimeScale,
    _standalone_dtm_profile: crate::controller::hal::BluetoothStandaloneAlwaysAwakeDtmProfile,
    config: BluetoothSchedulerSoftwareConfig,
    _scheduler_list: BluetoothSchedulerExclusiveListEpoch,
    runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    pub(crate) fn task_mut(&mut self) -> &mut BluetoothTaskResources {
        &mut self.task
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_phase(
        &self,
    ) -> crate::controller::time::BluetoothControllerTimeWorkerPhase {
        self.task.controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.task.controller_time_needs_recheck()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> BluetoothInterruptBankOwner {
        self._interrupts
            .take()
            .expect("private Controller invariant retains the interrupt owner until activation")
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(&mut self) -> (&mut BluetoothTaskResources, &mut P) {
        (&mut self.task, self._platform.platform_mut())
    }

    /// Number of fixed modem timer slots retained by the initialized epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        self.runtime.modem_timer_capacity()
    }

    /// Number of fixed scheduler reservations retained by this epoch.
    pub const fn scheduler_capacity(&self) -> usize {
        self.runtime.scheduler_capacity()
    }

    /// Return the scheduler scale retained by this exact hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Return the source-owned scheduler policy for this hardware epoch.
    pub const fn scheduler_config(&self) -> BluetoothSchedulerSoftwareConfig {
        self.config
    }

    /// Whether no software event has entered the initialized epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller::time::BluetoothControllerTimeRequest,
        crate::controller::time::BluetoothControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller::time::BluetoothControllerTimeRequest,
    ) -> Result<(), crate::controller::time::BluetoothControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller::time::BluetoothControllerTimeRequest,
    ) -> Result<
        crate::controller::time::BluetoothControllerTimeEventStep,
        crate::controller::time::BluetoothControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller::time::BluetoothControllerTimeEventStep,
        crate::controller::time::BluetoothControllerTimeEventError,
    > {
        self.task.drain_orphan_controller_time()
    }

    /// Admit one already projected first advertising event into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn admit_legacy_advertising_first_event<'a>(
        &mut self,
        candidate: BluetoothLegacyAdvertisingFirstEventCandidate<'a>,
        admission: BluetoothLegacyAdvertisingAdmissionObservation,
    ) -> Result<
        BluetoothLegacyAdvertisingFirstPreSequence<'a>,
        BluetoothLegacyAdvertisingFirstEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                raw_window.start(),
                raw_window.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(BluetoothLegacyAdvertisingFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(BluetoothLegacyAdvertisingFirstEventPreparationFailure {
                candidate,
                error: BluetoothLegacyAdvertisingFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the second deadline and encode the overlap-resolved event image.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn prepare_legacy_advertising_first_event<'a>(
        &mut self,
        admitted: BluetoothLegacyAdvertisingFirstPreSequence<'a>,
        sequence: BluetoothLegacyAdvertisingSequenceObservation,
    ) -> Result<
        BluetoothLegacyAdvertisingEventPrepared<'a>,
        BluetoothLegacyAdvertisingFirstEventPreparationFailure<'a>,
    > {
        let BluetoothLegacyAdvertisingFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(BluetoothLegacyAdvertisingFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyAdvertisingFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(resolved_window) {
            Ok(image) => Ok(BluetoothLegacyAdvertisingEventPrepared { image, reservation }),
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(BluetoothLegacyAdvertisingFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyAdvertisingFirstEventPreparationError::EventImage(error),
                })
            }
        }
    }

    /// Reserve one exact recurring advertising window without displacement.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    pub fn admit_legacy_advertising_recurring_event<'a>(
        &mut self,
        candidate: BluetoothLegacyAdvertisingRecurringEventCandidate<'a>,
    ) -> Result<
        BluetoothLegacyAdvertisingRecurringPreSequence<'a>,
        BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(raw_window.start(), raw_window.end(), timing_policy)
        {
            Ok(reservation) => Ok(BluetoothLegacyAdvertisingRecurringPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(BluetoothLegacyAdvertisingRecurringEventPreparationFailure {
                candidate,
                error: BluetoothLegacyAdvertisingRecurringEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the recurring deadline and encode its complete event chain.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    pub fn prepare_legacy_advertising_recurring_event<'a>(
        &mut self,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'a>,
        sequence: BluetoothLegacyAdvertisingSequenceObservation,
    ) -> Result<
        BluetoothLegacyAdvertisingEventPrepared<'a>,
        BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'a>,
    > {
        let BluetoothLegacyAdvertisingRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(BluetoothLegacyAdvertisingRecurringEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyAdvertisingRecurringEventPreparationError::Sequence(
                        error,
                    ),
                });
            }
        };
        let resolved_window = reservation.window();
        match candidate.prepare_resolved_event_image(resolved_window) {
            Ok(prepared) => {
                let (image, _, _, _) = prepared.into_parts();
                Ok(BluetoothLegacyAdvertisingEventPrepared { image, reservation })
            }
            Err(failure) => {
                let error = failure.error();
                let candidate = failure.into_candidate();
                self.release_scheduler_reservation(reservation);
                Err(BluetoothLegacyAdvertisingRecurringEventPreparationFailure {
                    candidate,
                    error: BluetoothLegacyAdvertisingRecurringEventPreparationError::EventImage(
                        error,
                    ),
                })
            }
        }
    }

    /// Release an unpublished first advertising event and restore both owners.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_legacy_advertising_first_event<'a>(
        &mut self,
        prepared: BluetoothLegacyAdvertisingEventPrepared<'a>,
    ) -> crate::BluetoothLegacyAdvertisingCancelled<'a> {
        let BluetoothLegacyAdvertisingEventPrepared { image, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        image.cancel()
    }

    /// Release an admitted first event before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_legacy_advertising_first_pre_sequence<'a>(
        &mut self,
        admitted: BluetoothLegacyAdvertisingFirstPreSequence<'a>,
    ) -> crate::BluetoothLegacyAdvertisingCancelled<'a> {
        let BluetoothLegacyAdvertisingFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    /// Release an admitted recurring event before its sequence sample arrives.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_legacy_advertising_recurring_pre_sequence<'a>(
        &mut self,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'a>,
    ) -> crate::BluetoothLegacyAdvertisingRecurringCancelled<'a> {
        let BluetoothLegacyAdvertisingRecurringPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    /// Admit one requested passive-scanner window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn admit_passive_scan_first_event(
        &mut self,
        candidate: BluetoothPassiveScanFirstEventCandidate,
        admission: BluetoothPassiveScanAdmissionObservation,
    ) -> Result<
        BluetoothPassiveScanFirstPreSequence,
        BluetoothPassiveScanFirstEventPreparationFailure,
    > {
        let requested = candidate.requested_window();
        let timing_policy =
            BluetoothSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_phase_locked_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(BluetoothPassiveScanFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(BluetoothPassiveScanFirstEventPreparationFailure {
                candidate,
                error: BluetoothPassiveScanFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the second deadline and only then encode the
    /// overlap-resolved scanner window into private SRAM.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn prepare_passive_scan_first_event(
        &mut self,
        admitted: BluetoothPassiveScanFirstPreSequence,
        sequence: BluetoothPassiveScanSequenceObservation,
    ) -> Result<BluetoothPassiveScanEventPrepared, BluetoothPassiveScanFirstEventPreparationFailure>
    {
        let BluetoothPassiveScanFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(BluetoothPassiveScanFirstEventPreparationFailure {
                    candidate,
                    error: BluetoothPassiveScanFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let graph = candidate.prepare_resolved_event(reservation.window());
        Ok(BluetoothPassiveScanEventPrepared { graph, reservation })
    }

    /// Release one unpublished scanner event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_passive_scan_first_event(
        &mut self,
        prepared: BluetoothPassiveScanEventPrepared,
    ) -> BluetoothPassiveScanMemoryGraphCpuOwned {
        let BluetoothPassiveScanEventPrepared { graph, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        graph.into_cpu_owned()
    }

    /// Release an admitted scanner candidate before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_passive_scan_first_pre_sequence(
        &mut self,
        admitted: BluetoothPassiveScanFirstPreSequence,
    ) -> BluetoothPassiveScanMemoryGraphCpuOwned {
        let BluetoothPassiveScanFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        self.release_scheduler_reservation(reservation);
        candidate.cancel()
    }

    /// Join one prepared advertising item to this epoch's empty scheduler list.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn prepare_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        prepared: BluetoothLegacyAdvertisingEventPrepared<'a>,
    ) -> Result<
        BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
        BluetoothLegacyAdvertisingEmptySchedulerMergeFailure<'a>,
    > {
        let BluetoothLegacyAdvertisingEventPrepared { image, reservation } = prepared;
        let item = image.prepare_scheduler_bookkeeping();
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothLegacyAdvertisingEmptySchedulerMergeFailure {
                error,
                prepared: BluetoothLegacyAdvertisingEventPrepared {
                    image: item.cancel(),
                    reservation,
                },
            });
        }
        Ok(BluetoothLegacyAdvertisingEmptySchedulerMergePrepared {
            item: item.prepare_empty_list_link(),
            reservation,
        })
    }

    /// Cancel a not-yet-published advertising merge through the same list epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "an identity rejection retains the complete advertising merge"
    )]
    pub fn cancel_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        merged: BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        BluetoothLegacyAdvertisingEventPrepared<'a>,
        BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let BluetoothLegacyAdvertisingEmptySchedulerMergePrepared { item, reservation } = merged;
        Ok(BluetoothLegacyAdvertisingEventPrepared {
            image: item.cancel().cancel(),
            reservation,
        })
    }

    /// Join the detached first scanner item to this epoch's empty scheduler list.
    ///
    /// The private scanner graph has already removed the item from its free
    /// chain. This transition atomically reserves the same address in the
    /// source-owned common list without publishing MMIO.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn prepare_passive_scan_empty_list_merge(
        &mut self,
        prepared: BluetoothPassiveScanEventPrepared,
    ) -> Result<
        BluetoothPassiveScanEmptySchedulerMergePrepared,
        BluetoothPassiveScanEmptySchedulerMergeFailure,
    > {
        let BluetoothPassiveScanEventPrepared { graph, reservation } = prepared;
        let graph = graph.prepare_scheduler_admission();
        let address = graph.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothPassiveScanEmptySchedulerMergeFailure {
                error,
                prepared: BluetoothPassiveScanEventPrepared {
                    graph: graph.cancel(),
                    reservation,
                },
            });
        }
        Ok(BluetoothPassiveScanEmptySchedulerMergePrepared { graph, reservation })
    }

    /// Restore an unpublished scanner merge through the same scheduler epoch.
    ///
    /// Success restores both the common empty-list proof and the selected
    /// scanner item's position in the private three-item free chain.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_passive_scan_empty_list_merge(
        &mut self,
        merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
    ) -> Result<BluetoothPassiveScanEventPrepared, BluetoothPassiveScanEmptySchedulerMergePrepared>
    {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let BluetoothPassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        Ok(BluetoothPassiveScanEventPrepared {
            graph: graph.cancel(),
            reservation,
        })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn release_scheduler_reservation<State>(
        &mut self,
        reservation: BluetoothSchedulerWindowReservation<State>,
    ) {
        self.runtime
            .scheduler_timeline_mut()
            .release(reservation)
            .expect("a reservation created by this Controller must release into the same timeline");
    }

    /// Publish one prepared advertising item through the common head edge.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection retains the complete advertising merge"
    )]
    pub(crate) fn publish_legacy_advertising_scheduler_head<'a>(
        &mut self,
        merged: BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<
        BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
        BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure<'a>,
    > {
        let address = merged.scheduler_item_address();
        let publication =
            match self.publish_first_scheduler_item_head(address, merged.hardware_list_index()) {
                Ok(publication) => publication,
                Err(error) => {
                    return Err(BluetoothLegacyAdvertisingSchedulerHeadPublicationFailure {
                        error,
                        merged,
                    });
                }
            };
        let BluetoothLegacyAdvertisingEmptySchedulerMergePrepared { item, reservation } = merged;
        let item = item.into_head_published(&publication);
        Ok(BluetoothLegacyAdvertisingSchedulerHeadPublished {
            item,
            publication,
            _reservation: reservation,
        })
    }

    /// Publish the complete lower passive-scanner transaction.
    ///
    /// The common list identity and scheduler-head encoding are checked before
    /// MMIO. An RX publication mismatch seals every owner after that first
    /// write; only a matching RX proof continues to the restricted scanner
    /// command and scheduler head in the reviewed hardware order.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the powered task owner and exact scanner graph jointly retain every PAC publication prerequisite"
    )]
    pub(crate) fn publish_passive_scan_scheduler_head(
        &mut self,
        merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
    ) -> Result<
        BluetoothPassiveScanSchedulerHeadPublished,
        BluetoothPassiveScanSchedulerHeadPublicationFailure,
    > {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(BluetoothPassiveScanSchedulerHeadPublicationFailure {
                    head_error: Some(error),
                    rx_publication_error: None,
                    owner: BluetoothPassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(
                        merged,
                    ),
                });
            }
        };
        let BluetoothPassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        let graph = graph.prepare_publication();
        let graph = match unsafe { self.task.publish_passive_scan_rx_memory(graph) } {
            Ok(graph) => graph,
            Err(mismatch) => {
                let error = mismatch.error();
                return Err(BluetoothPassiveScanSchedulerHeadPublicationFailure {
                    head_error: None,
                    rx_publication_error: Some(error),
                    owner:
                        BluetoothPassiveScanSchedulerHeadPublicationFailureOwner::RxPublication {
                            _mismatch: mismatch,
                            _reservation: reservation,
                            _head: head,
                        },
                });
            }
        };
        let graph = unsafe { self.task.publish_passive_scan_command(graph) };
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(BluetoothPassiveScanSchedulerHeadPublished {
            graph,
            publication,
            reservation,
        })
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the powered task owner and exclusive list identity jointly authorize the typed PAC publication"
    )]
    fn publish_first_scheduler_item_head(
        &mut self,
        address: BluetoothControllerSramAddress,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> Result<BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHeadPublicationError>
    {
        let head = self.validate_first_scheduler_item_head(address)?;
        Ok(self.publish_validated_first_scheduler_item_head(address, index, head))
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn validate_first_scheduler_item_head(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> Result<BluetoothSchedulerHardwareListHead, BluetoothSchedulerHeadPublicationError> {
        if !self._scheduler_list.can_publish_first_item(address) {
            return Err(BluetoothSchedulerHeadPublicationError::SchedulerIdentityMismatch);
        }
        Ok(BluetoothSchedulerHardwareListHead::from_address(address)?)
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "validation retained the exact source-owned list identity and typed hardware-head encoding"
    )]
    pub(crate) fn publish_validated_first_scheduler_item_head(
        &mut self,
        address: BluetoothControllerSramAddress,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        let publication = unsafe { self.task.publish_scheduler_hardware_list_head(index, head) };
        self._scheduler_list.retain_published_first_item(address);
        publication
    }

    /// Release the advertising memory, timeline and source-list owners together.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::legacy::completion::BluetoothLegacyAdvertisingCompletionRole<
                'a,
            >,
        >,
    ) -> BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
        let (item, removal, reservation) = ready.into_parts();
        let ready = BluetoothLegacyAdvertisingSchedulerRecycleReady {
            item,
            removal,
            reservation,
        };
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive {
                _ready: ready,
            };
        }
        let BluetoothLegacyAdvertisingSchedulerRecycleReady {
            item,
            removal,
            reservation,
        } = ready;
        let prepared = match item.prepare_recycle(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (item, removal) = failure.into_parts();
                return BluetoothLegacyAdvertisingSchedulerRecycleStep::MemoryIdentityMismatch {
                    _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady {
                        item,
                        removal,
                        reservation,
                    },
                    _error: error,
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
                let (item, removal) = prepared.into_parts();
                return BluetoothLegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch {
                    _ready: BluetoothLegacyAdvertisingSchedulerRecycleReady {
                        item,
                        removal,
                        reservation,
                    },
                };
            }
        };
        let item = prepared.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        BluetoothLegacyAdvertisingSchedulerRecycleStep::Recycled(
            BluetoothLegacyAdvertisingSchedulerRecycled { item },
        )
    }

    /// Extract RX packets and release the scanner memory and common-list owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_passive_scan_completed(
        &mut self,
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::le::scanning::passive::active::BluetoothPassiveScanCompletionRole,
        >,
    ) -> BluetoothPassiveScanSchedulerRecycleStep {
        let (graph, removal, reservation) = ready.into_parts();
        let ready = BluetoothPassiveScanSchedulerRecycleReady {
            graph,
            removal,
            reservation,
        };
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothPassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerRecycleStep::FinishedListDrainStillActive {
                _ready: ready,
            };
        }
        let BluetoothPassiveScanSchedulerRecycleReady {
            graph,
            removal,
            reservation,
        } = ready;
        let prepared = match graph.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_parts();
                return BluetoothPassiveScanSchedulerRecycleStep::MemoryIdentityMismatch {
                    _ready: BluetoothPassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                    _error: error,
                };
            }
        };
        let extracted = match prepared.extract_received() {
            Ok(extracted) => extracted,
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_prepared().into_parts();
                return BluetoothPassiveScanSchedulerRecycleStep::ReceiveInvalid {
                    _ready: BluetoothPassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                    _error: error,
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
                let (graph, removal) = extracted.into_prepared().into_parts();
                return BluetoothPassiveScanSchedulerRecycleStep::ReservationIdentityMismatch {
                    _ready: BluetoothPassiveScanSchedulerRecycleReady {
                        graph,
                        removal,
                        reservation,
                    },
                };
            }
        };
        let graph = extracted.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        BluetoothPassiveScanSchedulerRecycleStep::Recycled(BluetoothPassiveScanSchedulerRecycled {
            graph,
        })
    }

    /// Reclaim one response-capable advertising graph and classify its copied RX batch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_legacy_connectable_advertising_completed(
        &mut self,
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::connectable::completion::BluetoothLegacyConnectableAdvertisingCompletionRole,
        >,
    ) -> crate::le::advertising::connectable::completion::BluetoothLegacyConnectableAdvertisingRecycleStep
    {
        use crate::le::advertising::connectable::completion::{
            BluetoothLegacyConnectableAdvertisingRecycleReady,
            BluetoothLegacyConnectableAdvertisingRecycleStep,
        };

        let (item, removal, reservation) = ready.into_parts();
        let ready =
            BluetoothLegacyConnectableAdvertisingRecycleReady::new(item, removal, reservation);
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothLegacyConnectableAdvertisingRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyConnectableAdvertisingRecycleStep::FinishedListDrainStillActive {
                _ready: ready,
            };
        }

        let (item, removal, reservation) = ready.into_parts();
        let (memory, remainder) = item.into_parts();
        let prepared = match memory.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (memory, removal) = failure.into_parts();
                return BluetoothLegacyConnectableAdvertisingRecycleStep::MemoryIdentityMismatch {
                    _ready: BluetoothLegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingCompletionObserved::new(
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
                return BluetoothLegacyConnectableAdvertisingRecycleStep::ReceiveInvalid {
                    _ready: BluetoothLegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingCompletionObserved::new(
                            memory, remainder,
                        ),
                        removal,
                        reservation,
                    ),
                    _error: error,
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
                let (memory, removal) = extracted.into_prepared().into_parts();
                return BluetoothLegacyConnectableAdvertisingRecycleStep::ReservationIdentityMismatch {
                    _ready: BluetoothLegacyConnectableAdvertisingRecycleReady::new(
                        crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingCompletionObserved::new(
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
        self._scheduler_list.commit_recycled_first_item();
        BluetoothLegacyConnectableAdvertisingRecycleStep::Classified(
            remainder.classify_recycled(recycled),
        )
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Borrow the matching interrupt and task runtime endpoints from this
    /// initialized hardware epoch.
    ///
    /// This is the production entry into an executor adapter. The retained
    /// task, interrupt and platform owners cannot move or be rebound while
    /// either endpoint is alive.
    pub fn split_runtime(
        &mut self,
    ) -> (
        BluetoothControllerInterruptRuntime<'_>,
        BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>,
        BluetoothControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
    ) {
        let task = &mut self.task;
        let time_scale = self.time_scale;
        let standalone_dtm_profile = &self._standalone_dtm_profile;
        let config = self.config;
        let scheduler_list = &mut self._scheduler_list;
        let (interrupt, software, modem_timer) = self.runtime.split();
        (
            interrupt,
            BluetoothControllerPoweredTaskRuntime::new(
                software,
                task,
                time_scale,
                standalone_dtm_profile,
                config,
                scheduler_list,
            ),
            modem_timer,
        )
    }
}

impl<P> BluetoothControllerHalInitialized<P> {
    /// Initialize scheduler hardware and bind one static no-RTOS runtime.
    ///
    /// This consumes the completed controller HAL state before the first
    /// scheduler-table write. The supplied runtime must be pristine and is
    /// consumed into the same powered ownership epoch; it replaces the vendor
    /// event, broker-node and task containers instead of emulating their ABI.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize_scheduler<
        const MODEM_TIMER_CAPACITY: usize,
        const SCHEDULER_CAPACITY: usize,
    >(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |task| task.clear_scheduler_hardware_list_heads())
    }

    #[cfg(test)]
    pub(crate) fn initialize_scheduler_for_validation<
        const MODEM_TIMER_CAPACITY: usize,
        const SCHEDULER_CAPACITY: usize,
    >(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |_| {
            BluetoothSchedulerHardwareListsCleared::for_validation()
        })
    }

    fn initialize_scheduler_with<
        const MODEM_TIMER_CAPACITY: usize,
        const SCHEDULER_CAPACITY: usize,
    >(
        self,
        runtime: BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        initialize_hardware: impl FnOnce(
            &mut BluetoothTaskResources,
        ) -> BluetoothSchedulerHardwareListsCleared,
    ) -> BluetoothSchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        assert!(
            runtime.is_pristine(),
            "only a pristine Controller runtime can initialize a scheduler epoch"
        );
        let Self {
            mut task,
            interrupts,
            platform,
            time_scale,
            standalone_dtm_profile,
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        BluetoothSchedulerInitialized {
            task,
            _interrupts: Some(interrupts),
            _platform: platform,
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config: BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            _scheduler_list: BluetoothSchedulerExclusiveListEpoch::new(hardware_lists_cleared),
            runtime,
        }
    }
}

#[cfg(test)]
mod tests;
