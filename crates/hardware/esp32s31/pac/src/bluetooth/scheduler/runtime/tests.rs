use std::vec::Vec;

use super::{
    BluetoothDiagnosticControl, BluetoothSchedulerFinishedListControl,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerInterruptControl,
    BluetoothSchedulerSoftwareListRemovalControl, BluetoothSchedulerSoftwareListRemovalDisposition,
    BluetoothSchedulerSoftwareListRemovalInterruptStep,
    BluetoothSchedulerSoftwareListRemovalRecheckControl, DiagnosticSelector, DiagnosticValue,
    SchedulerStateObservation, execute_clear_scheduler_reference, execute_finished_list_transfer,
    execute_reference_gate_observation, execute_scheduler_status_sample,
    execute_software_list_removal_finish, execute_software_list_removal_recheck,
    execute_work_observation, execution_modify_repeats, execution_modify_settled,
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
    SampleBusy,
    ReadState,
    ClearReference,
}

struct InterruptRecorder {
    busy: bool,
    state: SchedulerStateObservation,
    operations: Vec<InterruptOperation>,
}

impl BluetoothSchedulerInterruptControl for InterruptRecorder {
    fn read_scheduler_busy(&mut self) -> bool {
        self.operations.push(InterruptOperation::SampleBusy);
        self.busy
    }

    fn read_scheduler_state(&mut self) -> SchedulerStateObservation {
        self.operations.push(InterruptOperation::ReadState);
        self.state
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
fn reference_gate_samples_busy_and_work_reads_state_after_the_clear() {
    let mut recorder = InterruptRecorder {
        busy: false,
        state: SchedulerStateObservation {
            busy: true,
            state_29: true,
            current_hardware_list: BluetoothSchedulerHardwareListIndex(9),
        },
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
            InterruptOperation::SampleBusy,
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
    let step = super::BluetoothSchedulerBusyObservation::from_busy_for_validation(true)
        .into_software_list_removal_gate();
    assert_eq!(
        step,
        BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending
    );

    let step = super::BluetoothSchedulerBusyObservation::from_busy_for_validation(false)
        .into_software_list_removal_gate();
    assert!(matches!(
        step,
        BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(_)
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticOperation {
    SelectSchedulerStatus,
    Select(DiagnosticSelector),
    ReadValue,
}

struct DiagnosticRecorder {
    values: Vec<DiagnosticValue>,
    operations: Vec<DiagnosticOperation>,
}

impl BluetoothDiagnosticControl for DiagnosticRecorder {
    fn select(&mut self, selector: DiagnosticSelector) {
        self.operations.push(match selector {
            DiagnosticSelector::SchedulerStatus => DiagnosticOperation::SelectSchedulerStatus,
            other => DiagnosticOperation::Select(other),
        });
    }

    fn read_diagnostic_value(&mut self) -> DiagnosticValue {
        self.operations.push(DiagnosticOperation::ReadValue);
        self.values.remove(0)
    }
}

const fn diagnostic(busy: bool, value_1: u8) -> DiagnosticValue {
    DiagnosticValue {
        value_0_low_7: 0x15,
        value_0_bit_7: busy,
        value_1,
        value_2: 0,
        value_3: 0,
    }
}

#[test]
fn scheduler_status_sample_selects_once_and_accepts_two_equal_reads() {
    let mut recorder = DiagnosticRecorder {
        values: Vec::from([diagnostic(true, 0), diagnostic(true, 0)]),
        operations: Vec::new(),
    };
    assert!(execute_scheduler_status_sample(&mut recorder));
    assert_eq!(
        recorder.operations,
        [
            DiagnosticOperation::SelectSchedulerStatus,
            DiagnosticOperation::ReadValue,
            DiagnosticOperation::ReadValue,
        ]
    );
}

#[test]
fn scheduler_status_sample_rereads_both_values_until_a_pair_is_equal() {
    // A change in any lane rejects the pair, including one the BUSY bit does
    // not occupy; the accepted pair decides BUSY.
    let mut recorder = DiagnosticRecorder {
        values: Vec::from([
            diagnostic(true, 0),
            diagnostic(true, 1),
            diagnostic(true, 1),
            diagnostic(false, 1),
            diagnostic(false, 1),
            diagnostic(false, 1),
        ]),
        operations: Vec::new(),
    };
    assert!(!execute_scheduler_status_sample(&mut recorder));
    assert!(recorder.values.is_empty());
    assert_eq!(
        recorder.operations.first(),
        Some(&DiagnosticOperation::SelectSchedulerStatus)
    );
    assert_eq!(
        recorder
            .operations
            .iter()
            .filter(|operation| **operation == DiagnosticOperation::SelectSchedulerStatus)
            .count(),
        1
    );
}

const fn raw(lane_0: u8, lane_1: u8) -> DiagnosticValue {
    DiagnosticValue {
        value_0_low_7: lane_0 & 0x7f,
        value_0_bit_7: lane_0 & 0x80 != 0,
        value_1: lane_1,
        value_2: 0,
        value_3: 0,
    }
}

#[test]
fn execution_modify_repeats_in_state_two_or_state_three_with_lane_one_marked() {
    assert!(execution_modify_repeats(raw(2, 0)));
    assert!(execution_modify_repeats(raw(0x0a, 0xff)));
    assert!(execution_modify_repeats(raw(3, 0x10)));
    assert!(execution_modify_repeats(raw(3, 0xdf)));
    assert!(!execution_modify_repeats(raw(3, 0x20)));
    assert!(!execution_modify_repeats(raw(3, 0)));
    for state in [0, 1, 4, 5, 6, 7] {
        assert!(!execution_modify_repeats(raw(state, 0x10)));
    }
}

#[test]
fn execution_modify_settles_once_the_complete_value_differs_from_nine() {
    assert!(!execution_modify_settled(raw(9, 0)));
    assert!(execution_modify_settled(raw(8, 0)));
    assert!(execution_modify_settled(raw(0x89, 0)));
    assert!(execution_modify_settled(raw(9, 1)));
    assert!(execution_modify_settled(DiagnosticValue {
        value_3: 1,
        ..raw(9, 0)
    }));
}
