//! Source-owned, bounded software queue for the modem low-power timer.
//!
//! This replaces the reference Controller's callback timer list with fixed
//! Rust-owned state. Queue mutation is serialized by the unique software-work
//! owner; expiration delivery crosses an atomic one-event backpressure cell.
//! No operation allocates, polls hardware in a loop or depends on an RTOS.

#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};

use open_esp_radio_esp32s31_hal::{
    BluetoothModemLpTimerCompareDisposition, BluetoothModemLpTimerEpoch,
    BluetoothModemLpTimerHandlerRegisterStep, BluetoothModemLpTimerInstant,
    BluetoothModemLpTimerInterruptReadyOwner, BluetoothModemLpTimerInterruptStep,
    BluetoothModemLpTimerSoftwarePendingOwner,
};

const EVENT_EMPTY: u8 = 0;
const EVENT_WRITING: u8 = 1;
const EVENT_READY: u8 = 2;
const EVENT_READING: u8 = 3;
const MAX_FORWARD_DELAY: u32 = i32::MAX as u32;

/// Result of publishing source-127 task readiness from interrupt context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerWorkerWakePublication {
    /// This entry opened a new wake epoch and must notify the worker.
    WakeWorker,
    /// Task readiness was already durable; an additional notification is not required.
    Coalesced,
}

/// Durable, coalescing source-127 ISR-to-task readiness cell.
///
/// Stable platform storage retains the affine software-pending timer owner;
/// this cell independently retains the fact that task context must acquire it.
/// A worker can therefore register after the interrupt without losing work.
pub struct BluetoothModemLpTimerWorkerWakeCell {
    pending: AtomicBool,
}

impl BluetoothModemLpTimerWorkerWakeCell {
    /// Construct one idle wake epoch.
    pub const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    fn publish_from_interrupt(&self) -> BluetoothModemLpTimerWorkerWakePublication {
        if self.pending.swap(true, Ordering::AcqRel) {
            BluetoothModemLpTimerWorkerWakePublication::Coalesced
        } else {
            BluetoothModemLpTimerWorkerWakePublication::WakeWorker
        }
    }

    /// Close the current wake epoch after task context acquired the owner.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }

    /// Whether stable source-127 task work is waiting for acquisition.
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

impl Default for BluetoothModemLpTimerWorkerWakeCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact stable-storage result of one finite source-127 interrupt entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerStableInterruptStep {
    /// The interrupt status was empty and the ready owner was restored.
    Spurious,
    /// Register work completed and restored the ready owner.
    Rearmed,
    /// A preceding entry already left task-owned work in stable storage.
    AwaitingSoftware,
    /// This entry moved the owner into stable software-pending state.
    SoftwarePending,
}

/// Controller publication result of one finite source-127 interrupt entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a source-127 wake disposition must reach the executor boundary"]
pub enum BluetoothModemLpTimerPublishedInterruptStep {
    /// No task work was created by an empty status observation.
    Spurious,
    /// Register work completed without task work.
    Rearmed,
    /// Existing task work remains durable with this wake disposition.
    AwaitingSoftware(BluetoothModemLpTimerWorkerWakePublication),
    /// Newly pending task work is durable with this wake disposition.
    SoftwarePending(BluetoothModemLpTimerWorkerWakePublication),
}

impl BluetoothModemLpTimerStableInterruptStep {
    /// Publish task readiness while preserving the exact register disposition.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn publish(
        self,
        worker_wake: &BluetoothModemLpTimerWorkerWakeCell,
    ) -> BluetoothModemLpTimerPublishedInterruptStep {
        match self {
            Self::Spurious => BluetoothModemLpTimerPublishedInterruptStep::Spurious,
            Self::Rearmed => BluetoothModemLpTimerPublishedInterruptStep::Rearmed,
            Self::AwaitingSoftware => {
                BluetoothModemLpTimerPublishedInterruptStep::AwaitingSoftware(
                    worker_wake.publish_from_interrupt(),
                )
            }
            Self::SoftwarePending => BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
                worker_wake.publish_from_interrupt(),
            ),
        }
    }
}

#[derive(Clone, Copy)]
struct BluetoothModemLpTimerSlot {
    deadline: Option<BluetoothModemLpTimerInstant>,
    generation: u32,
}

impl BluetoothModemLpTimerSlot {
    const EMPTY: Self = Self {
        deadline: None,
        generation: 0,
    };
}

