//! Fact-bounded scheduler initialization after the controller HAL component.

use crate::BluetoothSchedulerSoftwareConfig;
#[cfg(target_arch = "riscv32")]
use crate::dtm_event_prepare::{
    BluetoothDtmCompletionObservedEvent, BluetoothDtmHardwareOwnedEventCompletionObservation,
};
use crate::{
    BluetoothControllerInterruptRuntime, BluetoothControllerPoweredTaskRuntime,
    BluetoothControllerRuntimeResources, BluetoothDtmRole,
    BluetoothDtmSchedulerBookkeepingPrepared,
    controller_hal::BluetoothControllerHalInitialized,
    dtm_event_prepare::{BluetoothDtmEmptyListLinkPrepared, BluetoothDtmHardwareOwnedEvent},
    resources::{
        BluetoothInterruptBankOwner, BluetoothTaskResources, BluetoothTeardownPendingPlatform,
    },
};
#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemCompletionStatus;
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

/// Lossless failure to join a DTM item to the exclusive empty scheduler list.
#[must_use = "the unchanged scheduler-prepared item remains CPU-owned"]
pub struct BluetoothDtmEmptySchedulerMergeFailure<Role> {
    error: BluetoothDtmEmptySchedulerMergeError,
    item: BluetoothDtmSchedulerBookkeepingPrepared<Role>,
}

impl<Role> BluetoothDtmEmptySchedulerMergeFailure<Role> {
    /// Exact reason the list owner rejected this item.
    pub const fn error(&self) -> BluetoothDtmEmptySchedulerMergeError {
        self.error
    }

    /// Recover the unchanged CPU-owned item for retry or cancellation.
    pub fn into_item(self) -> BluetoothDtmSchedulerBookkeepingPrepared<Role> {
        self.item
    }
}

/// First DTM item joined to one exclusive, previously empty scheduler epoch.
///
/// The item-side descriptor transform and source-owned list state now agree on
/// one exact identity. This remains CPU-owned: no visibility fence, hardware
/// head, RUN command or radio-completion authority has been granted.
#[must_use = "the merged item must be published through the same scheduler or cancelled"]
pub struct BluetoothDtmEmptySchedulerMergePrepared<Role> {
    item: BluetoothDtmEmptyListLinkPrepared<Role>,
}

impl<Role> BluetoothDtmEmptySchedulerMergePrepared<Role> {
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
pub struct BluetoothDtmSchedulerHeadPublicationFailure<Role> {
    error: BluetoothDtmSchedulerHeadPublicationError,
    merged: BluetoothDtmEmptySchedulerMergePrepared<Role>,
}

impl<Role> BluetoothDtmSchedulerHeadPublicationFailure<Role> {
    /// Exact reason no scheduler head was published.
    pub const fn error(&self) -> BluetoothDtmSchedulerHeadPublicationError {
        self.error
    }

    /// Recover the unchanged CPU-owned merge.
    pub fn into_merged(self) -> BluetoothDtmEmptySchedulerMergePrepared<Role> {
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
    item: BluetoothDtmHardwareOwnedEvent<Role>,
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
        BluetoothDtmHardwareOwnedEvent<Role>,
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
    item: BluetoothDtmHardwareOwnedEvent<Role>,
    run: BluetoothSchedulerHardwareRunCommandPublished,
}

impl<Role> BluetoothDtmSchedulerRunning<Role> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn new(
        item: BluetoothDtmHardwareOwnedEvent<Role>,
        run: BluetoothSchedulerHardwareRunCommandPublished,
    ) -> Self {
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
/// hardware-owned. This state does not expose packet memory, cancellation or
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
        /// Unchanged running DTM graph.
        running: BluetoothDtmSchedulerRunning<Role>,
        /// Affine observation for the actual list owner.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        /// Whether the captured transfer retains another list.
        more: bool,
    },
    /// DTM list zero was reported but its status remains the in-flight sentinel.
    StillInFlight {
        /// Unchanged running DTM graph; a later attempt requires a fresh transfer.
        running: BluetoothDtmSchedulerRunning<Role>,
        /// Whether the captured transfer retains another list.
        more: bool,
    },
    /// One non-sentinel status was observed without returning CPU ownership.
    CompletionObserved {
        /// Hardware-owned completion observation retaining every graph owner.
        completed: BluetoothDtmSchedulerCompletionObserved<Role>,
        /// Whether the captured transfer retains another list.
        more: bool,
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
    /// Successful RX requires returned-header and swap-reserve ownership first.
    ReceiverSuccessRequiresReturnedHeader(BluetoothDtmSchedulerSoftwareListRemovalReady<Role>),
    /// Memory and timeline ownership returned to the source role.
    Recycled(crate::BluetoothDtmRecycledEvent<Role>),
}

/// Internal result of joining one fresh primary scheduler event to an already
/// unlinked DTM graph.
#[must_use = "the unlinked or removal-ready graph must remain owned"]
#[cfg(target_arch = "riscv32")]
pub(crate) enum BluetoothDtmSchedulerSoftwareListRemovalJoin<Role> {
    SchedulerIdentityMismatch(BluetoothDtmSchedulerSoftwareListUnlinked<Role>),
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

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn controller_time_phase(
        &self,
    ) -> crate::controller_time::BluetoothControllerTimeWorkerPhase {
        self.task.controller_time_phase()
    }

