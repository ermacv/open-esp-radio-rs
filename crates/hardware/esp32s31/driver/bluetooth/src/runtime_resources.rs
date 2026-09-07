//! Fixed no-RTOS software state for one future powered Controller epoch.
//!
//! These resources replace independently allocated vendor event, queue, task
//! and generic broker-node objects with one affine Rust owner. They contain no
//! HCI Host state and do not make the radio operational; stable ISR placement,
//! scheduler-item hardware publication and powered hardware transitions remain
//! separate stages.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerEpoch;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListHeadPublished, BluetoothSchedulerHardwareRunCommandPublished,
    BluetoothSchedulerRunEventPublished, BluetoothSchedulerRunInterruptsPrepared,
};

#[cfg(any(target_arch = "riscv32", test))]
use crate::{
    resources::TaskResources,
    scheduler::{core::SchedulerExclusiveListEpoch, timeline::SchedulerTimeline},
};

use crate::{
    interrupt::SchedulerWakeCell,
    modem_lp_timer_queue::{ModemLpTimerEventCell, ModemLpTimerQueue, ModemLpTimerWorkerWakeCell},
    scheduler::{
        SchedulerFinishedListWorker, SchedulerLockModifyEventCell, SchedulerLockModifyWorker,
    },
};

/// Allocation-free event and worker storage for exactly one Controller epoch.
///
/// The aggregate is intentionally neither `Copy` nor `Clone`. Moving the
/// complete value is allowed before stable ISR publication; splitting or
/// pinning it for a live route will be a later consuming transition.
#[must_use = "Controller runtime resources must remain owned by their hardware epoch"]
pub struct ControllerRuntimeResources<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize = 4,
> {
    scheduler_wake: SchedulerWakeCell,
    scheduler_lock_modify_events: SchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: SchedulerLockModifyWorker,
    scheduler_finished_lists: SchedulerFinishedListWorker,
    #[cfg(any(target_arch = "riscv32", test))]
    scheduler_timeline: SchedulerTimeline<SCHEDULER_CAPACITY>,
    modem_lp_timer_queue: ModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    modem_lp_timer_epoch: BluetoothModemLpTimerEpoch,
    modem_lp_timer_worker_wake: ModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: ModemLpTimerEventCell,
}

/// Shared interrupt-side publications for one borrowed Controller epoch.
///
/// This endpoint can only be produced together with the matching
/// [`ControllerTaskRuntime`]. It deliberately exposes no timer queue
/// or scheduler item storage: scheduler reservations are exclusively owned by
/// the matching task endpoint.
#[must_use = "the interrupt endpoint must remain paired with its task endpoint"]
pub struct ControllerInterruptRuntime<'runtime> {
    scheduler_wake: &'runtime SchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime SchedulerLockModifyEventCell,
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
}

impl ControllerInterruptRuntime<'_> {
    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &SchedulerLockModifyEventCell {
        self.scheduler_lock_modify_events
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &ModemLpTimerWorkerWakeCell {
        self.modem_lp_timer_worker_wake
    }
}

/// Task-side events and workers for the same borrowed Controller epoch.
///
/// The mutable worker references make a second task endpoint impossible. The
/// scheduler event cells are shared only with the matching interrupt endpoint
/// returned by the same [`ControllerRuntimeResources::split`] call.
#[must_use = "the task endpoint must remain paired with its interrupt endpoint"]
pub struct ControllerTaskRuntime<'runtime, const SCHEDULER_CAPACITY: usize = 4> {
    scheduler_wake: &'runtime SchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime SchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: &'runtime mut SchedulerLockModifyWorker,
    scheduler_finished_lists: &'runtime mut SchedulerFinishedListWorker,
    #[cfg(any(target_arch = "riscv32", test))]
    scheduler_timeline: &'runtime mut SchedulerTimeline<SCHEDULER_CAPACITY>,
}

impl<const SCHEDULER_CAPACITY: usize> ControllerTaskRuntime<'_, SCHEDULER_CAPACITY> {
    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &SchedulerLockModifyEventCell {
        self.scheduler_lock_modify_events
    }

    /// The sole scheduler lock/modify worker for this epoch.
    pub fn scheduler_lock_modify_worker(&mut self) -> &mut SchedulerLockModifyWorker {
        self.scheduler_lock_modify_worker
    }

    /// The sole bounded finished-list worker for this epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        self.scheduler_finished_lists
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn scheduler_finished_lists_mut(&mut self) -> &mut SchedulerFinishedListWorker {
        self.scheduler_finished_lists
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn scheduler_timeline_mut(&mut self) -> &mut SchedulerTimeline<SCHEDULER_CAPACITY> {
        self.scheduler_timeline
    }
}

