//! Fixed no-RTOS software state for one future powered Controller epoch.
//!
//! These resources replace independently allocated vendor event, queue, task
//! and generic broker-node objects with one affine Rust owner. They contain no
//! HCI Host state and do not make the radio operational; stable ISR placement,
//! scheduler-item hardware publication and powered hardware transitions remain
//! separate stages.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerEpoch;

#[cfg(any(target_arch = "riscv32", test))]
use crate::resources::BluetoothTaskResources;
#[cfg(any(target_arch = "riscv32", test))]
use crate::scheduler_timeline::BluetoothSchedulerTimeline;
use crate::{
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerQueue,
    BluetoothModemLpTimerWorkerWakeCell, BluetoothSchedulerFinishedListWorker,
    BluetoothSchedulerLockModifyEventCell, BluetoothSchedulerLockModifyWorker,
    BluetoothSchedulerWakeCell,
};

/// Allocation-free event and worker storage for exactly one Controller epoch.
///
/// The aggregate is intentionally neither `Copy` nor `Clone`. Moving the
/// complete value is allowed before stable ISR publication; splitting or
/// pinning it for a live route will be a later consuming transition.
#[must_use = "Controller runtime resources must remain owned by their hardware epoch"]
pub struct BluetoothControllerRuntimeResources<
    const MODEM_TIMER_CAPACITY: usize,
    const SCHEDULER_CAPACITY: usize = 4,
> {
    scheduler_wake: BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: BluetoothSchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: BluetoothSchedulerLockModifyWorker,
    scheduler_finished_lists: BluetoothSchedulerFinishedListWorker,
    #[cfg(any(target_arch = "riscv32", test))]
    scheduler_timeline: BluetoothSchedulerTimeline<SCHEDULER_CAPACITY>,
    modem_lp_timer_queue: BluetoothModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    modem_lp_timer_epoch: BluetoothModemLpTimerEpoch,
    modem_lp_timer_worker_wake: BluetoothModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: BluetoothModemLpTimerEventCell,
}

/// Shared interrupt-side publications for one borrowed Controller epoch.
///
/// This endpoint can only be produced together with the matching
/// [`BluetoothControllerTaskRuntime`]. It deliberately exposes no timer queue
/// or scheduler item storage: scheduler reservations are exclusively owned by
/// the matching task endpoint.
#[must_use = "the interrupt endpoint must remain paired with its task endpoint"]
pub struct BluetoothControllerInterruptRuntime<'runtime> {
    scheduler_wake: &'runtime BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime BluetoothSchedulerLockModifyEventCell,
    modem_lp_timer_worker_wake: &'runtime BluetoothModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: &'runtime BluetoothModemLpTimerEventCell,
}

impl BluetoothControllerInterruptRuntime<'_> {
    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &BluetoothSchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &BluetoothSchedulerLockModifyEventCell {
        self.scheduler_lock_modify_events
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &BluetoothModemLpTimerWorkerWakeCell {
        self.modem_lp_timer_worker_wake
    }

    /// Durable modem LP timer expiration handoff for this epoch.
    pub const fn modem_lp_timer_events(&self) -> &BluetoothModemLpTimerEventCell {
        self.modem_lp_timer_events
    }
}

/// Task-side events and workers for the same borrowed Controller epoch.
///
/// The mutable worker references make a second task endpoint impossible. The
/// event cells are shared only with the matching interrupt endpoint returned
/// by the same [`BluetoothControllerRuntimeResources::split`] call.
#[must_use = "the task endpoint must remain paired with its interrupt endpoint"]
pub struct BluetoothControllerTaskRuntime<'runtime, const SCHEDULER_CAPACITY: usize = 4> {
    scheduler_wake: &'runtime BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime BluetoothSchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: &'runtime mut BluetoothSchedulerLockModifyWorker,
    scheduler_finished_lists: &'runtime mut BluetoothSchedulerFinishedListWorker,
    modem_lp_timer_worker_wake: &'runtime BluetoothModemLpTimerWorkerWakeCell,
    modem_lp_timer_events: &'runtime BluetoothModemLpTimerEventCell,
}

impl<const SCHEDULER_CAPACITY: usize> BluetoothControllerTaskRuntime<'_, SCHEDULER_CAPACITY> {
    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &BluetoothSchedulerWakeCell {
        self.scheduler_wake
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &BluetoothSchedulerLockModifyEventCell {
        self.scheduler_lock_modify_events
    }

    /// The sole scheduler lock/modify worker for this epoch.
    pub fn scheduler_lock_modify_worker(&mut self) -> &mut BluetoothSchedulerLockModifyWorker {
        self.scheduler_lock_modify_worker
    }

    /// The sole bounded finished-list worker for this epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut BluetoothSchedulerFinishedListWorker {
        self.scheduler_finished_lists
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &BluetoothModemLpTimerWorkerWakeCell {
        self.modem_lp_timer_worker_wake
    }

    /// Durable modem LP timer expiration handoff for this epoch.
    pub const fn modem_lp_timer_events(&self) -> &BluetoothModemLpTimerEventCell {
        self.modem_lp_timer_events
    }
}

