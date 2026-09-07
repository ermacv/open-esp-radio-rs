//! One bounded primary source-124 interrupt disposition.
//!
//! The restricted PAC owns all register geometry and acknowledgement order;
//! the HAL owns the affine interrupt-register capability. This layer joins
//! those finite operations with the Controller classifier without callbacks,
//! allocation, polling or an RTOS queue.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerSoftwareListRemovalInterruptStep;

use crate::{
    interrupt::{
        PrimaryControllerFault, PrimaryInterruptClassification, SchedulerReferenceAction,
        SchedulerWakeCell, SchedulerWakePublication, SchedulerWorkerWake,
    },
    scheduler::{SchedulerLockModifyEventCell, SchedulerLockModifyEventPublication},
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkObservation, InterruptRegistersOwner,
};

use oer_esp32s31_pac::{BluetoothPrimaryInterruptEpoch, BluetoothSchedulerHardwareListIndex};

/// Terminal result of one bounded primary source-124 handler step.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged primary epoch must be retained or published"]
pub enum PrimaryInterruptStep {
    /// A reviewed baseline fault preempted ordinary scheduler work.
    Fault(PrimaryControllerFault),
    /// The epoch contained no reviewed dynamic scheduler source.
    NoSchedulerWork(PrimaryNoSchedulerWork),
    /// A scheduler wake and matching BUSY observation are ready for publication.
    Scheduler(PrimarySchedulerEvent),
}

/// Acknowledged primary epoch with no reviewed scheduler work.
#[derive(Debug, Eq, PartialEq)]
pub struct PrimaryNoSchedulerWork {
    classification: PrimaryInterruptClassification,
}

/// One classified scheduler publication derived from a single later state read.
///
/// The ordinary scheduler wake and lock/modify BUSY value are intentionally
/// paired here. Both are derived at the same temporal point, so the async
/// adapter cannot accidentally combine a wake from one IRQ with a BUSY sample
/// from another.
#[derive(Debug, Eq, PartialEq)]
pub struct PrimarySchedulerEvent {
    classification: PrimaryInterruptClassification,
    wake: SchedulerWorkerWake,
    work: BluetoothSchedulerWorkObservation,
}

/// Durable Controller disposition of one acknowledged primary epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "fault, recovery or scheduler publication must reach the Controller owner"]
pub enum PrimaryPublishedInterruptStep {
    /// A baseline or unclassified fault published no ordinary work.
    Fault(PrimaryControllerFault),
    /// The epoch contained no reviewed dynamic scheduler work.
    NoSchedulerWork(PrimaryNoSchedulerWork),
    /// Both durable scheduler cells accepted the same temporal event.
    Scheduler {
        /// Exact classified primary event.
        event: PrimarySchedulerEvent,
        /// Coalescing disposition of the general scheduler handoff.
        scheduler: SchedulerWakePublication,
        /// Latest-value disposition of the lock/modify handoff.
        lock_modify: SchedulerLockModifyEventPublication,
    },
}

impl PrimaryInterruptStep {
    /// Publish one classified primary result into its matching Controller cells.
    ///
    /// Fault and empty outcomes never publish ordinary scheduler work. A
    /// scheduler outcome updates both cells from the same later hardware
    /// observation before returning their wake dispositions.
    pub fn publish(
        self,
        scheduler_wake: &SchedulerWakeCell,
        lock_modify_events: &SchedulerLockModifyEventCell,
    ) -> PrimaryPublishedInterruptStep {
        match self {
            Self::Fault(fault) => PrimaryPublishedInterruptStep::Fault(fault),
            Self::NoSchedulerWork(epoch) => PrimaryPublishedInterruptStep::NoSchedulerWork(epoch),
            Self::Scheduler(event) => {
                let scheduler = scheduler_wake.publish_from_interrupt(event.wake().class());
                let lock_modify =
                    lock_modify_events.publish_from_interrupt(event.lock_modify_observation());
                PrimaryPublishedInterruptStep::Scheduler {
                    event,
                    scheduler,
                    lock_modify,
                }
            }
        }
    }
}

impl PrimarySchedulerEvent {
    /// Return the scheduler-worker wake classification.
    pub const fn wake(&self) -> SchedulerWorkerWake {
        self.wake
    }

    /// Return the lock/modify BUSY value captured at the same work point.
    pub const fn lock_modify_observation(
        &self,
    ) -> BluetoothSchedulerLockModifyInterruptObservation {
        BluetoothSchedulerLockModifyInterruptObservation::from_busy(self.work.is_busy())
    }

    /// Hardware-list index captured by the same scheduler-state read.
    pub const fn current_hardware_list(&self) -> BluetoothSchedulerHardwareListIndex {
        self.work.current_hardware_list()
    }

    /// Consume this exact primary-event scheduler sample at the interrupt-side
    /// software-list removal gate.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn into_software_list_removal_gate(
        self,
    ) -> BluetoothSchedulerSoftwareListRemovalInterruptStep {
        self.work.into_software_list_removal_gate()
    }
}

trait PrimaryInterruptBackend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch;
    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation;
    fn clear_scheduler_reference(&mut self);
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation;
}

impl PrimaryInterruptBackend for InterruptRegistersOwner {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.capture_primary_and_acknowledge()
    }

    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation {
        self.capture_scheduler_reference_gate()
    }

    fn clear_scheduler_reference(&mut self) {
        let _cleared = InterruptRegistersOwner::clear_scheduler_reference(self);
    }

    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.capture_scheduler_work()
    }
}

fn execute_primary_interrupt_step(
    backend: &mut impl PrimaryInterruptBackend,
) -> PrimaryInterruptStep {
    let classification =
        match PrimaryInterruptClassification::from_epoch(backend.capture_primary_and_acknowledge())
        {
            Ok(classification) => classification,
            Err(fault) => return PrimaryInterruptStep::Fault(fault),
        };

    if let Some(gate) = classification.reference_gate() {
        let observation = backend.capture_scheduler_reference_gate();
        if gate.classify(observation) == SchedulerReferenceAction::ClearReferenceAndContinue {
            backend.clear_scheduler_reference();
        }
    }

    let Some(work_classifier) = classification.work_classifier() else {
        return PrimaryInterruptStep::NoSchedulerWork(PrimaryNoSchedulerWork { classification });
    };
    let work = backend.capture_scheduler_work();
    let wake = work_classifier.classify(&work);
    PrimaryInterruptStep::Scheduler(PrimarySchedulerEvent {
        classification,
        wake,
        work,
    })
}

/// Capture, acknowledge and classify one primary source-124 interrupt epoch.
///
/// This function is finite. It performs at most the PAC acknowledgement
/// transaction, one reference-gate read and one later work read. It never
/// invokes a callback, waits for hardware, allocates or wakes an executor.
pub fn step_primary_interrupt(interrupts: &mut InterruptRegistersOwner) -> PrimaryInterruptStep {
    execute_primary_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests;
