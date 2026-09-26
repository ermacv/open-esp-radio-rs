//! Fixed no-RTOS software state for one powered Controller epoch.
//!
//! These resources replace independently allocated vendor event, queue, task
//! and generic broker-node objects with one affine Rust owner. They contain no
//! HCI Host state and do not make the radio operational; stable ISR placement
//! and powered hardware transitions remain separate stages.
//!
//! The powered task endpoint executes the scheduler executor's list-zero
//! steps: it performs their hardware actions, takes their wait observations,
//! starts the idle scheduler, captures finished lists and publishes the
//! global receive chains.

#![deny(unsafe_code)]

use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerEpoch;

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
use crate::resources::TaskResources;

use crate::{
    interrupt::SchedulerWakeCell,
    modem_lp_timer_queue::{ModemLpTimerEventCell, ModemLpTimerQueue, ModemLpTimerWorkerWakeCell},
    scheduler::SchedulerFinishedListWorker,
};

#[cfg(target_arch = "riscv32")]
use crate::scheduler::hardware::{SchedulerHardware, live::HalPublications};

/// Allocation-free event and worker storage for exactly one Controller epoch.
///
/// The aggregate is intentionally neither `Copy` nor `Clone`. Moving the
/// complete value is allowed before stable ISR publication.
#[must_use = "Controller runtime resources must remain owned by their hardware epoch"]
pub struct ControllerRuntimeResources<const MODEM_TIMER_CAPACITY: usize> {
    scheduler_wake: SchedulerWakeCell,
    scheduler_finished_lists: SchedulerFinishedListWorker,
    #[cfg(target_arch = "riscv32")]
    scheduler_hardware: SchedulerHardware<HalPublications>,
    modem_lp_timer_queue: ModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    modem_lp_timer_epoch: BluetoothModemLpTimerEpoch,
    modem_lp_timer_worker_wake: ModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: ModemLpTimerEventCell,
}

/// Shared interrupt-side publications for one borrowed Controller epoch.
///
/// This endpoint can only be produced together with the matching
/// [`ControllerTaskRuntime`]. It deliberately exposes no timer queue or
/// scheduler state.
#[must_use = "the interrupt endpoint must remain paired with its task endpoint"]
pub struct ControllerInterruptRuntime<'runtime> {
    scheduler_wake: &'runtime SchedulerWakeCell,
    modem_lp_timer_worker_wake: &'runtime ModemLpTimerWorkerWakeCell,
}

/// Unique mutable modem-timer runtime for one Controller epoch.
///
/// This endpoint is disjoint from both interrupt publication and command-task
/// scheduler ownership. It is kept crate-private at the final composition
/// boundary, where stable source-127 storage is joined to it by the typed modem
/// timer task.
#[must_use = "the modem-timer runtime must remain paired with its Controller epoch"]
pub struct ControllerModemTimerRuntime<'runtime, const MODEM_TIMER_CAPACITY: usize> {
    pub(crate) queue: &'runtime mut ModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) epoch: &'runtime mut BluetoothModemLpTimerEpoch,
    pub(crate) worker_wake: &'runtime ModemLpTimerWorkerWakeCell,
    pub(crate) events: &'runtime ModemLpTimerEventCell,
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY> {
    /// Borrow the durable source-127 readiness cell without acquiring work.
    pub const fn worker_wake(&self) -> &ModemLpTimerWorkerWakeCell {
        self.worker_wake
    }

    /// Borrow the durable expiration handoff without acquiring timer ownership.
    pub const fn events(&self) -> &ModemLpTimerEventCell {
        self.events
    }

    /// Whether this endpoint still owns an empty software queue.
    pub fn queue_is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Restore the started modem-timer epoch returned by a physical restart.
    #[cfg(target_arch = "riscv32")]
    pub fn restore_epoch(&mut self, epoch: BluetoothModemLpTimerEpoch) {
        *self.epoch = epoch;
    }
}

impl ControllerInterruptRuntime<'_> {
    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &ModemLpTimerWorkerWakeCell {
        self.modem_lp_timer_worker_wake
    }
}

