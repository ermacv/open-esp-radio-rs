//! Fact-bounded scheduler initialization after the controller HAL component.

use crate::BluetoothSchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
use crate::dtm_event_prepare::{
    BluetoothDtmCompletionObservedEvent, BluetoothDtmReviewedEventWordsPlan,
    BluetoothDtmRunningEventCompletionObservation, BluetoothDtmSchedulerBookkeepingPrepared,
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler_timeline::{
    BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerRecurringReserved,
    BluetoothSchedulerSequenceAuthorizationFailure,
};
use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerPoweredTaskRuntime,
    BluetoothControllerRuntimeResources, BluetoothDtmRole,
    controller_hal::BluetoothControllerHalInitialized,
    dtm_event_prepare::{
        BluetoothDtmEmptyListLinkPrepared, BluetoothDtmHeadPublishedEvent,
        BluetoothDtmRunningEvent, BluetoothDtmSchedulerItemPhase,
    },
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
    },
};
#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    BluetoothControllerTimeSample, BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerItemEvent,
    BluetoothDtmSchedulerTimingPolicy, BluetoothSchedulerReservation,
    BluetoothSchedulerReservationError, BluetoothSchedulerSequenceAuthorizationError,
    BluetoothSchedulerSequenceReady, controller_time::BluetoothControllerSchedulerNow,
};
#[cfg(target_arch = "riscv32")]
use crate::{
    BluetoothDtmActiveReceiverCpuOwned, BluetoothDtmActiveTransmitterCpuOwned, BluetoothDtmChannel,
    BluetoothDtmLinkStateReset, BluetoothDtmPayloadLength, BluetoothDtmPayloadPattern,
    BluetoothDtmPhy, BluetoothDtmPreparedTxGraph, BluetoothDtmReceiverCpuOwned,
    BluetoothDtmReceiverEvent, BluetoothDtmRxInitialEventWindow,
    BluetoothDtmRxRecurringEventWindow, BluetoothDtmSchedulerItemEventError,
    BluetoothDtmSchedulerMargin, BluetoothDtmTransmitterEvent, BluetoothDtmTxEventWindow,
    BluetoothDtmTxSchedulerTiming, BluetoothDtmTxTimingMicros,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmMemoryGraphCpuOwned, BluetoothDtmMemoryGraphPrepareError,
    BluetoothDtmSchedulerItemCompletionStatus,
};
use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHeadError,
    BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved,
    BluetoothSchedulerHardwareListHeadRetirementObservation,
    BluetoothSchedulerSoftwareListRemovalInterruptStep, BluetoothSchedulerSoftwareListRemovalJoin,
    BluetoothSchedulerSoftwareListRemovalReady,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

/// Exclusive empty scheduler-list epoch owned by the source controller.
///
/// The PAC proof establishes that no hardware-list head remains published.
/// This owner adds the independently constructed source-owned software list,
/// which starts empty and cannot be aliased through a vendor container.
struct BluetoothSchedulerExclusiveListEpoch {
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
    ) -> Result<(), BluetoothDtmEmptySchedulerMergeError> {
        if self.state != BluetoothSchedulerExclusiveListState::Empty {
            return Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty);
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

    fn retain_running_first_item(&mut self, address: BluetoothControllerSramAddress) {
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

/// Why the first DTM item could not consume the exclusive empty-list epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmEmptySchedulerMergeError {
    /// Another item already consumed this scheduler epoch's empty-list proof.
    ListNotEmpty,
}

/// Why one private DTM controller-time phase could not complete.
///
/// RF-ready, recurring-RX current, admission and sequence samples are acquired
/// by the Controller and never cross the public DTM preparation boundary. This
/// finite error retains only the logical acquisition outcome; the role-specific
/// preparation failure continues to own every retry resource.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmControllerTimeAcquisitionError {
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

/// Why the terminal Controller could not prepare one CPU-owned DTM event.
///
/// Reservation and sequence failures are rolled back against the same private
/// timeline before this error is returned. No variant leaks a timeline token
/// or leaves a hidden occupied slot behind.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmControllerEventPreparationError {
    /// The caller supplied a link-state reset for the other DTM role.
    LinkStateRoleMismatch {
        /// Role selected by this exact Controller operation.
        expected: BluetoothDtmRole,
        /// Role retained by the supplied reset.
        observed: BluetoothDtmRole,
    },
    /// The channel, PHY and role cannot form a reviewed scheduler item.
    #[cfg(target_arch = "riscv32")]
    SchedulerItem(BluetoothDtmSchedulerItemEventError),
    /// The Controller-owned bounded timeline rejected the requested window.
    Reservation(BluetoothSchedulerReservationError),
    /// The phase-appropriate fresh Controller-time sample closed the sequence gate.
    SequenceAuthorization(BluetoothSchedulerSequenceAuthorizationError),
    /// A private RF-ready, current, admission or sequence acquisition failed.
    ControllerTime(BluetoothDtmControllerTimeAcquisitionError),
    /// The bound SRAM graph rejected the positional event transaction.
    #[cfg(target_arch = "riscv32")]
    Graph(BluetoothDtmMemoryGraphPrepareError),
    /// The exclusive source-owned scheduler list still retains an item.
    EmptyList(BluetoothDtmEmptySchedulerMergeError),
}

