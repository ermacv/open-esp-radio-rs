//! Restricted scheduler MMIO shared by the primary ISR and its worker.

#![deny(unsafe_code)]

use super::{
    BluetoothInterruptRegisters, BluetoothTaskRegisters, device_fence,
    svd::{zero_based_field_write, zero_register_write},
};

/// First field-level `SCHEDULER_STATE` observation for bank-one source 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerReferenceGateObservation {
    busy: bool,
}

impl BluetoothSchedulerReferenceGateObservation {
    const fn new(busy: bool) -> Self {
        Self { busy }
    }

    /// Construct a semantic gate observation for upper-layer validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_busy_for_validation(busy: bool) -> Self {
        Self::new(busy)
    }

    /// Whether the scheduler was busy at the reference-gate temporal point.
    pub const fn is_busy(self) -> bool {
        self.busy
    }
}

/// Affine proof that the source-124 reference-clear MMIO and trailing device
/// fence completed.
///
/// This value carries no scheduler-consistency claim. The Controller must
/// consume it immediately in its post-clear selector-6 invariant action
/// before making the later scheduler-work observation.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the completed reference clear must feed the post-clear scheduler invariant"]
pub struct BluetoothSchedulerReferenceCleared {
    _private: (),
}

/// Later field-level `SCHEDULER_STATE` observation used for deferred work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWorkObservation {
    busy: bool,
    reference_path_state: bool,
}

impl BluetoothSchedulerWorkObservation {
    const fn new(busy: bool, reference_path_state: bool) -> Self {
        Self {
            busy,
            reference_path_state,
        }
    }

    /// Construct semantic deferred-work fields for upper-layer validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(busy: bool, reference_state_29: bool) -> Self {
        Self::new(busy, reference_state_29)
    }

    /// Whether the scheduler was busy at the deferred-work temporal point.
    pub const fn is_busy(self) -> bool {
        self.busy
    }

    /// Whether the reviewed reference path was active at this temporal point.
    pub const fn reference_path_active(self) -> bool {
        self.busy && self.reference_path_state
    }
}

/// Finished hardware-list mask transferred by one scheduler-worker pop attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler finished-list observation must be drained or explicitly discarded"]
pub struct BluetoothSchedulerFinishedListObservation(u16);

/// Task-owned positional command fields sampled for software-list removal.
///
/// The two hardware meanings remain unknown. This token exposes no individual
/// field value; it can only be joined with a fresh scheduler BUSY observation
/// and classified through the complete reviewed removal predicate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the task-side software-list-removal observation must be joined or discarded"]
pub struct BluetoothSchedulerSoftwareListRemovalTaskObservation {
    command_0_status_26: bool,
    command_1_status_18: bool,
}

impl BluetoothSchedulerSoftwareListRemovalTaskObservation {
    /// Construct semantic task fields for host-side controller validation.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_statuses_for_validation(
        command_0_status_26: bool,
        command_1_status_18: bool,
    ) -> Self {
        Self {
            command_0_status_26,
            command_1_status_18,
        }
    }
}

/// One cross-owner observation of the scheduler software-list removal gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerSoftwareListRemovalObservation {
    busy: bool,
    command_0_status_26: bool,
    command_1_status_18: bool,
}

/// Result of one finite scheduler software-list removal observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending removal must return to the executor; ready removal may advance"]
pub enum BluetoothSchedulerSoftwareListRemovalDisposition {
    /// Hardware has not reached the complete reviewed return predicate.
    Pending,
    /// Scheduler BUSY was clear and both positional command statuses were set.
    Ready,
}

impl BluetoothSchedulerSoftwareListRemovalObservation {
    /// Join interrupt- and task-owned values at one controller event point.
    ///
    /// This performs no MMIO. The caller must reject stale observations and
    /// values from different scheduler epochs.
    pub const fn from_split(
        scheduler: BluetoothSchedulerWorkObservation,
        task: BluetoothSchedulerSoftwareListRemovalTaskObservation,
    ) -> Self {
        Self {
            busy: scheduler.busy,
            command_0_status_26: task.command_0_status_26,
            command_1_status_18: task.command_1_status_18,
        }
    }