/// Why an absolute software timer cannot be inserted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerScheduleError {
    /// The deadline is more than half the wrapping domain ahead and therefore
    /// cannot be ordered unambiguously.
    DeadlineOutsideForwardHalfRange,
    /// Every reusable fixed slot is occupied.
    QueueFull,
    /// The only free slots exhausted their generation domain; reusing one
    /// could make a stale cancellation token target a later timer.
    GenerationExhausted,
}

/// Affine cancellation authority for one queued timer.
#[must_use = "dropping the token leaves the timer scheduled"]
pub struct BluetoothModemLpTimerToken {
    slot: usize,
    generation: u32,
}

/// One value-only timer expiration removed from the source-owned queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the expiration must be published before timer hardware is rearmed"]
pub struct BluetoothModemLpTimerExpiration {
    slot: usize,
    generation: u32,
    deadline: BluetoothModemLpTimerInstant,
}

impl BluetoothModemLpTimerExpiration {
    /// Return the stable slot identity for diagnostic/event dispatch.
    pub const fn slot(&self) -> usize {
        self.slot
    }

    /// Return the slot generation, distinguishing consecutive uses.
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    /// Return the absolute positional deadline owned by this expiration.
    pub const fn deadline(&self) -> BluetoothModemLpTimerInstant {
        self.deadline
    }
}

/// Fixed-capacity absolute-deadline queue owned by source 127.
///
/// Deadlines are admitted only within the forward half of the wrapping
/// 32-bit positional time domain. That makes due and earliest comparisons
/// unambiguous without assigning a physical time unit.
pub struct BluetoothModemLpTimerQueue<const CAPACITY: usize> {
    slots: [BluetoothModemLpTimerSlot; CAPACITY],
}

impl<const CAPACITY: usize> BluetoothModemLpTimerQueue<CAPACITY> {
    /// Construct an empty queue.
    pub const fn new() -> Self {
        Self {
            slots: [BluetoothModemLpTimerSlot::EMPTY; CAPACITY],
        }
    }

    /// Schedule one absolute deadline relative to a current positional sample.
    pub fn schedule(
        &mut self,
        now: BluetoothModemLpTimerInstant,
        deadline: BluetoothModemLpTimerInstant,
    ) -> Result<BluetoothModemLpTimerToken, BluetoothModemLpTimerScheduleError> {
        let forward = deadline.bits().wrapping_sub(now.bits());
        if forward > MAX_FORWARD_DELAY {
            return Err(BluetoothModemLpTimerScheduleError::DeadlineOutsideForwardHalfRange);
        }

        let mut exhausted = false;
        for (slot_index, slot) in self.slots.iter_mut().enumerate() {
            if slot.deadline.is_some() {
                continue;
            }
            let Some(generation) = slot.generation.checked_add(1) else {
                exhausted = true;
                continue;
            };
            slot.generation = generation;
            slot.deadline = Some(deadline);
            return Ok(BluetoothModemLpTimerToken {
                slot: slot_index,
                generation,
            });
        }

        Err(if exhausted {
            BluetoothModemLpTimerScheduleError::GenerationExhausted
        } else {
            BluetoothModemLpTimerScheduleError::QueueFull
        })
    }

    /// Cancel exactly the timer named by an affine token.
    ///
    /// `false` means it already expired or was otherwise removed. A stale
    /// generation can never cancel a later occupant of the same slot.
    pub fn cancel(&mut self, token: BluetoothModemLpTimerToken) -> bool {
        let Some(slot) = self.slots.get_mut(token.slot) else {
            return false;
        };
        if slot.generation != token.generation || slot.deadline.is_none() {
            return false;
        }
        slot.deadline = None;
        true
    }

    /// Whether no timer remains queued.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.deadline.is_none())
    }

    fn pop_due(
        &mut self,
        now: BluetoothModemLpTimerInstant,
    ) -> Option<BluetoothModemLpTimerExpiration> {
        let mut selected: Option<(usize, u32)> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            let Some(deadline) = slot.deadline else {
                continue;
            };
            let lateness = now.bits().wrapping_sub(deadline.bits());
            if lateness > MAX_FORWARD_DELAY {
                continue;
            }
            if selected.is_none_or(|(_, selected_lateness)| lateness > selected_lateness) {
                selected = Some((index, lateness));
            }
        }

        let (index, _) = selected?;
        let slot = &mut self.slots[index];
        let deadline = slot
            .deadline
            .take()
            .expect("selected slot remains occupied");
        Some(BluetoothModemLpTimerExpiration {
            slot: index,
            generation: slot.generation,
            deadline,
        })
    }

    fn next_deadline(
        &self,
        now: BluetoothModemLpTimerInstant,
    ) -> Option<BluetoothModemLpTimerInstant> {
        self.slots
            .iter()
            .filter_map(|slot| slot.deadline)
            .min_by_key(|deadline| {
                let forward = deadline.bits().wrapping_sub(now.bits());
                if forward == 0 || forward > MAX_FORWARD_DELAY {
                    (0, 0)
                } else {
                    (1, forward)
                }
            })
    }
}