/// Task-side software and register ownership for one powered Controller epoch.
///
/// This endpoint is produced only by the initialized scheduler lifecycle. It
/// joins the sole software workers with the exact task-side HAL
/// owner, so a live worker step never requires an independently recovered
/// register capability. The software-only [`BluetoothControllerTaskRuntime`]
/// remains useful to executor adapters that do not perform hardware work.
#[must_use = "the powered task endpoint retains the Controller task owner"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct BluetoothControllerPoweredTaskRuntime<'runtime, const SCHEDULER_CAPACITY: usize = 4> {
    software: BluetoothControllerTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
    hardware: &'runtime mut BluetoothTaskResources,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPoweredTaskRuntime<'_, SCHEDULER_CAPACITY>
{
    pub(crate) const fn new<'runtime>(
        software: BluetoothControllerTaskRuntime<'runtime, SCHEDULER_CAPACITY>,
        hardware: &'runtime mut BluetoothTaskResources,
    ) -> BluetoothControllerPoweredTaskRuntime<'runtime, SCHEDULER_CAPACITY> {
        BluetoothControllerPoweredTaskRuntime { software, hardware }
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_phase(
        &self,
    ) -> crate::controller_time::BluetoothControllerTimeWorkerPhase {
        self.hardware.controller_time_phase()
    }

    #[cfg(test)]
    pub(crate) const fn controller_time_needs_recheck(&self) -> bool {
        self.hardware.controller_time_needs_recheck()
    }

    /// Durable general scheduler handoff for this epoch.
    pub const fn scheduler_wake(&self) -> &BluetoothSchedulerWakeCell {
        self.software.scheduler_wake()
    }

    /// Durable scheduler lock/modify handoff for this epoch.
    pub const fn scheduler_lock_modify_events(&self) -> &BluetoothSchedulerLockModifyEventCell {
        self.software.scheduler_lock_modify_events()
    }

    /// The sole bounded finished-list worker for this powered epoch.
    pub fn scheduler_finished_lists(&mut self) -> &mut BluetoothSchedulerFinishedListWorker {
        self.software.scheduler_finished_lists()
    }

    /// Durable source-127 task-readiness handoff for this epoch.
    pub const fn modem_lp_timer_worker_wake(&self) -> &BluetoothModemLpTimerWorkerWakeCell {
        self.software.modem_lp_timer_worker_wake()
    }

    /// Durable modem LP timer expiration handoff for this epoch.
    pub const fn modem_lp_timer_events(&self) -> &BluetoothModemLpTimerEventCell {
        self.software.modem_lp_timer_events()
    }

    /// Advance one scheduler lock/modify transaction using the matching HAL
    /// task owner and exactly one interrupt-side observation.
    ///
    /// This operation is finite. A hardware-owned wait returns to the caller;
    /// no polling loop or executor-specific wake primitive is hidden here.
    #[cfg(target_arch = "riscv32")]
    pub fn step_scheduler_lock_modify(
        &mut self,
        event: crate::BluetoothSchedulerLockModifyEvent,
    ) -> crate::BluetoothSchedulerLockModifyWorkerStep {
        self.hardware
            .step_scheduler_lock_modify(self.software.scheduler_lock_modify_worker, event)
    }
}