/// Lossless terminal-Controller TX preparation rejection.
///
/// Packet readiness is deliberately reduced to ordinary CPU ownership on a
/// failed composed transaction. Pattern and length are retained so callers can
/// deterministically rebuild that proof before retrying.
#[cfg(target_arch = "riscv32")]
#[must_use = "the returned TX graph and complete DTM program remain owned"]
pub struct BluetoothDtmControllerTxPreparationFailure {
    error: BluetoothDtmControllerEventPreparationError,
    memory: BluetoothDtmMemoryGraphCpuOwned,
    pattern: BluetoothDtmPayloadPattern,
    length: BluetoothDtmPayloadLength,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmControllerTxPreparationFailure {
    fn from_prepared(
        error: BluetoothDtmControllerEventPreparationError,
        owner: BluetoothDtmPreparedTxGraph,
    ) -> Self {
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
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Recover the graph and complete TX program for deterministic retry.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmMemoryGraphCpuOwned,
        BluetoothDtmPayloadPattern,
        BluetoothDtmPayloadLength,
    ) {
        (self.memory, self.pattern, self.length)
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for BluetoothDtmControllerTxPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmControllerTxPreparationFailure")
            .field("error", &self.error)
            .field("pattern", &self.pattern)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller RX preparation rejection.
#[cfg(target_arch = "riscv32")]
#[must_use = "the returned RX graph and non-copyable session remain owned"]
pub struct BluetoothDtmControllerRxPreparationFailure {
    error: BluetoothDtmControllerEventPreparationError,
    owner: BluetoothDtmReceiverCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmControllerRxPreparationFailure {
    /// Exact finite reason no first-item merge was returned.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged graph/session aggregate for retry or Test End.
    pub fn into_owner(self) -> BluetoothDtmReceiverCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for BluetoothDtmControllerRxPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmControllerRxPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller rejection of one recurring TX event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active transmitter and its committed phase remain owned"]
pub struct BluetoothDtmControllerTxRecurringPreparationFailure {
    error: BluetoothDtmControllerEventPreparationError,
    owner: BluetoothDtmActiveTransmitterCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmControllerTxRecurringPreparationFailure {
    /// Exact finite reason no recurring TX merge was returned.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged active transmitter for retry or Test End.
    pub fn into_owner(self) -> BluetoothDtmActiveTransmitterCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for BluetoothDtmControllerTxRecurringPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmControllerTxRecurringPreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Lossless terminal-Controller rejection of one recurring RX event.
#[cfg(target_arch = "riscv32")]
#[must_use = "the active receiver and its committed phase remain owned"]
pub struct BluetoothDtmControllerRxRecurringPreparationFailure {
    error: BluetoothDtmControllerEventPreparationError,
    owner: BluetoothDtmActiveReceiverCpuOwned,
}

#[cfg(target_arch = "riscv32")]
impl BluetoothDtmControllerRxRecurringPreparationFailure {
    /// Exact finite reason no recurring RX merge was returned.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Recover the unchanged active receiver for retry or Test End.
    pub fn into_owner(self) -> BluetoothDtmActiveReceiverCpuOwned {
        self.owner
    }
}

#[cfg(target_arch = "riscv32")]
impl core::fmt::Debug for BluetoothDtmControllerRxRecurringPreparationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDtmControllerRxRecurringPreparationFailure")
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
pub(crate) struct BluetoothDtmTransmitterFirstStaged {
    owner: BluetoothDtmPreparedTxGraph,
    link_state: BluetoothDtmLinkStateReset,
    channel: BluetoothDtmChannel,
    phy: BluetoothDtmPhy,
    timing: BluetoothDtmTxSchedulerTiming,
    margin: BluetoothDtmSchedulerMargin,
    window: BluetoothDtmTxEventWindow,
    event: BluetoothDtmSchedulerItemEvent,
    now: BluetoothControllerSchedulerNow,
}

/// Initial receiver candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged receiver must enter admission or be cancelled"]
pub(crate) struct BluetoothDtmReceiverFirstStaged {
    owner: BluetoothDtmReceiverCpuOwned,
    link_state: BluetoothDtmLinkStateReset,
    channel: BluetoothDtmChannel,
    phy: BluetoothDtmPhy,
    margin: BluetoothDtmSchedulerMargin,
    window: BluetoothDtmRxInitialEventWindow,
    event: BluetoothDtmSchedulerItemEvent,
    now: BluetoothControllerSchedulerNow,
}

/// Recurring transmitter candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged transmitter must reserve or be cancelled"]
pub(crate) struct BluetoothDtmTransmitterRecurringStaged {
    owner: BluetoothDtmActiveTransmitterCpuOwned,
    window: BluetoothDtmTxEventWindow,
    event: BluetoothDtmSchedulerItemEvent,
    now: BluetoothControllerSchedulerNow,
}

/// Recurring receiver candidate formed from one private scheduler current.
#[cfg(target_arch = "riscv32")]
#[must_use = "the staged receiver must reserve or be cancelled"]
pub(crate) struct BluetoothDtmReceiverRecurringStaged {
    owner: BluetoothDtmActiveReceiverCpuOwned,
    window: BluetoothDtmRxRecurringEventWindow,
    event: BluetoothDtmSchedulerItemEvent,
    now: BluetoothControllerSchedulerNow,
}

/// Initial transmitter candidate after admission and overlap resolution.
#[cfg(target_arch = "riscv32")]
#[must_use = "the admitted transmitter must consume a later sequence sample or be cancelled"]
pub(crate) struct BluetoothDtmTransmitterFirstPreSequence {
    staged: BluetoothDtmTransmitterFirstStaged,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerInitialAdmissionResolved>,
}

/// Initial receiver candidate after admission and overlap resolution.
#[cfg(target_arch = "riscv32")]
#[must_use = "the admitted receiver must consume a later sequence sample or be cancelled"]
pub(crate) struct BluetoothDtmReceiverFirstPreSequence {
    staged: BluetoothDtmReceiverFirstStaged,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerInitialAdmissionResolved>,
}

/// Recurring transmitter candidate after its exact-window reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the reserved transmitter must consume a later sequence sample or be cancelled"]
pub(crate) struct BluetoothDtmTransmitterRecurringPreSequence {
    staged: BluetoothDtmTransmitterRecurringStaged,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerRecurringReserved>,
}

/// Recurring receiver candidate after its exact-window reservation.
#[cfg(target_arch = "riscv32")]
#[must_use = "the reserved receiver must consume a later sequence sample or be cancelled"]
pub(crate) struct BluetoothDtmReceiverRecurringPreSequence {
    staged: BluetoothDtmReceiverRecurringStaged,
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerRecurringReserved>,
}

/// Lossless failure to join a DTM item to the exclusive empty scheduler list.
#[must_use = "the unchanged scheduler-prepared item remains CPU-owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct BluetoothDtmEmptySchedulerMergeFailure<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    error: BluetoothDtmEmptySchedulerMergeError,
    item: BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase>,
}

#[cfg(target_arch = "riscv32")]
impl<Role, Phase> BluetoothDtmEmptySchedulerMergeFailure<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    /// Exact reason the list owner rejected this item.
    pub(crate) const fn error(&self) -> BluetoothDtmEmptySchedulerMergeError {
        self.error
    }

    /// Recover the unchanged CPU-owned item for retry or cancellation.
    pub(crate) fn into_item(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase> {
        self.item
    }
}

/// Sole DTM item joined to one exclusive, previously empty scheduler epoch.
///
/// The item-side descriptor transform and source-owned list state now agree on
/// one exact identity. This remains CPU-owned: no visibility fence, hardware
/// head, RUN command or radio-completion authority has been granted.
#[must_use = "the merged item must be published through the same scheduler or cancelled"]
pub struct BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    item: BluetoothDtmEmptyListLinkPrepared<Role, Phase>,
}

impl<Role, Phase> BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    /// Role retained by the exact prepared graph.
    pub const fn role(&self) -> BluetoothDtmRole {
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
pub struct BluetoothDtmInitialSchedulerItemPhase {
    _private: (),
}

/// Type-level proof that a merged DTM item belongs to an active command.
///
/// A recurring merge can only cancel back into its exact active owner; it
/// cannot cross the first-event recovery edge.
pub struct BluetoothDtmRecurringSchedulerItemPhase {
    _private: (),
}

/// Why a prepared first-item merge could not publish its scheduler head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerHeadPublicationError {
    /// The merge belongs to another scheduler epoch or list identity.
    SchedulerIdentityMismatch,
    /// The selected address aliases the reserved empty hardware-head image.
    EncodesEmptyHead,
}

impl From<BluetoothSchedulerHardwareListHeadError> for BluetoothDtmSchedulerHeadPublicationError {
    fn from(error: BluetoothSchedulerHardwareListHeadError) -> Self {
        match error {
            BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead => Self::EncodesEmptyHead,
        }
    }
}

/// Lossless rejection before scheduler-head MMIO publication.
#[must_use = "the unchanged CPU-owned merge can still be retried or cancelled"]
pub struct BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    error: BluetoothDtmSchedulerHeadPublicationError,
    merged: BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
}

impl<Role, Phase> BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>
where
    Phase: BluetoothDtmSchedulerItemPhase<Role>,
{
    /// Exact reason no scheduler head was published.
    pub const fn error(&self) -> BluetoothDtmSchedulerHeadPublicationError {
        self.error
    }

    /// Recover the unchanged CPU-owned merge.
    pub fn into_merged(self) -> BluetoothDtmEmptySchedulerMergePrepared<Role, Phase> {
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
pub struct BluetoothDtmSchedulerHeadPublished<Role> {
    item: BluetoothDtmHeadPublishedEvent<Role>,
    publication: BluetoothSchedulerHardwareListHeadPublished,
}

impl<Role> BluetoothDtmSchedulerHeadPublished<Role> {
    /// Role retained by the now hardware-visible graph.
    pub const fn role(&self) -> BluetoothDtmRole {
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
        BluetoothDtmHeadPublishedEvent<Role>,
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
pub struct BluetoothDtmSchedulerRunning<Role> {
    item: BluetoothDtmRunningEvent<Role>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

impl<Role> BluetoothDtmSchedulerRunning<Role> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn new(
        item: BluetoothDtmHeadPublishedEvent<Role>,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> Self {
        let item = item.into_running(&run);
        Self { item, run }
    }

    /// Role retained by the running scheduler graph.
    pub const fn role(&self) -> BluetoothDtmRole {
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

/// DTM graph with a non-sentinel status observed after a fresh fenced transfer.
///
/// The source-owned scheduler epoch remains occupied and the graph remains
/// controller-owned. This state does not expose packet memory, cancellation or
/// reclamation before the hardware/software unlink path is completed.
#[must_use = "the completion-observed graph must advance through unlink and recycle"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmSchedulerCompletionObserved<Role> {
    item: BluetoothDtmCompletionObservedEvent<Role>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmSchedulerCompletionObserved<Role> {
    /// Role retained by the completed scheduler item.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.item.role()
    }

    /// Exact scheduler-item identity whose status was observed.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic non-sentinel completion status.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
        self.item.status()
    }

    /// Hardware list retained through RUN and completion observation.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.run.index()
    }
}

/// Affine proof that one specific captured finished-list set remains pending.
///
/// This token is created only by a bounded drain step which consumed one list
/// and observed that the same capture retained another list. It cannot be
/// constructed, copied or detached from the DTM graph owner it protects.
#[must_use = "the retained finished-list capture must be continued or preserved"]
#[cfg(any(test, target_arch = "riscv32"))]
pub struct BluetoothDtmSchedulerFinishedListDrainPending<Owner> {
    owner: Owner,
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> BluetoothDtmSchedulerFinishedListDrainPending<Owner> {
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
#[must_use = "the DTM graph and any pending finished-list capture must be retained"]
#[cfg(any(test, target_arch = "riscv32"))]
pub enum BluetoothDtmSchedulerFinishedListDrainState<Owner> {
    /// The captured set is exhausted; no continuation is permitted.
    Drained(Owner),
    /// The same captured set retains another list.
    Pending(BluetoothDtmSchedulerFinishedListDrainPending<Owner>),
}

#[cfg(any(test, target_arch = "riscv32"))]
impl<Owner> BluetoothDtmSchedulerFinishedListDrainState<Owner> {
    fn from_worker_step(owner: Owner, more: bool) -> Self {
        if more {
            Self::Pending(BluetoothDtmSchedulerFinishedListDrainPending::new(owner))
        } else {
            Self::Drained(owner)
        }
    }
}

/// One bounded Controller-owned DTM completion attempt.
#[must_use = "the returned DTM graph and any unrelated affine list must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSchedulerCompletionStep<Role> {
    /// A generic finished-list drain is already active; no new transfer ran.
    DrainAlreadyActive(BluetoothDtmSchedulerRunning<Role>),
    /// The supplied running graph does not belong to this Controller epoch.
    SchedulerIdentityMismatch(BluetoothDtmSchedulerRunning<Role>),
    /// The fresh transfer contained no finished hardware list.
    NoFinishedList(BluetoothDtmSchedulerRunning<Role>),
    /// The selected list is not DTM list zero and remains available to dispatch.
    UnrelatedList {
        /// Running graph, paired with continuation provenance only when needed.
        drain: BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// DTM list zero was reported but its status remains the in-flight sentinel.
    StillInFlight(BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>),
    /// One non-sentinel status was observed without returning CPU ownership.
    CompletionObserved(
        BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerCompletionObserved<Role>>,
    ),
}

/// One bounded continuation of an already captured finished-list drain while
/// the DTM graph is still running.
///
/// This operation never captures a new hardware observation. It consumes at
/// most one list from the Controller's retained drain and returns every affine
/// owner, including an unrelated list observation, to the caller.
#[must_use = "the running graph and every drained list observation must be retained"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSchedulerRunningDrainStep<Role> {
    /// The supplied running graph does not belong to this Controller epoch.
    SchedulerIdentityMismatch(
        BluetoothDtmSchedulerFinishedListDrainPending<BluetoothDtmSchedulerRunning<Role>>,
    ),
    /// The capture identified by the token is no longer active. The token and
    /// graph owner are retained losslessly for fail-stop handling.
    DrainLost(BluetoothDtmSchedulerFinishedListDrainPending<BluetoothDtmSchedulerRunning<Role>>),
    /// One non-DTM list remains available to its owning dispatcher.
    UnrelatedList {
        /// Running graph, paired with continuation provenance only when needed.
        drain: BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// DTM list zero was selected but its item remains in flight.
    StillInFlight(BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerRunning<Role>>),
    /// DTM completion was observed while the same captured transfer may still
    /// retain unrelated lists.
    CompletionObserved(
        BluetoothDtmSchedulerFinishedListDrainState<BluetoothDtmSchedulerCompletionObserved<Role>>,
    ),
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
pub enum BluetoothDtmSchedulerCompletionObservedDrainStep<Role> {
    /// The supplied completion does not belong to this Controller epoch.
    SchedulerIdentityMismatch(
        BluetoothDtmSchedulerFinishedListDrainPending<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    ),
    /// The capture identified by the token is no longer active. The token and
    /// completion owner are retained losslessly for fail-stop handling.
    DrainLost(
        BluetoothDtmSchedulerFinishedListDrainPending<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    ),
    /// One unrelated list remains available to its owning dispatcher.
    UnrelatedList {
        /// Completion owner, paired with continuation provenance only when needed.
        drain: BluetoothDtmSchedulerFinishedListDrainState<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    /// The supposedly remaining set repeated DTM list zero. Both owners are
    /// retained for fail-stop handling.
    RepeatedDtmList {
        /// Completion owner, paired with continuation provenance only when needed.
        drain: BluetoothDtmSchedulerFinishedListDrainState<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
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
pub struct BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role> {
    item: BluetoothDtmCompletionObservedEvent<Role>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role> {
    /// Role retained by the completed event.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.item.role()
    }

    /// Exact scheduler item whose hardware head became empty.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained through the head observation.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
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
pub struct BluetoothDtmSchedulerSoftwareListUnlinked<Role> {
    item: BluetoothDtmCompletionObservedEvent<Role>,
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmSchedulerSoftwareListUnlinked<Role> {
    /// Role retained by the already-unlinked event.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.item.role()
    }

    /// Exact scheduler item removed from the source-owned software list.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained while awaiting the return gate.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
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
pub enum BluetoothDtmSchedulerSoftwareListUnlinkStep<Role> {
    /// The supplied empty-head graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
    /// The sole source-owned list item was removed exactly once.
    Unlinked(BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
}

/// DTM graph after the post-unlink scheduler return predicate became ready.
///
/// This state proves only the reviewed `idle + command statuses` predicate for
/// the exact already-unlinked item. It does not return CPU ownership, recycle
/// memory, or release the scheduler timeline reservation.
#[must_use = "the removal-ready graph must advance through recycle"]
#[cfg(target_arch = "riscv32")]
pub struct BluetoothDtmSchedulerSoftwareListRemovalReady<Role> {
    item: BluetoothDtmCompletionObservedEvent<Role>,
    _removal: BluetoothSchedulerSoftwareListRemovalReady,
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothDtmSchedulerSoftwareListRemovalReady<Role> {
    /// Role retained by the removal-ready event.
    pub const fn role(&self) -> BluetoothDtmRole {
        self.item.role()
    }

    /// Exact scheduler item retained through the removal return gate.
    pub const fn scheduler_item_address(&self) -> BluetoothControllerSramAddress {
        self.item.scheduler_item_address()
    }

    /// Semantic completion status retained through the removal return gate.
    pub const fn status(&self) -> BluetoothDtmSchedulerItemCompletionStatus {
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
pub enum BluetoothDtmSchedulerRecycleStep<Role> {
    /// The supplied graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
    /// A prior finished-list transfer still retains an unhandled list.
    FinishedListDrainStillActive(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
    /// The lower memory graph rejected the retained typed head identity.
    MemoryIdentityMismatch {
        /// Unchanged removal-ready graph.
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
        /// Exact lower identity mismatch.
        error: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphRecycleError,
    },
    /// The affine reservation does not belong to this Controller timeline.
    ReservationIdentityMismatch(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
    /// Successful RX must enter the role-specific drain/account/re-arm path.
    ReceiverSuccessRequiresSpecializedRecycle(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
    /// Memory and timeline ownership returned to the source role.
    Recycled(crate::BluetoothDtmRecycledEvent<Role>),
}

/// Result of the role-specific successful RX recycle/re-arm transaction.
#[must_use = "failure retains the exact RX owner; success retains the re-armed session"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSchedulerRxSuccessRecycleStep {
    /// The supplied graph belongs to another scheduler epoch.
    SchedulerIdentityMismatch(
        BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
    ),
    /// A prior finished-list transfer still retains an unhandled list.
    FinishedListDrainStillActive(
        BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
    ),
    /// The typed receiver owner does not carry a successful scheduler status.
    CompletionStatusMismatch(
        BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
    ),
    /// The lower graph rejected the retained hardware/removal identity.
    MemoryIdentityMismatch {
        /// Unchanged removal-ready receiver graph.
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
        /// Exact lower identity mismatch.
        error: open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphRecycleError,
    },
    /// The returned RX chain failed the lossless two-slot preflight.
    ReturnedTopologyRejected {
        /// Unchanged removal-ready receiver graph.
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
        /// Semantic lower topology or sentinel failure.
        error:
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmMemoryGraphRxSuccessRecycleError,
    },
    /// The retained reservation belongs to another timeline epoch.
    ReservationIdentityMismatch(
        BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
    ),
    /// RX memory, timeline and source-list ownership returned in re-armed form.
    Rearmed(crate::BluetoothDtmRxRearmedEvent),
}

/// Internal result of joining one primary scheduler event to an already
/// unlinked DTM graph.
///
/// The outer sealed consumer remains responsible for admitting only an event
/// whose publication followed this exact unlink.
#[must_use = "the unlinked or removal-ready graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmSchedulerSoftwareListRemovalJoin<Role> {
    SchedulerIdentityMismatch {
        unlinked: BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        event: crate::BluetoothPrimarySchedulerEvent,
    },
    Pending(BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
    Ready(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
}

/// One bounded post-completion hardware-head retirement attempt.
#[must_use = "the completion owner must enter fail-stop handling or advance"]
#[cfg(target_arch = "riscv32")]
pub enum BluetoothDtmSchedulerHardwareHeadRetirementStep<Role> {
    /// The supplied completion does not belong to this scheduler epoch.
    SchedulerIdentityMismatch(BluetoothDtmSchedulerCompletionObserved<Role>),
    /// The captured transfer still retains another impossible list bit.
    FinishedListDrainStillActive(BluetoothDtmSchedulerCompletionObserved<Role>),
    /// The expected head remains nonempty; the sole-item invariant failed.
    ExpectedHeadStillPublished {
        /// Completion owner retained for fail-stop handling.
        completed: BluetoothDtmSchedulerCompletionObserved<Role>,
        /// Fresh typed head retained for diagnostics without granting access.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// A different nonempty head appeared in the exclusive list; fail closed.
    UnexpectedHeadChanged {
        /// Completion owner retained without granting descriptor access.
        completed: BluetoothDtmSchedulerCompletionObserved<Role>,
        /// Fresh conflicting head identity.
        observed: BluetoothSchedulerHardwareListHead,
    },
    /// The exact list head was freshly observed empty after a device fence.
    EmptyObserved(BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>),
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
/// PHY, BTBB, IRQ, Controller or Link-Layer readiness. The next consuming
/// transition may bind a pristine HCI bootstrap epoch, but still establishes no
/// operational HCI or radio capability. Dropping this state is fail-stop because
/// no verified rollback exists after scheduler MMIO mutation.
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
    _standalone_always_awake: crate::controller_hal::BluetoothStandaloneAlwaysAwake,
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

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn request_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::BluetoothControllerTimeRequest,
        crate::controller_time::BluetoothControllerTimeRequestError,
    > {
        self._standalone_always_awake.gate_controller_time_request();
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

    #[cfg(any(target_arch = "riscv32", test))]
    fn admit_initial_dtm_event(
        &mut self,
        event: BluetoothDtmSchedulerItemEvent,
        now: &BluetoothControllerSchedulerNow,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothSchedulerReservation<BluetoothSchedulerInitialAdmissionResolved>,
        BluetoothSchedulerReservationError,
    > {
        let timing_policy =
            BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        self.runtime
            .scheduler_timeline_mut()
            .reserve_initial_dtm_event(event, now.epoch(), timing_policy, admission_sample)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn reserve_recurring_dtm_event(
        &mut self,
        event: BluetoothDtmSchedulerItemEvent,
        now: &BluetoothControllerSchedulerNow,
    ) -> Result<
        BluetoothSchedulerReservation<BluetoothSchedulerRecurringReserved>,
        BluetoothSchedulerReservationError,
    > {
        let timing_policy =
            BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(self.config, self.time_scale);
        self.runtime
            .scheduler_timeline_mut()
            .reserve_recurring_dtm_event(event, now.epoch(), timing_policy)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn finish_dtm_sequence_authorization<State>(
        &mut self,
        result: Result<
            BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
            BluetoothSchedulerSequenceAuthorizationFailure<State>,
        >,
    ) -> Result<
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
        BluetoothDtmControllerEventPreparationError,
    > {
        match result {
            Ok(reservation) => Ok(reservation),
            Err(failure) => {
                let error = failure.error();
                let reservation = failure.into_reservation();
                self.release_dtm_reservation(reservation);
                Err(BluetoothDtmControllerEventPreparationError::SequenceAuthorization(error))
            }
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn release_dtm_reservation<State>(
        &mut self,
        reservation: BluetoothSchedulerReservation<State>,
    ) {
        self.runtime
            .scheduler_timeline_mut()
            .release(reservation)
            .expect("a reservation created by this Controller must release into the same timeline");
    }

    /// Reject initial TX before RF readiness can form a scheduler candidate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reject_dtm_transmitter_first_before_stage(
        &mut self,
        owner: BluetoothDtmPreparedTxGraph,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerTxPreparationFailure {
        BluetoothDtmControllerTxPreparationFailure::from_prepared(
            BluetoothDtmControllerEventPreparationError::ControllerTime(error),
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
        owner: BluetoothDtmPreparedTxGraph,
        link_state: BluetoothDtmLinkStateReset,
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        requested_interval_micros: u16,
        now: BluetoothControllerSchedulerNow,
        rf_ready: crate::BluetoothDtmRfReady,
    ) -> Result<BluetoothDtmTransmitterFirstStaged, BluetoothDtmControllerTxPreparationFailure>
    {
        if link_state.role() != BluetoothDtmRole::Transmitter {
            return Err(BluetoothDtmControllerTxPreparationFailure::from_prepared(
                BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                    expected: BluetoothDtmRole::Transmitter,
                    observed: link_state.role(),
                },
                owner,
            ));
        }
        let timing =
            BluetoothDtmTxTimingMicros::new(owner.length(), phy, requested_interval_micros)
                .scheduler_timing();
        let margin = self.config.dtm_scheduler_margin();
        let current = dtm_scheduler_current(&now);
        let window =
            timing.initial_event_window(self.config, current, rf_ready.into_scheduler_instant());
        let event = match BluetoothDtmSchedulerItemEvent::new_transmitter(channel, phy, window) {
            Ok(event) => event,
            Err(error) => {
                return Err(BluetoothDtmControllerTxPreparationFailure::from_prepared(
                    BluetoothDtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                ));
            }
        };

        Ok(BluetoothDtmTransmitterFirstStaged {
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
        staged: BluetoothDtmTransmitterFirstStaged,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<BluetoothDtmTransmitterFirstPreSequence, BluetoothDtmControllerTxPreparationFailure>
    {
        let reservation =
            match self.admit_initial_dtm_event(staged.event, &staged.now, admission_sample) {
                Ok(reservation) => reservation,
                Err(error) => {
                    return Err(BluetoothDtmControllerTxPreparationFailure::from_prepared(
                        BluetoothDtmControllerEventPreparationError::Reservation(error),
                        staged.owner,
                    ));
                }
            };
        Ok(BluetoothDtmTransmitterFirstPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved initial transmitter after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_first_staged(
        &mut self,
        staged: BluetoothDtmTransmitterFirstStaged,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerTxPreparationFailure {
        self.reject_dtm_transmitter_first_before_stage(staged.owner, error)
    }

    /// Release initial TX admission and return every retry resource.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_first_pre_sequence(
        &mut self,
        pre_sequence: BluetoothDtmTransmitterFirstPreSequence,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerTxPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_transmitter_first_staged(pre_sequence.staged, error)
    }

    /// Authorize initial TX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_transmitter_first_item(
        &mut self,
        pre_sequence: BluetoothDtmTransmitterFirstPreSequence,
        sequence_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
        BluetoothDtmControllerTxPreparationFailure,
    > {
        let BluetoothDtmTransmitterFirstPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerTxPreparationFailure::from_prepared(
                    error,
                    staged.owner,
                ));
            }
        };
        let BluetoothDtmTransmitterFirstStaged {
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
        let plan =
            match BluetoothDtmReviewedEventWordsPlan::new_transmitter(link_state, reservation) {
                Ok(plan) => plan,
                Err(failure) => {
                    let reservation = failure.into_reservation();
                    self.release_dtm_reservation(reservation);
                    return Err(BluetoothDtmControllerTxPreparationFailure::from_prepared(
                        BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                            expected: BluetoothDtmRole::Transmitter,
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
                return Err(BluetoothDtmControllerTxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Graph(error),
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
                Err(BluetoothDtmControllerTxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::EmptyList(error),
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
        owner: BluetoothDtmReceiverCpuOwned,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxPreparationFailure {
        BluetoothDtmControllerRxPreparationFailure {
            error: BluetoothDtmControllerEventPreparationError::ControllerTime(error),
            owner,
        }
    }

    /// Form one initial receiver candidate from a private fresh current.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_receiver_first_item(
        &self,
        owner: BluetoothDtmReceiverCpuOwned,
        link_state: BluetoothDtmLinkStateReset,
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        now: BluetoothControllerSchedulerNow,
        rf_ready: crate::BluetoothDtmRfReady,
    ) -> Result<BluetoothDtmReceiverFirstStaged, BluetoothDtmControllerRxPreparationFailure> {
        if link_state.role() != BluetoothDtmRole::Receiver {
            return Err(BluetoothDtmControllerRxPreparationFailure {
                error: BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                    expected: BluetoothDtmRole::Receiver,
                    observed: link_state.role(),
                },
                owner,
            });
        }
        let margin = self.config.dtm_scheduler_margin();
        let current = dtm_scheduler_current(&now);
        let window = crate::BluetoothDtmRxInitialEventWindow::new(
            self.config,
            current,
            rf_ready.into_scheduler_instant(),
        );
        let event = match BluetoothDtmSchedulerItemEvent::new_initial_receiver(channel, phy, window)
        {
            Ok(event) => event,
            Err(error) => {
                return Err(BluetoothDtmControllerRxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                });
            }
        };

        Ok(BluetoothDtmReceiverFirstStaged {
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
        staged: BluetoothDtmReceiverFirstStaged,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<BluetoothDtmReceiverFirstPreSequence, BluetoothDtmControllerRxPreparationFailure>
    {
        let reservation =
            match self.admit_initial_dtm_event(staged.event, &staged.now, admission_sample) {
                Ok(reservation) => reservation,
                Err(error) => {
                    return Err(BluetoothDtmControllerRxPreparationFailure {
                        error: BluetoothDtmControllerEventPreparationError::Reservation(error),
                        owner: staged.owner,
                    });
                }
            };
        Ok(BluetoothDtmReceiverFirstPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved initial receiver after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_first_staged(
        &mut self,
        staged: BluetoothDtmReceiverFirstStaged,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxPreparationFailure {
        self.reject_dtm_receiver_first_before_stage(staged.owner, error)
    }

    /// Release initial RX admission and return every retry resource.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_first_pre_sequence(
        &mut self,
        pre_sequence: BluetoothDtmReceiverFirstPreSequence,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_receiver_first_staged(pre_sequence.staged, error)
    }

    /// Authorize initial RX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_receiver_first_item(
        &mut self,
        pre_sequence: BluetoothDtmReceiverFirstPreSequence,
        sequence_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
        BluetoothDtmControllerRxPreparationFailure,
    > {
        let BluetoothDtmReceiverFirstPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerRxPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let BluetoothDtmReceiverFirstStaged {
            owner,
            link_state,
            channel,
            phy,
            margin,
            window,
            event: _,
            now: _,
        } = staged;
        let plan = match BluetoothDtmReviewedEventWordsPlan::new_receiver(link_state, reservation) {
            Ok(plan) => plan,
            Err(failure) => {
                self.release_dtm_reservation(failure.into_reservation());
                return Err(BluetoothDtmControllerRxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: BluetoothDtmRole::Receiver,
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
                return Err(BluetoothDtmControllerRxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Graph(error),
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
                Err(BluetoothDtmControllerRxPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::EmptyList(error),
                    owner,
                })
            }
        }
    }

    /// Form one recurring transmitter candidate from a private fresh current.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_transmitter_recurring_item(
        &self,
        owner: BluetoothDtmActiveTransmitterCpuOwned,
        now: BluetoothControllerSchedulerNow,
    ) -> Result<
        BluetoothDtmTransmitterRecurringStaged,
        BluetoothDtmControllerTxRecurringPreparationFailure,
    > {
        let current = dtm_scheduler_current(&now);
        let next_window = owner
            .timing()
            .advance_event_window(self.config, owner.last_committed_window(), current)
            .window();
        let event = match BluetoothDtmSchedulerItemEvent::new_transmitter(
            owner.channel(),
            owner.phy(),
            next_window,
        ) {
            Ok(event) => event,
            Err(error) => {
                return Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                });
            }
        };

        Ok(BluetoothDtmTransmitterRecurringStaged {
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
        staged: BluetoothDtmTransmitterRecurringStaged,
    ) -> Result<
        BluetoothDtmTransmitterRecurringPreSequence,
        BluetoothDtmControllerTxRecurringPreparationFailure,
    > {
        let reservation = match self.reserve_recurring_dtm_event(staged.event, &staged.now) {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Reservation(error),
                    owner: staged.owner,
                });
            }
        };
        Ok(BluetoothDtmTransmitterRecurringPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved recurring transmitter after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_recurring_staged(
        &mut self,
        staged: BluetoothDtmTransmitterRecurringStaged,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerTxRecurringPreparationFailure {
        BluetoothDtmControllerTxRecurringPreparationFailure {
            error: BluetoothDtmControllerEventPreparationError::ControllerTime(error),
            owner: staged.owner,
        }
    }

    /// Release recurring TX reservation and return the unchanged active owner.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_transmitter_recurring_pre_sequence(
        &mut self,
        pre_sequence: BluetoothDtmTransmitterRecurringPreSequence,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerTxRecurringPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_transmitter_recurring_staged(pre_sequence.staged, error)
    }

    /// Authorize recurring TX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_transmitter_recurring_item(
        &mut self,
        pre_sequence: BluetoothDtmTransmitterRecurringPreSequence,
        sequence_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
        BluetoothDtmControllerTxRecurringPreparationFailure,
    > {
        let BluetoothDtmTransmitterRecurringPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let BluetoothDtmTransmitterRecurringStaged {
            owner,
            window: next_window,
            event: _,
            now: _,
        } = staged;
        let plan = match BluetoothDtmReviewedEventWordsPlan::new_transmitter(
            owner.link_state(),
            reservation,
        ) {
            Ok(plan) => plan,
            Err(failure) => {
                self.release_dtm_reservation(failure.into_reservation());
                return Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                        expected: BluetoothDtmRole::Transmitter,
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
                return Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Graph(error),
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
                Err(BluetoothDtmControllerTxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::EmptyList(error),
                    owner,
                })
            }
        }
    }

    /// Reject recurring RX before ordered RF/current inputs can form a candidate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn reject_dtm_receiver_recurring_before_stage(
        &mut self,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxRecurringPreparationFailure {
        BluetoothDtmControllerRxRecurringPreparationFailure {
            error: BluetoothDtmControllerEventPreparationError::ControllerTime(error),
            owner,
        }
    }

    /// Form one recurring receiver candidate from private fresh timing inputs.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn stage_dtm_receiver_recurring_item(
        &self,
        owner: BluetoothDtmActiveReceiverCpuOwned,
        now: BluetoothControllerSchedulerNow,
        rf_ready: crate::BluetoothDtmRfReady,
    ) -> Result<
        BluetoothDtmReceiverRecurringStaged,
        BluetoothDtmControllerRxRecurringPreparationFailure,
    > {
        let current = dtm_scheduler_current(&now);
        let next_window = BluetoothDtmRxRecurringEventWindow::new(
            self.config,
            current,
            rf_ready.into_scheduler_instant(),
        );
        let event = match BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
            owner.channel(),
            owner.phy(),
            next_window,
        ) {
            Ok(event) => event,
            Err(error) => {
                return Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::SchedulerItem(error),
                    owner,
                });
            }
        };

        Ok(BluetoothDtmReceiverRecurringStaged {
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
        staged: BluetoothDtmReceiverRecurringStaged,
    ) -> Result<
        BluetoothDtmReceiverRecurringPreSequence,
        BluetoothDtmControllerRxRecurringPreparationFailure,
    > {
        let reservation = match self.reserve_recurring_dtm_event(staged.event, &staged.now) {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Reservation(error),
                    owner: staged.owner,
                });
            }
        };
        Ok(BluetoothDtmReceiverRecurringPreSequence {
            staged,
            reservation,
        })
    }

    /// Return an unreserved recurring receiver after a time-phase failure.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_recurring_staged(
        &mut self,
        staged: BluetoothDtmReceiverRecurringStaged,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxRecurringPreparationFailure {
        self.reject_dtm_receiver_recurring_before_stage(staged.owner, error)
    }

    /// Release recurring RX reservation and return the unchanged active owner.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_dtm_receiver_recurring_pre_sequence(
        &mut self,
        pre_sequence: BluetoothDtmReceiverRecurringPreSequence,
        error: BluetoothDtmControllerTimeAcquisitionError,
    ) -> BluetoothDtmControllerRxRecurringPreparationFailure {
        self.release_dtm_reservation(pre_sequence.reservation);
        self.cancel_dtm_receiver_recurring_staged(pre_sequence.staged, error)
    }

    /// Authorize recurring RX sequence time, then prepare and merge the graph.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn finish_dtm_receiver_recurring_item(
        &mut self,
        pre_sequence: BluetoothDtmReceiverRecurringPreSequence,
        sequence_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
        BluetoothDtmControllerRxRecurringPreparationFailure,
    > {
        let BluetoothDtmReceiverRecurringPreSequence {
            staged,
            reservation,
        } = pre_sequence;
        let reservation = match self
            .finish_dtm_sequence_authorization(reservation.authorize_sequence(sequence_sample))
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                    error,
                    owner: staged.owner,
                });
            }
        };
        let BluetoothDtmReceiverRecurringStaged {
            owner,
            window: next_window,
            event: _,
            now: _,
        } = staged;
        let plan =
            match BluetoothDtmReviewedEventWordsPlan::new_receiver(owner.link_state(), reservation)
            {
                Ok(plan) => plan,
                Err(failure) => {
                    self.release_dtm_reservation(failure.into_reservation());
                    return Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                        error: BluetoothDtmControllerEventPreparationError::LinkStateRoleMismatch {
                            expected: BluetoothDtmRole::Receiver,
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
                return Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::Graph(error),
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
                Err(BluetoothDtmControllerRxRecurringPreparationFailure {
                    error: BluetoothDtmControllerEventPreparationError::EmptyList(error),
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
        item: BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase>,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
        BluetoothDtmEmptySchedulerMergeFailure<Role, Phase>,
    >
    where
        Phase: BluetoothDtmSchedulerItemPhase<Role>,
    {
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothDtmEmptySchedulerMergeFailure { error, item });
        }
        Ok(BluetoothDtmEmptySchedulerMergePrepared {
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
        merged: BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        BluetoothDtmSchedulerBookkeepingPrepared<Role, Phase>,
        BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    >
    where
        Phase: BluetoothDtmSchedulerItemPhase<Role>,
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
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        (
            BluetoothDtmMemoryGraphCpuOwned,
            BluetoothDtmPayloadPattern,
            BluetoothDtmPayloadLength,
        ),
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
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
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    ) -> Result<
        BluetoothDtmReceiverCpuOwned,
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
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
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        BluetoothDtmActiveTransmitterCpuOwned,
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
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
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
    ) -> Result<
        BluetoothDtmActiveReceiverCpuOwned,
        BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmRecurringSchedulerItemPhase,
        >,
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
    #[allow(
        unsafe_code,
        reason = "the terminal Controller state proves inactive routes while this scheduler retains the graph and exact list identity"
    )]
    pub(crate) fn publish_dtm_scheduler_head<Role, Phase>(
        &mut self,
        merged: BluetoothDtmEmptySchedulerMergePrepared<Role, Phase>,
    ) -> Result<
        BluetoothDtmSchedulerHeadPublished<Role>,
        BluetoothDtmSchedulerHeadPublicationFailure<Role, Phase>,
    >
    where
        Phase: BluetoothDtmSchedulerItemPhase<Role>,
    {
        let address = merged.scheduler_item_address();
        if !self._scheduler_list.can_publish_first_item(address) {
            return Err(BluetoothDtmSchedulerHeadPublicationFailure {
                error: BluetoothDtmSchedulerHeadPublicationError::SchedulerIdentityMismatch,
                merged,
            });
        }
        let head = match BluetoothSchedulerHardwareListHead::from_address(address) {
            Ok(head) => head,
            Err(error) => {
                return Err(BluetoothDtmSchedulerHeadPublicationFailure {
                    error: error.into(),
                    merged,
                });
            }
        };
        let publication = unsafe {
            self.task
                .publish_scheduler_hardware_list_head(merged.hardware_list_index(), head)
        };
        let item = merged.item.into_head_published(&publication);
        self._scheduler_list.retain_published_first_item(address);
        Ok(BluetoothDtmSchedulerHeadPublished { item, publication })
    }

    /// Retain that the exact published first item crossed the final RUN edge.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn retain_running_dtm_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) {
        self._scheduler_list.retain_running_first_item(address);
    }

    /// Perform one fresh, bounded DTM completion observation.
    ///
    /// The affine list token never crosses this Controller operation before it
    /// is joined to the matching running epoch. This prevents a caller from
    /// retaining a list-zero token and replaying it against a later DTM event.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_completion<Role>(
        &mut self,
        running: BluetoothDtmSchedulerRunning<Role>,
    ) -> BluetoothDtmSchedulerCompletionStep<Role> {
        let address = running.scheduler_item_address();
        if running.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothDtmSchedulerCompletionStep::SchedulerIdentityMismatch(running);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerCompletionStep::DrainAlreadyActive(running);
        }

        let capture = self
            .task
            .capture_scheduler_finished_lists(self.runtime.scheduler_finished_lists_mut());
        if capture.is_err() {
            return BluetoothDtmSchedulerCompletionStep::DrainAlreadyActive(running);
        }
        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothDtmSchedulerCompletionStep::NoFinishedList(running);
        };

        let BluetoothDtmSchedulerRunning { item, run } = running;
        match item.observe_completion(observed) {
            BluetoothDtmRunningEventCompletionObservation::ListMismatch { item, observed } => {
                BluetoothDtmSchedulerCompletionStep::UnrelatedList {
                    drain: BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerRunning { item, run },
                        more,
                    ),
                    observed,
                }
            }
            BluetoothDtmRunningEventCompletionObservation::StillInFlight(item) => {
                BluetoothDtmSchedulerCompletionStep::StillInFlight(
                    BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerRunning { item, run },
                        more,
                    ),
                )
            }
            BluetoothDtmRunningEventCompletionObservation::CompletionObserved(item) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothDtmSchedulerCompletionStep::CompletionObserved(
                    BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerCompletionObserved { item, run },
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
        pending: BluetoothDtmSchedulerFinishedListDrainPending<BluetoothDtmSchedulerRunning<Role>>,
    ) -> BluetoothDtmSchedulerRunningDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self._scheduler_list.retains_running_first_item(address)
        {
            return BluetoothDtmSchedulerRunningDrainStep::SchedulerIdentityMismatch(pending);
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerRunningDrainStep::DrainLost(pending);
        }

        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothDtmSchedulerRunningDrainStep::DrainLost(pending);
        };

        let running = pending.into_owner();
        let BluetoothDtmSchedulerRunning { item, run } = running;
        match item.observe_completion(observed) {
            BluetoothDtmRunningEventCompletionObservation::ListMismatch { item, observed } => {
                BluetoothDtmSchedulerRunningDrainStep::UnrelatedList {
                    drain: BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerRunning { item, run },
                        more,
                    ),
                    observed,
                }
            }
            BluetoothDtmRunningEventCompletionObservation::StillInFlight(item) => {
                BluetoothDtmSchedulerRunningDrainStep::StillInFlight(
                    BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerRunning { item, run },
                        more,
                    ),
                )
            }
            BluetoothDtmRunningEventCompletionObservation::CompletionObserved(item) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothDtmSchedulerRunningDrainStep::CompletionObserved(
                    BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                        BluetoothDtmSchedulerCompletionObserved { item, run },
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
        pending: BluetoothDtmSchedulerFinishedListDrainPending<
            BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    ) -> BluetoothDtmSchedulerCompletionObservedDrainStep<Role> {
        let address = pending.owner().scheduler_item_address();
        if pending.owner().hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothDtmSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                pending,
            );
        }
        if !self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
        }

        let step = self.runtime.scheduler_finished_lists_mut().step();
        let crate::BluetoothSchedulerFinishedListWorkerStep::List { observed, more } = step else {
            return BluetoothDtmSchedulerCompletionObservedDrainStep::DrainLost(pending);
        };
        let completed = pending.into_owner();
        if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
            BluetoothDtmSchedulerCompletionObservedDrainStep::RepeatedDtmList {
                drain: BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                    completed, more,
                ),
                observed,
            }
        } else {
            BluetoothDtmSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain: BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(
                    completed, more,
                ),
                observed,
            }
        }
    }

    /// Perform one fresh fenced hardware-head retirement observation for the
    /// exact completion retained by this Controller epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn observe_dtm_hardware_head_retirement<Role>(
        &mut self,
        completed: BluetoothDtmSchedulerCompletionObserved<Role>,
    ) -> BluetoothDtmSchedulerHardwareHeadRetirementStep<Role> {
        let address = completed.scheduler_item_address();
        if completed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_completion_observed_first_item(address)
        {
            return BluetoothDtmSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                completed,
            );
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                completed,
            );
        }

        let BluetoothDtmSchedulerCompletionObserved { item, run } = completed;
        match self
            .task
            .observe_scheduler_hardware_list_head_retirement(run)
        {
            BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished {
                run,
                observed,
            } => BluetoothDtmSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                completed: BluetoothDtmSchedulerCompletionObserved { item, run },
                observed,
            },
            BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged {
                run,
                observed,
            } => BluetoothDtmSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                completed: BluetoothDtmSchedulerCompletionObserved { item, run },
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
                BluetoothDtmSchedulerHardwareHeadRetirementStep::EmptyObserved(
                    BluetoothDtmSchedulerHardwareHeadEmptyObserved { item, head },
                )
            }
        }
    }

    /// Remove the exact empty-head DTM item from the source-owned software
    /// list without recreating the vendor intrusive container.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn unlink_dtm_software_list<Role>(
        &mut self,
        observed: BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> BluetoothDtmSchedulerSoftwareListUnlinkStep<Role> {
        let address = observed.scheduler_item_address();
        if observed.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self
                ._scheduler_list
                .unlink_software_list_first_item(address)
        {
            return BluetoothDtmSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                observed,
            );
        }

        let BluetoothDtmSchedulerHardwareHeadEmptyObserved { item, head } = observed;
        BluetoothDtmSchedulerSoftwareListUnlinkStep::Unlinked(
            BluetoothDtmSchedulerSoftwareListUnlinked { item, head },
        )
    }

    /// Join one freshly serviced primary scheduler event to the exact
    /// already-unlinked DTM item.
    ///
    /// A busy event performs no task-side command read. Any pending result
    /// consumes that event and retains the unlinked graph for a later event.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn join_dtm_software_list_removal<Role>(
        &mut self,
        unlinked: BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        event: crate::BluetoothPrimarySchedulerEvent,
    ) -> BluetoothDtmSchedulerSoftwareListRemovalJoin<Role> {
        let address = unlinked.scheduler_item_address();
        if unlinked.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || self.runtime.scheduler_finished_lists_mut().is_active()
            || !self._scheduler_list.retains_unlinked_first_item(address)
        {
            return BluetoothDtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                unlinked,
                event,
            };
        }

        let idle = match event.into_software_list_removal_gate() {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending => {
                return BluetoothDtmSchedulerSoftwareListRemovalJoin::Pending(unlinked);
            }
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(idle) => idle,
        };
        let BluetoothDtmSchedulerSoftwareListUnlinked { item, head } = unlinked;
        match self.task.finish_scheduler_software_list_removal(idle, head) {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head } => {
                BluetoothDtmSchedulerSoftwareListRemovalJoin::Pending(
                    BluetoothDtmSchedulerSoftwareListUnlinked { item, head },
                )
            }
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(removal) => {
                self._scheduler_list
                    .retain_software_list_removal_ready_first_item(address);
                BluetoothDtmSchedulerSoftwareListRemovalJoin::Ready(
                    BluetoothDtmSchedulerSoftwareListRemovalReady {
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
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> BluetoothDtmSchedulerRecycleStep<Role> {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothDtmSchedulerRecycleStep::SchedulerIdentityMismatch(ready);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerRecycleStep::FinishedListDrainStillActive(ready);
        }
        if ready.role() == BluetoothDtmRole::Receiver
            && ready.status() == BluetoothDtmSchedulerItemCompletionStatus::Zero
        {
            return BluetoothDtmSchedulerRecycleStep::ReceiverSuccessRequiresSpecializedRecycle(
                ready,
            );
        }

        let BluetoothDtmSchedulerSoftwareListRemovalReady { item, _removal } = ready;
        match item.recycle(self.runtime.scheduler_timeline_mut(), _removal) {
            Ok(timeline_released) => {
                self._scheduler_list.commit_recycled_first_item();
                BluetoothDtmSchedulerRecycleStep::Recycled(
                    timeline_released.finish_source_list_release(),
                )
            }
            Err(failure) => {
                let (error, item, removal) = failure.into_parts();
                let ready = BluetoothDtmSchedulerSoftwareListRemovalReady {
                    item,
                    _removal: removal,
                };
                match error {
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::MemoryIdentity(
                        error,
                    ) => BluetoothDtmSchedulerRecycleStep::MemoryIdentityMismatch {
                        ready,
                        error,
                    },
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::ReservationIdentityMismatch => {
                        BluetoothDtmSchedulerRecycleStep::ReservationIdentityMismatch(ready)
                    }
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::ReceiverSuccessMemory(_) => {
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
        ready: BluetoothDtmSchedulerSoftwareListRemovalReady<crate::BluetoothDtmReceiverEvent>,
    ) -> BluetoothDtmSchedulerRxSuccessRecycleStep {
        let address = ready.scheduler_item_address();
        if ready.hardware_list_index() != BluetoothSchedulerHardwareListIndex::ZERO
            || !self
                ._scheduler_list
                .retains_software_list_removal_ready_first_item(address)
        {
            return BluetoothDtmSchedulerRxSuccessRecycleStep::SchedulerIdentityMismatch(ready);
        }
        if self.runtime.scheduler_finished_lists_mut().is_active() {
            return BluetoothDtmSchedulerRxSuccessRecycleStep::FinishedListDrainStillActive(ready);
        }
        if ready.status() != BluetoothDtmSchedulerItemCompletionStatus::Zero {
            return BluetoothDtmSchedulerRxSuccessRecycleStep::CompletionStatusMismatch(ready);
        }

        let BluetoothDtmSchedulerSoftwareListRemovalReady { item, _removal } = ready;
        match item.recycle_receiver_success(self.runtime.scheduler_timeline_mut(), _removal) {
            Ok(timeline_released) => {
                self._scheduler_list.commit_recycled_first_item();
                BluetoothDtmSchedulerRxSuccessRecycleStep::Rearmed(
                    timeline_released.finish_source_list_release(),
                )
            }
            Err(failure) => {
                let (error, item, removal) = failure.into_parts();
                let ready = BluetoothDtmSchedulerSoftwareListRemovalReady {
                    item,
                    _removal: removal,
                };
                match error {
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::MemoryIdentity(
                        error,
                    ) => BluetoothDtmSchedulerRxSuccessRecycleStep::MemoryIdentityMismatch {
                        ready,
                        error,
                    },
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::ReceiverSuccessMemory(
                        error,
                    ) => BluetoothDtmSchedulerRxSuccessRecycleStep::ReturnedTopologyRejected {
                        ready,
                        error,
                    },
                    crate::dtm_event_prepare::BluetoothDtmCompletionRecycleError::ReservationIdentityMismatch => {
                        BluetoothDtmSchedulerRxSuccessRecycleStep::ReservationIdentityMismatch(ready)
                    }
                }
            }
        }
    }

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
    ) {
        let task = &mut self.task;
        let (interrupt, software) = self.runtime.split();
        (
            interrupt,
            BluetoothControllerPoweredTaskRuntime::new(software, task),
        )
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn modem_lp_timer_software_parts_mut(
        &mut self,
    ) -> (
        &mut crate::BluetoothModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
        &mut open_esp_radio_esp32s31_hal::BluetoothModemLpTimerEpoch,
        &crate::BluetoothModemLpTimerEventCell,
    ) {
        self.runtime.modem_lp_timer_software_parts_mut()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn primary_interrupt_publications(
        &self,
    ) -> (
        &crate::BluetoothSchedulerWakeCell,
        &crate::BluetoothSchedulerLockModifyEventCell,
    ) {
        self.runtime.primary_interrupt_publications()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn modem_lp_timer_worker_wake(
        &self,
    ) -> &crate::BluetoothModemLpTimerWorkerWakeCell {
        self.runtime.modem_lp_timer_worker_wake()
    }
}

#[cfg(any(target_arch = "riscv32", test))]
const fn dtm_scheduler_current(
    now: &BluetoothControllerSchedulerNow,
) -> BluetoothDtmSchedulerInstant {
    BluetoothDtmSchedulerInstant::from_image(now.scheduler_image())
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
            standalone_always_awake,
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        BluetoothSchedulerInitialized {
            task,
            _interrupts: Some(interrupts),
            _platform: platform,
            time_scale,
            _standalone_always_awake: standalone_always_awake,
            config: BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            _scheduler_list: BluetoothSchedulerExclusiveListEpoch::new(hardware_lists_cleared),
            runtime,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use open_esp_radio_esp32s31_pac::RadioHardware;

    use std::{cell::RefCell, rc::Rc, vec::Vec};

    use crate::{
        BluetoothClockedResources, BluetoothControllerRuntimeResources,
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
        BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerItemEvent, BluetoothStopped,
        controller_time::BluetoothControllerSchedulerNow,
    };

    use super::{
        BluetoothDtmControllerEventPreparationError, BluetoothDtmEmptySchedulerMergeError,
        BluetoothDtmSchedulerFinishedListDrainState, BluetoothSchedulerExclusiveListEpoch,
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
            BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(drained_owner, false);
        let BluetoothDtmSchedulerFinishedListDrainState::Drained(drained_owner) = drained else {
            panic!("an exhausted capture must return the ordinary owner");
        };
        assert!(Rc::ptr_eq(&drained_owner, &drained_identity));

        let pending_owner = Rc::new(());
        let pending_identity = Rc::clone(&pending_owner);
        let pending =
            BluetoothDtmSchedulerFinishedListDrainState::from_worker_step(pending_owner, true);
        let BluetoothDtmSchedulerFinishedListDrainState::Pending(pending) = pending else {
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
            Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty)
        );
        assert!(!list.cancel_first_item(other));
        assert!(list.cancel_first_item(first));
        assert_eq!(list.prepare_first_item(other), Ok(()));
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
            Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty)
        );
        list.retain_completion_observed_first_item(first);
        assert!(list.retains_completion_observed_first_item(first));
        assert!(!list.retains_running_first_item(first));
        assert!(!list.cancel_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty)
        );
        list.retain_hardware_head_empty_first_item(first);
        assert!(!list.retains_completion_observed_first_item(first));
        assert!(list.retains_hardware_head_empty_first_item(first));
        assert!(list.unlink_software_list_first_item(first));
        assert!(!list.unlink_software_list_first_item(first));
        assert!(list.retains_unlinked_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty)
        );
        list.retain_software_list_removal_ready_first_item(first);
        assert!(list.retains_software_list_removal_ready_first_item(first));
        assert_eq!(
            list.prepare_first_item(other),
            Err(BluetoothDtmEmptySchedulerMergeError::ListNotEmpty)
        );
        list.commit_recycled_first_item();
        assert_eq!(list.prepare_first_item(other), Ok(()));
    }

    #[test]
    fn controller_hal_precedes_complete_scheduler_init_and_arms_fail_stop() {
        PLATFORM_DROPS.store(0, Ordering::Relaxed);
        let stopped =
            BluetoothStopped::from_hardware(FakePlatform, RadioHardware::for_validation());
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
        let (interrupt, task) = scheduler.split_runtime();
        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        assert_eq!(
            task.controller_time_phase(),
            crate::controller_time::BluetoothControllerTimeWorkerPhase::Idle
        );
        assert!(!task.controller_time_needs_recheck());
        drop((interrupt, task));
        drop(scheduler);
        assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rejected_initial_sequence_gate_releases_the_controller_owned_reservation() {
        struct AdmissionPlatform;

        let stopped =
            BluetoothStopped::from_hardware(AdmissionPlatform, RadioHardware::for_validation());
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
                BluetoothDtmSchedulerInstant::from_image(900),
                BluetoothDtmSchedulerInstant::from_image(1_020),
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
            super::dtm_scheduler_current(&now),
            BluetoothDtmSchedulerInstant::from_image(1_000)
        );

        let reservation = scheduler
            .admit_initial_dtm_event(
                event,
                &now,
                BluetoothControllerTimeSample::for_validation(92),
            )
            .expect("the fresh admission sample keeps the initial deadline open");
        assert!(!scheduler.runtime_is_pristine());
        let result = scheduler.finish_dtm_sequence_authorization(
            reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
        );

        assert_eq!(
            result.expect_err("the deliberately late second sample must fail"),
            BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
                crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
            )
        );
        assert!(scheduler.runtime_is_pristine());
    }

    #[test]
    fn rejected_recurring_sequence_gate_releases_the_controller_owned_reservation() {
        struct RecurringPlatform;

        let stopped =
            BluetoothStopped::from_hardware(RecurringPlatform, RadioHardware::for_validation());
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
                BluetoothDtmSchedulerInstant::from_image(900),
                BluetoothDtmSchedulerInstant::from_image(1_020),
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
            super::dtm_scheduler_current(&now),
            BluetoothDtmSchedulerInstant::from_image(1_000)
        );

        let reservation = scheduler
            .reserve_recurring_dtm_event(event, &now)
            .expect("the exact recurring window is initially free");
        assert!(!scheduler.runtime_is_pristine());
        let result = scheduler.finish_dtm_sequence_authorization(
            reservation.authorize_sequence(BluetoothControllerTimeSample::for_validation(1_000)),
        );

        assert_eq!(
            result.expect_err("the deliberately late sequence sample must fail"),
            BluetoothDtmControllerEventPreparationError::SequenceAuthorization(
                crate::BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
            )
        );
        assert!(scheduler.runtime_is_pristine());
    }
}
