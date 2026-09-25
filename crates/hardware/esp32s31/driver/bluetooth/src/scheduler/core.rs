//! Fact-bounded scheduler initialization after the controller HAL component.

#[cfg(target_arch = "riscv32")]
mod single_item;

#[cfg(target_arch = "riscv32")]
pub(crate) use single_item::*;

use crate::scheduler::SchedulerSoftwareConfig;

#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler::timeline::SchedulerWindowReservation;

use oer_esp32s31_pac::BluetoothControllerTimeScale;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadPublished,
};

use crate::{
    controller_hal::ControllerHalInitialized,
    resources::{InterruptBankOwner, TaskResources, TeardownPendingPlatform},
    runtime_resources::{
        ControllerInterruptRuntime, ControllerModemTimerRuntime, ControllerPoweredTaskRuntime,
        ControllerRuntimeResources,
    },
};

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListIndex;
use {
    oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadError,
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

    pub(crate) fn prepare_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> Result<(), SchedulerEmptyListMergeError> {
        if self.state != SchedulerExclusiveListState::Empty {
            return Err(SchedulerEmptyListMergeError::ListNotEmpty);
        }
        self.state = SchedulerExclusiveListState::FirstItemPrepared { address };
        Ok(())
    }

    pub(crate) fn cancel_first_item(&mut self, address: BluetoothControllerSramAddress) -> bool {
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

    pub(crate) fn retains_running_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemRunning { address }
    }

    pub(crate) fn retain_completion_observed_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_running_first_item(address) {
            return false;
        }
        self.state = SchedulerExclusiveListState::FirstItemCompletionObserved { address };
        true
    }

    pub(crate) fn retains_completion_observed_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemCompletionObserved { address }
    }

    pub(crate) fn retain_hardware_head_empty_first_item(
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

    pub(crate) fn unlink_software_list_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_hardware_head_empty_first_item(address) {
            return false;
        }
        self.state =
            SchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
                address,
            };
        true
    }

    pub(crate) fn retains_unlinked_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state
            == SchedulerExclusiveListState::FirstItemSoftwareListUnlinkedAwaitingRemovalGate {
                address,
            }
    }

    pub(crate) fn retain_software_list_removal_ready_first_item(
        &mut self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        if !self.retains_unlinked_first_item(address) {
            return false;
        }
        self.state = SchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address };
        true
    }

    pub(crate) fn retains_software_list_removal_ready_first_item(
        &self,
        address: BluetoothControllerSramAddress,
    ) -> bool {
        self.state == SchedulerExclusiveListState::FirstItemSoftwareListRemovalReady { address }
    }

    pub(crate) fn commit_recycled_first_item(&mut self) {
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

    pub(crate) const fn owner(&self) -> &Owner {
        &self.owner
    }

    pub(crate) fn into_owner(self) -> Owner {
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
    pub(crate) fn from_worker_step(owner: Owner, more: bool) -> Self {
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
    task: crate::resources::runtime_owner::RuntimeOwnerSlot<TaskResources>,
    _interrupts: Option<InterruptBankOwner>,
    _platform: crate::resources::runtime_owner::RuntimeOwnerSlot<TeardownPendingPlatform<P>>,
    time_scale: BluetoothControllerTimeScale,
    _standalone_dtm_profile: crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    config: SchedulerSoftwareConfig,
    _scheduler_list: SchedulerExclusiveListEpoch,
    runtime: ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>,
}

#[cfg(target_arch = "riscv32")]
pub(crate) struct SchedulerRestartParts<P, const MT: usize, const SC: usize> {
    pub(crate) task: TaskResources,
    pub(crate) platform: TeardownPendingPlatform<P>,
    pub(crate) time_scale: BluetoothControllerTimeScale,
    pub(crate) config: SchedulerSoftwareConfig,
    pub(crate) scheduler_list: SchedulerExclusiveListEpoch,
    pub(crate) runtime: ControllerRuntimeResources<MT, SC>,
}

#[cfg(target_arch = "riscv32")]
impl<P, const MT: usize, const SC: usize> SchedulerInitialized<P, MT, SC> {
    pub(crate) fn into_restart_parts(self) -> SchedulerRestartParts<P, MT, SC> {
        assert!(
            self._interrupts.is_none(),
            "interrupt partition already staged"
        );
        SchedulerRestartParts {
            task: self.task.into_unclaimed(),
            platform: self._platform.into_unclaimed(),
            time_scale: self.time_scale,
            config: self.config,
            scheduler_list: self._scheduler_list,
            runtime: self.runtime,
        }
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    pub(crate) fn task_mut(&mut self) -> &mut TaskResources {
        self.task
            .as_mut()
            .expect("pre-split scheduler owns its task")
    }

    #[cfg(test)]
    pub(crate) fn controller_time_phase(
        &self,
    ) -> crate::controller_time::ControllerTimeWorkerPhase {
        self.task
            .as_ref()
            .expect("pre-split task")
            .controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) fn controller_time_needs_recheck(&self) -> bool {
        self.task
            .as_ref()
            .expect("pre-split task")
            .controller_time_needs_recheck()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn take_interrupt_owner(&mut self) -> InterruptBankOwner {
        self._interrupts
            .take()
            .expect("private Controller invariant retains the interrupt owner until activation")
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn common_phy_parts_mut(&mut self) -> (&mut TaskResources, &mut P) {
        (
            self.task.as_mut().expect("pre-split task"),
            self._platform
                .as_mut()
                .expect("pre-split platform")
                .platform_mut(),
        )
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
        crate::controller_time::ControllerTimeRequest,
        crate::controller_time::ControllerTimeRequestError,
    > {
        self._standalone_dtm_profile.gate_controller_time_request();
        self.task.request_controller_time()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn cancel_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<(), crate::controller_time::ControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn recheck_owned_controller_time(
        &mut self,
        request: crate::controller_time::ControllerTimeRequest,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.recheck_owned_controller_time(request)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<
        crate::controller_time::ControllerTimeEventStep,
        crate::controller_time::ControllerTimeEventError,
    > {
        self.task.drain_orphan_controller_time()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn release_scheduler_reservation<State>(
        &mut self,
        reservation: SchedulerWindowReservation<State>,
    ) {
        self.runtime
            .scheduler_timeline_mut()
            .release(reservation)
            .expect("a reservation created by this Controller must release into the same timeline");
    }

    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the powered task owner and exclusive list identity jointly authorize the typed PAC publication"
    )]
    pub(crate) fn publish_first_scheduler_item_head(
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
        // SAFETY: `head` was validated against the exclusively owned source list
        // and typed head encoding; `self` holds the powered task epoch that
        // serializes scheduler-list MMIO.
        let publication = unsafe { self.task.publish_scheduler_hardware_list_head(index, head) };
        self._scheduler_list.retain_published_first_item(address);
        publication
    }
}

impl<P, const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    SchedulerInitialized<P, MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Claim the task HAL slot once while borrowing stable software workers.
    ///
    /// The task endpoint carries an exclusive, non-cloneable lease. HCI
    /// retirement later extracts the actual register and controller-time owners.
    /// The fourth endpoint leases the platform separately; command states never
    /// contain its generic type. Interrupt publications and software queues stay borrowed.
    /// A later split returns `None`, including after a runtime was dropped;
    /// dropping it neither restores hardware nor authorizes a new epoch.
    pub fn split_runtime(
        &mut self,
    ) -> Option<(
        ControllerInterruptRuntime<'_>,
        ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>,
        ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
        crate::resources::platform_retirement::ControllerPlatformLease<'_, P>,
    )> {
        let task = self.task.lease()?;
        let platform = crate::resources::platform_retirement::ControllerPlatformLease::claim(
            &mut self._platform,
        )
        .expect("task and platform are claimed in one split");
        let time_scale = self.time_scale;
        let standalone_dtm_profile = &self._standalone_dtm_profile;
        let config = self.config;
        let scheduler_list = &mut self._scheduler_list;
        let (interrupt, software, modem_timer) = self.runtime.split();
        Some((
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
            platform,
        ))
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
            task: crate::resources::runtime_owner::RuntimeOwnerSlot::new(task),
            _interrupts: Some(interrupts),
            _platform: crate::resources::runtime_owner::RuntimeOwnerSlot::new(platform),
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
