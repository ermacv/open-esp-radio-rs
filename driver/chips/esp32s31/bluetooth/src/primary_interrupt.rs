//! One bounded primary source-124 interrupt disposition.
//!
//! The restricted PAC owns all register geometry and acknowledgement order;
//! the HAL owns the affine interrupt-register capability. This layer joins
//! those finite operations with the Controller classifier without callbacks,
//! allocation, polling or an RTOS queue.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothSchedulerLockModifyInterruptObservation,
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};
use open_esp_radio_esp32s31_pac::{
    BluetoothPrimaryInterruptEpoch, BluetoothPrimaryInterruptObservation,
};

use crate::{
    BluetoothPrimaryControllerFault, BluetoothPrimaryInterruptClassification,
    BluetoothSchedulerReferenceAction, BluetoothSchedulerWorkerWake,
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
    /// The reference gate requires the still-missing affine selector-6
    /// consistency action before the later scheduler observation is legal.
    ReferenceRecoveryRequired(BluetoothPrimaryReferenceRecoveryRequired),
}

/// Acknowledged primary epoch with no reviewed scheduler work.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryNoSchedulerWork {
    classification: BluetoothPrimaryInterruptClassification,
}

impl BluetoothPrimaryNoSchedulerWork {
    /// Return the complete acknowledged primary observation.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.classification.observation()
    }
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
    lock_modify: BluetoothSchedulerLockModifyInterruptObservation,
}

impl BluetoothPrimarySchedulerEvent {
    /// Return the complete acknowledged primary observation.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.classification.observation()
    }

    /// Return the scheduler-worker wake classification.
    pub const fn wake(&self) -> BluetoothSchedulerWorkerWake {
        self.wake
    }

    /// Return the lock/modify BUSY value captured at the same work point.
    pub const fn lock_modify_observation(
        &self,
    ) -> BluetoothSchedulerLockModifyInterruptObservation {
        self.lock_modify
    }
}

/// Fail-closed primary epoch awaiting the open selector-6 invariant.
///
/// The PAC has acknowledged the interrupt banks, but this step deliberately
/// does not clear `SCHEDULER_REFERENCE`: the reference implementation executes
/// a mandatory scheduler transaction/list check immediately after that write.
/// Returning the affine classification prevents the later work read from
/// moving ahead of an action that the open scheduler cannot yet perform.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryReferenceRecoveryRequired {
    classification: BluetoothPrimaryInterruptClassification,
    gate: BluetoothSchedulerReferenceGateObservation,
}

impl BluetoothPrimaryReferenceRecoveryRequired {
    /// Return the complete acknowledged primary observation.
    pub const fn observation(&self) -> BluetoothPrimaryInterruptObservation {
        self.classification.observation()
    }

    /// Return the semantic gate observation that required recovery.
    pub const fn gate_observation(&self) -> BluetoothSchedulerReferenceGateObservation {
        self.gate
    }
}

trait BluetoothPrimaryInterruptBackend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch;
    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation;
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation;
}

impl BluetoothPrimaryInterruptBackend for BluetoothInterruptRegistersOwner {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.capture_primary_and_acknowledge()
    }

    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation {
        self.capture_scheduler_reference_gate()
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
            == BluetoothSchedulerReferenceAction::ClearReferenceAndRunPostClearSchedulerAction
        {
            return BluetoothPrimaryInterruptStep::ReferenceRecoveryRequired(
                BluetoothPrimaryReferenceRecoveryRequired {
                    classification,
                    gate: observation,
                },
            );
        }
    }

    let Some(work_classifier) = classification.work_classifier() else {
        return BluetoothPrimaryInterruptStep::NoSchedulerWork(BluetoothPrimaryNoSchedulerWork {
            classification,
        });
    };
    let work = backend.capture_scheduler_work();
    let wake = work_classifier.classify(work);
    BluetoothPrimaryInterruptStep::Scheduler(BluetoothPrimarySchedulerEvent {
        classification,
        wake,
        lock_modify: BluetoothSchedulerLockModifyInterruptObservation::from_busy(work.is_busy()),
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
        Work,
    }

    struct Backend {
        epoch: Option<BluetoothPrimaryInterruptEpoch>,
        gate: BluetoothSchedulerReferenceGateObservation,
        work: BluetoothSchedulerWorkObservation,
        operations: Vec<Operation>,
    }

    impl Backend {
        fn dynamic(
            source_21_pending: bool,
            sources_27_or_28_pending: bool,
            source_3_pending: bool,
            gate_busy: bool,
            work_busy: bool,
            work_reference_state_29: bool,
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
                work: BluetoothSchedulerWorkObservation::from_fields_for_validation(
                    work_busy,
                    work_reference_state_29,
                ),
                operations: Vec::new(),
            }
        }

        fn fault() -> Self {
            let mut backend = Self::dynamic(false, false, false, false, false, false);
            backend.epoch = Some(BluetoothPrimaryInterruptEpoch::for_fault_validation());
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

        fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
            self.operations.push(Operation::Work);
            self.work
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
        assert_eq!(event.wake().reference_state_publication(), Some(true));
        assert!(event.lock_modify_observation().is_busy());
        assert_eq!(
            backend.operations,
            [Operation::PrimaryEpoch, Operation::Work]
        );
    }

    #[test]
    fn idle_reference_gate_stops_before_clear_and_later_work_read() {
        let mut backend = Backend::dynamic(false, true, true, false, true, true);

        let BluetoothPrimaryInterruptStep::ReferenceRecoveryRequired(required) =
            execute_primary_interrupt_step(&mut backend)
        else {
            panic!("idle reference gate must fail closed");
        };

        assert!(!required.gate_observation().is_busy());
        assert_eq!(
            backend.operations,
            [Operation::PrimaryEpoch, Operation::ReferenceGate,]
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
}
