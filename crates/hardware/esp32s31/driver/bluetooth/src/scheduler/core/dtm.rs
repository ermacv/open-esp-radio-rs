//! DTM-specific scheduler preparation, publication and completion.

use super::{
    ControllerTimeAcquisitionError, SchedulerEmptyListMergeError, SchedulerHeadPublicationError,
};
#[cfg(target_arch = "riscv32")]
use super::{SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState};
#[cfg(target_arch = "riscv32")]
use crate::le::dtm::event::prepare::{
    DtmCompletionObservedEvent, DtmReviewedEventWordsPlan, DtmRunningEventCompletionObservation,
    DtmSchedulerBookkeepingPrepared,
};

use crate::le::dtm::event::prepare::{
    DtmEmptyListLinkPrepared, DtmHeadPublishedEvent, DtmRunningEvent, DtmSchedulerItemPhase,
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    ControllerTimeSample, SchedulerInstant,
    controller::time::ControllerSchedulerNow,
    scheduler::{
        SchedulerTimingPolicy,
        timeline::{SchedulerInitialAdmissionResolved, SchedulerRecurringReserved},
    },
};
#[cfg(target_arch = "riscv32")]
use crate::{
    DtmLinkStateReset, DtmRxInitialEventWindow, DtmRxRecurringEventWindow, DtmTxEventWindow,
    interrupt::SchedulerWakeBatch,
    le::dtm::{
        DtmActiveReceiverCpuOwned, DtmActiveTransmitterCpuOwned, DtmChannel, DtmPayloadLength,
        DtmPayloadPattern, DtmPhy, DtmPreparedTxGraph, DtmReceiverCpuOwned, DtmReceiverEvent,
        DtmSchedulerItemEventError, DtmTransmitterEvent, DtmTxSchedulerTiming, DtmTxTimingMicros,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    DtmSchedulerReservation, DtmSchedulerSequenceAuthorizationFailure,
    le::dtm::DtmSchedulerItemEvent,
    scheduler::{
        SchedulerReservationError, SchedulerSequenceAuthorizationError, SchedulerSequenceReady,
    },
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    DtmMemoryGraphCpuOwned, DtmMemoryGraphPrepareError, DtmSchedulerItemCompletionStatus,
};

use crate::{le::dtm::DtmRole, runtime_resources::ControllerPoweredTaskRuntime};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerSoftwareListRemovalJoin,
    BluetoothSchedulerSoftwareListRemovalReady,
};

use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadPublished,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex,
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareRunCommandPublished,
    oer_esp32s31_hal::types::BluetoothControllerSramAddress,
};

#[cfg(any(target_arch = "riscv32", test))]
pub(super) const fn dtm_scheduler_current(now: &ControllerSchedulerNow) -> SchedulerInstant {
    SchedulerInstant::from_image(now.micros())
}

/// Why the terminal Controller could not prepare one CPU-owned DTM event.
///
/// Reservation and sequence failures are rolled back against the same private
/// timeline before this error is returned. No variant leaks a timeline token
/// or leaves a hidden occupied slot behind.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmControllerEventPreparationError {
    /// The caller supplied a link-state reset for the other DTM role.
    LinkStateRoleMismatch {
        /// Role selected by this exact Controller operation.
        expected: DtmRole,
        /// Role retained by the supplied reset.
        observed: DtmRole,
    },
    /// The channel, PHY and role cannot form a reviewed scheduler item.
    #[cfg(target_arch = "riscv32")]
    SchedulerItem(DtmSchedulerItemEventError),
    /// The Controller-owned bounded timeline rejected the requested window.
    Reservation(SchedulerReservationError),
    /// The phase-appropriate fresh Controller-time sample closed the sequence gate.
    SequenceAuthorization(SchedulerSequenceAuthorizationError),
    /// A private post-enable timing, current, admission or sequence acquisition
    /// failed.
    ControllerTime(ControllerTimeAcquisitionError),
    /// The bound SRAM graph rejected the positional event transaction.
    #[cfg(target_arch = "riscv32")]
    Graph(DtmMemoryGraphPrepareError),
    /// The exclusive source-owned scheduler list still retains an item.
    EmptyList(SchedulerEmptyListMergeError),
}

/// Closed first-start completion class after the exact idle graph is restored.
///
/// This classification is chip policy rather than an Embassy decision. It
/// deliberately distinguishes finite timing/resource rejection from poisoned
/// ownership, identity and graph invariants before HCI command authority can
/// be reopened.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtmFirstPreparationCompletionClass {
    HardwareFailure,
    FailStop,
}

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) const fn classify_dtm_first_preparation_completion(
    error: DtmControllerEventPreparationError,
) -> DtmFirstPreparationCompletionClass {
    match error {
        DtmControllerEventPreparationError::Reservation(
            SchedulerReservationError::InitialDeadlineExpired
            | SchedulerReservationError::TimelineFull,
        )
        | DtmControllerEventPreparationError::SequenceAuthorization(
            SchedulerSequenceAuthorizationError::DeadlineExpired,
        )
        | DtmControllerEventPreparationError::ControllerTime(
            ControllerTimeAcquisitionError::Busy,
        ) => DtmFirstPreparationCompletionClass::HardwareFailure,
        DtmControllerEventPreparationError::LinkStateRoleMismatch { .. }
        | DtmControllerEventPreparationError::Reservation(
            SchedulerReservationError::WindowOutsideForwardHalfRange
            | SchedulerReservationError::OverlapResolutionOutsideForwardHalfRange
            | SchedulerReservationError::RecurringOverlapUnsupported
            | SchedulerReservationError::GenerationExhausted,
        )
        | DtmControllerEventPreparationError::ControllerTime(
            ControllerTimeAcquisitionError::OwnershipCollision
            | ControllerTimeAcquisitionError::GenerationExhausted
            | ControllerTimeAcquisitionError::RequestMismatch
            | ControllerTimeAcquisitionError::OwnershipLost
            | ControllerTimeAcquisitionError::Faulted
            | ControllerTimeAcquisitionError::Cancelled,
        )
        | DtmControllerEventPreparationError::EmptyList(_) => {
            DtmFirstPreparationCompletionClass::FailStop
        }
        #[cfg(target_arch = "riscv32")]
        DtmControllerEventPreparationError::SchedulerItem(_)
        | DtmControllerEventPreparationError::Graph(_) => {
            DtmFirstPreparationCompletionClass::FailStop
        }
    }
}

/// Lossless terminal-Controller TX preparation rejection.
///
/// Packet readiness is deliberately reduced to ordinary CPU ownership on a
/// failed composed transaction. Pattern and length are retained so callers can
/// deterministically rebuild that proof before retrying.
#[cfg(target_arch = "riscv32")]
#[must_use = "the returned TX graph and complete DTM program remain owned"]
pub struct DtmControllerTxPreparationFailure {
    error: DtmControllerEventPreparationError,
    memory: DtmMemoryGraphCpuOwned,
    pattern: DtmPayloadPattern,
    length: DtmPayloadLength,
}

#[cfg(target_arch = "riscv32")]
impl DtmControllerTxPreparationFailure {
    fn from_prepared(error: DtmControllerEventPreparationError, owner: DtmPreparedTxGraph) -> Self {
        let pattern = owner.pattern();
        let length = owner.length();
        Self {
            error,
            memory: owner.discard(),
            pattern,
            length,
        }
    }

