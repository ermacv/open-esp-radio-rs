//! One bounded primary source-124 interrupt disposition.
//!
//! The restricted PAC owns all register geometry and acknowledgement order;
//! the HAL owns the affine interrupt-register capability. This layer joins
//! those finite operations with the Controller classifier without callbacks,
//! allocation, polling or an RTOS queue.

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use open_esp_radio_esp32s31_hal::BluetoothSchedulerSoftwareListRemovalInterruptStep;
use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};
use open_esp_radio_esp32s31_pac::BluetoothPrimaryInterruptEpoch;
use open_esp_radio_esp32s31_pac::BluetoothSchedulerHardwareListIndex;

use crate::{
    BluetoothPrimaryControllerFault, BluetoothPrimaryInterruptClassification,
    BluetoothSchedulerLockModifyEventCell, BluetoothSchedulerLockModifyEventPublication,
    BluetoothSchedulerReferenceAction, BluetoothSchedulerWakeCell,
    BluetoothSchedulerWakePublication, BluetoothSchedulerWorkerWake,
};

/// Terminal result of one bounded primary source-124 handler step.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged primary epoch must be retained or published"]
pub enum BluetoothPrimaryInterruptStep {
    /// A reviewed baseline fault preempted ordinary scheduler work.
    Fault(BluetoothPrimaryControllerFault),
    /// The epoch contained no reviewed dynamic scheduler source.
    NoSchedulerWork(BluetoothPrimaryNoSchedulerWork),
    /// A scheduler wake and matching BUSY observation are ready for publication.
    Scheduler(BluetoothPrimarySchedulerEvent),
}

/// Acknowledged primary epoch with no reviewed scheduler work.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryNoSchedulerWork {
    classification: BluetoothPrimaryInterruptClassification,
}

/// One classified scheduler publication derived from a single later state read.
///
/// The ordinary scheduler wake and lock/modify BUSY value are intentionally
/// paired here. Both are derived at the same temporal point, so the async
/// adapter cannot accidentally combine a wake from one IRQ with a BUSY sample
/// from another.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothPrimarySchedulerEvent {
    classification: BluetoothPrimaryInterruptClassification,
    wake: BluetoothSchedulerWorkerWake,
    work: BluetoothSchedulerWorkObservation,
}

/// Durable Controller disposition of one acknowledged primary epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "fault, recovery or scheduler publication must reach the Controller owner"]
pub enum BluetoothPrimaryPublishedInterruptStep {
    /// A baseline or unclassified fault published no ordinary work.
    Fault(BluetoothPrimaryControllerFault),
    /// The epoch contained no reviewed dynamic scheduler work.
    NoSchedulerWork(BluetoothPrimaryNoSchedulerWork),
    /// Both durable scheduler cells accepted the same temporal event.
    Scheduler {
        /// Exact classified primary event.
        event: BluetoothPrimarySchedulerEvent,
        /// Coalescing disposition of the general scheduler handoff.
        scheduler: BluetoothSchedulerWakePublication,
        /// Latest-value disposition of the lock/modify handoff.
        lock_modify: BluetoothSchedulerLockModifyEventPublication,
    },
}

impl BluetoothPrimaryInterruptStep {
    /// Publish one classified primary result into its matching Controller cells.
    ///
    /// Fault and empty outcomes never publish ordinary scheduler work. A
    /// scheduler outcome updates both cells from the same later hardware
    /// observation before returning their wake dispositions.
    pub fn publish(
        self,
        scheduler_wake: &BluetoothSchedulerWakeCell,
        lock_modify_events: &BluetoothSchedulerLockModifyEventCell,
    ) -> BluetoothPrimaryPublishedInterruptStep {
        match self {
            Self::Fault(fault) => BluetoothPrimaryPublishedInterruptStep::Fault(fault),
            Self::NoSchedulerWork(epoch) => {
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch)
            }
            Self::Scheduler(event) => {
                let scheduler = scheduler_wake.publish_from_interrupt(event.wake().class());
                let lock_modify =
                    lock_modify_events.publish_from_interrupt(event.lock_modify_observation());
                BluetoothPrimaryPublishedInterruptStep::Scheduler {
                    event,
                    scheduler,
                    lock_modify,
                }
            }
        }
    }
}

