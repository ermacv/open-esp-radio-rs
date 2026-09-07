//! Fact-bounded scheduler initialization after the controller HAL component.

#[cfg(any(target_arch = "riscv32", test))]
mod connectable_advertising;
#[cfg(target_arch = "riscv32")]
pub(crate) use connectable_advertising::{
    LegacyConnectableAdvertisingAdmissionObservation,
    LegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    LegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    LegacyConnectableAdvertisingEventPreparationError,
    LegacyConnectableAdvertisingEventPreparationFailure, LegacyConnectableAdvertisingEventPrepared,
    LegacyConnectableAdvertisingPreSequence, LegacyConnectableAdvertisingSequenceObservation,
};
mod dtm;
#[cfg(target_arch = "riscv32")]
mod single_item;

pub use dtm::{
    DtmControllerEventPreparationError, DtmEmptySchedulerMergePrepared,
    DtmInitialSchedulerItemPhase, DtmRecurringSchedulerItemPhase,
    DtmSchedulerHeadPublicationFailure, DtmSchedulerHeadPublished, DtmSchedulerRunning,
};
#[cfg(target_arch = "riscv32")]
pub use dtm::{
    DtmControllerRxPreparationFailure, DtmControllerRxRecurringPreparationFailure,
    DtmControllerTxPreparationFailure, DtmControllerTxRecurringPreparationFailure,
    DtmSchedulerCompletionObserved, DtmSchedulerCompletionObservedDrainStep,
    DtmSchedulerCompletionStep, DtmSchedulerHardwareHeadEmptyObserved,
    DtmSchedulerHardwareHeadRetirementStep, DtmSchedulerRecycleStep, DtmSchedulerRunningDrainStep,
    DtmSchedulerRxSuccessRecycleStep, DtmSchedulerSoftwareListRemovalReady,
    DtmSchedulerSoftwareListUnlinkStep, DtmSchedulerSoftwareListUnlinked,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use dtm::{
    DtmFirstPreparationCompletionClass, DtmReceiverFirstPreSequence, DtmReceiverFirstStaged,
    DtmReceiverRecurringPreSequence, DtmSchedulerSoftwareListRemovalJoin,
    DtmSchedulerSoftwareListRemovalRecheck, DtmTransmitterFirstPreSequence,
    DtmTransmitterFirstStaged, DtmTransmitterRecurringPreSequence,
    classify_dtm_first_preparation_completion,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use single_item::*;

#[cfg(any(target_arch = "riscv32", test))]
mod peripheral_connection;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use peripheral_connection::{
    PeripheralConnectionAdmissionObservation, PeripheralConnectionSequenceObservation,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use peripheral_connection::{
    PeripheralConnectionEmptySchedulerMergePrepared, PeripheralConnectionFirstEventPreparationError,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::{
    PeripheralConnectionFirstPreSequence, PeripheralConnectionSchedulerCompletionClassification,
};
#[cfg(target_arch = "riscv32")]
pub use peripheral_connection::{
    PeripheralConnectionRecurringCandidateError,
    PeripheralConnectionRecurringEmptySchedulerMergeFailure,
    PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    PeripheralConnectionRecurringEventCandidate,
    PeripheralConnectionRecurringEventPreparationError,
    PeripheralConnectionRecurringEventPreparationFailure,
    PeripheralConnectionRecurringEventPrepared, PeripheralConnectionRecurringPreSequence,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::{
    PeripheralConnectionRecurringSchedulerPublicationFailStop,
    PeripheralConnectionRecurringSchedulerValidationFailure,
};
#[cfg(target_arch = "riscv32")]
pub use peripheral_connection::{
    PeripheralConnectionSchedulerCompleted, PeripheralConnectionSchedulerHeadPublicationFailure,
    PeripheralConnectionSchedulerHeadPublished, PeripheralConnectionSchedulerRecycled,
};

#[cfg(target_arch = "riscv32")]
use crate::le::advertising::legacy::{
    LegacyAdvertisingCompletionObservedEvent, LegacyAdvertisingRecurringEventCandidate,
};

use crate::scheduler::SchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
use crate::scheduler::timeline::SchedulerRecurringReserved;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler::timeline::{SchedulerInitialAdmissionResolved, SchedulerWindowReservation};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    ControllerTimeSample,
    le::advertising::LegacyAdvertisingFirstEventCandidate,
    scheduler::{
        SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerSequenceReady,
        SchedulerTimingPolicy,
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    LeReceivedBatch, LeRxError, PassiveScanMemoryGraphCommandPublished,
    PassiveScanMemoryGraphCompletionObserved, PassiveScanMemoryGraphPublicationError,
    PassiveScanMemoryGraphPublicationMismatch, PassiveScanMemoryGraphRecycleError,
    PassiveScanMemoryGraphRecycled, PassiveScanSchedulerItemCompletionStatus,
};
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::{
    PassiveScanMemoryGraphCpuOwned, PassiveScanMemoryGraphEventPrepared,
    PassiveScanMemoryGraphSchedulerAdmissionPrepared, PassiveScanPrimaryChannel,
    PassiveScanSchedulerWindow, PassiveScanStartSelection,
};

use oer_esp32s31_pac::BluetoothControllerTimeScale;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use crate::{
    controller::hal::ControllerHalInitialized,
    resources::{InterruptBankOwner, TaskResources, TeardownPendingPlatform},
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
        ControllerRuntimeResources,
    },
};

use {
    oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadError,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListsCleared,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

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

/// Fresh initial-admission sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner admission observation must be consumed or retained"]
pub struct PassiveScanAdmissionObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// Fresh post-overlap sequence sample sealed by the controller-time worker.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the fresh scanner sequence observation must be consumed or retained"]
pub struct PassiveScanSequenceObservation {
    pub(crate) sample: ControllerTimeSample,
}

/// CPU-owned scanner graph with a requested window not yet admitted to the
/// common scheduler timeline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the scanner candidate must enter common scheduling or be returned"]
pub struct PassiveScanFirstEventCandidate {
    graph: PassiveScanMemoryGraphCpuOwned,
    channel: PassiveScanPrimaryChannel,
    requested_window: crate::scheduler::SchedulerRawWindow,
    controller_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanFirstEventCandidate {
    pub(crate) const fn new(
        graph: PassiveScanMemoryGraphCpuOwned,
        channel: PassiveScanPrimaryChannel,
        requested_window: crate::scheduler::SchedulerRawWindow,
        controller_time: BluetoothControllerLatchedTime,
    ) -> Self {
        Self {
            graph,
            channel,
            requested_window,
            controller_time,
        }
    }

    pub const fn requested_window(&self) -> crate::scheduler::SchedulerRawWindow {
        self.requested_window
    }

    pub fn cancel(self) -> PassiveScanMemoryGraphCpuOwned {
        self.graph
    }

    fn prepare_resolved_event(
        self,
        resolved_window: crate::scheduler::SchedulerRawWindow,
    ) -> PassiveScanMemoryGraphEventPrepared {
        let window = PassiveScanSchedulerWindow::from_controller_ticks(
            resolved_window.start(),
            resolved_window.end(),
        )
        .expect("a timeline reservation retains a non-empty forward window");
        let selection = if resolved_window.start() == self.requested_window.start() {
            PassiveScanStartSelection::Requested
        } else {
            PassiveScanStartSelection::EarliestAvailable
        };
        self.graph
            .prepare_first_event(self.channel, window, selection, self.controller_time)
    }
}

/// First scanner event after common timeline admission and before the second
/// sequence deadline.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the admitted scanner event must pass sequence authorization or be cancelled"]
pub struct PassiveScanFirstPreSequence {
    candidate: PassiveScanFirstEventCandidate,
    reservation: SchedulerWindowReservation<SchedulerInitialAdmissionResolved>,
}

/// Scanner event image paired with the exact timeline interval encoded into it.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the prepared scanner event must be merged, cancelled, or retained"]
pub struct PassiveScanEventPrepared {
    graph: PassiveScanMemoryGraphEventPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEventPrepared {
    pub const fn channel(&self) -> PassiveScanPrimaryChannel {
        self.graph.channel()
    }

    pub const fn window(&self) -> PassiveScanSchedulerWindow {
        self.graph.window()
    }
}

/// Finite scanner preparation rejection before any MMIO publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub enum PassiveScanFirstEventPreparationError {
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
}

/// Lossless first-scanner-event preparation rejection.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged scanner graph must be retried, cancelled, or retained"]
pub struct PassiveScanFirstEventPreparationFailure {
    candidate: PassiveScanFirstEventCandidate,
    error: PassiveScanFirstEventPreparationError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanFirstEventPreparationFailure {
    pub const fn error(&self) -> PassiveScanFirstEventPreparationError {
        self.error
    }

    pub fn into_candidate(self) -> PassiveScanFirstEventCandidate {
        self.candidate
    }
}

/// Lossless rejection while joining one detached scanner item to the empty list.
#[cfg(any(target_arch = "riscv32", test))]
#[must_use = "the unchanged detached scanner event remains CPU-owned"]
pub struct PassiveScanEmptySchedulerMergeFailure {
    error: SchedulerEmptyListMergeError,
    prepared: PassiveScanEventPrepared,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEmptySchedulerMergeFailure {
    /// Exact reason the exclusive scheduler epoch rejected the scanner item.
    pub const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    /// Recover the unchanged detached scanner graph.
    pub fn into_prepared(self) -> PassiveScanEventPrepared {
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
pub struct PassiveScanEmptySchedulerMergePrepared {
    graph: PassiveScanMemoryGraphSchedulerAdmissionPrepared,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl PassiveScanEmptySchedulerMergePrepared {
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
enum PassiveScanSchedulerHeadPublicationFailureOwner {
    PrePublication(PassiveScanEmptySchedulerMergePrepared),
    RxPublication {
        _mismatch: PassiveScanMemoryGraphPublicationMismatch,
        _reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
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
pub struct PassiveScanSchedulerHeadPublicationFailure {
    head_error: Option<SchedulerHeadPublicationError>,
    rx_publication_error: Option<PassiveScanMemoryGraphPublicationError>,
    owner: PassiveScanSchedulerHeadPublicationFailureOwner,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerHeadPublicationFailure {
    /// Exact reason the common scheduler head could not be prepared.
    pub const fn head_error(&self) -> Option<SchedulerHeadPublicationError> {
        self.head_error
    }

    /// Return the typed RX publication mismatch after the first MMIO write.
    pub const fn rx_publication_error(&self) -> Option<PassiveScanMemoryGraphPublicationError> {
        self.rx_publication_error
    }

    /// Recover the unchanged merge only for a pre-MMIO validation failure.
    pub fn into_retryable_merged(self) -> Result<PassiveScanEmptySchedulerMergePrepared, Self> {
        match self {
            Self {
                owner: PassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(merged),
                ..
            } => Ok(merged),
            failure @ Self {
                owner: PassiveScanSchedulerHeadPublicationFailureOwner::RxPublication { .. },
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
pub struct PassiveScanSchedulerHeadPublished {
    graph: PassiveScanMemoryGraphCommandPublished,
    publication: BluetoothSchedulerHardwareListHeadPublished,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerHeadPublished {
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
        PassiveScanMemoryGraphCommandPublished,
        BluetoothSchedulerHardwareListHeadPublished,
        SchedulerWindowReservation<SchedulerSequenceReady>,
    ) {
        (self.graph, self.publication, self.reservation)
    }
}

/// CPU-owned scanner graph and copied receive results.
#[cfg(target_arch = "riscv32")]
#[must_use = "return the graph and received packets to the scanner role owner"]
pub(crate) struct PassiveScanSchedulerRecycled {
    graph: PassiveScanMemoryGraphRecycled,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerRecycled {
    pub fn into_parts(
        self,
    ) -> (
        oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphCpuOwned,
        LeReceivedBatch,
        PassiveScanSchedulerItemCompletionStatus,
    ) {
        self.graph.into_parts()
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct PassiveScanSchedulerRecycleReady {
    graph: PassiveScanMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: SchedulerWindowReservation<SchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl PassiveScanSchedulerRecycleReady {
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
pub(crate) enum PassiveScanSchedulerRecycleStep {
    SchedulerIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    FinishedListDrainStillActive {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    MemoryIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
        _error: PassiveScanMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        _ready: PassiveScanSchedulerRecycleReady,
        _error: LeRxError,
    },
    ReservationIdentityMismatch {
        _ready: PassiveScanSchedulerRecycleReady,
    },
    Recycled(PassiveScanSchedulerRecycled),
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

/// Exclusive empty scheduler-list epoch owned by the source controller.
///
/// The PAC proof establishes that no hardware-list head remains published.
/// This owner adds the independently constructed source-owned software list,
/// which starts empty and cannot be aliased through a vendor container.
pub(crate) struct SchedulerExclusiveListEpoch {
    _hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared,
    state: SchedulerExclusiveListState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SchedulerExclusiveListState {
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

impl SchedulerExclusiveListEpoch {
    const fn new(hardware_lists_cleared: BluetoothSchedulerHardwareListsCleared) -> Self {
        Self {
            _hardware_lists_cleared: hardware_lists_cleared,
            state: SchedulerExclusiveListState::Empty,
        }
    }

    fn prepare_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> Result<(), SchedulerEmptyListMergeError> {
        if self.state != SchedulerExclusiveListState::Empty {
            return Err(SchedulerEmptyListMergeError::ListNotEmpty);
        }
        self.state = SchedulerExclusiveListState::FirstItemPrepared { address };
        Ok(())
    }

    fn cancel_first_item(&mut self, address: BluetoothControllerSramAddress) -> bool {
        if self.state != (SchedulerExclusiveListState::FirstItemPrepared { address }) {
            return false;
        }
        self.state = SchedulerExclusiveListState::Empty;
        true
    }

    fn can_publish_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        matches!(
            self.state,
            SchedulerExclusiveListState::FirstItemPrepared {
                address: prepared
            } if prepared == address
        )
    }

    fn retain_published_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert!(
            self.can_publish_first_item(address),
            "only the merge-selected first item can become the hardware head"
        );
        self.state = SchedulerExclusiveListState::FirstItemHeadPublished { address };
    }

    pub(crate) fn retain_running_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert_eq!(
            self.state,
            SchedulerExclusiveListState::FirstItemHeadPublished { address },
            "only the published first item can enter the running scheduler phase"
        );
        self.state = SchedulerExclusiveListState::FirstItemRunning { address };
    }

    fn retains_running_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemRunning { address }
    }

    fn retain_completion_observed_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_running_first_item(address) {
            return false;
        }
        self.state = SchedulerExclusiveListState::FirstItemCompletionObserved { address };
        true
    }

    fn retains_completion_observed_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemCompletionObserved { address }
    }

    fn retain_hardware_head_empty_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_completion_observed_first_item(address) {
            return false;
        }
        self.state = SchedulerExclusiveListState::FirstItemHardwareHeadEmptyObserved { address };
        true
    }

    fn retains_hardware_head_empty_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemHardwareHeadEmptyObserved { address }
    }

    fn unlink_software_list_first_item(&mut self, address: BluetoothControllerSramAddress) -> bool {
        if !self.retains_hardware_head_empty_first_item(address) {
            return false;
        }
        self.state =
            SchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
                address,
            };
        true
    }

    fn retains_unlinked_first_item(&self, address: BluetoothControllerSramAddress) -> bool {
        self.state
            == SchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
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
        self.state = SchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address };
        true
    }

    fn retains_software_list_removal_ready_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address }
    }

    fn commit_recycled_first_item(&mut self) {
        self.state = SchedulerExclusiveListState::Empty;
    }
}

/// Why a first scheduler item could not consume the exclusive empty-list epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerEmptyListMergeError {
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
pub enum ControllerTimeAcquisitionError {
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
pub enum SchedulerHeadPublicationError {
    /// The merge belongs to another scheduler epoch or list identity.
    SchedulerIdentityMismatch,
    /// The selected address aliases the reserved empty hardware-head image.
    EncodesEmptyHead,
}

impl From<BluetoothSchedulerHardwareListHeadError> for SchedulerHeadPublicationError {
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
pub struct SchedulerFinishedListDrainPending<Owner> {
    owner: Owner,
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> SchedulerFinishedListDrainPending<Owner> {
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
pub enum SchedulerFinishedListDrainState<Owner> {
    /// The captured set is exhausted; no continuation is permitted.
    Drained(Owner),
    /// The same captured set retains another list.
    Pending(SchedulerFinishedListDrainPending<Owner>),
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> SchedulerFinishedListDrainState<Owner> {
    fn from_worker_step(owner: Owner, more: bool) -> Self {
        if more {
            Self::Pending(SchedulerFinishedListDrainPending::new(owner))
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
pub struct SchedulerInitialized<
    P,
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize,
> {
    task: TaskResources,
    _interrupts: Option<InterruptBankOwner>,
    _platform: TeardownPendingPlatform<P>,
    time_scale: BluetoothControllerTimeScale,
    _standalone_dtm_profile: crate::controller::hal::StandaloneAlwaysAwakeDtmProfile,
    config: SchedulerSoftwareConfig,
    _scheduler_list: SchedulerExclusiveListEpoch,
    runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    pub(crate) fn task_mut(&mut self) -> &mut TaskResources {
        &mut self.task
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_phase(
        &self,
    ) -> crate::controller::time::ControllerTimeWorkerPhase {
        self.task.controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.task.controller_time_needs_recheck()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> InterruptBankOwner {
        self._interrupts
            .take()
            .expect("private Controller invariant retains the interrupt owner until activation")
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(&mut self) -> (&mut TaskResources, &mut P) {
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
    pub const fn scheduler_config(&self) -> SchedulerSoftwareConfig {
        self.config
    }

    /// Whether no software event has entered the initialized epoch.
    pub fn runtime_is_pristine(&self) -> bool {
        self.runtime.is_pristine()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller::time::ControllerTimeRequest,
        crate::controller::time::ControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller::time::ControllerTimeRequest,
    ) -> Result<(), crate::controller::time::ControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller::time::ControllerTimeRequest,
    ) -> Result<
        crate::controller::time::ControllerTimeEventStep,
        crate::controller::time::ControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller::time::ControllerTimeEventStep,
        crate::controller::time::ControllerTimeEventError,
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
        candidate: LegacyAdvertisingFirstEventCandidate<'a>,
        admission: LegacyAdvertisingAdmissionObservation,
    ) -> Result<
        LegacyAdvertisingFirstPreSequence<'a>,
        LegacyAdvertisingFirstEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
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

    /// Authorize the second deadline and encode the overlap-resolved event image.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn prepare_legacy_advertising_first_event<'a>(
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

    /// Reserve one exact recurring advertising window without displacement.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    pub fn admit_legacy_advertising_recurring_event<'a>(
        &mut self,
        candidate: LegacyAdvertisingRecurringEventCandidate<'a>,
    ) -> Result<
        LegacyAdvertisingRecurringPreSequence<'a>,
        LegacyAdvertisingRecurringEventPreparationFailure<'a>,
    > {
        let raw_window = candidate.raw_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(raw_window.start(), raw_window.end(), timing_policy)
        {
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

    /// Authorize the recurring deadline and encode its complete event chain.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the complete recurring event"
    )]
    pub fn prepare_legacy_advertising_recurring_event<'a>(
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

    /// Release an unpublished first advertising event and restore both owners.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_legacy_advertising_first_event<'a>(
        &mut self,
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'a> {
        let LegacyAdvertisingEventPrepared { image, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        image.cancel()
    }

    /// Release an admitted first event before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_legacy_advertising_first_pre_sequence<'a>(
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

    /// Release an admitted recurring event before its sequence sample arrives.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_legacy_advertising_recurring_pre_sequence<'a>(
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

    /// Admit one requested passive-scanner window into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn admit_passive_scan_first_event(
        &mut self,
        candidate: PassiveScanFirstEventCandidate,
        admission: PassiveScanAdmissionObservation,
    ) -> Result<PassiveScanFirstPreSequence, PassiveScanFirstEventPreparationFailure> {
        let requested = candidate.requested_window();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        match self
            .runtime
            .scheduler_timeline_mut()
            .reserve_phase_locked_initial_window(
                requested.start(),
                requested.end(),
                timing_policy,
                admission.sample,
            ) {
            Ok(reservation) => Ok(PassiveScanFirstPreSequence {
                candidate,
                reservation,
            }),
            Err(error) => Err(PassiveScanFirstEventPreparationFailure {
                candidate,
                error: PassiveScanFirstEventPreparationError::Timeline(error),
            }),
        }
    }

    /// Authorize the second deadline and only then encode the
    /// overlap-resolved scanner window into private SRAM.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn prepare_passive_scan_first_event(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
        sequence: PassiveScanSequenceObservation,
    ) -> Result<PassiveScanEventPrepared, PassiveScanFirstEventPreparationFailure> {
        let PassiveScanFirstPreSequence {
            candidate,
            reservation,
        } = admitted;
        let reservation = match reservation.authorize_sequence(sequence.sample) {
            Ok(reservation) => reservation,
            Err(failure) => {
                let error = failure.error();
                self.release_scheduler_reservation(failure.into_reservation());
                return Err(PassiveScanFirstEventPreparationFailure {
                    candidate,
                    error: PassiveScanFirstEventPreparationError::Sequence(error),
                });
            }
        };
        let graph = candidate.prepare_resolved_event(reservation.window());
        Ok(PassiveScanEventPrepared { graph, reservation })
    }

    /// Release one unpublished scanner event and its exact timeline slot.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_passive_scan_first_event(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> PassiveScanMemoryGraphCpuOwned {
        let PassiveScanEventPrepared { graph, reservation } = prepared;
        self.release_scheduler_reservation(reservation);
        graph.into_cpu_owned()
    }

    /// Release an admitted scanner candidate before its sequence sample arrives.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn cancel_passive_scan_first_pre_sequence(
        &mut self,
        admitted: PassiveScanFirstPreSequence,
    ) -> PassiveScanMemoryGraphCpuOwned {
        let PassiveScanFirstPreSequence {
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
        prepared: LegacyAdvertisingEventPrepared<'a>,
    ) -> Result<
        LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
        LegacyAdvertisingEmptySchedulerMergeFailure<'a>,
    > {
        let LegacyAdvertisingEventPrepared { image, reservation } = prepared;
        let item = image.prepare_scheduler_bookkeeping();
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
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

    /// Cancel a not-yet-published advertising merge through the same list epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    #[expect(
        clippy::result_large_err,
        reason = "an identity rejection retains the complete advertising merge"
    )]
    pub fn cancel_legacy_advertising_empty_list_merge<'a>(
        &mut self,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'a>,
    ) -> Result<LegacyAdvertisingEventPrepared<'a>, LegacyAdvertisingEmptySchedulerMergePrepared<'a>>
    {
        if !self
            ._scheduler_list
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

    /// Join the detached first scanner item to this epoch's empty scheduler list.
    ///
    /// The private scanner graph has already removed the item from its free
    /// chain. This transition atomically reserves the same address in the
    /// source-owned common list without publishing MMIO.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn prepare_passive_scan_empty_list_merge(
        &mut self,
        prepared: PassiveScanEventPrepared,
    ) -> Result<PassiveScanEmptySchedulerMergePrepared, PassiveScanEmptySchedulerMergeFailure> {
        let PassiveScanEventPrepared { graph, reservation } = prepared;
        let graph = graph.prepare_scheduler_admission();
        let address = graph.scheduler_head();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(PassiveScanEmptySchedulerMergeFailure {
                error,
                prepared: PassiveScanEventPrepared {
                    graph: graph.cancel(),
                    reservation,
                },
            });
        }
        Ok(PassiveScanEmptySchedulerMergePrepared { graph, reservation })
    }

    /// Restore an unpublished scanner merge through the same scheduler epoch.
    ///
    /// Success restores both the common empty-list proof and the selected
    /// scanner item's position in the private three-item free chain.
    #[cfg(any(target_arch = "riscv32", test))]
    pub fn cancel_passive_scan_empty_list_merge(
        &mut self,
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanEventPrepared, PassiveScanEmptySchedulerMergePrepared> {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        let PassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        Ok(PassiveScanEventPrepared {
            graph: graph.cancel(),
            reservation,
        })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn release_scheduler_reservation<State>(
        &mut self,
        reservation: SchedulerWindowReservation<State>,
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
        merged: PassiveScanEmptySchedulerMergePrepared,
    ) -> Result<PassiveScanSchedulerHeadPublished, PassiveScanSchedulerHeadPublicationFailure> {
        let address = merged.scheduler_item_address();
        let index = merged.hardware_list_index();
        let head = match self.validate_first_scheduler_item_head(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(PassiveScanSchedulerHeadPublicationFailure {
                    head_error: Some(error),
                    rx_publication_error: None,
                    owner: PassiveScanSchedulerHeadPublicationFailureOwner::PrePublication(merged),
                });
            }
        };
        let PassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        let graph = graph.prepare_publication();
        let graph = match unsafe { self.task.publish_passive_scan_rx_memory(graph) } {
            Ok(graph) => graph,
            Err(mismatch) => {
                let error = mismatch.error();
                return Err(PassiveScanSchedulerHeadPublicationFailure {
                    head_error: None,
                    rx_publication_error: Some(error),
                    owner: PassiveScanSchedulerHeadPublicationFailureOwner::RxPublication {
                        _mismatch: mismatch,
                        _reservation: reservation,
                        _head: head,
                    },
                });
            }
        };
        let graph = unsafe { self.task.publish_passive_scan_command(graph) };
        let publication = self.publish_validated_first_scheduler_item_head(address, index, head);
        Ok(PassiveScanSchedulerHeadPublished {
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
    ) -> Result<BluetoothSchedulerHardwareListHeadPublished, SchedulerHeadPublicationError> {
        let head = self.validate_first_scheduler_item_head(address)?;
        Ok(self.publish_validated_first_scheduler_item_head(address, index, head))
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn validate_first_scheduler_item_head(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> Result<BluetoothSchedulerHardwareListHead, SchedulerHeadPublicationError> {
        if !self._scheduler_list.can_publish_first_item(address) {
            return Err(SchedulerHeadPublicationError::SchedulerIdentityMismatch);
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
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return LegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
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
        let release = match self
            .runtime
            .scheduler_timeline_mut()
            .prepare_release(reservation)
        {
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
        self._scheduler_list.commit_recycled_first_item();
        LegacyAdvertisingSchedulerRecycleStep::Recycled(LegacyAdvertisingSchedulerRecycled { item })
    }

    /// Extract RX packets and release the scanner memory and common-list owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_passive_scan_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
    ) -> PassiveScanSchedulerRecycleStep {
        let (graph, removal, reservation) = ready.into_parts();
        let ready = PassiveScanSchedulerRecycleReady {
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
            return PassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch { _ready: ready };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return PassiveScanSchedulerRecycleStep::FinishedListDrainStillActive { _ready: ready };
        }
        let PassiveScanSchedulerRecycleReady {
            graph,
            removal,
            reservation,
        } = ready;
        let prepared = match graph.prepare_recycle_after_software_list_removal(removal) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (graph, removal) = failure.into_parts();
                return PassiveScanSchedulerRecycleStep::MemoryIdentityMismatch {
                    _ready: PassiveScanSchedulerRecycleReady {
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
                return PassiveScanSchedulerRecycleStep::ReceiveInvalid {
                    _ready: PassiveScanSchedulerRecycleReady {
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
                return PassiveScanSchedulerRecycleStep::ReservationIdentityMismatch {
                    _ready: PassiveScanSchedulerRecycleReady {
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
        PassiveScanSchedulerRecycleStep::Recycled(PassiveScanSchedulerRecycled { graph })
    }

    /// Reclaim one response-capable advertising graph and classify its copied RX batch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_legacy_connectable_advertising_completed(
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
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return LegacyConnectableAdvertisingRecycleStep::SchedulerIdentityMismatch {
                _ready: ready,
            };
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return LegacyConnectableAdvertisingRecycleStep::FinishedListDrainStillActive {
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
        let release = match self
            .runtime
            .scheduler_timeline_mut()
            .prepare_release(reservation)
        {
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
        self._scheduler_list.commit_recycled_first_item();
        LegacyConnectableAdvertisingRecycleStep::Classified(remainder.classify_recycled(recycled))
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
        ControllerInterruptRuntime<'_>,
        ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>,
        ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
    ) {
        let task = &mut self.task;
        let time_scale = self.time_scale;
        let standalone_dtm_profile = &self._standalone_dtm_profile;
        let config = self.config;
        let scheduler_list = &mut self._scheduler_list;
        let (interrupt, software, modem_timer) = self.runtime.split();
        (
            interrupt,
            ControllerPoweredTaskRuntime::new(
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

impl<P> ControllerHalInitialized<P> {
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
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |task| task.clear_scheduler_hardware_list_heads())
    }

    #[cfg(test)]
    pub(crate) fn initialize_scheduler_for_validation<
        const MODEM_TIMER_CAPACITY: usize,
        const SCHEDULER_CAPACITY: usize,
    >(
        self,
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
        self.initialize_scheduler_with(runtime, |_| {
            BluetoothSchedulerHardwareListsCleared::for_validation()
        })
    }

    fn initialize_scheduler_with<
        const MODEM_TIMER_CAPACITY: usize,
        const SCHEDULER_CAPACITY: usize,
    >(
        self,
        runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
        initialize_hardware: impl FnOnce(&mut TaskResources) -> BluetoothSchedulerHardwareListsCleared,
    ) -> SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY> {
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
        SchedulerInitialized {
            task,
            _interrupts: Some(interrupts),
            _platform: platform,
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config: SchedulerSoftwareConfig::reviewed_standalone(),
            _scheduler_list: SchedulerExclusiveListEpoch::new(hardware_lists_cleared),
            runtime,
        }
    }
}

#[cfg(test)]
mod tests;
