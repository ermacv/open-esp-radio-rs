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
            gate: BluetoothSchedulerReferenceGateObservation::from_busy_for_validation(gate_busy),
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

    fn capture_scheduler_reference_gate(&mut self) -> BluetoothSchedulerReferenceGateObservation {
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

    let BluetoothPrimaryInterruptStep::Fault(fault) = execute_primary_interrupt_step(&mut backend)
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

    let published =
        execute_primary_interrupt_step(&mut backend).publish(&scheduler_wake, &lock_modify_events);

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