    /// Exact finite reason no first-item merge was returned.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Recover the graph and complete TX program for deterministic retry.
    pub fn into_parts(self) -> (DtmMemoryGraphCpuOwned, DtmPayloadPattern, DtmPayloadLength) {
        (self.memory, self.pattern, self.length)
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for DtmControllerTxPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmControllerTxPreparationFailure")
            .field("error", &self.error)
            .field("pattern", &self.pattern)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller RX preparation rejection.
#[cfg(target_arch = "riscv32")]
#[must_use = "the returned RX graph and non-copyable session remain owned"]
pub struct DtmControllerRxPreparationFailure {
    error: DtmControllerEventPreparationError,
    owner: DtmReceiverCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl DtmControllerRxPreparationFailure {
    /// Exact finite reason no first-item merge was returned.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged graph/session aggregate for retry or Test End.
    pub fn into_owner(self) -> DtmReceiverCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for DtmControllerRxPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmControllerRxPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller rejection of one recurring TX event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active transmitter and its committed phase remain owned"]
pub struct DtmControllerTxRecurringPreparationFailure {
    error: DtmControllerEventPreparationError,
    owner: DtmActiveTransmitterCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl DtmControllerTxRecurringPreparationFailure {
    /// Exact finite reason no recurring TX merge was returned.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged active transmitter for retry or Test End.
    pub fn into_owner(self) -> DtmActiveTransmitterCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for DtmControllerTxRecurringPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmControllerTxRecurringPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller rejection of one recurring RX event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active receiver and its committed phase remain owned"]
pub struct DtmControllerRxRecurringPreparationFailure {
    error: DtmControllerEventPreparationError,
    owner: DtmActiveReceiverCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl DtmControllerRxRecurringPreparationFailure {
    /// Exact finite reason no recurring RX merge was returned.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged active receiver for retry or Test End.
    pub fn into_owner(self) -> DtmActiveReceiverCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for DtmControllerRxRecurringPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DtmControllerRxRecurringPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Initial transmitter candidate formed from one private scheduler current.
///
/// This state owns no timeline slot. It can only enter initial admission or be
/// reduced to the ordinary role-specific retry owner.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged transmitter must enter admission or be cancelled"]
pub(crate) struct DtmTransmitterFirstStaged {
    owner: DtmPreparedTxGraph,
    link_state: DtmLinkStateReset,
    channel: DtmChannel,
    phy: DtmPhy,
    timing: DtmTxSchedulerTiming,
    margin: u32,
    window: DtmTxEventWindow,
    event: DtmSchedulerItemEvent,
    now: ControllerSchedulerNow,
}

/// Initial receiver candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged receiver must enter admission or be cancelled"]
pub(crate) struct DtmReceiverFirstStaged {
    owner: DtmReceiverCpuOwned,
    link_state: DtmLinkStateReset,
    channel: DtmChannel,
    phy: DtmPhy,
    margin: u32,
    window: DtmRxInitialEventWindow,
    event: DtmSchedulerItemEvent,
    now: ControllerSchedulerNow,
}

/// Recurring transmitter candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged transmitter must reserve or be cancelled"]
pub(crate) struct DtmTransmitterRecurringStaged {
    owner: DtmActiveTransmitterCpuOwned,
    window: DtmTxEventWindow,
    event: DtmSchedulerItemEvent,
    now: ControllerSchedulerNow,
}

/// Recurring receiver candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged receiver must reserve or be cancelled"]
pub(crate) struct DtmReceiverRecurringStaged {
    owner: DtmActiveReceiverCpuOwned,
    window: DtmRxRecurringEventWindow,
    event: DtmSchedulerItemEvent,
    now: ControllerSchedulerNow,
}

/// Initial transmitter candidate after admission and overlap resolution.
#[cfg(target_arch = "riscv32")]
#[must_use = "the admitted transmitter must consume a later sequence sample or be cancelled"]
pub(crate) struct DtmTransmitterFirstPreSequence {
    staged: DtmTransmitterFirstStaged,
    reservation: DtmSchedulerReservation<SchedulerInitialAdmissionResolved>,
}

/// Initial receiver candidate after admission and overlap resolution.
#[cfg(target_arch = "riscv32")]
#[must_use = "the admitted receiver must consume a later sequence sample or be cancelled"]
pub(crate) struct DtmReceiverFirstPreSequence {
    staged: DtmReceiverFirstStaged,
    reservation: DtmSchedulerReservation<SchedulerInitialAdmissionResolved>,
}

/// Recurring transmitter candidate after its exact-window reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the reserved transmitter must consume a later sequence sample or be cancelled"]
pub(crate) struct DtmTransmitterRecurringPreSequence {
    staged: DtmTransmitterRecurringStaged,
    reservation: DtmSchedulerReservation<SchedulerRecurringReserved>,
}

/// Recurring receiver candidate after its exact-window reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the reserved receiver must consume a later sequence sample or be cancelled"]
pub(crate) struct DtmReceiverRecurringPreSequence {
    staged: DtmReceiverRecurringStaged,
    reservation: DtmSchedulerReservation<SchedulerRecurringReserved>,
}

/// Lossless failure to join a DTM item to the exclusive empty scheduler list.
#[must_use = "the unchanged scheduler-prepared item remains CPU-owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct DtmEmptySchedulerMergeFailure<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    error: SchedulerEmptyListMergeError,
    item: DtmSchedulerBookkeepingPrepared<Role, Phase>,
}

#[cfg(target_arch = "riscv32")]
impl<Role, Phase> DtmEmptySchedulerMergeFailure<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    /// Exact reason the list owner rejected this item.
    pub(crate) const fn error(&self) -> SchedulerEmptyListMergeError {
        self.error
    }

    /// Recover the unchanged CPU-owned item for retry or cancellation.
    pub(crate) fn into_item(self) -> DtmSchedulerBookkeepingPrepared<Role, Phase> {
        self.item
    }
}

/// Sole DTM item joined to one exclusive, previously empty scheduler epoch.
///
/// The item-side descriptor transform and source-owned list state now agree on
/// one exact identity. This remains CPU-owned: no visibility fence, hardware
/// head, RUN command or radio-completion authority has been granted.
#[must_use = "the merged item must be published through the same scheduler or cancelled"]
pub struct DtmEmptySchedulerMergePrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    item: DtmEmptyListLinkPrepared<Role, Phase>,
}

impl<Role, Phase> DtmEmptySchedulerMergePrepared<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    /// Role retained by the exact prepared graph.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Address selected by this first-item merge.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Hardware list assigned to DTM by its zeroed private context.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.item.hardware_list_index()
    }
}

/// Type-level proof that a merged DTM item is the command's first event.
///
/// The marker is created only by the Controller preparation transaction. It
/// keeps lossless first-event cancellation separate from recurring-session
/// recovery before the scheduler head becomes visible to hardware.
pub struct DtmInitialSchedulerItemPhase {
    _private: (),
}

/// Type-level proof that a merged DTM item belongs to an active command.
///
/// A recurring merge can only cancel back into its exact active owner; it
/// cannot cross the first-event recovery edge.
pub struct DtmRecurringSchedulerItemPhase {
    _private: (),
}

/// Lossless rejection before scheduler-head MMIO publication.
#[must_use = "the unchanged CPU-owned merge can still be retried or cancelled"]
pub struct DtmSchedulerHeadPublicationFailure<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    error: SchedulerHeadPublicationError,
    merged: DtmEmptySchedulerMergePrepared<Role, Phase>,
}

impl<Role, Phase> DtmSchedulerHeadPublicationFailure<Role, Phase>
where
    Phase: DtmSchedulerItemPhase<Role>,
{
    /// Exact reason no scheduler head was published.
    pub const fn error(&self) -> SchedulerHeadPublicationError {
        self.error
    }

    /// Recover the unchanged CPU-owned merge.
    pub fn into_merged(self) -> DtmEmptySchedulerMergePrepared<Role, Phase> {
        self.merged
    }
}

/// DTM graph whose scheduler item is visible as one hardware-list head.
///
/// The descriptor-before-head and trailing device fences have completed. This
/// state retains the pinned graph and affine list identity, so cancellation and
/// CPU mutation are no longer available. Dynamic interrupt preparation,
/// scheduler event publication, RUN and completion ownership remain absent.
#[must_use = "the published scheduler head must advance to RUN or fail-stop ownership"]
pub struct DtmSchedulerHeadPublished<Role> {
    item: DtmHeadPublishedEvent<Role>,
    publication: BluetoothSchedulerHardwareListHeadPublished,
}

impl<Role> DtmSchedulerHeadPublished<Role> {
    /// Role retained by the now hardware-visible graph.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact item retained by both graph and published head evidence.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Hardware list whose head now addresses this item.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.publication.index()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_parts(
        self,
    ) -> (
        DtmHeadPublishedEvent<Role>,
        BluetoothSchedulerHardwareListHeadPublished,
    ) {
        (self.item, self.publication)
    }
}

/// DTM graph admitted to scheduler execution by the complete run transaction.
///
/// This state retains the pinned graph and the affine proof of
/// `head -> dynamic interrupts -> synchronous BTMAC event -> RUN`. It does not
/// claim that the radio completed the item or that CPU access may resume.
#[must_use = "the running DTM graph must advance through owned completion or quiescence"]
pub struct DtmSchedulerRunning<Role> {
    item: DtmRunningEvent<Role>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

impl<Role> DtmSchedulerRunning<Role> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn new(
        item: DtmHeadPublishedEvent<Role>,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> Self {
        let item = item.into_running(&run);
        Self { item, run }
    }

    /// Role retained by the running scheduler graph.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact scheduler-item address retained while hardware owns the graph.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Hardware list admitted by the complete run transaction.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

/// Owned results of common stop for the exclusive DTM list.
#[cfg(target_arch = "riscv32")]
#[must_use]
pub(crate) enum DtmSchedulerStopStep<Role> {
    Pending {
        running: DtmSchedulerRunning<Role>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    },
    IdentityMismatch {
        _running: DtmSchedulerRunning<Role>,
        _stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    },
    UnexpectedFinishedList {
        _running: DtmSchedulerRunning<Role>,
        _stopped: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStopped,
        _observed: oer_esp32s31_hal::bluetooth::BluetoothSchedulerFinishedListPop,
    },
    HeadRejected {
        _item: DtmRunningEvent<Role>,
        _observed:
            oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadRetirementObservation,
    },
    MemoryRejected {
        _item: DtmRunningEvent<Role>,
        _stopped: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStoppedItem,
    },
    Retired(DtmSchedulerHardwareHeadRetirementStep<Role>),
}

/// DTM graph with a non-sentinel status observed after a fresh fenced transfer.
///
/// The source-owned scheduler epoch remains occupied and the graph remains
/// controller-owned. This state does not expose packet memory, cancellation or
/// reclamation before the hardware/software unlink path is completed.
#[must_use = "the completion-observed graph must advance through unlink and recycle"]
#[cfg(target_arch = "riscv32")]
pub struct DtmSchedulerCompletionObserved<Role> {
    item: DtmCompletionObservedEvent<Role>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmSchedulerCompletionObserved<Role> {
    /// Role retained by the completed scheduler item.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact scheduler-item identity whose status was observed.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic non-sentinel completion status.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.item.status()
    }

    /// Hardware list retained through RUN and completion observation.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

/// One bounded Controller-owned DTM completion attempt.
#[must_use = "the returned DTM graph and any unrelated affine list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerCompletionStep<Role> {
    /// A generic finished-list drain is already active; no new transfer ran.
    DrainAlreadyActive(DtmSchedulerRunning<Role>),
    /// The supplied running graph does not belong to this Controller epoch.
    SchedulerIdentityMismatch(DtmSchedulerRunning<Role>),
    /// The fresh transfer contained no finished hardware list.
    NoFinishedList(DtmSchedulerRunning<Role>),
    /// The selected list is not DTM list zero and remains available to dispatch.
    UnrelatedList {
        /// Running graph, paired with continuation provenance only when needed.
        drain: SchedulerFinishedListDrainState<DtmSchedulerRunning<Role>>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// DTM list zero was reported but its status remains the in-flight sentinel.
    StillInFlight(SchedulerFinishedListDrainState<DtmSchedulerRunning<Role>>),
    /// One non-sentinel status was observed without returning CPU ownership.
    CompletionObserved(SchedulerFinishedListDrainState<DtmSchedulerCompletionObserved<Role>>),
}

/// One bounded continuation of an already captured finished-list drain while
/// the DTM graph is still running.
///
/// This operation never captures a new hardware observation. It consumes at
/// most one list from the Controller's retained drain and returns every affine
/// owner, including an unrelated list observation, to the caller.
#[must_use = "the running graph and every drained list observation must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerRunningDrainStep<Role> {
    /// The supplied running graph does not belong to this Controller epoch.
    SchedulerIdentityMismatch(SchedulerFinishedListDrainPending<DtmSchedulerRunning<Role>>),
    /// The capture identified by the token is no longer active. The token and
    /// graph owner are retained losslessly for fail-stop handling.
    DrainLost(SchedulerFinishedListDrainPending<DtmSchedulerRunning<Role>>),
    /// One non-DTM list remains available to its owning dispatcher.
    UnrelatedList {
        /// Running graph, paired with continuation provenance only when needed.
        drain: SchedulerFinishedListDrainState<DtmSchedulerRunning<Role>>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// DTM list zero was selected but its item remains in flight.
    StillInFlight(SchedulerFinishedListDrainState<DtmSchedulerRunning<Role>>),
    /// DTM completion was observed while the same captured transfer may still
    /// retain unrelated lists.
    CompletionObserved(SchedulerFinishedListDrainState<DtmSchedulerCompletionObserved<Role>>),
}

/// One bounded continuation of a captured finished-list drain after DTM list
/// zero already produced a completion observation.
///
/// The captured set cannot contain list zero twice. Every ordinary result is
/// therefore an unrelated affine list token paired with the unchanged DTM
/// completion owner. A repeated list-zero token is retained explicitly as a
/// fail-stop invariant violation instead of being discarded.
#[must_use = "the completed graph and every remaining list observation must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerCompletionObservedDrainStep<Role> {
    /// The supplied completion does not belong to this Controller epoch.
    SchedulerIdentityMismatch(
        SchedulerFinishedListDrainPending<DtmSchedulerCompletionObserved<Role>>,
    ),
    /// The capture identified by the token is no longer active. The token and
    /// completion owner are retained losslessly for fail-stop handling.
    DrainLost(SchedulerFinishedListDrainPending<DtmSchedulerCompletionObserved<Role>>),
    /// One unrelated list remains available to its owning dispatcher.
    UnrelatedList {
        /// Completion owner, paired with continuation provenance only when needed.
        drain: SchedulerFinishedListDrainState<DtmSchedulerCompletionObserved<Role>>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// The supposedly remaining set repeated DTM list zero. Both owners are
    /// retained for fail-stop handling.
    RepeatedDtmList {
        /// Completion owner, paired with continuation provenance only when needed.
        drain: SchedulerFinishedListDrainState<DtmSchedulerCompletionObserved<Role>>,
        /// Impossible repeated list-zero observation.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

/// Completed DTM graph after its exact hardware-list head was freshly empty.
///
/// The descriptor remains unavailable to CPU mutation until the independent
/// software-list removal gate and recycle transition complete.
#[must_use = "the empty-head graph must advance through software-list removal and recycle"]
#[cfg(target_arch = "riscv32")]
pub struct DtmSchedulerHardwareHeadEmptyObserved<Role> {
    item: DtmCompletionObservedEvent<Role>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmSchedulerHardwareHeadEmptyObserved<Role> {
    /// Role retained by the completed event.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact scheduler item whose hardware head became empty.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained through the head observation.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.item.status()
    }

    /// Hardware list retained by the original RUN and empty observation.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

/// Completed DTM graph removed from the source-owned software list.
///
/// The open scheduler does not reproduce the vendor intrusive list container:
/// this affine state is the sole-item removal itself. The graph and reservation
/// remain retained, and no descriptor or packet memory is returned to callers.
#[must_use = "the unlinked graph must pass the finite removal return gate"]
#[cfg(target_arch = "riscv32")]
pub struct DtmSchedulerSoftwareListUnlinked<Role> {
    item: DtmCompletionObservedEvent<Role>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmSchedulerSoftwareListUnlinked<Role> {
    /// Role retained by the already-unlinked event.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact scheduler item removed from the source-owned software list.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained while awaiting the return gate.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.item.status()
    }

    /// Hardware list retained through the empty-head and unlink states.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }
}

/// Result of removing the sole DTM item from the source-owned software list.
#[must_use = "identity mismatch must retain the empty-head graph; success must continue"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerSoftwareListUnlinkStep<Role> {
    /// The supplied empty-head graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(DtmSchedulerHardwareHeadEmptyObserved<Role>),
    /// The sole source-owned list item was removed exactly once.
    Unlinked(DtmSchedulerSoftwareListUnlinked<Role>),
}

/// DTM graph after the post-unlink scheduler return predicate became ready.
///
/// This state proves only the reviewed `idle + command statuses` predicate for
/// the exact already-unlinked item. It does not return CPU ownership, recycle
/// memory, or release the scheduler timeline reservation.
#[must_use = "the removal-ready graph must advance through recycle"]
#[cfg(target_arch = "riscv32")]
pub struct DtmSchedulerSoftwareListRemovalReady<Role> {
    item: DtmCompletionObservedEvent<Role>,
    _removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl<Role> DtmSchedulerSoftwareListRemovalReady<Role> {
    /// Role retained by the removal-ready event.
    pub const fn role(&self) -> DtmRole {
        self.item.role()
    }

    /// Exact scheduler item retained through the removal return gate.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained through the removal return gate.
    pub const fn status(&self) -> DtmSchedulerItemCompletionStatus {
        self.item.status()
    }

    /// Hardware list retained by the exact empty-head observation.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self._removal.index()
    }
}

/// Result of returning one removal-ready DTM graph to source-owned CPU state.
#[must_use = "failure retains the removal-ready graph; success retains the CPU graph"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerRecycleStep<Role> {
    /// The supplied graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(DtmSchedulerSoftwareListRemovalReady<Role>),
    /// A prior finished-list transfer still retains an unhandled list.
    FinishedListDrainStillActive(DtmSchedulerSoftwareListRemovalReady<Role>),
    /// The lower memory graph rejected the retained typed head identity.
    MemoryIdentityMismatch {
        /// Unchanged removal-ready graph.
        ready: DtmSchedulerSoftwareListRemovalReady<Role>,
        /// Exact lower identity mismatch.
        error: oer_esp32s31_bluetooth_memory::DtmMemoryGraphRecycleError,
    },
    /// The affine reservation does not belong to this Controller timeline.
    ReservationIdentityMismatch(DtmSchedulerSoftwareListRemovalReady<Role>),
    /// Successful RX must enter the role-specific drain/account/re-arm path.
    ReceiverSuccessRequiresSpecializedRecycle(DtmSchedulerSoftwareListRemovalReady<Role>),
    /// Memory and timeline ownership returned to the source role.
    Recycled(crate::le::dtm::DtmRecycledEvent<Role>),
}

/// Result of the role-specific successful RX recycle/re-arm transaction.
#[must_use = "failure retains the exact RX owner; success retains the re-armed session"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerRxSuccessRecycleStep {
    /// The supplied graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(
        DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
    ),
    /// A prior finished-list transfer still retains an unhandled list.
    FinishedListDrainStillActive(
        DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
    ),
    /// The typed receiver owner does not carry a successful scheduler status.
    CompletionStatusMismatch(
        DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
    ),
    /// The lower graph rejected the retained hardware/removal identity.
    MemoryIdentityMismatch {
        /// Unchanged removal-ready receiver graph.
        ready: DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
        /// Exact lower identity mismatch.
        error: oer_esp32s31_bluetooth_memory::DtmMemoryGraphRecycleError,
    },
    /// The returned RX chain failed the lossless two-slot preflight.
    ReturnedTopologyRejected {
        /// Unchanged removal-ready receiver graph.
        ready: DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
        /// Semantic lower topology or sentinel failure.
        error: oer_esp32s31_bluetooth_memory::DtmMemoryGraphRxSuccessRecycleError,
    },
    /// The retained reservation belongs to another timeline epoch.
    ReservationIdentityMismatch(
        DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
    ),
    /// RX memory, timeline and source-list ownership returned in re-armed form.
    Rearmed(crate::le::dtm::DtmRxRearmedEvent),
}

/// Internal result of joining one primary scheduler event to an already
/// unlinked DTM graph.
///
/// The outer sealed consumer remains responsible for admitting only an event
/// whose publication followed this exact unlink.
#[must_use = "the unlinked or removal-ready graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum DtmSchedulerSoftwareListRemovalJoin<Role> {
    SchedulerIdentityMismatch {
        unlinked: DtmSchedulerSoftwareListUnlinked<Role>,
        event: crate::interrupt::PrimarySchedulerEvent,
    },
    Pending(DtmSchedulerSoftwareListUnlinked<Role>),
    Ready(DtmSchedulerSoftwareListRemovalReady<Role>),
}

/// Internal result of one direct task-side post-unlink hardware recheck.
#[must_use = "the unlinked or removal-ready graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum DtmSchedulerSoftwareListRemovalRecheck<Role> {
    SchedulerIdentityMismatch(DtmSchedulerSoftwareListUnlinked<Role>),
    StorageUnavailable(DtmSchedulerSoftwareListUnlinked<Role>),
    Pending(DtmSchedulerSoftwareListUnlinked<Role>),
    Ready(DtmSchedulerSoftwareListRemovalReady<Role>),
}

/// One bounded post-completion hardware-head retirement attempt.
#[must_use = "the completion owner must enter fail-stop handling or advance"]
#[cfg(target_arch = "riscv32")]
pub enum DtmSchedulerHardwareHeadRetirementStep<Role> {
    /// The supplied completion does not belong to this scheduler epoch.
    SchedulerIdentityMismatch(DtmSchedulerCompletionObserved<Role>),
    /// The captured transfer still retains another impossible list bit.
    FinishedListDrainStillActive(DtmSchedulerCompletionObserved<Role>),
    /// The expected head remains nonempty; the sole-item invariant failed.
    ExpectedHeadStillPublished {
        /// Completion owner retained for fail-stop handling.
        completed: DtmSchedulerCompletionObserved<Role>,
        /// Fresh typed head retained for diagnostics without granting access.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// A different nonempty head appeared in the exclusive list; fail closed.
    UnexpectedHeadChanged {
        /// Completion owner retained without granting descriptor access.
        completed: DtmSchedulerCompletionObserved<Role>,
        /// Fresh conflicting head identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// The exact list head was freshly observed empty after a device fence.
    EmptyObserved(DtmSchedulerHardwareHeadEmptyObserved<Role>),
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    #[cfg(any(target_arch = "riscv32", test))]
    pub(super) fn admit_initial_dtm_event(
        &mut self,
        event: DtmSchedulerItemEvent,
        now: &ControllerSchedulerNow,
        admission_sample: ControllerTimeSample,
    ) -> Result<DtmSchedulerReservation<SchedulerInitialAdmissionResolved>, SchedulerReservationError>
    {
        let epoch = now.epoch();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        let window = self
            .runtime
            .scheduler_timeline_mut()
            .reserve_initial_window(
                event.raw_start(epoch),
                event.raw_end(epoch),
                timing_policy,
                admission_sample,
            )?;
        Ok(DtmSchedulerReservation::new(window, event, epoch))
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(super) fn reserve_recurring_dtm_event(
        &mut self,
        event: DtmSchedulerItemEvent,
        now: &ControllerSchedulerNow,
    ) -> Result<DtmSchedulerReservation<SchedulerRecurringReserved>, SchedulerReservationError>
    {
        let epoch = now.epoch();
        let timing_policy =
            SchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        let window = self
            .runtime
            .scheduler_timeline_mut()
            .reserve_recurring_window(
                event.raw_start(epoch),
                event.raw_end(epoch),
                timing_policy,
            )?;
        Ok(DtmSchedulerReservation::new(window, event, epoch))
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(super) fn finish_dtm_sequence_authorization<State>(
        &mut self,
        result: Result<
            DtmSchedulerReservation<SchedulerSequenceReady>,
            DtmSchedulerSequenceAuthorizationFailure<State>,
        >,
    ) -> Result<DtmSchedulerReservation<SchedulerSequenceReady>, DtmControllerEventPreparationError>
    {
        match result {
            Ok(reservation) => Ok(reservation),
            Err(failure) => {
                let error = failure.error();
                let reservation = failure.into_reservation();
                self.release_dtm_reservation(reservation);
                Err(DtmControllerEventPreparationError::SequenceAuthorization(
                    error,
                ))
            }
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn release_dtm_reservation<State>(&mut self, reservation: DtmSchedulerReservation<State>) {
        self.release_scheduler_reservation(reservation.into_window());
    }

    /// Reject initial TX before RF readiness can form a scheduler candidate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reject_dtm_transmitter_first_before_stage(
        &mut self,
        owner: DtmPreparedTxGraph,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerTxPreparationFailure {
        DtmControllerTxPreparationFailure::from_prepared(
            DtmControllerEventPreparationError::ControllerTime(error),
            owner,
        )
    }

    /// Form one initial transmitter candidate from a private fresh current.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::too_many_arguments,
        reason = "each typed input is a distinct reviewed DTM authority"
    )]
    pub(crate) fn stage_dtm_transmitter_first_item(
        &self,
        owner: DtmPreparedTxGraph,
        link_state: DtmLinkStateReset,
        channel: DtmChannel,
        phy: DtmPhy,
        requested_interval_micros: u16,
        now: ControllerSchedulerNow,
        timing_ready: crate::AlwaysAwakeTimingReady,
    ) -> Result<DtmTransmitterFirstStaged, DtmControllerTxPreparationFailure> {
        if link_state.role() != DtmRole::Transmitter {
            return Err(DtmControllerTxPreparationFailure::from_prepared(
                DtmControllerEventPreparationError::LinkStateRoleMismatch {
                    expected: DtmRole::Transmitter,
                    observed: link_state.role(),
                },
                owner,
            ));
        }
        let timing = DtmTxTimingMicros::new(owner.length(), phy, requested_interval_micros)
            .scheduler_timing();
        let margin = self.config.preparation_lead_micros();
        let current = dtm_scheduler_current(&now);
        let window = timing.initial_event_window(
            self.config,
            current,
            timing_ready.into_scheduler_instant(),
        );
        let event = match DtmSchedulerItemEvent::new_transmitter(channel, phy, window) {
            Ok(event) => event,
            Err(error) => {
                return Err(DtmControllerTxPreparationFailure::from_prepared(
                    DtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                ));
            }
        };

        Ok(DtmTransmitterFirstStaged {
            owner,
            link_state,
            channel,
            phy,
            timing,
            margin,
            window,
            event,
            now,
        })
    }

    /// Consume the initial admission sample and retain the resolved reservation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn admit_dtm_transmitter_first_item(
        &mut self,
        staged: DtmTransmitterFirstStaged,
        admission_sample: ControllerTimeSample,
    ) -> Result<DtmTransmitterFirstPreSequence, DtmControllerTxPreparationFailure> {
        let reservation =
            match self.admit_initial_dtm_event(staged.event, &staged.now, admission_sample) {
                Ok(reservation) => reservation,
                Err(error) => {
                    return Err(DtmControllerTxPreparationFailure::from_prepared(
                        DtmControllerEventPreparationError::Reservation(error),
                        staged.owner,
                    ));
                }
            };
        Ok(DtmTransmitterFirstPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved initial transmitter after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_first_staged(
        &mut self,
        staged: DtmTransmitterFirstStaged,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerTxPreparationFailure {
        self.reject_dtm_transmitter_first_before_stage(staged.owner, error)
    }

    /// Release initial TX admission and return every retry resource.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_first_pre_sequence(
        &mut self,
        pre_sequence: DtmTransmitterFirstPreSequence,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerTxPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_transmitter_first_staged(pre_sequence.staged, error)
    }

    /// Authorize initial TX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_transmitter_first_item(
        &mut self,
        pre_sequence: DtmTransmitterFirstPreSequence,
        sequence_sample: ControllerTimeSample,
    ) -> Result<
        DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase>,
        DtmControllerTxPreparationFailure,
    > {
        let DtmTransmitterFirstPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerTxPreparationFailure::from_prepared(
                    error,
                    staged.owner,
                ));
            }
        };
        let DtmTransmitterFirstStaged {
            owner,
            link_state,
            channel,
            phy,
            timing,
            margin,
            window,
            event: _,
            now: _,
        } = staged;
        let plan = match DtmReviewedEventWordsPlan::new_transmitter(link_state, reservation) {
            Ok(plan) => plan,
            Err(failure) => {
                let reservation = failure.into_reservation();
                self.release_dtm_reservation(reservation);
                return Err(DtmControllerTxPreparationFailure::from_prepared(
                    DtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: DtmRole::Transmitter,
                        observed: link_state.role(),
                    },
                    owner,
                ));
            }
        };
        let prepared = match plan.prepare_first(owner, channel, phy, timing, margin, window) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (memory, error, pattern, length, plan) = failure.into_retry();
                self.release_dtm_reservation(plan.into_reservation());
                return Err(DtmControllerTxPreparationFailure {
                    error: DtmControllerEventPreparationError::Graph(error),
                    memory,
                    pattern,
                    length,
                });
            }
        };
        let item = prepared.prepare_scheduler_bookkeeping();
        match self.prepare_dtm_empty_list_merge(item) {
            Ok(merged) => Ok(merged),
            Err(failure) => {
                let error = failure.error();
                let item = failure.into_item();
                let pattern = item.packet_pattern();
                let length = item.packet_length();
                let (memory, reservation) = item.cancel().cancel_first();
                self.release_dtm_reservation(reservation);
                Err(DtmControllerTxPreparationFailure {
                    error: DtmControllerEventPreparationError::EmptyList(error),
                    memory,
                    pattern,
                    length,
                })
            }
        }
    }

    /// Reject initial RX before RF readiness can form a scheduler candidate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reject_dtm_receiver_first_before_stage(
        &mut self,
        owner: DtmReceiverCpuOwned,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxPreparationFailure {
        DtmControllerRxPreparationFailure {
            error: DtmControllerEventPreparationError::ControllerTime(error),
            owner,
        }
    }

    /// Form one initial receiver candidate from a private fresh current.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_receiver_first_item(
        &self,
        owner: DtmReceiverCpuOwned,
        link_state: DtmLinkStateReset,
        channel: DtmChannel,
        phy: DtmPhy,
        now: ControllerSchedulerNow,
        timing_ready: crate::AlwaysAwakeTimingReady,
    ) -> Result<DtmReceiverFirstStaged, DtmControllerRxPreparationFailure> {
        if link_state.role() != DtmRole::Receiver {
            return Err(DtmControllerRxPreparationFailure {
                error: DtmControllerEventPreparationError::LinkStateRoleMismatch {
                    expected: DtmRole::Receiver,
                    observed: link_state.role(),
                },
                owner,
            });
        }
        let margin = self.config.preparation_lead_micros();
        let current = dtm_scheduler_current(&now);
        let window = crate::DtmRxInitialEventWindow::new(
            self.config,
            current,
            timing_ready.into_scheduler_instant(),
        );
        let event = match DtmSchedulerItemEvent::new_initial_receiver(channel, phy, window) {
            Ok(event) => event,
            Err(error) => {
                return Err(DtmControllerRxPreparationFailure {
                    error: DtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                });
            }
        };

        Ok(DtmReceiverFirstStaged {
            owner,
            link_state,
            channel,
            phy,
            margin,
            window,
            event,
            now,
        })
    }

    /// Consume the initial admission sample and retain the resolved reservation.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn admit_dtm_receiver_first_item(
        &mut self,
        staged: DtmReceiverFirstStaged,
        admission_sample: ControllerTimeSample,
    ) -> Result<DtmReceiverFirstPreSequence, DtmControllerRxPreparationFailure> {
        let reservation =
            match self.admit_initial_dtm_event(staged.event, &staged.now, admission_sample) {
                Ok(reservation) => reservation,
                Err(error) => {
                    return Err(DtmControllerRxPreparationFailure {
                        error: DtmControllerEventPreparationError::Reservation(error),
                        owner: staged.owner,
                    });
                }
            };
        Ok(DtmReceiverFirstPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved initial receiver after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_first_staged(
        &mut self,
        staged: DtmReceiverFirstStaged,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxPreparationFailure {
        self.reject_dtm_receiver_first_before_stage(staged.owner, error)
    }

    /// Release initial RX admission and return every retry resource.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_first_pre_sequence(
        &mut self,
        pre_sequence: DtmReceiverFirstPreSequence,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_receiver_first_staged(pre_sequence.staged, error)
    }

    /// Authorize initial RX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_receiver_first_item(
        &mut self,
        pre_sequence: DtmReceiverFirstPreSequence,
        sequence_sample: ControllerTimeSample,
    ) -> Result<
        DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase>,
        DtmControllerRxPreparationFailure,
    > {
        let DtmReceiverFirstPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerRxPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let DtmReceiverFirstStaged {
            owner,
            link_state,
            channel,
            phy,
            margin,
            window,
            event: _,
            now: _,
        } = staged;
        let plan = match DtmReviewedEventWordsPlan::new_receiver(link_state, reservation) {
            Ok(plan) => plan,
            Err(failure) => {
                self.release_dtm_reservation(failure.into_reservation());
                return Err(DtmControllerRxPreparationFailure {
                    error: DtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: DtmRole::Receiver,
                        observed: link_state.role(),
                    },
                    owner,
                });
            }
        };
        let prepared = match plan.prepare_first(owner, channel, phy, margin, window) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (owner, error, plan) = failure.into_retry();
                self.release_dtm_reservation(plan.into_reservation());
                return Err(DtmControllerRxPreparationFailure {
                    error: DtmControllerEventPreparationError::Graph(error),
                    owner,
                });
            }
        };
        let item = prepared.prepare_scheduler_bookkeeping();
        match self.prepare_dtm_empty_list_merge(item) {
            Ok(merged) => Ok(merged),
            Err(failure) => {
                let error = failure.error();
                let item = failure.into_item();
                let (owner, reservation) = item.cancel().cancel_first();
                self.release_dtm_reservation(reservation);
                Err(DtmControllerRxPreparationFailure {
                    error: DtmControllerEventPreparationError::EmptyList(error),
                    owner,
                })
            }
        }
    }

    /// Form one recurring transmitter candidate from a private fresh current.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_transmitter_recurring_item(
        &self,
        owner: DtmActiveTransmitterCpuOwned,
        now: ControllerSchedulerNow,
    ) -> Result<DtmTransmitterRecurringStaged, DtmControllerTxRecurringPreparationFailure> {
        let current = dtm_scheduler_current(&now);
        let next_window = owner
            .timing()
            .advance_event_window(self.config, owner.last_committed_window(), current)
            .window();
        let event =
            match DtmSchedulerItemEvent::new_transmitter(owner.channel(), owner.phy(), next_window)
            {
                Ok(event) => event,
                Err(error) => {
                    return Err(DtmControllerTxRecurringPreparationFailure {
                        error: DtmControllerEventPreparationError::SchedulerItem(error),
                        owner,
                    });
                }
            };

        Ok(DtmTransmitterRecurringStaged {
            owner,
            window: next_window,
            event,
            now,
        })
    }

    /// Reserve the exact recurring TX window before sequence acquisition.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reserve_dtm_transmitter_recurring_item(
        &mut self,
        staged: DtmTransmitterRecurringStaged,
    ) -> Result<DtmTransmitterRecurringPreSequence, DtmControllerTxRecurringPreparationFailure>
    {
        let reservation = match self.reserve_recurring_dtm_event(staged.event, &staged.now) {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerTxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::Reservation(error),
                    owner: staged.owner,
                });
            }
        };
        Ok(DtmTransmitterRecurringPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved recurring transmitter after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_recurring_staged(
        &mut self,
        staged: DtmTransmitterRecurringStaged,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerTxRecurringPreparationFailure {
        DtmControllerTxRecurringPreparationFailure {
            error: DtmControllerEventPreparationError::ControllerTime(error),
            owner: staged.owner,
        }
    }

    /// Release recurring TX reservation and return the unchanged active owner.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_recurring_pre_sequence(
        &mut self,
        pre_sequence: DtmTransmitterRecurringPreSequence,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerTxRecurringPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_transmitter_recurring_staged(pre_sequence.staged, error)
    }

    /// Authorize recurring TX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_transmitter_recurring_item(
        &mut self,
        pre_sequence: DtmTransmitterRecurringPreSequence,
        sequence_sample: ControllerTimeSample,
    ) -> Result<
        DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase>,
        DtmControllerTxRecurringPreparationFailure,
    > {
        let DtmTransmitterRecurringPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerTxRecurringPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let DtmTransmitterRecurringStaged {
            owner,
            window: next_window,
            event: _,
            now: _,
        } = staged;
        let plan = match DtmReviewedEventWordsPlan::new_transmitter(owner.link_state(), reservation)
        {
            Ok(plan) => plan,
            Err(failure) => {
                self.release_dtm_reservation(failure.into_reservation());
                return Err(DtmControllerTxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: DtmRole::Transmitter,
                        observed: owner.link_state().role(),
                    },
                    owner,
                });
            }
        };
        let prepared = match plan.prepare_recurring(owner, next_window) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (owner, error, plan) = failure.into_retry();
                self.release_dtm_reservation(plan.into_reservation());
                return Err(DtmControllerTxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::Graph(error),
                    owner,
                });
            }
        };
        let item = prepared.prepare_scheduler_bookkeeping();
        match self.prepare_dtm_empty_list_merge(item) {
            Ok(merged) => Ok(merged),
            Err(failure) => {
                let error = failure.error();
                let item = failure.into_item();
                let (owner, reservation) = item.cancel().cancel_recurring();
                self.release_dtm_reservation(reservation);
                Err(DtmControllerTxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::EmptyList(error),
                    owner,
                })
            }
        }
    }

    /// Reject recurring RX before ordered RF/current inputs can form a candidate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reject_dtm_receiver_recurring_before_stage(
        &mut self,
        owner: DtmActiveReceiverCpuOwned,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxRecurringPreparationFailure {
        DtmControllerRxRecurringPreparationFailure {
            error: DtmControllerEventPreparationError::ControllerTime(error),
            owner,
        }
    }

    /// Form one recurring receiver candidate from private fresh timing inputs.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_receiver_recurring_item(
        &self,
        owner: DtmActiveReceiverCpuOwned,
        now: ControllerSchedulerNow,
        timing_ready: crate::AlwaysAwakeTimingReady,
    ) -> Result<DtmReceiverRecurringStaged, DtmControllerRxRecurringPreparationFailure> {
        let current = dtm_scheduler_current(&now);
        let next_window = DtmRxRecurringEventWindow::for_runtime(
            self.config,
            current,
            timing_ready.into_scheduler_instant(),
        );
        let event = match DtmSchedulerItemEvent::new_recurring_receiver(
            owner.channel(),
            owner.phy(),
            next_window,
        ) {
            Ok(event) => event,
            Err(error) => {
                return Err(DtmControllerRxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                });
            }
        };

        Ok(DtmReceiverRecurringStaged {
            owner,
            window: next_window,
            event,
            now,
        })
    }

    /// Reserve the exact recurring RX window before sequence acquisition.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reserve_dtm_receiver_recurring_item(
        &mut self,
        staged: DtmReceiverRecurringStaged,
    ) -> Result<DtmReceiverRecurringPreSequence, DtmControllerRxRecurringPreparationFailure> {
        let reservation = match self.reserve_recurring_dtm_event(staged.event, &staged.now) {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerRxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::Reservation(error),
                    owner: staged.owner,
                });
            }
        };
        Ok(DtmReceiverRecurringPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved recurring receiver after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_recurring_staged(
        &mut self,
        staged: DtmReceiverRecurringStaged,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxRecurringPreparationFailure {
        self.reject_dtm_receiver_recurring_before_stage(staged.owner, error)
    }

    /// Release recurring RX reservation and return the unchanged active owner.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_recurring_pre_sequence(
        &mut self,
        pre_sequence: DtmReceiverRecurringPreSequence,
        error: ControllerTimeAcquisitionError,
    ) -> DtmControllerRxRecurringPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_receiver_recurring_staged(pre_sequence.staged, error)
    }

    /// Authorize recurring RX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_receiver_recurring_item(
        &mut self,
        pre_sequence: DtmReceiverRecurringPreSequence,
        sequence_sample: ControllerTimeSample,
    ) -> Result<
        DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase>,
        DtmControllerRxRecurringPreparationFailure,
    > {
        let DtmReceiverRecurringPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        #[cfg(feature = "dtm-diagnostics")]
        let lead_ticks = reservation
            .window()
            .start()
            .wrapping_sub(sequence_sample.raw_ticks()) as i32;
        let authorized =
            self.finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample));
        #[cfg(feature = "dtm-diagnostics")]
        crate::le::dtm::diagnostics::record_sequence(
            lead_ticks,
            matches!(
                &authorized,
                Err(DtmControllerEventPreparationError::SequenceAuthorization(_))
            ),
        );
        let reservation = match authorized {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(DtmControllerRxRecurringPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let DtmReceiverRecurringStaged {
            owner,
            window: next_window,
            event: _,
            now: _,
        } = staged;
        let plan = match DtmReviewedEventWordsPlan::new_receiver(owner.link_state(), reservation) {
            Ok(plan) => plan,
            Err(failure) => {
                self.release_dtm_reservation(failure.into_reservation());
                return Err(DtmControllerRxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: DtmRole::Receiver,
                        observed: owner.link_state().role(),
                    },
                    owner,
                });
            }
        };
        let prepared = match plan.prepare_recurring(owner, next_window) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (owner, error, plan) = failure.into_retry();
                self.release_dtm_reservation(plan.into_reservation());
                return Err(DtmControllerRxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::Graph(error),
                    owner,
                });
            }
        };
        let item = prepared.prepare_scheduler_bookkeeping();
        match self.prepare_dtm_empty_list_merge(item) {
            Ok(merged) => Ok(merged),
            Err(failure) => {
                let error = failure.error();
                let item = failure.into_item();
                let (owner, reservation) = item.cancel().cancel_recurring();
                self.release_dtm_reservation(reservation);
                Err(DtmControllerRxRecurringPreparationFailure {
                    error: DtmControllerEventPreparationError::EmptyList(error),
                    owner,
                })
            }
        }
    }

    /// Join one prepared DTM item to this epoch's still-empty scheduler list.
    ///
    /// This consumes no hardware permission. The returned state merely proves
    /// that the source-owned list and the item-side empty-list links were
    /// advanced together.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the complete affine CPU-owned DTM item"
    )]
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn prepare_dtm_empty_list_merge<Role, Phase>(
        &mut self,
        item: DtmSchedulerBookkeepingPrepared<Role, Phase>,
    ) -> Result<
        DtmEmptySchedulerMergePrepared<Role, Phase>,
        DtmEmptySchedulerMergeFailure<Role, Phase>,
    >
    where
        Phase: DtmSchedulerItemPhase<Role>,
    {
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(DtmEmptySchedulerMergeFailure { error, item });
        }
        Ok(DtmEmptySchedulerMergePrepared {
            item: item.prepare_empty_list_link(),
        })
    }

    /// Cancel a not-yet-published sole-item merge through the same epoch.
    ///
    /// A state from another or already advanced scheduler is returned
    /// unchanged and cannot reopen this list.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure returns the complete affine merged item"
    )]
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_empty_list_merge<Role, Phase>(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        DtmSchedulerBookkeepingPrepared<Role, Phase>,
        DtmEmptySchedulerMergePrepared<Role, Phase>,
    >
    where
        Phase: DtmSchedulerItemPhase<Role>,
    {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        Ok(merged.item.cancel())
    }

    /// Cancel one not-yet-published TX event and release its private timeline
    /// reservation before returning the graph and complete test program.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub(crate) fn cancel_dtm_transmitter_first_item(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase>,
    ) -> Result<
        (DtmMemoryGraphCpuOwned, DtmPayloadPattern, DtmPayloadLength),
        DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase>,
    > {
        let item = self.cancel_dtm_empty_list_merge(merged)?;
        let pattern = item.packet_pattern();
        let length = item.packet_length();
        let (memory, reservation) = item.cancel().cancel_first();
        self.release_dtm_reservation(reservation);
        Ok((memory, pattern, length))
    }

    /// Cancel one not-yet-published RX event and release its private timeline
    /// reservation before returning the graph/session aggregate.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub(crate) fn cancel_dtm_receiver_first_item(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase>,
    ) -> Result<
        DtmReceiverCpuOwned,
        DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase>,
    > {
        let item = self.cancel_dtm_empty_list_merge(merged)?;
        let (owner, reservation) = item.cancel().cancel_first();
        self.release_dtm_reservation(reservation);
        Ok(owner)
    }

    /// Cancel one not-yet-published recurring TX event and recover the exact
    /// active command owner retained before this candidate was prepared.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub(crate) fn cancel_dtm_transmitter_recurring_item(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase>,
    ) -> Result<
        DtmActiveTransmitterCpuOwned,
        DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmRecurringSchedulerItemPhase>,
    > {
        let item = self.cancel_dtm_empty_list_merge(merged)?;
        let (owner, reservation) = item.cancel().cancel_recurring();
        self.release_dtm_reservation(reservation);
        Ok(owner)
    }

    /// Cancel one not-yet-published recurring RX event and recover the exact
    /// active command owner retained before this candidate was prepared.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure retains the complete affine merged graph"
    )]
    pub(crate) fn cancel_dtm_receiver_recurring_item(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase>,
    ) -> Result<
        DtmActiveReceiverCpuOwned,
        DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmRecurringSchedulerItemPhase>,
    > {
        let item = self.cancel_dtm_empty_list_merge(merged)?;
        let (owner, reservation) = item.cancel().cancel_recurring();
        self.release_dtm_reservation(reservation);
        Ok(owner)
    }

    /// Publish the merge-selected sole item after the complete hardware
    /// initialization chain has made interrupt routes stable but inactive.
    ///
    /// This remains crate-private so an early scheduler state cannot publish a
    /// graph before PHY, BTBB, BLE-PHY and stable interrupt ownership exist.
    #[cfg(target_arch = "riscv32")]
    #[expect(
        clippy::result_large_err,
        reason = "pre-MMIO rejection returns the complete no-alloc affine DTM graph"
    )]
    pub(crate) fn publish_dtm_scheduler_head<Role, Phase>(
        &mut self,
        merged: DtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<DtmSchedulerHeadPublished<Role>, DtmSchedulerHeadPublicationFailure<Role, Phase>>
    where
        Phase: DtmSchedulerItemPhase<Role>,
    {
        let address = merged.scheduler_item_address();
        let publication =
            match self.publish_first_scheduler_item_head(address, merged.hardware_list_index()) {
                Ok(publication) => publication,
                Err(error) => {
                    return Err(DtmSchedulerHeadPublicationFailure { error, merged });
                }
            };
        let item = merged.item.into_head_published(&publication);
        Ok(DtmSchedulerHeadPublished { item, publication })
    }

    /// Stop only the source-owned sole DTM item. No finished-list transfer is
    /// performed until the common stop has completed; it is captured once.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn step_dtm_stop<Role>(
        &mut self,
        storage: &impl crate::controller::SchedulerRunInterruptStorage,
        running: DtmSchedulerRunning<Role>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> DtmSchedulerStopStep<Role> {
        use oer_esp32s31_hal::bluetooth::{
            BluetoothSchedulerFinishedListPop, BluetoothSchedulerStopStep,
        };
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
            || self.runtime.scheduler_finished_lists_mut().is_active()
        {
            return DtmSchedulerStopStep::IdentityMismatch {
                _running: running,
                _stop: stop,
            };
        }
        let stopped = match self.task.step_scheduler_stop(storage, stop) {
            Ok(BluetoothSchedulerStopStep::Stopped(stopped)) => stopped,
            Ok(BluetoothSchedulerStopStep::Pending(stop)) | Err(stop) => {
                return DtmSchedulerStopStep::Pending { running, stop };
            }
        };
        let captured = self
            .task
            .transfer_stopped_scheduler_finished_lists(&stopped);
        let rest = match captured.pop_lowest() {
            BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO => {
                remaining.pop_lowest()
            }
            other => other,
        };
        if !matches!(rest, BluetoothSchedulerFinishedListPop::Complete) {
            return DtmSchedulerStopStep::UnexpectedFinishedList {
                _running: running,
                _stopped: stopped,
                _observed: rest,
            };
        }
        let DtmSchedulerRunning { item, run } = running;
        let stopped = match self.task.retire_stopped_scheduler_head(stopped, run) {
            oer_esp32s31_hal::bluetooth::BluetoothSchedulerStoppedHeadRetirement::Retired(
                stopped,
            ) => stopped,
            oer_esp32s31_hal::bluetooth::BluetoothSchedulerStoppedHeadRetirement::Rejected(
                observed,
            ) => {
                return DtmSchedulerStopStep::HeadRejected {
                    _item: item,
                    _observed: observed,
                };
            }
        };
        let (item, head) = match item.observe_stopped(stopped) {
            Ok(completed) => completed,
            Err((item, stopped)) => {
                return DtmSchedulerStopStep::MemoryRejected {
                    _item: item,
                    _stopped: stopped,
                };
            }
        };
        self._scheduler_list
            .retain_completion_observed_first_item(address);
        self._scheduler_list
            .retain_hardware_head_empty_first_item(address);
        DtmSchedulerStopStep::Retired(DtmSchedulerHardwareHeadRetirementStep::EmptyObserved(
            DtmSchedulerHardwareHeadEmptyObserved { item, head },
        ))
    }

    /// Perform one fresh, bounded DTM completion observation.
    ///
    /// The affine list token never crosses this Controller operation before it
    /// is joined to the matching running epoch. This prevents a caller from
    /// retaining a list-zero token and replaying it against a later DTM event.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_completion<Role>(
        &mut self,
        running: DtmSchedulerRunning<Role>,
        wake: SchedulerWakeBatch,
    ) -> DtmSchedulerCompletionStep<Role> {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return DtmSchedulerCompletionStep::SchedulerIdentityMismatch(running);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerCompletionStep::DrainAlreadyActive(running);
        }

        let capture = self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut(), wake);
        if capture.is_err() {
            return DtmSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } = step
        else {
            return DtmSchedulerCompletionStep::NoFinishedList(running);
        };

        let DtmSchedulerRunning { item, run } = running;
        match item.observe_completion(observed) {
            DtmRunningEventCompletionObservation::ListMismatch { item, observed } => {
                DtmSchedulerCompletionStep::UnrelatedList {
                    drain: SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerRunning { item, run },
                        more,
                    ),
                    observed,
                }
            }
            DtmRunningEventCompletionObservation::StillInFlight(item) => {
                DtmSchedulerCompletionStep::StillInFlight(
                    SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerRunning { item, run },
                        more,
                    ),
                )
            }
            DtmRunningEventCompletionObservation::CompletionObserved(item) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                DtmSchedulerCompletionStep::CompletionObserved(
                    SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerCompletionObserved { item, run },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue one already captured finished-list drain for a running DTM
    /// graph without performing another hardware transfer.
    ///
    /// The caller can reach this edge only with the opaque provenance token
    /// returned by the preceding step. Exactly one list from that same capture
    /// is consumed. This operation never performs a fresh capture.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_dtm_running_finished_list_drain<Role>(
        &mut self,
        pending: SchedulerFinishedListDrainPending<DtmSchedulerRunning<Role>>,
    ) -> DtmSchedulerRunningDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return DtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending);
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerRunningDrainStep::DrainLost(pending);
        }

        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } = step
        else {
            return DtmSchedulerRunningDrainStep::DrainLost(pending);
        };

        let running = pending.into_owner();
        let DtmSchedulerRunning { item, run } = running;
        match item.observe_completion(observed) {
            DtmRunningEventCompletionObservation::ListMismatch { item, observed } => {
                DtmSchedulerRunningDrainStep::UnrelatedList {
                    drain: SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerRunning { item, run },
                        more,
                    ),
                    observed,
                }
            }
            DtmRunningEventCompletionObservation::StillInFlight(item) => {
                DtmSchedulerRunningDrainStep::StillInFlight(
                    SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerRunning { item, run },
                        more,
                    ),
                )
            }
            DtmRunningEventCompletionObservation::CompletionObserved(item) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                DtmSchedulerRunningDrainStep::CompletionObserved(
                    SchedulerFinishedListDrainState::from_worker_step(
                        DtmSchedulerCompletionObserved { item, run },
                        more,
                    ),
                )
            }
        }
    }

    /// Continue the same captured finished-list drain after DTM list zero has
    /// already yielded a non-sentinel completion.
    ///
    /// This operation never captures hardware. Its opaque input proves that
    /// the same capture retained another list. It returns one unrelated list
    /// token and either the ordinary completion owner or a new continuation
    /// token for the same capture.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn continue_dtm_completed_finished_list_drain<Role>(
        &mut self,
        pending: SchedulerFinishedListDrainPending<DtmSchedulerCompletionObserved<Role>>,
    ) -> DtmSchedulerCompletionObservedDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return DtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(pending);
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
        }

        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::scheduler::SchedulerFinishedListWorkerStep::List { observed, more } = step
        else {
            return DtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            DtmSchedulerCompletionObservedDrainStep::RepeatedDtmList {
                drain: SchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        } else {
            DtmSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: SchedulerFinishedListDrainState::from_worker_step(completed, more),
                observed,
            }
        }
    }

    /// Perform one fresh fenced hardware-head retirement observation for the
    /// exact completion retained by this Controller epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_hardware_head_retirement<Role>(
        &mut self,
        completed: DtmSchedulerCompletionObserved<Role>,
    ) -> DtmSchedulerHardwareHeadRetirementStep<Role> {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return DtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(completed);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(completed);
        }

        let DtmSchedulerCompletionObserved { item, run } = completed;
        match self
            .task
            .observe_scheduler_hardware_list_head_retirement(run)
        {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => DtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: DtmSchedulerCompletionObserved { item, run },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => DtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: DtmSchedulerCompletionObserved { item, run },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::EmptyObserved(head) => {
                assert_eq!(
                    head.completed_head().address(),
                    Some(address),
                    "the retired hardware head must retain the exact completed DTM identity"
                );
                self._scheduler_list
                    .retain_hardware_head_empty_first_item(address);
                DtmSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    DtmSchedulerHardwareHeadEmptyObserved { item, head },
                )
            }
        }
    }

    /// Remove the exact empty-head DTM item from the source-owned software
    /// list without recreating the vendor intrusive container.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_dtm_software_list<Role>(
        &mut self,
        observed: DtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> DtmSchedulerSoftwareListUnlinkStep<Role> {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return DtmSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed);
        }

        let DtmSchedulerHardwareHeadEmptyObserved { item, head } = observed;
        DtmSchedulerSoftwareListUnlinkStep::Unlinked(DtmSchedulerSoftwareListUnlinked {
            item,
            head,
        })
    }

    /// Join one freshly serviced primary scheduler event to the exact
    /// already-unlinked DTM item.
    ///
    /// A busy event performs no task-side command read. Any pending result
    /// consumes that event and retains the unlinked graph for a later event.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_dtm_software_list_removal<Role>(
        &mut self,
        unlinked: DtmSchedulerSoftwareListUnlinked<Role>,
        event: crate::interrupt::PrimarySchedulerEvent,
    ) -> DtmSchedulerSoftwareListRemovalJoin<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return DtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }

        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return DtmSchedulerSoftwareListRemovalJoin::Pending(unlinked);
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let DtmSchedulerSoftwareListUnlinked { item, head } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                DtmSchedulerSoftwareListRemovalJoin::Pending(DtmSchedulerSoftwareListUnlinked {
                    item,
                    head,
                })
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                DtmSchedulerSoftwareListRemovalJoin::Ready(DtmSchedulerSoftwareListRemovalReady {
                    item,
                    _removal: removal,
                })
            }
        }
    }

    /// Recheck one already-unlinked DTM graph without requiring another
    /// primary interrupt edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_dtm_software_list_removal<Role>(
        &mut self,
        storage: &impl crate::controller::SchedulerRunInterruptStorage,
        unlinked: DtmSchedulerSoftwareListUnlinked<Role>,
    ) -> DtmSchedulerSoftwareListRemovalRecheck<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return DtmSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked);
        }

        let DtmSchedulerSoftwareListUnlinked { item, head } = unlinked;
        let join = match self
            .task
            .recheck_scheduler_software_list_removal(storage, head)
        {
            Ok(join) => join,
            Err(head) => {
                return DtmSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                    DtmSchedulerSoftwareListUnlinked { item, head },
                );
            }
        };
        match join {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                DtmSchedulerSoftwareListRemovalRecheck::Pending(DtmSchedulerSoftwareListUnlinked {
                    item,
                    head,
                })
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                DtmSchedulerSoftwareListRemovalRecheck::Ready(
                    DtmSchedulerSoftwareListRemovalReady {
                        item,
                        _removal: removal,
                    },
                )
            }
        }
    }

    /// Return one removal-ready DTM graph to source-owned CPU state.
    ///
    /// TX and RX non-success outcomes may recycle directly. RX success is
    /// retained for the specialized drain/account/re-arm transaction. The
    /// timeline reservation and reviewed descriptor links are released in one
    /// bounded transaction.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_dtm_completed<Role>(
        &mut self,
        ready: DtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> DtmSchedulerRecycleStep<Role> {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return DtmSchedulerRecycleStep::SchedulerIdentityMismatch(ready);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerRecycleStep::FinishedListDrainStillActive(ready);
        }
        if ready.role() == DtmRole::Receiver
            && ready.status() == DtmSchedulerItemCompletionStatus::Zero
        {
            return DtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(ready);
        }

        let DtmSchedulerSoftwareListRemovalReady { item, _removal } = ready;
        match item.recycle(self.runtime.scheduler_timeline_mut(), _removal) {
            Ok(timeline_released) => {
                self._scheduler_list.commit_recycled_first_item();
                DtmSchedulerRecycleStep::Recycled(timeline_released.finish_source_list_release())
            }
            Err(failure) => {
                let (error, item, removal) = failure.into_parts();
                let ready = DtmSchedulerSoftwareListRemovalReady {
                    item,
                    _removal: removal,
                };
                match error {
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::MemoryIdentity(
                        error,
                    ) => DtmSchedulerRecycleStep::MemoryIdentityMismatch {
                        ready,
                        error,
                    },
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::ReservationIdentityMismatch => {
                        DtmSchedulerRecycleStep::ReservationIdentityMismatch(ready)
                    }
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::ReceiverSuccessMemory(_) => {
                        unreachable!("generic recycle cannot enter the RX-success preflight")
                    }
                }
            }
        }
    }

    /// Drain, account and re-arm one successful removal-ready RX event.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recycle_dtm_receiver_success(
        &mut self,
        ready: DtmSchedulerSoftwareListRemovalReady<crate::le::dtm::DtmReceiverEvent>,
    ) -> DtmSchedulerRxSuccessRecycleStep {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return DtmSchedulerRxSuccessRecycleStep::SchedulerIdentityMismatch(ready);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return DtmSchedulerRxSuccessRecycleStep::FinishedListDrainStillActive(ready);
        }
        if ready.status() != DtmSchedulerItemCompletionStatus::Zero {
            return DtmSchedulerRxSuccessRecycleStep::CompletionStatusMismatch(ready);
        }

        let DtmSchedulerSoftwareListRemovalReady { item, _removal } = ready;
        match item.recycle_receiver_success(self.runtime.scheduler_timeline_mut(), _removal) {
            Ok(timeline_released) => {
                self._scheduler_list.commit_recycled_first_item();
                DtmSchedulerRxSuccessRecycleStep::Rearmed(
                    timeline_released.finish_source_list_release(),
                )
            }
            Err(failure) => {
                let (error, item, removal) = failure.into_parts();
                let ready = DtmSchedulerSoftwareListRemovalReady {
                    item,
                    _removal: removal,
                };
                match error {
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::MemoryIdentity(
                        error,
                    ) => DtmSchedulerRxSuccessRecycleStep::MemoryIdentityMismatch {
                        ready,
                        error,
                    },
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::ReceiverSuccessMemory(
                        error,
                    ) => DtmSchedulerRxSuccessRecycleStep::ReturnedTopologyRejected {
                        ready,
                        error,
                    },
                    crate::le::dtm::event::prepare::DtmCompletionRecycleError::ReservationIdentityMismatch => {
                        DtmSchedulerRxSuccessRecycleStep::ReservationIdentityMismatch(ready)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