impl<const CAPACITY: usize> Default for BluetoothModemLpTimerQueue<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of putting one expiration into the atomic task handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothModemLpTimerEventPublication {
    /// The event is durable and the sole receiver must be woken.
    WakeWorker,
}

/// Allocation-free one-event handoff with explicit producer backpressure.
///
/// A full cell never overwrites an older expiration. The source-127 software
/// owner remains trapped in its publication-required state until the receiver
/// frees the slot, so final hardware rearm cannot race ahead of event delivery.
pub struct BluetoothModemLpTimerEventCell {
    state: AtomicU8,
    timer_slot: AtomicUsize,
    generation: AtomicU32,
    deadline: AtomicU32,
}

impl BluetoothModemLpTimerEventCell {
    /// Construct an empty event handoff.
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(EVENT_EMPTY),
            timer_slot: AtomicUsize::new(0),
            generation: AtomicU32::new(0),
            deadline: AtomicU32::new(0),
        }
    }

    fn publish(
        &self,
        event: BluetoothModemLpTimerExpiration,
    ) -> Result<BluetoothModemLpTimerEventPublication, BluetoothModemLpTimerExpiration> {
        if self
            .state
            .compare_exchange(
                EVENT_EMPTY,
                EVENT_WRITING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return Err(event);
        }
        self.timer_slot.store(event.slot, Ordering::Relaxed);
        self.generation.store(event.generation, Ordering::Relaxed);
        self.deadline
            .store(event.deadline.bits(), Ordering::Relaxed);
        self.state.store(EVENT_READY, Ordering::Release);
        Ok(BluetoothModemLpTimerEventPublication::WakeWorker)
    }

    /// Remove the pending expiration, if any.
    pub fn take(&self) -> Option<BluetoothModemLpTimerExpiration> {
        if self
            .state
            .compare_exchange(
                EVENT_READY,
                EVENT_READING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return None;
        }
        let event = BluetoothModemLpTimerExpiration {
            slot: self.timer_slot.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
            deadline: BluetoothModemLpTimerInstant::from_bits(
                self.deadline.load(Ordering::Relaxed),
            ),
        };
        self.state.store(EVENT_EMPTY, Ordering::Release);
        Some(event)
    }

    /// Whether one expiration is waiting for the receiver.
    pub fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == EVENT_READY
    }
}

impl Default for BluetoothModemLpTimerEventCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Controller-level disposition of one bounded source-127 hard-handler entry.
#[must_use = "retain the ready interrupt owner or continue its software work"]
pub enum BluetoothModemLpTimerInterruptRuntimeStep<'queue, const CAPACITY: usize> {
    /// The register-only path completed and the affine owner is ready again.
    Ready(BluetoothModemLpTimerInterruptReadyOwner),
    /// Acknowledged state requires bounded source-owned queue work.
    Software(BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY>),
}

/// Execute the source-127 PAC/HAL prefix and enter source-owned software work.
///
/// This composes both finite HAL register phases. The function either returns
/// the ISR-ready owner immediately or advances the positional epoch and traps
/// ownership in [`BluetoothModemLpTimerSoftwareWork`]. It never dispatches a
/// callback or loops over the queue in hard-interrupt context.
pub fn step_modem_lp_timer_interrupt<'queue, const CAPACITY: usize>(
    ready: BluetoothModemLpTimerInterruptReadyOwner,
    queue: &'queue mut BluetoothModemLpTimerQueue<CAPACITY>,
    epoch: &'queue mut BluetoothModemLpTimerEpoch,
) -> BluetoothModemLpTimerInterruptRuntimeStep<'queue, CAPACITY> {
    match ready.step() {
        BluetoothModemLpTimerInterruptStep::Spurious(ready) => {
            BluetoothModemLpTimerInterruptRuntimeStep::Ready(ready)
        }
        BluetoothModemLpTimerInterruptStep::HandlerPending(pending) => {
            match pending.step_registers() {
                BluetoothModemLpTimerHandlerRegisterStep::Rearmed(ready) => {
                    BluetoothModemLpTimerInterruptRuntimeStep::Ready(ready)
                }
                BluetoothModemLpTimerHandlerRegisterStep::SoftwarePending(pending) => {
                    BluetoothModemLpTimerInterruptRuntimeStep::Software(
                        BluetoothModemLpTimerSoftwareWork::begin(queue, epoch, pending),
                    )
                }
            }
        }
    }
}

