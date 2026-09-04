//! Fact-bounded scheduler initialization after the controller HAL component.

mod dtm;
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

#[cfg(any(target_arch = "riscv32", test))]
mod peripheral_connection;
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::BluetoothPeripheralConnectionFirstPreSequence;
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
    BluetoothPeripheralConnectionSchedulerCompleted,
    BluetoothPeripheralConnectionSchedulerCompletionObserved,
    BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep,
    BluetoothPeripheralConnectionSchedulerCompletionStep,
    BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep,
    BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
    BluetoothPeripheralConnectionSchedulerHeadPublished,
    BluetoothPeripheralConnectionSchedulerRecycleStep,
    BluetoothPeripheralConnectionSchedulerRecycled, BluetoothPeripheralConnectionSchedulerRunning,
    BluetoothPeripheralConnectionSchedulerRunningDrainStep,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep,
    BluetoothPeripheralConnectionSchedulerSoftwareListUnlinked,
};
#[cfg(target_arch = "riscv32")]
pub(crate) use peripheral_connection::{
    BluetoothPeripheralConnectionSchedulerCompletionClassification,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck,
};

use crate::BluetoothSchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
use crate::BluetoothSchedulerWakeBatch;
#[cfg(target_arch = "riscv32")]
use crate::legacy_advertising::{
    BluetoothLegacyAdvertisingCompletionObservedEvent,
    BluetoothLegacyAdvertisingRecurringEventCandidate,
    BluetoothLegacyAdvertisingRunningEventCompletionObservation,
};
#[cfg(target_arch = "riscv32")]
use crate::peripheral_connection::{
    BluetoothPeripheralConnectionCompletedEvent,
    BluetoothPeripheralConnectionCompletionClassification,
    BluetoothPeripheralConnectionFirstEventCompletionObservation,
    BluetoothPeripheralConnectionFirstEventCompletionObserved,
    BluetoothPeripheralConnectionFirstEventRunning,
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
use crate::scheduler_timeline::BluetoothSchedulerRecurringReserved;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler_timeline::{
    BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerWindowReservation,
};
use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerModemTimerRuntime,
    BluetoothControllerPoweredTaskRuntime, BluetoothControllerRuntimeResources,
    controller_hal::BluetoothControllerHalInitialized,
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
    BluetoothPassiveScanMemoryGraphCompletionObservation,
    BluetoothPassiveScanMemoryGraphCompletionObserved, BluetoothPassiveScanMemoryGraphRecycleError,
    BluetoothPassiveScanMemoryGraphRecycled, BluetoothPassiveScanMemoryGraphRunning,
    BluetoothPassiveScanSchedulerItemCompletionStatus,
    BluetoothPeripheralConnectionMemoryGraphRecycleError,
    BluetoothPeripheralConnectionSchedulerItemCompletionStatus,
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
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothSchedulerHardwareListHeadPublished,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerSoftwareListRemovalJoin,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

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
    image: crate::legacy_advertising::BluetoothLegacyAdvertisingEventImagePrepared<'a>,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(any(target_arch = "riscv32", test))]
impl BluetoothLegacyAdvertisingEventPrepared<'_> {
    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertisingEventIdentity {
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
    item: crate::legacy_advertising::BluetoothLegacyAdvertisingEmptyListLinkPrepared<'a>,
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

/// Lossless rejection before any scanner or scheduler MMIO publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the unchanged CPU-owned scanner merge can be retried or cancelled"]
pub struct BluetoothPassiveScanSchedulerHeadPublicationFailure {
    error: BluetoothSchedulerHeadPublicationError,
    merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerHeadPublicationFailure {
    /// Exact reason the common scheduler head could not be prepared.
    pub const fn error(&self) -> BluetoothSchedulerHeadPublicationError {
        self.error
    }

    /// Recover the unchanged scanner merge. No MMIO was performed.
    pub fn into_merged(self) -> BluetoothPassiveScanEmptySchedulerMergePrepared {
        self.merged
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

/// Passive scanner graph admitted through the complete common RUN suffix.
///
/// The graph remains hardware-owned. Completion, RX extraction and recycling
/// require later typed transitions and are deliberately not exposed here.
#[cfg(target_arch = "riscv32")]
#[must_use = "the running scanner graph must advance through owned completion"]
pub struct BluetoothPassiveScanSchedulerRunning {
    graph: BluetoothPassiveScanMemoryGraphRunning,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerRunning {
    pub(crate) fn new(
        graph: BluetoothPassiveScanMemoryGraphCommandPublished,
        run: BluetoothSchedulerHardwareRunCommandPublished,
        reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) -> Self {
        let graph = graph.into_running(&run);
        Self {
            graph,
            run,
            reservation,
        }
    }

    /// Exact scanner item retained while hardware owns the graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    /// Hardware list admitted by the complete run transaction.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

/// Scanner graph after one non-sentinel scheduler completion observation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed scanner graph must advance through unlink and recycle"]
pub struct BluetoothPassiveScanSchedulerCompletionObserved {
    graph: BluetoothPassiveScanMemoryGraphCompletionObserved,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerCompletionObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }

    pub const fn status(&self) -> BluetoothPassiveScanSchedulerItemCompletionStatus {
        self.graph.status()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the empty scanner head must advance through software unlink"]
pub struct BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved {
    graph: BluetoothPassiveScanMemoryGraphCompletionObserved,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the unlinked scanner graph must pass the removal gate"]
pub struct BluetoothPassiveScanSchedulerSoftwareListUnlinked {
    graph: BluetoothPassiveScanMemoryGraphCompletionObserved,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerSoftwareListUnlinked {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the removal-ready scanner graph must be extracted and recycled"]
pub struct BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
    graph: BluetoothPassiveScanMemoryGraphCompletionObserved,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.graph.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

/// CPU-owned scanner graph and copied receive results.
#[cfg(target_arch = "riscv32")]
#[must_use = "return the graph and received packets to the scanner role owner"]
pub struct BluetoothPassiveScanSchedulerRecycled {
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
#[must_use = "the scanner graph and every captured list remain owned"]
pub enum BluetoothPassiveScanSchedulerCompletionStep {
    DrainAlreadyActive(BluetoothPassiveScanSchedulerRunning),
    SchedulerIdentityMismatch(BluetoothPassiveScanSchedulerRunning),
    NoFinishedList(BluetoothPassiveScanSchedulerRunning),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerRunning>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerRunning>),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerCompletionObserved>,
    ),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner graph and captured drain remain owned"]
pub enum BluetoothPassiveScanSchedulerRunningDrainStep {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerRunning>,
    ),
    DrainLost(BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerRunning>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerRunning>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerRunning>),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<BluetoothPassiveScanSchedulerCompletionObserved>,
    ),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the completed scanner graph and captured drain remain owned"]
pub enum BluetoothPassiveScanSchedulerCompletionObservedDrainStep {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerCompletionObserved>,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerCompletionObserved>,
    ),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothPassiveScanSchedulerCompletionObserved,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RepeatedScannerList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothPassiveScanSchedulerCompletionObserved,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the completed scanner graph must retain its head retirement state"]
pub enum BluetoothPassiveScanSchedulerHardwareHeadRetirementStep {
    SchedulerIdentityMismatch(BluetoothPassiveScanSchedulerCompletionObserved),
    FinishedListDrainStillActive(BluetoothPassiveScanSchedulerCompletionObserved),
    ExpectedHeadStillPublished {
        completed: BluetoothPassiveScanSchedulerCompletionObserved,
        observed: BluetoothSchedulerHardwareListHead,
    },
    UnexpectedHeadChanged {
        completed: BluetoothPassiveScanSchedulerCompletionObserved,
        observed: BluetoothSchedulerHardwareListHead,
    },
    EmptyObserved(BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner unlink owner must be retained"]
pub enum BluetoothPassiveScanSchedulerSoftwareListUnlinkStep {
    SchedulerIdentityMismatch(BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved),
    Unlinked(BluetoothPassiveScanSchedulerSoftwareListUnlinked),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner unlinked or removal-ready owner must be retained"]
pub enum BluetoothPassiveScanSchedulerSoftwareListRemovalJoin {
    SchedulerIdentityMismatch {
        unlinked: BluetoothPassiveScanSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    Pending(BluetoothPassiveScanSchedulerSoftwareListUnlinked),
    Ready(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the scanner unlinked or removal-ready owner must be retained"]
pub enum BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck {
    SchedulerIdentityMismatch(BluetoothPassiveScanSchedulerSoftwareListUnlinked),
    StorageUnavailable(BluetoothPassiveScanSchedulerSoftwareListUnlinked),
    Pending(BluetoothPassiveScanSchedulerSoftwareListUnlinked),
    Ready(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the scanner graph; success returns CPU ownership"]
pub enum BluetoothPassiveScanSchedulerRecycleStep {
    SchedulerIdentityMismatch(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
    FinishedListDrainStillActive(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
    MemoryIdentityMismatch {
        ready: BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
        error: BluetoothPassiveScanMemoryGraphRecycleError,
    },
    ReceiveInvalid {
        ready: BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
        error: BluetoothLeRxError,
    },
    ReservationIdentityMismatch(BluetoothPassiveScanSchedulerSoftwareListRemovalReady),
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
    item: crate::legacy_advertising::BluetoothLegacyAdvertisingHeadPublishedEvent<'a>,
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
        crate::legacy_advertising::BluetoothLegacyAdvertisingHeadPublishedEvent<'a>,
        BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) {
        (self.item, self.publication, self._reservation)
    }
}

/// Advertising graph admitted through head, interrupt, event and RUN publication.
#[cfg(target_arch = "riscv32")]
#[must_use = "the running advertising graph must advance through owned completion"]
pub struct BluetoothLegacyAdvertisingSchedulerRunning<'a> {
    item: crate::legacy_advertising::BluetoothLegacyAdvertisingRunningEvent<'a>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    _reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

/// Advertising graph with one non-sentinel scheduler completion observation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the completed advertising graph must advance through unlink and recycle"]
pub struct BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a> {
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
    _reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingSchedulerCompletionObserved<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }

    pub const fn statuses(
        &self,
    ) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingEventCompletionStatuses
    {
        self.item.statuses()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the empty-head advertising graph must advance through removal"]
pub struct BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a> {
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the unlinked advertising graph must pass the removal gate"]
pub struct BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a> {
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the removal-ready advertising graph must be recycled"]
pub struct BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a> {
    item: BluetoothLegacyAdvertisingCompletionObservedEvent<'a>,
    removal: BluetoothSchedulerSoftwareListRemovalReady,
    reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'_> {
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.removal.index()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the completed event must advance the LL owner exactly once"]
pub struct BluetoothLegacyAdvertisingSchedulerRecycled<'a> {
    item: crate::legacy_advertising::BluetoothLegacyAdvertisingRecycledEvent<'a>,
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingSchedulerRecycled<'a> {
    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertisingEventIdentity {
        self.item.identity()
    }

    pub const fn statuses(
        &self,
    ) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingEventCompletionStatuses
    {
        self.item.statuses()
    }

    /// Advance the exact LL event while retaining S31 diagnostic statuses.
    pub fn complete_event(
        self,
    ) -> crate::legacy_advertising::BluetoothLegacyAdvertisingEventCompleted<'a> {
        self.item.complete_event()
    }
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the advertising completion owner must be retained"]
pub enum BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep<'a> {
    SchedulerIdentityMismatch(BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>),
    FinishedListDrainStillActive(BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>),
    ExpectedHeadStillPublished {
        completed: BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        observed: BluetoothSchedulerHardwareListHead,
    },
    UnexpectedHeadChanged {
        completed: BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        observed: BluetoothSchedulerHardwareListHead,
    },
    EmptyObserved(BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the advertising unlink owner must be retained"]
pub enum BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep<'a> {
    SchedulerIdentityMismatch(BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>),
    Unlinked(BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the unlinked or ready advertising owner must be retained"]
pub enum BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin<'a> {
    SchedulerIdentityMismatch {
        unlinked: BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    Pending(BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
    Ready(BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "the unlinked or ready advertising owner must be retained"]
pub enum BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck<'a> {
    SchedulerIdentityMismatch(BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
    StorageUnavailable(BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
    Pending(BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>),
    Ready(BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>),
}

#[cfg(target_arch = "riscv32")]
#[must_use = "failure retains the advertising graph; success retains CPU ownership"]
pub enum BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
    SchedulerIdentityMismatch(BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>),
    FinishedListDrainStillActive(
        BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
    ),
    MemoryIdentityMismatch {
        ready: BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
        error: open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyAdvertisingMemoryGraphRecycleError,
    },
    ReservationIdentityMismatch(
        BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
    ),
    Recycled(BluetoothLegacyAdvertisingSchedulerRecycled<'a>),
}

#[cfg(target_arch = "riscv32")]
impl<'a> BluetoothLegacyAdvertisingSchedulerRunning<'a> {
    pub(crate) fn new(
        item: crate::legacy_advertising::BluetoothLegacyAdvertisingHeadPublishedEvent<'a>,
        run: BluetoothSchedulerHardwareRunCommandPublished,
        reservation: BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    ) -> Self {
        let item = item.into_running(&run);
        Self {
            item,
            run,
            _reservation: reservation,
        }
    }

    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
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

    fn retain_completion_observed_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert!(
            self.retains_running_first_item(address),
            "only the running first item can enter completion-observed"
        );
        self.state = BluetoothSchedulerExclusiveListState::FirstItemCompletionObserved { address };
    }

    fn retains_completion_observed_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == BluetoothSchedulerExclusiveListState::FirstItemCompletionObserved { address }
    }

    fn retain_hardware_head_empty_first_item(&mut self, address: BluetoothControllerSramAddress) {
        assert!(
            self.retains_completion_observed_first_item(address),
            "only the completion-observed first item can retire its hardware head"
        );
        self.state =
            BluetoothSchedulerExclusiveListState::FirstItemHardwareHeadEmptyObserved { address };
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
    ) {
        assert!(
            self.retains_unlinked_first_item(address),
            "only the already-unlinked first item can pass the removal return gate"
        );
        self.state =
            BluetoothSchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address };
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

/// One bounded Controller-owned advertising completion attempt.
#[must_use = "the advertising graph and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingSchedulerCompletionStep<'a> {
    DrainAlreadyActive(BluetoothLegacyAdvertisingSchedulerRunning<'a>),
    SchedulerIdentityMismatch(BluetoothLegacyAdvertisingSchedulerRunning<'a>),
    NoFinishedList(BluetoothLegacyAdvertisingSchedulerRunning<'a>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothLegacyAdvertisingSchedulerRunning<'a>>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ),
}

/// One bounded continuation while an advertising graph remains running.
#[must_use = "the advertising graph and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingSchedulerRunningDrainStep<'a> {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<BluetoothLegacyAdvertisingSchedulerRunning<'a>>,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<BluetoothLegacyAdvertisingSchedulerRunning<'a>>,
    ),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    StillInFlight(
        BluetoothSchedulerFinishedListDrainState<BluetoothLegacyAdvertisingSchedulerRunning<'a>>,
    ),
    CompletionObserved(
        BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ),
}

/// One bounded continuation after advertising completion was observed.
#[must_use = "the advertising completion and every observed list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep<'a> {
    SchedulerIdentityMismatch(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ),
    DrainLost(
        BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RepeatedAdvertisingList {
        drain: BluetoothSchedulerFinishedListDrainState<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
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
    _standalone_dtm_profile: crate::controller_hal::BluetoothStandaloneAlwaysAwakeDtmProfile,
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
    ) -> crate::controller_time::BluetoothControllerTimeWorkerPhase {
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
        crate::controller_time::BluetoothControllerTimeRequest,
        crate::controller_time::BluetoothControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller_time::BluetoothControllerTimeRequest,
    ) -> Result<(), crate::controller_time::BluetoothControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller_time::BluetoothControllerTimeRequest,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeEventStep,
        crate::controller_time::BluetoothControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeEventStep,
        crate::controller_time::BluetoothControllerTimeEventError,
    > {
        self.task.drain_orphan_controller_time()
    }

    /// Admit one already projected first advertising event into the common timeline.
    #[cfg(any(target_arch = "riscv32", test))]
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure returns the exact affine candidate"
        )
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure returns the exact affine candidate"
        )
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
    #[cfg_attr(
        target_pointer_width = "64",
        expect(
            clippy::result_large_err,
            reason = "the no-alloc failure retains the complete affine event"
        )
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
    /// The common list identity and scheduler-head encoding are checked while
    /// every scanner resource is still CPU-owned. The infallible suffix then
    /// publishes the RX list, restricted scanner command and scheduler head in
    /// the reviewed hardware order.
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
                return Err(BluetoothPassiveScanSchedulerHeadPublicationFailure { error, merged });
            }
        };
        let BluetoothPassiveScanEmptySchedulerMergePrepared { graph, reservation } = merged;
        let graph = graph.prepare_publication();
        let graph = unsafe { self.task.publish_passive_scan_rx_memory(graph) };
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
    fn validate_first_scheduler_item_head(
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
    fn publish_validated_first_scheduler_item_head(
        &mut self,
        address: BluetoothControllerSramAddress,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) -> BluetoothSchedulerHardwareListHeadPublished {
        let publication = unsafe { self.task.publish_scheduler_hardware_list_head(index, head) };
        self._scheduler_list.retain_published_first_item(address);
        publication
    }

    /// Perform one fresh fenced completion-list transfer for advertising.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_legacy_advertising_completion<'a>(
        &mut self,
        running: BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothLegacyAdvertisingSchedulerCompletionStep<'a> {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerCompletionStep::SchedulerIdentityMismatch(
                running,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerCompletionStep::DrainAlreadyActive(running);
        }

        if self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
            .is_err()
        {
            return BluetoothLegacyAdvertisingSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothLegacyAdvertisingSchedulerCompletionStep::NoFinishedList(running);
        };

        let BluetoothLegacyAdvertisingSchedulerRunning {
            item,
            run,
            _reservation,
        } = running;
        match item.observe_completion(observed) {
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::ListMismatch {
                item,
                observed,
            } => BluetoothLegacyAdvertisingSchedulerCompletionStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothLegacyAdvertisingSchedulerRunning {
                        item,
                        run,
                        _reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::StillInFlight(item) => {
                BluetoothLegacyAdvertisingSchedulerCompletionStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothLegacyAdvertisingSchedulerRunning {
                            item,
                            run,
                            _reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::CompletionObserved(
                item,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothLegacyAdvertisingSchedulerCompletionStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothLegacyAdvertisingSchedulerCompletionObserved {
                            item,
                            run,
                            _reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured drain while advertising remains running.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_legacy_advertising_running_finished_list_drain<'a>(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        >,
    ) -> BluetoothLegacyAdvertisingSchedulerRunningDrainStep<'a> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerRunningDrainStep::DrainLost(pending);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothLegacyAdvertisingSchedulerRunningDrainStep::DrainLost(pending);
        };

        let BluetoothLegacyAdvertisingSchedulerRunning {
            item,
            run,
            _reservation,
        } = pending.into_owner();
        match item.observe_completion(observed) {
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::ListMismatch {
                item,
                observed,
            } => BluetoothLegacyAdvertisingSchedulerRunningDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothLegacyAdvertisingSchedulerRunning {
                        item,
                        run,
                        _reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::StillInFlight(item) => {
                BluetoothLegacyAdvertisingSchedulerRunningDrainStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothLegacyAdvertisingSchedulerRunning {
                            item,
                            run,
                            _reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothLegacyAdvertisingRunningEventCompletionObservation::CompletionObserved(
                item,
            ) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothLegacyAdvertisingSchedulerRunningDrainStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothLegacyAdvertisingSchedulerCompletionObserved {
                            item,
                            run,
                            _reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured drain after advertising completion.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_legacy_advertising_completed_finished_list_drain<'a>(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ) -> BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep<'a> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::DrainLost(
                pending,
            );
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::RepeatedAdvertisingList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        } else {
            BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        }
    }

    /// Observe retirement of the exact advertising hardware-list head.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_legacy_advertising_hardware_head_retirement<'a>(
        &mut self,
        completed: BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
    ) -> BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep<'a> {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
        }
        let BluetoothLegacyAdvertisingSchedulerCompletionObserved {
            item,
            run,
            _reservation,
        } = completed;
        match self
            .task
            .observe_scheduler_hardware_list_head_retirement(run)
        {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: BluetoothLegacyAdvertisingSchedulerCompletionObserved {
                    item,
                    run,
                    _reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: BluetoothLegacyAdvertisingSchedulerCompletionObserved {
                    item,
                    run,
                    _reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                assert_eq!(
                    head.completed_head().address(),
                    Some(address),
                    "the retired head must retain the completed advertising identity"
                );
                self._scheduler_list
                    .retain_hardware_head_empty_first_item(address);
                BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved {
                        item,
                        head,
                        reservation: _reservation,
                    },
                )
            }
        }
    }

    /// Remove the sole advertising item from the source-owned software list.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_legacy_advertising_software_list<'a>(
        &mut self,
        observed: BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>,
    ) -> BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep<'a> {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed);
        }
        let BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved {
            item,
            head,
            reservation,
        } = observed;
        BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
                item,
                head,
                reservation,
            },
        )
    }

    /// Join a freshly serviced scheduler event to an unlinked advertising item.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_legacy_advertising_software_list_removal<'a>(
        &mut self,
        unlinked: BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin<'a> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::Pending(
                    unlinked,
                );
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
            item,
            head,
            reservation,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Recheck an already-unlinked advertising item without a new IRQ edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_legacy_advertising_software_list_removal<'a>(
        &mut self,
        storage: &impl crate::BluetoothSchedulerRunInterruptStorage,
        unlinked: BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked<'a>,
    ) -> BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck<'a> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked);
        }
        let BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
            item,
            head,
            reservation,
        } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        reservation,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::Pending(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinked {
                        item,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::Ready(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Release the advertising memory, timeline and source-list owners together.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
    ) -> BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothLegacyAdvertisingSchedulerRecycleStep::SchedulerIdentityMismatch(
                ready,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothLegacyAdvertisingSchedulerRecycleStep::FinishedListDrainStillActive(
                ready,
            );
        }
        let BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady {
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
                    ready: BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady {
                        item,
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
                let (item, removal) = prepared.into_parts();
                return BluetoothLegacyAdvertisingSchedulerRecycleStep::ReservationIdentityMismatch(
                    BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady {
                        item,
                        removal,
                        reservation,
                    },
                );
            }
        };
        let item = prepared.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        BluetoothLegacyAdvertisingSchedulerRecycleStep::Recycled(
            BluetoothLegacyAdvertisingSchedulerRecycled { item },
        )
    }

    /// Perform one fresh, fenced scanner completion-list transfer.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_passive_scan_completion(
        &mut self,
        running: BluetoothPassiveScanSchedulerRunning,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothPassiveScanSchedulerCompletionStep {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPassiveScanSchedulerCompletionStep::SchedulerIdentityMismatch(running);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        if self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake)
            .is_err()
        {
            return BluetoothPassiveScanSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothPassiveScanSchedulerCompletionStep::NoFinishedList(running);
        };
        let BluetoothPassiveScanSchedulerRunning {
            graph,
            run,
            reservation,
        } = running;
        match graph.observe_completion(observed) {
            BluetoothPassiveScanMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => BluetoothPassiveScanSchedulerCompletionStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPassiveScanSchedulerRunning {
                        graph: running,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPassiveScanMemoryGraphCompletionObservation::StillInFlight(graph) => {
                BluetoothPassiveScanSchedulerCompletionStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPassiveScanSchedulerRunning {
                            graph,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPassiveScanMemoryGraphCompletionObservation::CompletionObserved(graph) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPassiveScanSchedulerCompletionStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPassiveScanSchedulerCompletionObserved {
                            graph,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured drain while the scanner remains running.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_passive_scan_running_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<BluetoothPassiveScanSchedulerRunning>,
    ) -> BluetoothPassiveScanSchedulerRunningDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothPassiveScanSchedulerRunningDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerRunningDrainStep::DrainLost(pending);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothPassiveScanSchedulerRunningDrainStep::DrainLost(pending);
        };
        let BluetoothPassiveScanSchedulerRunning {
            graph,
            run,
            reservation,
        } = pending.into_owner();
        match graph.observe_completion(observed) {
            BluetoothPassiveScanMemoryGraphCompletionObservation::ListMismatch {
                running,
                observed,
            } => BluetoothPassiveScanSchedulerRunningDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(
                    BluetoothPassiveScanSchedulerRunning {
                        graph: running,
                        run,
                        reservation,
                    },
                    more,
                ),
                observed,
            },
            BluetoothPassiveScanMemoryGraphCompletionObservation::StillInFlight(graph) => {
                BluetoothPassiveScanSchedulerRunningDrainStep::StillInFlight(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPassiveScanSchedulerRunning {
                            graph,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
            BluetoothPassiveScanMemoryGraphCompletionObservation::CompletionObserved(graph) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothPassiveScanSchedulerRunningDrainStep::CompletionObserved(
                    BluetoothSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothPassiveScanSchedulerCompletionObserved {
                            graph,
                            run,
                            reservation,
                        },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue a captured drain after scanner completion was observed.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_passive_scan_completed_finished_list_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothPassiveScanSchedulerCompletionObserved,
        >,
    ) -> BluetoothPassiveScanSchedulerCompletionObservedDrainStep {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPassiveScanSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerCompletionObservedDrainStep::DrainLost(pending);
        }
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } =
            self.runtime.scheduler_finished_lists_mut().step()
        else {
            return BluetoothPassiveScanSchedulerCompletionObservedDrainStep::DrainLost(pending);
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothPassiveScanSchedulerCompletionObservedDrainStep::RepeatedScannerList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        } else {
            BluetoothPassiveScanSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: BluetoothSchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        }
    }

    /// Observe retirement of the exact scanner hardware-list head.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_passive_scan_hardware_head_retirement(
        &mut self,
        completed: BluetoothPassiveScanSchedulerCompletionObserved,
    ) -> BluetoothPassiveScanSchedulerHardwareHeadRetirementStep {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
        }
        let BluetoothPassiveScanSchedulerCompletionObserved {
            graph,
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
            } => BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: BluetoothPassiveScanSchedulerCompletionObserved {
                    graph,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: BluetoothPassiveScanSchedulerCompletionObserved {
                    graph,
                    run,
                    reservation,
                },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                assert_eq!(
                    head.completed_head().address(),
                    Some(address),
                    "the retired head must retain the completed scanner identity"
                );
                self._scheduler_list
                    .retain_hardware_head_empty_first_item(address);
                BluetoothPassiveScanSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved {
                        graph,
                        head,
                        reservation,
                    },
                )
            }
        }
    }

    /// Remove the sole scanner item from the source-owned software list.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_passive_scan_software_list(
        &mut self,
        observed: BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved,
    ) -> BluetoothPassiveScanSchedulerSoftwareListUnlinkStep {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothPassiveScanSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                observed,
            );
        }
        let BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved {
            graph,
            head,
            reservation,
        } = observed;
        BluetoothPassiveScanSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothPassiveScanSchedulerSoftwareListUnlinked {
                graph,
                head,
                reservation,
            },
        )
    }

    /// Join a fresh primary event to the unlinked scanner item.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_passive_scan_software_list_removal(
        &mut self,
        unlinked: BluetoothPassiveScanSchedulerSoftwareListUnlinked,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> BluetoothPassiveScanSchedulerSoftwareListRemovalJoin {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }
        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::Pending(unlinked);
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothPassiveScanSchedulerSoftwareListUnlinked {
            graph,
            head,
            reservation,
        } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothPassiveScanSchedulerSoftwareListUnlinked {
                        graph,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
                        graph,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Recheck an unlinked scanner item without requiring another IRQ edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_passive_scan_software_list_removal(
        &mut self,
        storage: &impl crate::BluetoothSchedulerRunInterruptStorage,
        unlinked: BluetoothPassiveScanSchedulerSoftwareListUnlinked,
    ) -> BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                unlinked,
            );
        }
        let BluetoothPassiveScanSchedulerSoftwareListUnlinked {
            graph,
            head,
            reservation,
        } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    BluetoothPassiveScanSchedulerSoftwareListUnlinked {
                        graph,
                        head,
                        reservation,
                    },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::Pending(
                    BluetoothPassiveScanSchedulerSoftwareListUnlinked {
                        graph,
                        head,
                        reservation,
                    },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::Ready(
                    BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
                        graph,
                        removal,
                        reservation,
                    },
                )
            }
        }
    }

    /// Extract RX packets and release the scanner memory and common-list owners.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_passive_scan_completed(
        &mut self,
        ready: BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
    ) -> BluetoothPassiveScanSchedulerRecycleStep {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothPassiveScanSchedulerRecycleStep::SchedulerIdentityMismatch(ready);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothPassiveScanSchedulerRecycleStep::FinishedListDrainStillActive(ready);
        }
        let BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
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
                    ready: BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
                        graph,
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
                let (graph, removal) = failure.into_prepared().into_parts();
                return BluetoothPassiveScanSchedulerRecycleStep::ReceiveInvalid {
                    ready: BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
                        graph,
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
                let (graph, removal) = extracted.into_prepared().into_parts();
                return BluetoothPassiveScanSchedulerRecycleStep::ReservationIdentityMismatch(
                    BluetoothPassiveScanSchedulerSoftwareListRemovalReady {
                        graph,
                        removal,
                        reservation,
                    },
                );
            }
        };
        let graph = extracted.commit();
        release.commit();
        self._scheduler_list.commit_recycled_first_item();
        BluetoothPassiveScanSchedulerRecycleStep::Recycled(BluetoothPassiveScanSchedulerRecycled {
            graph,
        })
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
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use open_esp_radio_bluetooth_ll::{
        LeDeviceAddress, LeDeviceAddressKind,
        advertiser::LegacyAdvertiserStandby,
        advertising::{
            AdvertisingInterval, LegacyAdvertisingData, LegacyNonconnectableAdvertisement,
            LegacyNonconnectableAdvertisingSet, PrimaryAdvertisingChannelMap,
        },
        connection::{
            LEGACY_CONNECT_IND_PAYLOAD_BYTES, LEGACY_CONNECT_IND_PDU_BYTES,
            LeLegacyConnectionRequest, LePeripheralConnection,
        },
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDirectionFindingWorkspaceLink, BluetoothDirectionFindingWorkspaceModelAddress,
        BluetoothDirectionFindingWorkspaceStorage, BluetoothLegacyAdvertisingMemoryGraphCpuOwned,
        BluetoothLegacyAdvertisingMemoryGraphModelAddress,
        BluetoothLegacyAdvertisingMemoryGraphStorage, BluetoothNonScanningRxMemoryModelAddress,
        BluetoothNonScanningRxMemoryStorage, BluetoothPassiveScanDefaultTxPowerDbm,
        BluetoothPassiveScanMemoryGraphModelAddress, BluetoothPassiveScanMemoryGraphStorage,
        BluetoothPassiveScanPrimaryChannel, BluetoothPassiveScanResetConfig,
        BluetoothPassiveScanSchedulerAllocationConfig,
        BluetoothPeripheralConnectionDefaultTxPowerDbm,
        BluetoothPeripheralConnectionMemoryGraphModelAddress,
        BluetoothPeripheralConnectionMemoryGraphStorage,
    };
    use open_esp_radio_esp32s31_hal::BluetoothControllerLatchedTime;
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use crate::{
        BluetoothClockedResources, BluetoothControllerRuntimeResources,
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
        BluetoothDtmSchedulerItemEvent, BluetoothRadioHardware,
        BluetoothSchedulerHardwareListIndex, BluetoothSchedulerInstant, BluetoothStopped,
        controller_time::BluetoothControllerSchedulerNow,
    };

    fn legacy_advertiser_enabled()
    -> open_esp_radio_bluetooth_ll::advertiser::LegacyAdvertiserEnabled<'static> {
        let advertisement = LegacyNonconnectableAdvertisement::new(
            LeDeviceAddress::from_wire_bytes([6, 5, 4, 3, 2, 1], LeDeviceAddressKind::Public),
            LegacyAdvertisingData::new(&[2, 1, 6]).expect("the fixed data fits"),
        );
        LegacyAdvertiserStandby::new()
            .configure(LegacyNonconnectableAdvertisingSet::new(
                advertisement,
                PrimaryAdvertisingChannelMap::all(),
                AdvertisingInterval::new(AdvertisingInterval::MIN_UNITS)
                    .expect("the minimum interval is valid"),
            ))
            .enable()
            .expect("the first generation is available")
    }

    fn legacy_advertising_memory() -> BluetoothLegacyAdvertisingMemoryGraphCpuOwned {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothLegacyAdvertisingMemoryGraphStorage::new(),
        ));
        let base = BluetoothLegacyAdvertisingMemoryGraphModelAddress::new(0x2f00_0100)
            .expect("the model base uses controller SRAM syntax");
        BluetoothLegacyAdvertisingMemoryGraphStorage::pin_static_model(storage, base)
            .expect("the advertising graph fits physical controller SRAM")
    }

    fn passive_scan_candidate() -> super::BluetoothPassiveScanFirstEventCandidate {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPassiveScanMemoryGraphStorage::new(),
        ));
        let base = BluetoothPassiveScanMemoryGraphModelAddress::new(0x2f00_1000)
            .expect("the model base uses controller SRAM syntax");
        let reset = BluetoothPassiveScanResetConfig::le_1m_public_accept_all(
            BluetoothPassiveScanDefaultTxPowerDbm::new(0),
            BluetoothControllerLatchedTime::from_bits(10_000),
        );
        let allocation = BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the restricted product limits fit the scanner graph");
        let graph = BluetoothPassiveScanMemoryGraphStorage::pin_static_model(
            storage, base, reset, allocation,
        )
        .expect("the scanner graph fits physical controller SRAM");
        super::BluetoothPassiveScanFirstEventCandidate::new(
            graph,
            BluetoothPassiveScanPrimaryChannel::Channel37,
            crate::BluetoothSchedulerRawWindow::from_projected_scheduler_window(11_000, 12_000)
                .expect("the scanner window is non-empty and forward"),
            BluetoothControllerLatchedTime::from_bits(10_100),
        )
    }

    fn peripheral_connection_candidate() -> (
        crate::BluetoothPeripheralConnectionRuntimeResources,
        crate::peripheral_connection::BluetoothPeripheralConnectionFirstEventCandidate,
        BluetoothDirectionFindingWorkspaceLink,
    ) {
        let graph_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothPeripheralConnectionMemoryGraphStorage::new(),
        ));
        let receive_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothNonScanningRxMemoryStorage::new(),
        ));
        let graph_base = BluetoothPeripheralConnectionMemoryGraphModelAddress::new(0x2f00_3000)
            .expect("the model connection graph base is valid");
        let receive_base = BluetoothNonScanningRxMemoryModelAddress::new(0x2f00_5000)
            .expect("the model receive-pool base is valid");
        let mut runtime = crate::BluetoothPeripheralConnectionRuntimeResources::claim_static_model(
            graph_storage,
            graph_base,
            receive_storage,
            receive_base,
            crate::BluetoothPeripheralConnectionRuntimeConfig::new(
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
            ),
        )
        .expect("the connection graph and receive pool fit controller SRAM");
        let request = LeLegacyConnectionRequest::decode(&connection_request())
            .expect("the fixed CONNECT_IND is valid");
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(300),
            20_000,
            scale,
        );
        let candidate = runtime
            .begin_event()
            .expect("the sole connection allocation starts idle")
            .prepare_first_event(
                LePeripheralConnection::from_request(request),
                crate::BluetoothLe1MPacketStartTiming::from_scheduler_micros(21_000),
            )
            .project_scheduler_window(
                epoch,
                crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            )
            .unwrap_or_else(|_| panic!("the fixed first connection window projects"));

        let workspace_storage = std::boxed::Box::leak(std::boxed::Box::new(
            BluetoothDirectionFindingWorkspaceStorage::new(),
        ));
        let workspace_base = BluetoothDirectionFindingWorkspaceModelAddress::new(0x2f00_7000)
            .expect("the model direction-finding workspace base is valid");
        let workspace = BluetoothDirectionFindingWorkspaceStorage::pin_static_model(
            workspace_storage,
            workspace_base,
        )
        .expect("the direction-finding workspace fits controller SRAM");
        (runtime, candidate, workspace.binding().link())
    }

    fn connection_request() -> [u8; LEGACY_CONNECT_IND_PDU_BYTES] {
        let mut pdu = [0; LEGACY_CONNECT_IND_PDU_BYTES];
        pdu[0] = 0x25;
        pdu[1] = LEGACY_CONNECT_IND_PAYLOAD_BYTES as u8;
        pdu[2..8].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        pdu[8..14].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
        pdu[14..18].copy_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        pdu[18..21].copy_from_slice(&[0x33, 0x22, 0x11]);
        pdu[21] = 2;
        pdu[22..24].copy_from_slice(&1u16.to_le_bytes());
        pdu[24..26].copy_from_slice(&24u16.to_le_bytes());
        pdu[28..30].copy_from_slice(&200u16.to_le_bytes());
        pdu[30..35].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]);
        pdu[35] = 5;
        pdu
    }

    use super::{
        BluetoothDtmControllerEventPreparationError, BluetoothSchedulerEmptyListMergeError,
        BluetoothSchedulerExclusiveListEpoch, BluetoothSchedulerFinishedListDrainState,
        BluetoothSchedulerHardwareListsCleared,
    };

    static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

    struct FakePlatform;

    impl Drop for FakePlatform {
        fn drop(&mut self) {
            PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn finished_list_drain_exposes_owner_only_after_the_capture_is_exhausted() {
        let drained_owner = Rc::new(());
        let drained_identity = Rc::clone(&drained_owner);
        let drained =
            BluetoothSchedulerFinishedListDrainState::from_worker_step(drained_owner, false);
        let BluetoothSchedulerFinishedListDrainState::Drained(drained_owner) = drained else {
            panic!("an exhausted capture must return the ordinary owner");
        };
        assert!(Rc::ptr_eq(&drained_owner, &drained_identity));

        let pending_owner = Rc::new(());
        let pending_identity = Rc::clone(&pending_owner);
        let pending =
            BluetoothSchedulerFinishedListDrainState::from_worker_step(pending_owner, true);
        let BluetoothSchedulerFinishedListDrainState::Pending(pending) = pending else {
            panic!("a retained capture must keep continuation provenance");
        };
        assert!(Rc::ptr_eq(pending.owner(), &pending_identity));
        assert!(Rc::ptr_eq(&pending.into_owner(), &pending_identity));
    }

    #[test]
    fn exclusive_empty_epoch_rejects_alias_and_wrong_identity_cancel() {
        let mut list = BluetoothSchedulerExclusiveListEpoch::new(
            BluetoothSchedulerHardwareListsCleared::for_validation(),
        );
        let first = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
            .expect("first item lies in controller SRAM");
        let other = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0200)
            .expect("second item lies in controller SRAM");

        assert_eq!(list.prepare_first_item(first), Ok(()));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
        );
        assert!(!list.cancel_first_item(other));
        assert!(list.cancel_first_item(first));
        assert_eq!(list.prepare_first_item(other), Ok(()));
    }

    #[test]
    fn passive_scanner_merge_cancellation_restores_both_cpu_owned_lists() {
        struct ScannerPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ScannerPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let admitted = task
            .admit_passive_scan_first_event(
                passive_scan_candidate(),
                super::BluetoothPassiveScanAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(10_000),
                },
            )
            .unwrap_or_else(|_| panic!("the requested scanner window must be admitted"));
        let event = task
            .prepare_passive_scan_first_event(
                admitted,
                super::BluetoothPassiveScanSequenceObservation {
                    sample: BluetoothControllerTimeSample::for_validation(10_001),
                },
            )
            .unwrap_or_else(|_| panic!("the retained scanner deadline must remain open"));
        let channel = event.channel();
        let window = event.window();
        let merged = task
            .prepare_passive_scan_empty_list_merge(event)
            .unwrap_or_else(|_| panic!("the pristine common list must accept the scanner item"));
        let event = task
            .cancel_passive_scan_empty_list_merge(merged)
            .unwrap_or_else(|_| panic!("the same epoch must restore the scanner item"));
        assert_eq!(event.channel(), channel);
        assert_eq!(event.window(), window);

        let merged = task
            .prepare_passive_scan_empty_list_merge(event)
            .unwrap_or_else(|_| panic!("cancellation must reopen the common list"));
        let event = task
            .cancel_passive_scan_empty_list_merge(merged)
            .unwrap_or_else(|_| panic!("the restored private chain must remain cancellable"));
        assert_eq!(event.channel(), channel);
        assert_eq!(event.window(), window);
        let _graph = task.cancel_passive_scan_first_event(event);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn passive_scanner_pre_sequence_cancellation_releases_the_timeline() {
        struct ScannerPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ScannerPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let admitted = task
            .admit_passive_scan_first_event(
                passive_scan_candidate(),
                super::BluetoothPassiveScanAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(10_000),
                },
            )
            .unwrap_or_else(|_| panic!("the first scanner candidate must be admitted"));
        let _graph = task.cancel_passive_scan_first_pre_sequence(admitted);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn connection_pre_sequence_cancellation_releases_the_timeline() {
        struct ConnectionPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ConnectionPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
        let admission_sample = candidate.requested_window().start().wrapping_sub(1_000);
        let admitted = task
            .admit_peripheral_connection_first_event(
                candidate,
                super::BluetoothPeripheralConnectionAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(admission_sample),
                },
            )
            .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
        let (allocation, connection) =
            task.cancel_peripheral_connection_first_pre_sequence(admitted);

        connection_runtime
            .restore_idle(allocation)
            .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
        assert!(connection_runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn connection_merge_cancellation_restores_private_and_common_lists() {
        struct ConnectionPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ConnectionPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
        let requested = candidate.requested_window();
        let admitted = task
            .admit_peripheral_connection_first_event(
                candidate,
                super::BluetoothPeripheralConnectionAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        requested.start().wrapping_sub(1_000),
                    ),
                },
            )
            .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
        let event = task
            .prepare_peripheral_connection_first_event(
                admitted,
                super::BluetoothPeripheralConnectionSequenceObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        requested.start().wrapping_sub(500),
                    ),
                },
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
                workspace,
            )
            .unwrap_or_else(|_| panic!("the second connection deadline must remain open"));
        assert_eq!(event.requested_window(), requested);
        assert_eq!(event.resolved_window(), requested);

        let merged = task
            .prepare_peripheral_connection_empty_list_merge(event)
            .unwrap_or_else(|_| panic!("the empty common list must accept the connection item"));
        assert_eq!(
            merged.hardware_list_index(),
            BluetoothSchedulerHardwareListIndex::ZERO
        );
        let event = task
            .cancel_peripheral_connection_empty_list_merge(merged)
            .unwrap_or_else(|_| panic!("the same epoch must restore the connection item"));
        let merged = task
            .prepare_peripheral_connection_empty_list_merge(event)
            .unwrap_or_else(|_| panic!("restoration must reopen both scheduler lists"));
        let event = task
            .cancel_peripheral_connection_empty_list_merge(merged)
            .unwrap_or_else(|_| panic!("the repeated merge must remain reversible"));
        let (allocation, connection) = task.cancel_peripheral_connection_first_event(event);

        connection_runtime
            .restore_idle(allocation)
            .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
        assert!(connection_runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn connection_admission_failure_returns_the_unchanged_candidate() {
        struct ConnectionPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ConnectionPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let (mut connection_runtime, candidate, _) = peripheral_connection_candidate();
        let requested = candidate.requested_window();
        let blocker = task
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                requested.start(),
                requested.end(),
                super::BluetoothSchedulerTimingPolicy::from_scheduler_config(
                    task.config,
                    task.time_scale,
                ),
                BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(1_000),
                ),
            )
            .expect("the pristine timeline accepts the blocking window");
        let failure = match task.admit_peripheral_connection_first_event(
            candidate,
            super::BluetoothPeripheralConnectionAdmissionObservation {
                sample: BluetoothControllerTimeSample::for_validation(
                    requested.start().wrapping_sub(1_000),
                ),
            },
        ) {
            Ok(_) => panic!("the occupied timeline must reject the connection window"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            super::BluetoothPeripheralConnectionFirstEventPreparationError::Timeline(
                super::BluetoothSchedulerReservationError::TimelineFull,
            )
        );
        let (allocation, connection) = failure.into_candidate().cancel();
        connection_runtime
            .restore_idle(allocation)
            .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
        assert!(connection_runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);
        task.release_scheduler_reservation(blocker);

        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn connection_merge_failure_preserves_the_prepared_event() {
        struct ConnectionPlatform;

        let stopped = BluetoothStopped::from_hardware(
            ConnectionPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let (mut connection_runtime, candidate, workspace) = peripheral_connection_candidate();
        let requested = candidate.requested_window();
        let admitted = task
            .admit_peripheral_connection_first_event(
                candidate,
                super::BluetoothPeripheralConnectionAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        requested.start().wrapping_sub(1_000),
                    ),
                },
            )
            .unwrap_or_else(|_| panic!("the first connection window must be admitted"));
        let event = task
            .prepare_peripheral_connection_first_event(
                admitted,
                super::BluetoothPeripheralConnectionSequenceObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        requested.start().wrapping_sub(500),
                    ),
                },
                BluetoothPeripheralConnectionDefaultTxPowerDbm::new(0),
                workspace,
            )
            .unwrap_or_else(|_| panic!("the second connection deadline must remain open"));
        let occupied =
            open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
                .expect("the occupying item lies in controller SRAM");
        task._scheduler_list
            .prepare_first_item(occupied)
            .expect("the common list starts empty");

        let failure = match task.prepare_peripheral_connection_empty_list_merge(event) {
            Ok(_) => panic!("the occupied common list must reject the connection item"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            BluetoothSchedulerEmptyListMergeError::ListNotEmpty
        );
        let event = failure.into_prepared();
        assert!(task._scheduler_list.cancel_first_item(occupied));
        let (allocation, connection) = task.cancel_peripheral_connection_first_event(event);
        connection_runtime
            .restore_idle(allocation)
            .unwrap_or_else(|_| panic!("the exact allocation returns to its runtime"));
        assert!(connection_runtime.allocation_is_idle());
        assert_eq!(connection.event_counter(), 0);

        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn published_first_item_cannot_be_cancelled_or_replaced() {
        let mut list = BluetoothSchedulerExclusiveListEpoch::new(
            BluetoothSchedulerHardwareListsCleared::for_validation(),
        );
        let first = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
            .expect("first item lies in controller SRAM");
        let other = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0200)
            .expect("second item lies in controller SRAM");

        assert_eq!(list.prepare_first_item(first), Ok(()));
        assert!(list.can_publish_first_item(first));
        assert!(!list.can_publish_first_item(other));
        list.retain_published_first_item(first);

        assert!(!list.can_publish_first_item(first));
        assert!(!list.cancel_first_item(first));
        list.retain_running_first_item(first);
        assert!(list.retains_running_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
        );
        list.retain_completion_observed_first_item(first);
        assert!(list.retains_completion_observed_first_item(first));
        assert!(!list.retains_running_first_item(first));
        assert!(!list.cancel_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
        );
        list.retain_hardware_head_empty_first_item(first);
        assert!(!list.retains_completion_observed_first_item(first));
        assert!(list.retains_hardware_head_empty_first_item(first));
        assert!(list.unlink_software_list_first_item(first));
        assert!(!list.unlink_software_list_first_item(first));
        assert!(list.retains_unlinked_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
        );
        list.retain_software_list_removal_ready_first_item(first);
        assert!(list.retains_software_list_removal_ready_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothSchedulerEmptyListMergeError::ListNotEmpty)
        );
        list.commit_recycled_first_item();
        assert_eq!(list.prepare_first_item(other), Ok(()));
    }

    #[test]
    fn powered_task_split_retains_the_same_running_list_identity() {
        struct TaskSplitPlatform;

        let stopped = BluetoothStopped::from_hardware(
            TaskSplitPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let address = open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress::new(0x2f00_0100)
            .expect("test item lies in Controller SRAM");
        scheduler
            ._scheduler_list
            .prepare_first_item(address)
            .expect("exclusive list starts empty");
        scheduler
            ._scheduler_list
            .retain_published_first_item(address);

        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        task.retain_running_first_item(address);
        drop((interrupt, task, modem_timer));

        assert!(
            scheduler
                ._scheduler_list
                .retains_running_first_item(address)
        );
    }

    #[test]
    fn controller_hal_precedes_complete_scheduler_init_and_arms_fail_stop() {
        PLATFORM_DROPS.store(0, Ordering::Relaxed);
        let stopped =
            BluetoothStopped::from_hardware(FakePlatform, BluetoothRadioHardware::for_validation());
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let operations = Rc::new(RefCell::new(Vec::new()));
        let hal_operations = Rc::clone(&operations);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {
            hal_operations.borrow_mut().push("controller-hal");
        });
        let time_scale = initialized.controller_time_scale();
        let scheduler_operations = Rc::clone(&operations);
        let mut scheduler = initialized.initialize_scheduler_with(
            BluetoothControllerRuntimeResources::<4, 3>::new(),
            |_| {
                scheduler_operations.borrow_mut().push("scheduler-hardware");
                BluetoothSchedulerHardwareListsCleared::for_validation()
            },
        );
        assert_eq!(
            operations.borrow().as_slice(),
            ["controller-hal", "scheduler-hardware"]
        );
        assert_eq!(scheduler.controller_time_scale(), time_scale);
        assert_eq!(
            scheduler.controller_time_phase(),
            crate::controller_time::BluetoothControllerTimeWorkerPhase::Idle
        );
        assert!(!scheduler.controller_time_needs_recheck());
        assert_eq!(scheduler.modem_timer_capacity(), 4);
        assert_eq!(scheduler.scheduler_capacity(), 3);
        assert!(scheduler.runtime_is_pristine());
        let (interrupt, task, modem_timer) = scheduler.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        assert_eq!(
            task.controller_time_phase(),
            crate::controller_time::BluetoothControllerTimeWorkerPhase::Idle
        );
        assert!(!task.controller_time_needs_recheck());
        drop((interrupt, task, modem_timer));
        drop(scheduler);
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rejected_initial_sequence_gate_releases_the_controller_owned_reservation() {
        struct AdmissionPlatform;

        let stopped = BluetoothStopped::from_hardware(
            AdmissionPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
            BluetoothDtmChannel::new(5).expect("channel five is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRxInitialEventWindow::new(
                crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothSchedulerInstant::from_image(900),
                BluetoothSchedulerInstant::from_image(1_020),
            ),
        )
        .expect("initial receiver event is role-valid");
        let time_scale = scheduler.controller_time_scale();
        let now = BluetoothControllerSchedulerNow::from_retained_epoch(
            BluetoothControllerSchedulerEpoch::new(
                BluetoothControllerTimeSample::for_validation(100),
                1_000,
                time_scale,
            ),
            BluetoothControllerTimeSample::for_validation(100),
        );
        assert_eq!(
            super::dtm::dtm_scheduler_current(&now),
            BluetoothSchedulerInstant::from_image(1_000)
        );

        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let reservation = task
            .admit_initial_dtm_event(
                event,
                &now,
                BluetoothControllerTimeSample::for_validation(92),
            )
            .expect("the fresh admission sample keeps the initial deadline open");
        let result = task.finish_dtm_sequence_authorization(
            reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
        );

        assert_eq!(
            result.expect_err("the deliberately late second sample must fail"),
            BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
                crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
            )
        );
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn first_advertising_event_uses_common_admission_and_cancels_losslessly() {
        struct AdvertisingPlatform;

        let stopped = BluetoothStopped::from_hardware(
            AdvertisingPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let scale = scheduler.controller_time_scale();
        let config = scheduler.scheduler_config();
        let reset = crate::BluetoothLegacyAdvertisingPrepared::prepare(
            legacy_advertiser_enabled(),
            legacy_advertising_memory(),
        )
        .expect("the bounded portable packet fits")
        .reset_link_state(crate::BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
        .expect("the portable packet selects the restricted reset");
        let candidate = reset
            .form_first_event_candidate(
                crate::BluetoothLegacyAdvertisingTimingObservation {
                    current: BluetoothSchedulerInstant::from_image(10_000),
                    radio_ready: BluetoothSchedulerInstant::from_image(11_999),
                    epoch: BluetoothControllerSchedulerEpoch::new(
                        BluetoothControllerTimeSample::for_validation(100),
                        1_000,
                        scale,
                    ),
                },
                config,
            )
            .expect("the first event projects into the retained epoch");
        let identity = candidate.identity();
        let raw_start = candidate.raw_window().start();

        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let admitted = task
            .admit_legacy_advertising_first_event(
                candidate,
                super::BluetoothLegacyAdvertisingAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        raw_start.wrapping_sub(100),
                    ),
                },
            )
            .expect("the first guarded deadline remains open");
        let (enabled, memory) = task
            .cancel_legacy_advertising_first_pre_sequence(admitted)
            .into_parts();
        let reset = crate::BluetoothLegacyAdvertisingPrepared::prepare(enabled, memory)
            .expect("the cancelled portable packet remains bounded")
            .reset_link_state(crate::BluetoothLegacyAdvertisingDefaultTxPowerDbm::new(0))
            .expect("the cancelled packet retains the restricted reset");
        let candidate = reset
            .form_first_event_candidate(
                crate::BluetoothLegacyAdvertisingTimingObservation {
                    current: BluetoothSchedulerInstant::from_image(10_000),
                    radio_ready: BluetoothSchedulerInstant::from_image(11_999),
                    epoch: BluetoothControllerSchedulerEpoch::new(
                        BluetoothControllerTimeSample::for_validation(100),
                        1_000,
                        scale,
                    ),
                },
                config,
            )
            .expect("the restored first event projects into the same epoch");
        let raw_start = candidate.raw_window().start();
        let admitted = task
            .admit_legacy_advertising_first_event(
                candidate,
                super::BluetoothLegacyAdvertisingAdmissionObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        raw_start.wrapping_sub(100),
                    ),
                },
            )
            .expect("cancellation released the first guarded reservation");
        let prepared = task
            .prepare_legacy_advertising_first_event(
                admitted,
                super::BluetoothLegacyAdvertisingSequenceObservation {
                    sample: BluetoothControllerTimeSample::for_validation(
                        raw_start.wrapping_sub(50),
                    ),
                },
            )
            .expect("the second guarded deadline remains open");
        assert_eq!(prepared.identity(), identity);
        assert_eq!(prepared.pdu(), &[0x02, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);

        let merged = match task.prepare_legacy_advertising_empty_list_merge(prepared) {
            Ok(merged) => merged,
            Err(_) => panic!("the pristine exclusive list must accept the advertising item"),
        };
        let prepared = match task.cancel_legacy_advertising_empty_list_merge(merged) {
            Ok(prepared) => prepared,
            Err(_) => panic!("the same scheduler epoch must restore the unpublished event"),
        };
        let cancelled = task.cancel_legacy_advertising_first_event(prepared);
        let (enabled, memory) = cancelled.into_parts();
        assert_eq!(enabled.prepare_event().identity(), identity);
        assert!(memory.prepare_packet(&[0x02, 6, 1, 2, 3, 4, 5, 6]).is_ok());
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn rejected_recurring_sequence_gate_releases_the_controller_owned_reservation() {
        struct RecurringPlatform;

        let stopped = BluetoothStopped::from_hardware(
            RecurringPlatform,
            BluetoothRadioHardware::for_validation(),
        );
        let (registers, platform) = stopped.into_parts();
        let clocked = BluetoothClockedResources::for_validation(registers, platform);
        let initialized = clocked.initialize_controller_hal_with(|_, _| {});
        let mut scheduler =
            initialized
                .initialize_scheduler_for_validation(
                    BluetoothControllerRuntimeResources::<1, 1>::new(),
                );
        let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
            BluetoothDtmChannel::new(5).expect("channel five is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRxRecurringEventWindow::new(
                crate::BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothSchedulerInstant::from_image(900),
                BluetoothSchedulerInstant::from_image(1_020),
            ),
        )
        .expect("receiver event is role-valid");
        let time_scale = scheduler.controller_time_scale();
        let now = BluetoothControllerSchedulerNow::from_retained_epoch(
            BluetoothControllerSchedulerEpoch::new(
                BluetoothControllerTimeSample::for_validation(100),
                1_000,
                time_scale,
            ),
            BluetoothControllerTimeSample::for_validation(100),
        );
        assert_eq!(
            super::dtm::dtm_scheduler_current(&now),
            BluetoothSchedulerInstant::from_image(1_000)
        );

        let (interrupt, mut task, modem_timer) = scheduler.split_runtime();
        let reservation = task
            .reserve_recurring_dtm_event(event, &now)
            .expect("the exact recurring window is initially free");
        let result = task.finish_dtm_sequence_authorization(
            reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
        );

        assert_eq!(
            result.expect_err("the deliberately late sequence sample must fail"),
            BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
                crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
            )
        );
        drop((interrupt, task, modem_timer));
        assert!(scheduler.runtime_is_pristine());
    }
}
