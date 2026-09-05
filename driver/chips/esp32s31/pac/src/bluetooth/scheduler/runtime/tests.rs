use std::vec::Vec;

use super::{
    BluetoothSchedulerFinishedListControl, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerInterruptControl, BluetoothSchedulerSoftwareListRemovalControl,
    BluetoothSchedulerSoftwareListRemovalDisposition,
    BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalRecheckControl, SchedulerStateObservation,
    execute_clear_scheduler_reference, execute_finished_list_transfer,
    execute_reference_gate_observation, execute_software_list_removal_finish,
    execute_software_list_removal_recheck, execute_work_observation,
};

#[test]
fn hardware_list_index_rejects_values_outside_the_scheduler_domain() {
    assert_eq!(
        BluetoothSchedulerHardwareListIndex::new(15).unwrap().get(),
        15
    );
    assert_eq!(BluetoothSchedulerHardwareListIndex::new(16), None);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptOperation {
    ReadState,
    ClearReference,
}

struct InterruptRecorder {
    states: [SchedulerStateObservation; 2],
    next_state: usize,
    operations: Vec<InterruptOperation>,
}

impl BluetoothSchedulerInterruptControl for InterruptRecorder {
    fn read_scheduler_state(&mut self) -> SchedulerStateObservation {
        let state = self.states[self.next_state];
        self.next_state += 1;
        self.operations.push(InterruptOperation::ReadState);
        state
    }

    fn clear_scheduler_reference(&mut self) {
        self.operations.push(InterruptOperation::ClearReference);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinishedListOperation {
    ReadStatus,
    WriteReport,
}

struct FinishedListRecorder {
    status: u16,
    operations: Vec<FinishedListOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemovalOperation {
    ReadCommandZero,
    ReadCommandOne,
}

struct RemovalRecorder {
    command_zero: bool,
    command_one: bool,
    operations: Vec<RemovalOperation>,
}

struct RemovalRecheckRecorder {
    scheduler_busy: bool,
    command_zero: bool,
    command_one: bool,
    operations: Vec<RemovalRecheckOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemovalRecheckOperation {
    SchedulerBusy,
    CommandZero,
    CommandOne,
}

impl BluetoothSchedulerSoftwareListRemovalRecheckControl for RemovalRecheckRecorder {
    fn read_scheduler_busy(&mut self) -> bool {
        self.operations.push(RemovalRecheckOperation::SchedulerBusy);
        self.scheduler_busy
    }

    fn read_command_0_status_26(&mut self) -> bool {
        self.operations.push(RemovalRecheckOperation::CommandZero);
        self.command_zero
    }

    fn read_command_1_status_18(&mut self) -> bool {
        self.operations.push(RemovalRecheckOperation::CommandOne);
        self.command_one
    }
}

impl BluetoothSchedulerSoftwareListRemovalControl for RemovalRecorder {
    fn read_command_0_status_26(&mut self) -> bool {
        self.operations.push(RemovalOperation::ReadCommandZero);
        self.command_zero
    }

    fn read_command_1_status_18(&mut self) -> bool {
        self.operations.push(RemovalOperation::ReadCommandOne);
        self.command_one
    }
}

impl BluetoothSchedulerFinishedListControl for FinishedListRecorder {
    fn read_finished_list_status(&mut self) -> u16 {
        self.operations.push(FinishedListOperation::ReadStatus);
        self.status
    }

    fn write_finished_list_report(&mut self, _value: u16) {
        self.operations.push(FinishedListOperation::WriteReport);
    }
}

#[test]
fn temporal_state_reads_remain_distinct_across_reference_clear() {
    let mut recorder = InterruptRecorder {
        states: [
            SchedulerStateObservation {
                busy: false,
                state_29: false,
                current_hardware_list: BluetoothSchedulerHardwareListIndex(3),
            },
            SchedulerStateObservation {
                busy: true,
                state_29: true,
                current_hardware_list: BluetoothSchedulerHardwareListIndex(9),
            },
        ],
        next_state: 0,
        operations: Vec::new(),
    };

    let gate = execute_reference_gate_observation(&mut recorder);
    execute_clear_scheduler_reference(&mut recorder);
    let work = execute_work_observation(&mut recorder);

    assert!(!gate.is_busy());
    assert!(work.is_busy());
    assert!(work.deferred_work_requested());
    assert_eq!(work.current_hardware_list().get(), 9);
    assert_eq!(
        recorder.operations,
        [
            InterruptOperation::ReadState,
            InterruptOperation::ClearReference,
            InterruptOperation::ReadState,
        ]
    );
}

#[test]
fn worker_finished_list_transfer_reads_before_complete_low_halfword_report() {
    let mut recorder = FinishedListRecorder {
        status: 0xa55a,
        operations: Vec::new(),
    };

    let _observation = execute_finished_list_transfer(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            FinishedListOperation::ReadStatus,
            FinishedListOperation::WriteReport,
        ]
    );
}

#[test]
fn software_list_removal_finish_preserves_short_circuit_reads() {
    let mut blocked_at_zero = RemovalRecorder {
        command_zero: false,
        command_one: true,
        operations: Vec::new(),
    };
    let blocked = execute_software_list_removal_finish(&mut blocked_at_zero);
    assert_eq!(
        blocked_at_zero.operations,
        [RemovalOperation::ReadCommandZero]
    );
    assert_eq!(
        blocked,
        BluetoothSchedulerSoftwareListRemovalDisposition::Pending
    );

    let mut blocked_at_one = RemovalRecorder {
        command_zero: true,
        command_one: false,
        operations: Vec::new(),
    };
    let blocked = execute_software_list_removal_finish(&mut blocked_at_one);
    assert_eq!(
        blocked_at_one.operations,
        [
            RemovalOperation::ReadCommandZero,
            RemovalOperation::ReadCommandOne,
        ]
    );
    assert_eq!(
        blocked,
        BluetoothSchedulerSoftwareListRemovalDisposition::Pending
    );

    let mut ready = RemovalRecorder {
        command_zero: true,
        command_one: true,
        operations: Vec::new(),
    };
    let ready_observation = execute_software_list_removal_finish(&mut ready);
    assert_eq!(
        ready.operations,
        [
            RemovalOperation::ReadCommandZero,
            RemovalOperation::ReadCommandOne,
        ]
    );
    assert_eq!(
        ready_observation,
        BluetoothSchedulerSoftwareListRemovalDisposition::Ready
    );
}

#[test]
fn direct_software_list_removal_recheck_preserves_all_short_circuit_edges() {
    for (scheduler_busy, command_zero, command_one, expected, operations) in [
        (
            true,
            true,
            true,
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending,
            &[RemovalRecheckOperation::SchedulerBusy][..],
        ),
        (
            false,
            false,
            true,
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending,
            &[
                RemovalRecheckOperation::SchedulerBusy,
                RemovalRecheckOperation::CommandZero,
            ][..],
        ),
        (
            false,
            true,
            false,
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending,
            &[
                RemovalRecheckOperation::SchedulerBusy,
                RemovalRecheckOperation::CommandZero,
                RemovalRecheckOperation::CommandOne,
            ][..],
        ),
        (
            false,
            true,
            true,
            BluetoothSchedulerSoftwareListRemovalDisposition::Ready,
            &[
                RemovalRecheckOperation::SchedulerBusy,
                RemovalRecheckOperation::CommandZero,
                RemovalRecheckOperation::CommandOne,
            ][..],
        ),
    ] {
        let mut recorder = RemovalRecheckRecorder {
            scheduler_busy,
            command_zero,
            command_one,
            operations: Vec::new(),
        };

        assert_eq!(
            execute_software_list_removal_recheck(&mut recorder),
            expected
        );
        assert_eq!(recorder.operations, operations);
    }
}

#[test]
fn busy_scheduler_cannot_authorize_task_side_command_reads() {
    let step = super::BluetoothSchedulerWorkObservation::from_fields_for_validation(true, false, 0)
        .into_software_list_removal_gate();
    assert_eq!(
        step,
        BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending
    );

    let step =
        super::BluetoothSchedulerWorkObservation::from_fields_for_validation(false, false, 0)
            .into_software_list_removal_gate();
    assert!(matches!(
        step,
        BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(_)
    ));
}