/// Source-127 software work retaining the unique HAL owner and queue borrow.
#[must_use = "software work must publish every expiration and rearm the interrupt owner"]
pub struct BluetoothModemLpTimerSoftwareWork<'queue, const CAPACITY: usize> {
    queue: &'queue mut BluetoothModemLpTimerQueue<CAPACITY>,
    epoch: &'queue mut BluetoothModemLpTimerEpoch,
    owner: BluetoothModemLpTimerSoftwarePendingOwner,
    dispatch_requested: bool,
}

impl<'queue, const CAPACITY: usize> BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY> {
    /// Begin the software consequence of one acknowledged source-127 epoch.
    pub fn begin(
        queue: &'queue mut BluetoothModemLpTimerQueue<CAPACITY>,
        epoch: &'queue mut BluetoothModemLpTimerEpoch,
        owner: BluetoothModemLpTimerSoftwarePendingOwner,
    ) -> Self {
        let registers = owner.register_observation();
        epoch.advance_for_handler_registers(registers);
        Self {
            queue,
            epoch,
            owner,
            dispatch_requested: registers.state_0024_low_byte_was_nonzero(),
        }
    }

    /// Execute one bounded software step.
    ///
    /// At most one due timer is removed. If no expiration is due, the method
    /// performs exactly one compare disposition and the final fresh read only
    /// after every removed expiration has crossed the publication gate.
    pub fn step(mut self) -> BluetoothModemLpTimerSoftwareStep<'queue, CAPACITY> {
        let now = self.owner.sample_counter(self.epoch).instant();
        if self.dispatch_requested
            && let Some(event) = self.queue.pop_due(now)
        {
            return BluetoothModemLpTimerSoftwareStep::Expiration(
                BluetoothModemLpTimerExpirationPending { work: self, event },
            );
        }

        let disposition = match self.queue.next_deadline(now) {
            Some(deadline) => self.owner.program_compare(deadline, *self.epoch),
            None => {
                self.owner.disable_compare();
                return BluetoothModemLpTimerSoftwareStep::Rearmed(self.owner.complete_software());
            }
        };
        if disposition == BluetoothModemLpTimerCompareDisposition::Immediate {
            self.dispatch_requested = true;
            BluetoothModemLpTimerSoftwareStep::Recheck(self)
        } else {
            BluetoothModemLpTimerSoftwareStep::Rearmed(self.owner.complete_software())
        }
    }
}

/// One due expiration that must become durable before software work resumes.
#[must_use = "publish the expiration or retain the source-127 owner"]
pub struct BluetoothModemLpTimerExpirationPending<'queue, const CAPACITY: usize> {
    work: BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY>,
    event: BluetoothModemLpTimerExpiration,
}

impl<'queue, const CAPACITY: usize> BluetoothModemLpTimerExpirationPending<'queue, CAPACITY> {
    /// Return the value that is waiting for durable publication.
    pub const fn event(&self) -> BluetoothModemLpTimerExpiration {
        self.event
    }

    /// Publish the event or return the entire unchanged pending owner on
    /// backpressure. Successful publication is the only path back to `step`.
    pub fn publish(
        self,
        events: &BluetoothModemLpTimerEventCell,
    ) -> Result<
        (
            BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY>,
            BluetoothModemLpTimerEventPublication,
        ),
        Self,
    > {
        match events.publish(self.event) {
            Ok(publication) => Ok((self.work, publication)),
            Err(event) => Err(Self {
                work: self.work,
                event,
            }),
        }
    }
}

