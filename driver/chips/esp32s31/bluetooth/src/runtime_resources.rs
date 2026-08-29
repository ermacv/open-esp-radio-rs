//! Fixed no-RTOS software state for one future powered Controller epoch.
//!
//! These resources replace independently allocated vendor event, queue, task
//! and generic broker-node objects with one affine Rust owner. They contain no
//! HCI Host state and do not make the radio operational; the open scheduler
//! item queue, stable ISR placement and powered hardware transitions remain
//! separate stages.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerEpoch;

use crate::{
    BluetoothModemLpTimerEventCell, BluetoothModemLpTimerQueue,
    BluetoothSchedulerFinishedListWorker, BluetoothSchedulerLockModifyEventCell,
    BluetoothSchedulerLockModifyWorker, BluetoothSchedulerWakeCell,
};

/// Allocation-free event and worker storage for exactly one Controller epoch.
///
/// The aggregate is intentionally neither `Copy` nor `Clone`. Moving the
/// complete value is allowed before stable ISR publication; splitting or
/// pinning it for a live route will be a later consuming transition.
#[must_use = "Controller runtime resources must remain owned by their hardware epoch"]
pub struct BluetoothControllerRuntimeResources<const MODEM_TIMER_CAPACITY: usize> {
    scheduler_wake: BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: BluetoothSchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: BluetoothSchedulerLockModifyWorker,
    scheduler_finished_lists: BluetoothSchedulerFinishedListWorker,
    modem_lp_timer_queue: BluetoothModemLpTimerQueue<MODEM_TIMER_CAPACITY>,
    modem_lp_timer_epoch: BluetoothModemLpTimerEpoch,
    modem_lp_timer_events: BluetoothModemLpTimerEventCell,
}

/// Shared interrupt-side publications for one borrowed Controller epoch.
///
/// This endpoint can only be produced together with the matching
/// [`BluetoothControllerTaskRuntime`]. It deliberately exposes no timer queue
/// or scheduler item storage: their interrupt/task synchronization contract
/// has not been proven yet.
#[must_use = "the interrupt endpoint must remain paired with its task endpoint"]
pub struct BluetoothControllerInterruptRuntime<'runtime> {
    scheduler_wake: &'runtime BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime BluetoothSchedulerLockModifyEventCell,
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
pub struct BluetoothControllerTaskRuntime<'runtime> {
    scheduler_wake: &'runtime BluetoothSchedulerWakeCell,
    scheduler_lock_modify_events: &'runtime BluetoothSchedulerLockModifyEventCell,
    scheduler_lock_modify_worker: &'runtime mut BluetoothSchedulerLockModifyWorker,
    scheduler_finished_lists: &'runtime mut BluetoothSchedulerFinishedListWorker,
    modem_lp_timer_events: &'runtime BluetoothModemLpTimerEventCell,
}

impl BluetoothControllerTaskRuntime<'_> {
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

    /// Durable modem LP timer expiration handoff for this epoch.
    pub const fn modem_lp_timer_events(&self) -> &BluetoothModemLpTimerEventCell {
        self.modem_lp_timer_events
    }
}

impl<const MODEM_TIMER_CAPACITY: usize> BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY> {
    /// Construct one pristine runtime epoch without allocation or MMIO.
    pub const fn new() -> Self {
        assert!(
            MODEM_TIMER_CAPACITY > 0,
            "a Controller runtime needs at least one modem timer slot"
        );
        Self {
            scheduler_wake: BluetoothSchedulerWakeCell::new(),
            scheduler_lock_modify_events: BluetoothSchedulerLockModifyEventCell::new(),
            scheduler_lock_modify_worker: BluetoothSchedulerLockModifyWorker::new(),
            scheduler_finished_lists: BluetoothSchedulerFinishedListWorker::new(),
            modem_lp_timer_queue: BluetoothModemLpTimerQueue::new(),
            modem_lp_timer_epoch: BluetoothModemLpTimerEpoch::new(),
            modem_lp_timer_events: BluetoothModemLpTimerEventCell::new(),
        }
    }

    /// Number of fixed modem timer slots owned by this epoch.
    pub const fn modem_timer_capacity(&self) -> usize {
        MODEM_TIMER_CAPACITY
    }

    /// Whether no event, request, completion drain or timer has entered this
    /// epoch yet.
    pub fn is_pristine(&self) -> bool {
        !self.scheduler_wake.is_pending()
            && !self.scheduler_lock_modify_events.is_pending()
            && self.scheduler_lock_modify_worker.is_idle()
            && !self.scheduler_finished_lists.is_active()
            && self.modem_lp_timer_queue.is_empty()
            && self.modem_lp_timer_epoch.high_byte() == 0
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
        BluetoothControllerTaskRuntime<'_>,
    ) {
        let scheduler_wake = &self.scheduler_wake;
        let scheduler_lock_modify_events = &self.scheduler_lock_modify_events;
        let modem_lp_timer_events = &self.modem_lp_timer_events;
        (
            BluetoothControllerInterruptRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                modem_lp_timer_events,
            },
            BluetoothControllerTaskRuntime {
                scheduler_wake,
                scheduler_lock_modify_events,
                scheduler_lock_modify_worker: &mut self.scheduler_lock_modify_worker,
                scheduler_finished_lists: &mut self.scheduler_finished_lists,
                modem_lp_timer_events,
            },
        )
    }
}

impl<const MODEM_TIMER_CAPACITY: usize> Default
    for BluetoothControllerRuntimeResources<MODEM_TIMER_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::BluetoothControllerRuntimeResources;

    #[test]
    fn one_aggregate_starts_as_one_pristine_bounded_epoch() {
        let resources = BluetoothControllerRuntimeResources::<4>::new();

        assert_eq!(resources.modem_timer_capacity(), 4);
        assert!(resources.is_pristine());
    }

    #[test]
    #[should_panic(expected = "at least one modem timer slot")]
    fn zero_capacity_profile_is_rejected() {
        let _resources = BluetoothControllerRuntimeResources::<0>::new();
    }

    #[test]
    fn split_borrows_one_matching_interrupt_and_task_epoch() {
        let mut resources = BluetoothControllerRuntimeResources::<4>::new();
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
        assert!(task.scheduler_lock_modify_worker().is_idle());
        assert!(!task.scheduler_finished_lists().is_active());
    }
}