    /// Classify one observation without polling or assigning undocumented
    /// meanings to the positional command statuses.
    pub const fn classify(self) -> BluetoothSchedulerSoftwareListRemovalDisposition {
        if !self.busy && self.command_0_status_26 && self.command_1_status_18 {
            BluetoothSchedulerSoftwareListRemovalDisposition::Ready
        } else {
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending
        }
    }
}

/// One of the sixteen scheduler hardware-list indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerHardwareListIndex(u8);

impl BluetoothSchedulerHardwareListIndex {
    /// Validate one positional hardware-list index.
    pub const fn new(index: u8) -> Option<Self> {
        if index < 16 { Some(Self(index)) } else { None }
    }

    /// Return the zero-based hardware-list index in `0..16`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Result of consuming at most one list from a captured finished-list set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the selected list or remaining observation must be handled"]
pub enum BluetoothSchedulerFinishedListPop {
    /// The captured observation contains no remaining finished list.
    Complete,
    /// The lowest-numbered finished list was selected.
    List {
        /// Positional hardware-list index selected in this step.
        index: BluetoothSchedulerHardwareListIndex,
        /// Remaining captured lists after consuming `index`.
        remaining: BluetoothSchedulerFinishedListObservation,
    },
}

impl BluetoothSchedulerFinishedListObservation {
    /// Construct a semantic set of finished hardware lists for host-only
    /// controller tests.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn from_lists_for_validation(lists: &[u8]) -> Option<Self> {
        let mut image = 0u16;
        for &list in lists {
            if list >= 16 {
                return None;
            }
            image |= 1u16 << list;
        }
        Some(Self(image))
    }

    /// Whether no hardware list was reported finished.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Consume at most one lowest-numbered finished list.
    ///
    /// The physical mask layout remains private to the PAC. Callers retain an
    /// opaque observation between executor turns and receive only a semantic
    /// list index for item selection.
    pub const fn pop_lowest(self) -> BluetoothSchedulerFinishedListPop {
        if self.0 == 0 {
            BluetoothSchedulerFinishedListPop::Complete
        } else {
            let index = self.0.trailing_zeros() as u8;
            BluetoothSchedulerFinishedListPop::List {
                index: BluetoothSchedulerHardwareListIndex(index),
                remaining: Self(self.0 & !(1u16 << index)),
            }
        }
    }
}

trait BluetoothSchedulerInterruptControl {
    fn read_scheduler_state(&mut self) -> SchedulerStateObservation;
    fn clear_scheduler_reference(&mut self);
}

#[derive(Clone, Copy)]
struct SchedulerStateObservation {
    busy: bool,
    reference_path_state: bool,
}

struct HardwareSchedulerInterruptControl<'a> {
    registers: &'a super::svd::BluetoothSchedulerInterruptRuntime,
}

impl BluetoothSchedulerInterruptControl for HardwareSchedulerInterruptControl<'_> {
    fn read_scheduler_state(&mut self) -> SchedulerStateObservation {
        let (busy, reference_path_state) =
            super::svd::field_snapshot_read::observe_bluetooth_scheduler_interrupt_state(
                self.registers,
            );
        SchedulerStateObservation {
            busy,
            reference_path_state,
        }
    }

    fn clear_scheduler_reference(&mut self) {
        zero_register_write::clear_bluetooth_scheduler_reference(self.registers);
    }
}

trait BluetoothSchedulerFinishedListControl {
    fn read_finished_list_status(&mut self) -> u16;
    fn write_finished_list_report(&mut self, value: u16);
}