/// Task-side software and register ownership for one powered Controller epoch.
///
/// This endpoint is produced only by the initialized scheduler lifecycle. It
/// joins the sole software workers with the exact task-side HAL
/// owner, so a live worker step never requires an independently recovered
/// register capability. The software-only [`ControllerTaskRuntime`]
/// remains useful to executor adapters that do not perform hardware work.
#[must_use = "the powered task endpoint retains the Controller task owner"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct ControllerPoweredTaskRuntime<'runtime, const SCHEDULER_CAPACITY: usize = 4> {
    pub(crate) runtime: ControllerTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    pub(crate) task: &'runtime mut TaskResources,
    pub(crate) time_scale: oer_esp32s31_pac::BluetoothControllerTimeScale,
    pub(crate) _standalone_dtm_profile:
        &'runtime crate::controller::hal::StandaloneAlwaysAwakeDtmProfile,
    pub(crate) config: crate::scheduler::SchedulerSoftwareConfig,
    pub(crate) _scheduler_list: &'runtime mut SchedulerExclusiveListEpoch,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize> ControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY> {
    pub(crate) const fn new<'runtime>(
        runtime: ControllerTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
        task: &'runtime mut TaskResources,
        time_scale: oer_esp32s31_pac::BluetoothControllerTimeScale,
        standalone_dtm_profile: &'runtime crate::controller::hal::StandaloneAlwaysAwakeDtmProfile,
        config: crate::scheduler::SchedulerSoftwareConfig,
        scheduler_list: &'runtime mut SchedulerExclusiveListEpoch,
    ) -> ControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY> {
        ControllerPoweredTaskRuntime {
            runtime,
            task,
            time_scale,
            _standalone_dtm_profile: standalone_dtm_profile,
            config,
            _scheduler_list: scheduler_list,
        }
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

    /// Scheduler time scale retained by this exact powered Controller epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn controller_time_scale(
        &self,
    ) -> oer_esp32s31_pac::BluetoothControllerTimeScale {
        self.time_scale
    }

    /// Source-owned scheduler policy retained by this powered epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn scheduler_config(&self) -> crate::scheduler::SchedulerSoftwareConfig {
        self.config
    }

    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &SchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &SchedulerLockModifyEventCell {
        self.runtime.scheduler_lock_modify_events()
    }

    /// The sole bounded finished-list worker for this powered epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut SchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }

    /// Advance one scheduler lock/modify transaction using the matching HAL
    /// task owner and exactly one interrupt-side observation.
    ///
    /// This operation is finite. A controller-owned wait returns to the caller;
    /// no polling loop or executor-specific wake primitive is hidden here.
    #[cfg(target_arch = "riscv32")]
    pub fn step_scheduler_lock_modify(
        &mut self,
        event: crate::scheduler::SchedulerLockModifyEvent,
    ) -> crate::scheduler::SchedulerLockModifyWorkerStep {
        self.task
            .step_scheduler_lock_modify(self.runtime.scheduler_lock_modify_worker, event)
    }

    /// Publish the synchronous scheduler event after the matching hardware head
    /// and dynamic-interrupt preparation proofs have both been obtained.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn publish_scheduler_run_event(
        &mut self,
        head: BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> BluetoothSchedulerRunEventPublished {
        self.task.publish_scheduler_run_event(head, interrupts)
    }

    /// Consume one scheduler-event proof into the typed hardware RUN command.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn publish_scheduler_hardware_run_command(
        &mut self,
        event: BluetoothSchedulerRunEventPublished,
    ) -> BluetoothSchedulerHardwareRunCommandPublished {
        self.task.publish_scheduler_hardware_run_command(event)
    }

    /// Advance the exclusive source-owned list identity across the matching RUN
    /// edge.
    ///
    /// The list owner is borrowed into this task endpoint by the same scheduler
    /// split as the HAL task owner. A mismatched address therefore fails closed
    /// against the retained published-head identity.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn retain_running_first_item(
        &mut self,
        address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
    ) {
        self._scheduler_list.retain_running_first_item(address);
    }
}

impl<const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    /// Construct one pristine runtime epoch without allocation or MMIO.
    pub const fn new() -> Self {
        assert!(
            MODEM_TIMER_CAPACITY > 0,
            "a Controller runtime needs at least one modem timer slot"
        );
        assert!(
            SCHEDULER_CAPACITY > 0,
            "a Controller runtime needs at least one scheduler slot"
        );
        Self {
            scheduler_wake: SchedulerWakeCell::new(),
            scheduler_lock_modify_events: SchedulerLockModifyEventCell::new(),
            scheduler_lock_modify_worker: SchedulerLockModifyWorker::new(),
            scheduler_finished_lists: SchedulerFinishedListWorker::new(),
            #[cfg(any(target_arch = "riscv32", test))]
            scheduler_timeline: SchedulerTimeline::new(),
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

    /// Number of fixed software scheduler slots owned by this epoch.
    pub const fn scheduler_capacity(&self) -> usize {
        SCHEDULER_CAPACITY
    }

    /// Whether no event, request, completion drain or timer has entered this
    /// epoch yet.
    pub fn is_pristine(&self) -> bool {
        #[cfg(any(target_arch = "riscv32", test))]
        let scheduler_timeline_is_empty = self.scheduler_timeline.is_empty();
        #[cfg(not(any(target_arch = "riscv32", test)))]
        let scheduler_timeline_is_empty = true;

        !self.scheduler_wake.is_pending()
            && !self.scheduler_lock_modify_events.is_pending()
            && self.scheduler_lock_modify_worker.is_idle()
            && !self.scheduler_finished_lists.is_active()
            && scheduler_timeline_is_empty
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
        ControllerTaskRuntime<'_, SCHEDULER_CAPACITY>,
        ControllerModemTimerRuntime<'_, MODEM_TIMER_CAPACITY>,
    ) {
        let scheduler_wake = &self.scheduler_wake;
        let scheduler_lock_modify_events = &self.scheduler_lock_modify_events;
        let modem_lp_timer_worker_wake = &self.modem_lp_timer_worker_wake;
        let modem_lp_timer_events = &self.modem_lp_timer_events;
        (
            ControllerInterruptRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                modem_lp_timer_worker_wake,
            },
            ControllerTaskRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                scheduler_lock_modify_worker: &mut self.scheduler_lock_modify_worker,
                scheduler_finished_lists: &mut self.scheduler_finished_lists,
                #[cfg(any(target_arch = "riscv32", test))]
                scheduler_timeline: &mut self.scheduler_timeline,
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

impl<const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize> Default
    for ControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