/// Task-side events and workers for the same borrowed Controller epoch.
///
/// The mutable worker references make a second task endpoint impossible. The
/// scheduler wake is shared only with the matching interrupt endpoint returned
/// by the same [`ControllerRuntimeResources::split`] call.
#[must_use = "the task endpoint must remain paired with its interrupt endpoint"]
pub struct ControllerTaskRuntime<'runtime> {
    scheduler_wake: &'runtime SchedulerWakeCell,
    scheduler_finished_lists: &'runtime mut SchedulerFinishedListWorker,
    #[cfg(target_arch = "riscv32")]
    scheduler_hardware: &'runtime mut SchedulerHardware<HalPublications>,
}

impl ControllerTaskRuntime<'_> {
    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.scheduler_wake
    }

    /// The sole bounded finished-list worker for this epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        self.scheduler_finished_lists
    }

    /// Whether scheduler workers hold no transaction that retirement would
    /// lose.
    #[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
    pub(crate) fn retirement_ready(&self) -> Result<(), ControllerRuntimeRetirementError> {
        if self.scheduler_finished_lists.is_active() {
            return Err(ControllerRuntimeRetirementError::FinishedLists);
        }
        #[cfg(target_arch = "riscv32")]
        if !self.scheduler_hardware.is_quiet() {
            return Err(ControllerRuntimeRetirementError::ListTransaction);
        }
        Ok(())
    }

    /// Coalesced readiness notifications are not work ownership. Discard only
    /// after physical cold release, with the original IRQ routes still absent.
    #[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
    pub(crate) fn clear_notifications_after_cold_release(&self) {
        let _ = self.scheduler_wake.take();
    }
}

/// Task-side software and register ownership for one powered Controller epoch.
///
/// This endpoint is produced only by the initialized scheduler lifecycle. It
/// joins borrowed software workers with an exclusive lease of the task-side HAL
/// slot, so a live worker step never requires an independently recovered
/// register capability.
#[must_use = "the powered task endpoint retains the Controller task owner"]
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub struct ControllerPoweredTaskRuntime<'runtime> {
    pub(crate) runtime: ControllerTaskRuntime<'runtime>,
    pub(crate) task: crate::resources::runtime_owner::RuntimeOwnerLease<'runtime, TaskResources>,
    pub(crate) time_scale: oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale,
    pub(crate) _standalone_dtm_profile:
        &'runtime crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
    pub(crate) config: crate::scheduler::SchedulerSoftwareConfig,
}

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
impl<'runtime> ControllerPoweredTaskRuntime<'runtime> {
    pub(crate) const fn new(
        runtime: ControllerTaskRuntime<'runtime>,
        task: crate::resources::runtime_owner::RuntimeOwnerLease<'runtime, TaskResources>,
        time_scale: oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale,
        standalone_dtm_profile: &'runtime crate::controller_hal::StandaloneAlwaysAwakeDtmProfile,
        config: crate::scheduler::SchedulerSoftwareConfig,
    ) -> Self {
        Self {
            runtime,
            task,
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config,
        }
    }