impl<const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
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
            scheduler_wake: BluetoothSchedulerWakeCell::new(),
            scheduler_lock_modify_events: BluetoothSchedulerLockModifyEventCell::new(),
            scheduler_lock_modify_worker: BluetoothSchedulerLockModifyWorker::new(),
            scheduler_finished_lists: BluetoothSchedulerFinishedListWorker::new(),
            #[cfg(any(target_arch = "riscv32", test))]
            scheduler_timeline: BluetoothSchedulerTimeline::new(),
            modem_lp_timer_queue: BluetoothModemLpTimerQueue::new(),
            modem_lp_timer_epoch: BluetoothModemLpTimerEpoch::new(),
            modem_lp_timer_worker_wake: BluetoothModemLpTimerWorkerWakeCell::new(),
            modem_lp_timer_events: BluetoothModemLpTimerEventCell::new(),
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

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn scheduler_timeline_mut(
        &mut self,
    ) -> &mut BluetoothSchedulerTimeline<SCHEDULER_CAPACITY> {
        &mut self.scheduler_timeline
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
        BluetoothControllerInterruptRuntime<'_>,
        BluetoothControllerTaskRuntime<'_, SCHEDULER_CAPACITY>,
    ) {
        let scheduler_wake = &self.scheduler_wake;
        let scheduler_lock_modify_events = &self.scheduler_lock_modify_events;
        let modem_lp_timer_worker_wake = &self.modem_lp_timer_worker_wake;
        let modem_lp_timer_events = &self.modem_lp_timer_events;
        (
            BluetoothControllerInterruptRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                modem_lp_timer_worker_wake,
                modem_lp_timer_events,
            },
            BluetoothControllerTaskRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                scheduler_lock_modify_worker: &mut self.scheduler_lock_modify_worker,
                scheduler_finished_lists: &mut self.scheduler_finished_lists,
                modem_lp_timer_worker_wake,
                modem_lp_timer_events,
            },
        )
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn primary_interrupt_publications(
        &self,
    ) -> (
        &BluetoothSchedulerWakeCell,
        &BluetoothSchedulerLockModifyEventCell,
    ) {
        (&self.scheduler_wake, &self.scheduler_lock_modify_events)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn modem_lp_timer_worker_wake(&self) -> &BluetoothModemLpTimerWorkerWakeCell {
        &self.modem_lp_timer_worker_wake
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn modem_lp_timer_software_parts_mut(
        &mut self,
    ) -> (
        &mut BluetoothModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
        &mut BluetoothModemLpTimerEpoch,
        &BluetoothModemLpTimerEventCell,
    ) {
        (
            &mut self.modem_lp_timer_queue,
            &mut self.modem_lp_timer_epoch,
            &self.modem_lp_timer_events,
        )
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn scheduler_finished_lists_mut(
        &mut self,
    ) -> &mut BluetoothSchedulerFinishedListWorker {
        &mut self.scheduler_finished_lists
    }
}

impl<const MODEM_TIMER_CAPACITY: usize, const SCHEDULER_CAPACITY: usize> Default
    for BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY, SCHEDULER_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::BluetoothControllerRuntimeResources;
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRxRecurringEventWindow, BluetoothDtmSchedulerInstant,
        BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerMargin,
        BluetoothDtmSchedulerTimingPolicy, BluetoothSchedulerSoftwareConfig,
    };

    #[test]
    fn one_aggregate_starts_as_one_pristine_bounded_epoch() {
        let resources = BluetoothControllerRuntimeResources::<4, 3>::new();

        assert_eq!(resources.modem_timer_capacity(), 4);
        assert_eq!(resources.scheduler_capacity(), 3);
        assert!(resources.is_pristine());
    }

    #[test]
    #[should_panic(expected = "at least one modem timer slot")]
    fn zero_modem_timer_capacity_profile_is_rejected() {
        let _resources = BluetoothControllerRuntimeResources::<0, 1>::new();
    }

    #[test]
    #[should_panic(expected = "at least one scheduler slot")]
    fn zero_scheduler_capacity_profile_is_rejected() {
        let _resources = BluetoothControllerRuntimeResources::<1, 0>::new();
    }

    #[test]
    fn split_borrows_one_matching_interrupt_and_task_epoch() {
        let mut resources = BluetoothControllerRuntimeResources::<4, 3>::new();
        let (interrupt, mut task) = resources.split();

        assert!(core::ptr::eq(
            interrupt.scheduler_wake(),
            task.scheduler_wake()
        ));
        assert!(core::ptr::eq(
            interrupt.scheduler_lock_modify_events(),
            task.scheduler_lock_modify_events()
        ));
        assert!(core::ptr::eq(
            interrupt.modem_lp_timer_events(),
            task.modem_lp_timer_events()
        ));
        assert!(core::ptr::eq(
            interrupt.modem_lp_timer_worker_wake(),
            task.modem_lp_timer_worker_wake()
        ));
        assert!(task.scheduler_lock_modify_worker().is_idle());
        assert!(!task.scheduler_finished_lists().is_active());
        drop((interrupt, task));
        assert!(resources.scheduler_timeline_mut().is_empty());
    }

    #[test]
    fn controller_reservation_remains_in_the_runtime_epoch_until_explicit_release() {
        let mut resources = BluetoothControllerRuntimeResources::<4, 2>::new();
        let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
        let epoch = BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(0),
            0,
            scale,
        );
        let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
            BluetoothDtmChannel::new(0).expect("channel zero is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRxRecurringEventWindow::new(
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothDtmSchedulerInstant::from_image(45),
                BluetoothDtmSchedulerInstant::from_image(100),
                BluetoothDtmSchedulerMargin::from_image(0),
            ),
        )
        .expect("receiver event is valid");
        let timing_policy = BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            scale,
        );

        let reservation = resources
            .scheduler_timeline_mut()
            .reserve_recurring_dtm_event(event, epoch, timing_policy)
            .expect("one runtime-owned scheduler slot is free");
        assert!(!resources.is_pristine());

        assert!(
            resources
                .scheduler_timeline_mut()
                .release(reservation)
                .is_ok()
        );
        assert!(resources.is_pristine());
    }
}