    #[cfg(any(target_arch = "riscv32", test))]
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

    /// Join one prepared DTM item to this epoch's still-empty scheduler list.
    ///
    /// This consumes no hardware permission. The returned state merely proves
    /// that the source-owned list and the item-side empty-list links were
    /// advanced together.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure returns the complete affine CPU-owned DTM item"
    )]
    pub fn prepare_dtm_empty_list_merge<Role>(
        &mut self,
        item: BluetoothDtmSchedulerBookkeepingPrepared<Role>,
    ) -> Result<
        BluetoothDtmEmptySchedulerMergePrepared<Role>,
        BluetoothDtmEmptySchedulerMergeFailure<Role>,
    > {
        let address = item.scheduler_item_address();
        if let Err(error) = self._scheduler_list.prepare_first_item(address) {
            return Err(BluetoothDtmEmptySchedulerMergeFailure { error, item });
        }
        Ok(BluetoothDtmEmptySchedulerMergePrepared {
            item: item.prepare_empty_list_link(),
        })
    }

    /// Cancel a not-yet-published first-item merge through the same epoch.
    ///
    /// A state from another or already advanced scheduler is returned
    /// unchanged and cannot reopen this list.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc identity failure returns the complete affine merged item"
    )]
    pub fn cancel_dtm_empty_list_merge<Role>(
        &mut self,
        merged: BluetoothDtmEmptySchedulerMergePrepared<Role>,
    ) -> Result<
        BluetoothDtmSchedulerBookkeepingPrepared<Role>,
        BluetoothDtmEmptySchedulerMergePrepared<Role>,
    > {
        if !self
            ._scheduler_list
            .cancel_first_item(merged.scheduler_item_address())
        {
            return Err(merged);
        }
        Ok(merged.item.cancel())
    }

    /// Publish the merge-selected first item after the complete hardware
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
    pub(crate) fn publish_dtm_first_scheduler_head<Role>(
        &mut self,
        merged: BluetoothDtmEmptySchedulerMergePrepared<Role>,
    ) -> Result<
        BluetoothDtmSchedulerHeadPublished<Role>,
        BluetoothDtmSchedulerHeadPublicationFailure<Role>,
    > {
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
        let item = merged.item.into_hardware_owned(&publication);
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
            BluetoothDtmHardwareOwnedEventCompletionObservation::ListMismatch {
                item,
                observed,
            } => BluetoothDtmSchedulerCompletionStep::UnrelatedList {
                running: BluetoothDtmSchedulerRunning { item, run },
                observed,
                more,
            },
            BluetoothDtmHardwareOwnedEventCompletionObservation::StillInFlight(item) => {
                BluetoothDtmSchedulerCompletionStep::StillInFlight {
                    running: BluetoothDtmSchedulerRunning { item, run },
                    more,
                }
            }
            BluetoothDtmHardwareOwnedEventCompletionObservation::CompletionObserved(item) => {
                self._scheduler_list
                    .retain_completion_observed_first_item(address);
                BluetoothDtmSchedulerCompletionStep::CompletionObserved {
                    completed: BluetoothDtmSchedulerCompletionObserved { item, run },
                    more,
                }
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
            return BluetoothDtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch(
                unlinked,
            );
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
    /// TX and RX non-success outcomes may recycle directly. RX success remains
    /// blocked because the returned-header and swap-reserve ownership branch
    /// is not yet affine. The timeline reservation and reviewed descriptor
    /// links are released in one bounded transaction.
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
            && ready.status() == BluetoothDtmSchedulerItemCompletionStatus::Success
        {
            return BluetoothDtmSchedulerRecycleStep::ReceiverSuccessRequiresReturnedHeader(ready);
        }

        let BluetoothDtmSchedulerSoftwareListRemovalReady { item, _removal } = ready;
        match item.recycle(self.runtime.scheduler_timeline_mut(), _removal) {
            Ok(recycled) => {
                self._scheduler_list.commit_recycled_first_item();
                BluetoothDtmSchedulerRecycleStep::Recycled(recycled)
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
        } = self;
        let hardware_lists_cleared = initialize_hardware(&mut task);
        BluetoothSchedulerInitialized {
            task,
            _interrupts: Some(interrupts),
            _platform: platform,
            time_scale,
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

    use crate::{BluetoothClockedResources, BluetoothControllerRuntimeResources, BluetoothStopped};

    use super::{
        BluetoothDtmEmptySchedulerMergeError, BluetoothSchedulerExclusiveListEpoch,
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
}