impl BluetoothPrimarySchedulerEvent {
    /// Return the scheduler-worker wake classification.
    pub const fn wake(&self) -> BluetoothSchedulerWorkerWake {
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

trait BluetoothPrimaryInterruptBackend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch;
    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation;
    fn clear_scheduler_reference(&mut self);
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation;
}

impl BluetoothPrimaryInterruptBackend for BluetoothInterruptRegistersOwner {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.capture_primary_and_acknowledge()
    }

    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation {
        self.capture_scheduler_reference_gate()
    }

    fn clear_scheduler_reference(&mut self) {
        let _cleared = BluetoothInterruptRegistersOwner::clear_scheduler_reference(self);
    }

    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.capture_scheduler_work()
    }
}

fn execute_primary_interrupt_step(
    backend: &mut impl BluetoothPrimaryInterruptBackend,
) -> BluetoothPrimaryInterruptStep {
    let classification = match BluetoothPrimaryInterruptClassification::from_epoch(
        backend.capture_primary_and_acknowledge(),
    ) {
        Ok(classification) => classification,
        Err(fault) => return BluetoothPrimaryInterruptStep::Fault(fault),
    };

    if let Some(gate) = classification.reference_gate() {
        let observation = backend.capture_scheduler_reference_gate();
        if gate.classify(observation)
            == BluetoothSchedulerReferenceAction::ClearReferenceAndContinue
        {
            backend.clear_scheduler_reference();
        }
    }

    let Some(work_classifier) = classification.work_classifier() else {
        return BluetoothPrimaryInterruptStep::NoSchedulerWork(BluetoothPrimaryNoSchedulerWork {
            classification,
        });
    };
    let work = backend.capture_scheduler_work();
    let wake = work_classifier.classify(&work);
    BluetoothPrimaryInterruptStep::Scheduler(BluetoothPrimarySchedulerEvent {
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
pub fn step_primary_interrupt(
    interrupts: &mut BluetoothInterruptRegistersOwner,
) -> BluetoothPrimaryInterruptStep {
    execute_primary_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use open_esp_radio_esp32s31_pac::BluetoothPrimaryInterruptEpoch;

    use super::*;
    use crate::BluetoothSchedulerWorkerWakeClass;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        PrimaryEpoch,
        ReferenceGate,
        ReferenceClear,
        Work,
    }

    struct Backend {
        epoch: Option<BluetoothPrimaryInterruptEpoch>,
        gate: BluetoothSchedulerReferenceGateObservation,
        work: Option<BluetoothSchedulerWorkObservation>,
        operations: Vec<Operation>,
    }

    impl Backend {
        fn dynamic(
            source_21_pending: bool,
            sources_27_or_28_pending: bool,
            source_3_pending: bool,
            gate_busy: bool,
            work_busy: bool,
            work_state_29: bool,
        ) -> Self {
            Self {
                epoch: Some(BluetoothPrimaryInterruptEpoch::for_dynamic_validation(
                    source_21_pending,
                    sources_27_or_28_pending,
                    source_3_pending,
                )),
                gate: BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(
                    gate_busy,
                ),
                work: Some(
                    BluetoothSchedulerWorkObservation::from_fields_for_validation(
                        work_busy,
                        work_state_29,
                        7,
                    ),
                ),
                operations: Vec::new(),
            }
        }

        fn fault() -> Self {
            let mut backend = Self::dynamic(false, false, false, false, false, false);
            backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_fault_validation());
            backend
        }

        fn unclassified() -> Self {
            let mut backend = Self::dynamic(false, false, false, false, false, false);
            backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_unclassified_validation());
            backend
        }
    }

    impl BluetoothPrimaryInterruptBackend for Backend {
        fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
            self.operations.push(Operation::PrimaryEpoch);
            self.epoch.take().expect("one primary epoch")
        }

        fn capture_scheduler_reference_gate(
            &mut self,
        ) -> BluetoothSchedulerReferenceGateObservation {
            self.operations.push(Operation::ReferenceGate);
            self.gate
        }

        fn clear_scheduler_reference(&mut self) {
            self.operations.push(Operation::ReferenceClear);
        }

        fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
            self.operations.push(Operation::Work);
            self.work.take().expect("one scheduler work observation")
        }
    }

    #[test]
    fn ordinary_dynamic_epoch_produces_one_paired_scheduler_event() {
        let mut backend = Backend::dynamic(true, false, false, false, true, true);

        let BluetoothPrimaryInterruptStep::Scheduler(event) =
            execute_primary_interrupt_step(&mut backend)
        else {
            panic!("dynamic source must produce scheduler work");
        };

        assert_eq!(
            event.wake().class(),
            BluetoothSchedulerWorkerWakeClass::Ordinary
        );
        assert_eq!(event.wake().deferred_work_publication(), Some(true));
        assert!(event.lock_modify_observation().is_busy());
        assert_eq!(event.current_hardware_list().get(), 7);
        assert_eq!(
            backend.operations,
            [Operation::PrimaryEpoch, Operation::Work]
        );
    }

    #[test]
    fn idle_reference_gate_clears_before_fresh_work_read() {
        let mut backend = Backend::dynamic(false, true, true, false, true, true);

        let BluetoothPrimaryInterruptStep::Scheduler(event) =
            execute_primary_interrupt_step(&mut backend)
        else {
            panic!("idle reference gate must continue after the ordered clear");
        };

        assert!(event.lock_modify_observation().is_busy());
        assert_eq!(
            backend.operations,
            [
                Operation::PrimaryEpoch,
                Operation::ReferenceGate,
                Operation::ReferenceClear,
                Operation::Work,
            ]
        );
    }

    #[test]
    fn busy_reference_gate_preserves_reference_then_uses_a_fresh_work_read() {
        let mut backend = Backend::dynamic(false, true, true, true, false, false);

        let BluetoothPrimaryInterruptStep::Scheduler(event) =
            execute_primary_interrupt_step(&mut backend)
        else {
            panic!("busy reference gate must continue to deferred work");
        };

        assert_eq!(
            event.wake().class(),
            BluetoothSchedulerWorkerWakeClass::Ordinary
        );
        assert!(!event.lock_modify_observation().is_busy());
        assert_eq!(
            backend.operations,
            [
                Operation::PrimaryEpoch,
                Operation::ReferenceGate,
                Operation::Work,
            ]
        );
    }

    #[test]
    fn fault_and_empty_epochs_do_not_touch_scheduler_state() {
        let mut fault = Backend::fault();
        assert!(matches!(
            execute_primary_interrupt_step(&mut fault),
            BluetoothPrimaryInterruptStep::Fault(_)
        ));
        assert_eq!(fault.operations, [Operation::PrimaryEpoch]);

        let mut empty = Backend::dynamic(false, false, false, false, false, false);
        assert!(matches!(
            execute_primary_interrupt_step(&mut empty),
            BluetoothPrimaryInterruptStep::NoSchedulerWork(_)
        ));
        assert_eq!(empty.operations, [Operation::PrimaryEpoch]);
    }

    #[test]
    fn unclassified_status_fails_closed_without_reading_scheduler_state() {
        let mut backend = Backend::unclassified();

        let BluetoothPrimaryInterruptStep::Fault(fault) =
            execute_primary_interrupt_step(&mut backend)
        else {
            panic!("unclassified status must fail closed");
        };

        assert!(fault.sources().unclassified_pending());
        assert_eq!(backend.operations, [Operation::PrimaryEpoch]);
    }

    #[test]
    fn one_primary_scheduler_event_updates_both_durable_cells_from_one_observation() {
        let mut backend = Backend::dynamic(false, true, true, true, true, true);
        let scheduler_wake = BluetoothSchedulerWakeCell::new();
        let lock_modify_events = BluetoothSchedulerLockModifyEventCell::new();

        let published = execute_primary_interrupt_step(&mut backend)
            .publish(&scheduler_wake, &lock_modify_events);

        assert!(matches!(
            published,
            BluetoothPrimaryPublishedInterruptStep::Scheduler {
                scheduler: BluetoothSchedulerWakePublication::WakeWorker,
                lock_modify: BluetoothSchedulerLockModifyEventPublication::WakeWorker,
                ..
            }
        ));
        assert!(
            scheduler_wake
                .take()
                .expect("scheduler work must be durable")
                .is_marked()
        );
        assert!(
            lock_modify_events
                .take()
                .expect("the matching BUSY observation must be durable")
                .is_busy()
        );

        let empty = execute_primary_interrupt_step(&mut Backend::dynamic(
            false, false, false, false, false, false,
        ))
        .publish(&scheduler_wake, &lock_modify_events);
        assert!(matches!(
            empty,
            BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(_)
        ));
        assert!(!scheduler_wake.is_pending());
        assert!(!lock_modify_events.is_pending());
    }
}