trait BluetoothSchedulerSoftwareListRemovalControl {
    fn read_command_0_status_26(&mut self) -> bool;
    fn read_command_1_status_18(&mut self) -> bool;
}

struct HardwareSchedulerSoftwareListRemovalControl<'a> {
    registers: &'a super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerSoftwareListRemovalControl
    for HardwareSchedulerSoftwareListRemovalControl<'_>
{
    fn read_command_0_status_26(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_scheduler_software_list_command_0_status_26(
            self.registers,
        )
    }

    fn read_command_1_status_18(&mut self) -> bool {
        super::svd::field_read::observe_bluetooth_scheduler_software_list_command_1_status_18(
            self.registers,
        )
    }
}

struct HardwareSchedulerFinishedListControl<'a> {
    registers: &'a super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerFinishedListControl for HardwareSchedulerFinishedListControl<'_> {
    fn read_finished_list_status(&mut self) -> u16 {
        super::svd::field_read::observe_bluetooth_scheduler_finished_lists(self.registers)
    }

    fn write_finished_list_report(&mut self, value: u16) {
        zero_based_field_write::bluetooth_scheduler_finished_list_report(self.registers, value);
    }
}

fn execute_reference_gate_observation(
    control: &mut impl BluetoothSchedulerInterruptControl,
) -> BluetoothSchedulerReferenceGateObservation {
    BluetoothSchedulerReferenceGateObservation::new(control.read_scheduler_state().busy)
}

fn execute_clear_scheduler_reference(control: &mut impl BluetoothSchedulerInterruptControl) {
    control.clear_scheduler_reference();
}

fn execute_work_observation(
    control: &mut impl BluetoothSchedulerInterruptControl,
) -> BluetoothSchedulerWorkObservation {
    let state = control.read_scheduler_state();
    BluetoothSchedulerWorkObservation::new(state.busy, state.reference_path_state)
}

fn execute_finished_list_transfer(
    control: &mut impl BluetoothSchedulerFinishedListControl,
) -> BluetoothSchedulerFinishedListObservation {
    let value = control.read_finished_list_status();
    control.write_finished_list_report(value);
    BluetoothSchedulerFinishedListObservation(value)
}

fn execute_software_list_removal_task_observation(
    control: &mut impl BluetoothSchedulerSoftwareListRemovalControl,
) -> BluetoothSchedulerSoftwareListRemovalTaskObservation {
    let command_0_status_26 = control.read_command_0_status_26();
    let command_1_status_18 = command_0_status_26 && control.read_command_1_status_18();
    BluetoothSchedulerSoftwareListRemovalTaskObservation {
        command_0_status_26,
        command_1_status_18,
    }
}

impl BluetoothInterruptRegisters {
    /// Read the first scheduler-state image required only by bank-one source 3.
    ///
    /// The later work observation is intentionally a separate MMIO method:
    /// the complete source-124 handler can clear `SCHEDULER_REFERENCE` between
    /// these two reads, so one sampled image cannot stand in for both.
    pub fn capture_scheduler_reference_gate(
        &mut self,
    ) -> BluetoothSchedulerReferenceGateObservation {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_reference_gate_observation(&mut control)
    }

    /// Publish the complete zero image to `SCHEDULER_REFERENCE`.
    ///
    /// The primary classifier authorizes this only after bank-one source 3
    /// observed `SCHEDULER_STATE.BUSY == 0` at the reference gate. This MMIO
    /// operation does not perform the mandatory post-clear scheduler action;
    /// the future live interrupt owner must compose both in order.
    pub fn clear_scheduler_reference(&mut self) -> BluetoothSchedulerReferenceCleared {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_clear_scheduler_reference(&mut control);
        device_fence();
        BluetoothSchedulerReferenceCleared { _private: () }
    }

    /// Read the later scheduler-state image used to classify deferred work.
    pub fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_work_observation(&mut control)
    }
}

