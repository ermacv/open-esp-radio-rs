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

/// One of the sixteen scheduler hardware-list indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerFinishedListIndex(u8);

impl BluetoothSchedulerFinishedListIndex {
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
        index: BluetoothSchedulerFinishedListIndex,
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
                index: BluetoothSchedulerFinishedListIndex(index),
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
        let state = self.registers.scheduler_state().read();
        SchedulerStateObservation {
            busy: state.busy().bit(),
            reference_path_state: state.reference_path_state().bit(),
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

struct HardwareSchedulerFinishedListControl<'a> {
    registers: &'a super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerFinishedListControl for HardwareSchedulerFinishedListControl<'_> {
    fn read_finished_list_status(&mut self) -> u16 {
        self.registers
            .scheduler_finished_list_status()
            .read()
            .finished_list_mask()
            .bits()
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
        BluetoothSchedulerFinishedListControl, BluetoothSchedulerInterruptControl,
        SchedulerStateObservation, execute_clear_scheduler_reference,
        execute_finished_list_transfer, execute_reference_gate_observation,
        execute_work_observation,
    };
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
}