/// Result of one bounded source-127 software queue step.
#[must_use = "retain recheck/work ownership or the rearmed interrupt owner"]
pub enum BluetoothModemLpTimerSoftwareStep<'queue, const CAPACITY: usize> {
    /// One due expiration must cross the durable event handoff.
    Expiration(BluetoothModemLpTimerExpirationPending<'queue, CAPACITY>),
    /// Compare programming detected an immediate deadline; yield before one
    /// fresh counter/queue recheck.
    Recheck(BluetoothModemLpTimerSoftwareWork<'queue, CAPACITY>),
    /// The queue is armed or empty and the final fresh read has completed.
    Rearmed(BluetoothModemLpTimerInterruptReadyOwner),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(bits: u32) -> BluetoothModemLpTimerInstant {
        BluetoothModemLpTimerInstant::from_bits(bits)
    }

    #[test]
    fn queue_expires_oldest_due_timer_one_step_at_a_time() {
        let mut queue = BluetoothModemLpTimerQueue::<3>::new();
        let _later = queue.schedule(instant(10), instant(30)).unwrap();
        let _first = queue.schedule(instant(10), instant(20)).unwrap();
        let _middle = queue.schedule(instant(10), instant(25)).unwrap();

        assert_eq!(queue.pop_due(instant(24)).unwrap().deadline(), instant(20));
        assert!(queue.pop_due(instant(24)).is_none());
        assert_eq!(queue.next_deadline(instant(24)), Some(instant(25)));
    }

    #[test]
    fn rearm_selection_prefers_an_overdue_timer_to_a_future_timer() {
        let mut queue = BluetoothModemLpTimerQueue::<2>::new();
        let _overdue = queue.schedule(instant(10), instant(20)).unwrap();
        let _future = queue.schedule(instant(10), instant(40)).unwrap();

        assert_eq!(queue.next_deadline(instant(30)), Some(instant(20)));
    }

    #[test]
    fn cancellation_generation_cannot_target_a_reused_slot() {
        let mut queue = BluetoothModemLpTimerQueue::<1>::new();
        let old = queue.schedule(instant(0), instant(1)).unwrap();
        let expired = queue.pop_due(instant(1)).unwrap();
        let current = queue.schedule(instant(1), instant(2)).unwrap();

        assert_ne!(expired.generation(), current.generation);
        assert!(!queue.cancel(old));
        assert!(queue.cancel(current));
        assert!(queue.is_empty());
    }

    #[test]
    fn wrapping_deadlines_are_ordered_within_the_forward_half_range() {
        let mut queue = BluetoothModemLpTimerQueue::<2>::new();
        let _first = queue.schedule(instant(u32::MAX - 2), instant(1)).unwrap();
        let _second = queue.schedule(instant(u32::MAX - 2), instant(4)).unwrap();

        assert_eq!(queue.pop_due(instant(2)).unwrap().deadline(), instant(1));
        assert_eq!(queue.next_deadline(instant(2)), Some(instant(4)));
        assert!(matches!(
            queue.schedule(instant(2), instant(1)),
            Err(BluetoothModemLpTimerScheduleError::DeadlineOutsideForwardHalfRange)
        ));
    }

    #[test]
    fn full_event_cell_applies_backpressure_without_overwrite() {
        let cell = BluetoothModemLpTimerEventCell::new();
        let first = BluetoothModemLpTimerExpiration {
            slot: 1,
            generation: 7,
            deadline: instant(100),
        };
        let second = BluetoothModemLpTimerExpiration {
            slot: 2,
            generation: 9,
            deadline: instant(200),
        };

        assert_eq!(
            cell.publish(first),
            Ok(BluetoothModemLpTimerEventPublication::WakeWorker)
        );
        assert_eq!(cell.publish(second), Err(second));
        assert_eq!(cell.take(), Some(first));
        assert_eq!(
            cell.publish(second),
            Ok(BluetoothModemLpTimerEventPublication::WakeWorker)
        );
        assert_eq!(cell.take(), Some(second));
    }

    #[test]
    fn source_127_task_readiness_survives_late_acquisition_and_coalesces_reentry() {
        let cell = BluetoothModemLpTimerWorkerWakeCell::new();

        assert_eq!(
            BluetoothModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
            BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
                BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
            )
        );
        assert!(cell.is_pending());
        assert_eq!(
            BluetoothModemLpTimerStableInterruptStep::AwaitingSoftware.publish(&cell),
            BluetoothModemLpTimerPublishedInterruptStep::AwaitingSoftware(
                BluetoothModemLpTimerWorkerWakePublication::Coalesced,
            )
        );

        assert!(cell.take());
        assert!(!cell.is_pending());
        assert_eq!(
            BluetoothModemLpTimerStableInterruptStep::SoftwarePending.publish(&cell),
            BluetoothModemLpTimerPublishedInterruptStep::SoftwarePending(
                BluetoothModemLpTimerWorkerWakePublication::WakeWorker,
            )
        );
    }
}