    /// The HAL task owner lease of this powered Controller epoch.
    pub fn task_owner(
        &mut self,
    ) -> &mut crate::resources::runtime_owner::RuntimeOwnerLease<'runtime, TaskResources> {
        &mut self.task
    }

    #[cfg(test)]
    pub(crate) fn controller_time_phase(
        &self,
    ) -> crate::controller_time::ControllerTimeWorkerPhase {
        self.task.controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) fn controller_time_needs_recheck(&self) -> bool {
        self.task.controller_time_needs_recheck()
    }

    /// Whether scheduler workers hold no transaction that retirement would lose.
    pub fn runtime_retirement_ready(&self) -> Result<(), ControllerRuntimeRetirementError> {
        self.runtime.retirement_ready()
    }

    /// Discard coalesced readiness notifications after physical cold release,
    /// with the original IRQ routes still absent.
    pub fn clear_notifications_after_cold_release(&self) {
        self.runtime.clear_notifications_after_cold_release();
    }

    /// Scheduler time scale retained by this exact powered Controller epoch.
    pub const fn controller_time_scale(
        &self,
    ) -> oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Source-owned scheduler policy retained by this powered epoch.
    pub const fn scheduler_config(&self) -> crate::scheduler::SchedulerSoftwareConfig {
        self.config
    }

    /// Durable scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// The sole bounded finished-list worker for this powered epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerRuntimeResources<MODEM_TIMER_CAPACITY> {
    /// Construct one pristine runtime epoch without allocation or MMIO.
    pub const fn new() -> Self {
        assert!(
            MODEM_TIMER_CAPACITY > 0,
            "a Controller runtime needs at least one modem timer slot"
        );
        Self {
            scheduler_wake: SchedulerWakeCell::new(),
            scheduler_finished_lists: SchedulerFinishedListWorker::new(),
            #[cfg(target_arch = "riscv32")]
            scheduler_hardware: SchedulerHardware::new(),
            modem_lp_timer_queue: ModemLpTimerQueue::new(),
            modem_lp_timer_epoch: BluetoothModemLpTimerEpoch::new(),
            modem_lp_timer_worker_wake: ModemLpTimerWorkerWakeCell::new(),
            modem_lp_timer_events: ModemLpTimerEventCell::new(),
        }
    }

    /// Number of fixed modem timer slots owned by this epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        MODEM_TIMER_CAPACITY
    }

    /// Whether no event, completion drain, list transaction or timer has
    /// entered this epoch yet.
    pub fn is_pristine(&self) -> bool {
        #[cfg(target_arch = "riscv32")]
        let scheduler_hardware_is_pristine =
            self.scheduler_hardware.is_quiet() && !self.scheduler_hardware.has_run();
        #[cfg(not(target_arch = "riscv32"))]
        let scheduler_hardware_is_pristine = true;

        !self.scheduler_wake.is_pending()
            && !self.scheduler_finished_lists.is_active()
            && scheduler_hardware_is_pristine
            && self.modem_lp_timer_queue.is_empty()
            && self.modem_lp_timer_epoch.high_byte() == 0
            && !self.modem_lp_timer_worker_wake.is_pending()
            && !self.modem_lp_timer_events.is_pending()
    }

    /// Borrow the only interrupt publisher and task worker endpoints for this
    /// runtime epoch.
    ///
    /// Keeping both endpoints alive retains the mutable borrow of the
    /// aggregate, so the same event cells and workers cannot be split into a
    /// second executor/interrupt pair.
    pub fn split(
        &mut self,
    ) -> (
        ControllerInterruptRuntime<'_>,
        ControllerTaskRuntime<'_>,
        ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
    ) {
        let scheduler_wake = &self.scheduler_wake;
        let modem_lp_timer_worker_wake = &self.modem_lp_timer_worker_wake;
        let modem_lp_timer_events = &self.modem_lp_timer_events;
        (
            ControllerInterruptRuntime {
                scheduler_wake,
                modem_lp_timer_worker_wake,
            },
            ControllerTaskRuntime {
                scheduler_wake,
                scheduler_finished_lists: &mut self.scheduler_finished_lists,
                #[cfg(target_arch = "riscv32")]
                scheduler_hardware: &mut self.scheduler_hardware,
            },
            ControllerModemTimerRuntime {
                queue: &mut self.modem_lp_timer_queue,
                epoch: &mut self.modem_lp_timer_epoch,
                worker_wake: modem_lp_timer_worker_wake,
                events: modem_lp_timer_events,
            },
        )
    }
}

impl<const MODEM_TIMER_CAPACITY: usize> Default
    for ControllerRuntimeResources<MODEM_TIMER_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "riscv32")]
impl<const MT: usize> ControllerRuntimeResources<MT> {
    pub fn into_started_modem_epoch(self) -> BluetoothModemLpTimerEpoch {
        self.modem_lp_timer_epoch
    }
}

/// Actual software ownership preventing a cold restart boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRuntimeRetirementError {
    /// A captured finished-list set is not drained.
    FinishedLists,
    /// A list transaction holds a hardware publication.
    ListTransaction,
}

#[cfg(target_arch = "riscv32")]
mod scheduler;

#[cfg(target_arch = "riscv32")]
pub use scheduler::ControllerRxChainError;

#[cfg(test)]
mod tests;