impl BluetoothTaskRegisters {
    /// Capture the two task-owned command statuses used by the complete
    /// software-list removal return gate.
    ///
    /// This preserves the vendor short-circuit order: command one is not read
    /// when command zero is clear. The operation performs at most two reads
    /// and always returns immediately.
    pub fn capture_scheduler_software_list_removal_task(
        &mut self,
    ) -> BluetoothSchedulerSoftwareListRemovalTaskObservation {
        let mut control = HardwareSchedulerSoftwareListRemovalControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_software_list_removal_task_observation(&mut control)
    }
}

impl BluetoothTaskRegisters {
    /// Transfer the exact finished-list mask preceding one worker pop attempt.
    ///
    /// This reads `0x2010_125c`, truncates to its low halfword, then writes
    /// that value as a complete zero-high image to `0x2010_1260`. The method
    /// deliberately calls the second operation a report: its hardware clear
    /// or acknowledgement semantics are not independently established. The
    /// complete reference worker dispatches the returned mask through its
    /// finished-item selector before every software completed-list pop.
    pub fn transfer_scheduler_finished_lists(
        &mut self,
    ) -> BluetoothSchedulerFinishedListObservation {
        let mut control = HardwareSchedulerFinishedListControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        let observation = execute_finished_list_transfer(&mut control);
        device_fence();
        observation
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BluetoothSchedulerFinishedListControl, BluetoothSchedulerHardwareListIndex,
        BluetoothSchedulerInterruptControl, BluetoothSchedulerSoftwareListRemovalControl,
        BluetoothSchedulerSoftwareListRemovalDisposition,
        BluetoothSchedulerSoftwareListRemovalObservation,
        BluetoothSchedulerSoftwareListRemovalTaskObservation, SchedulerStateObservation,
        execute_clear_scheduler_reference, execute_finished_list_transfer,
        execute_reference_gate_observation, execute_software_list_removal_task_observation,
        execute_work_observation,
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
                    reference_path_state: false,
                },
                SchedulerStateObservation {
                    busy: true,
                    reference_path_state: true,
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
        assert!(work.reference_path_active());
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
    fn software_list_removal_task_observation_preserves_short_circuit_reads() {
        let mut blocked_at_zero = RemovalRecorder {
            command_zero: false,
            command_one: true,
            operations: Vec::new(),
        };
        let blocked = execute_software_list_removal_task_observation(&mut blocked_at_zero);
        assert_eq!(
            blocked_at_zero.operations,
            [RemovalOperation::ReadCommandZero]
        );
        assert_eq!(
            BluetoothSchedulerSoftwareListRemovalObservation::from_split(
                super::BluetoothSchedulerWorkObservation::from_fields_for_validation(false, false),
                blocked,
            )
            .classify(),
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending
        );

        let mut ready = RemovalRecorder {
            command_zero: true,
            command_one: true,
            operations: Vec::new(),
        };
        let ready_observation = execute_software_list_removal_task_observation(&mut ready);
        assert_eq!(
            ready.operations,
            [
                RemovalOperation::ReadCommandZero,
                RemovalOperation::ReadCommandOne,
            ]
        );
        assert_eq!(
            BluetoothSchedulerSoftwareListRemovalObservation::from_split(
                super::BluetoothSchedulerWorkObservation::from_fields_for_validation(false, false),
                ready_observation,
            )
            .classify(),
            BluetoothSchedulerSoftwareListRemovalDisposition::Ready
        );
    }

    #[test]
    fn software_list_removal_stays_pending_while_scheduler_is_busy() {
        let task =
            BluetoothSchedulerSoftwareListRemovalTaskObservation::from_statuses_for_validation(
                true, true,
            );
        assert_eq!(
            BluetoothSchedulerSoftwareListRemovalObservation::from_split(
                super::BluetoothSchedulerWorkObservation::from_fields_for_validation(true, false),
                task,
            )
            .classify(),
            BluetoothSchedulerSoftwareListRemovalDisposition::Pending
        );
    }
}
