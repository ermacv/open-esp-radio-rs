//! Restricted scheduler MMIO shared by the primary ISR and its worker.

#![deny(unsafe_code)]

use super::{
    BluetoothInterruptRegisters, BluetoothTaskRegisters, device_fence,
    svd::{zero_based_field_write, zero_register_write},
};

/// First complete `SCHEDULER_STATE` image read for bank-one source 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerReferenceGateObservation(u32);

impl BluetoothSchedulerReferenceGateObservation {
    /// Wrap one complete register image at the reference-gate temporal point.
    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Construct a semantic gate observation for upper-layer validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_busy_for_validation(busy: bool) -> Self {
        Self(if busy { 1 << 31 } else { 0 })
    }

    /// Whether the scheduler was busy at the reference-gate temporal point.
    pub const fn is_busy(self) -> bool {
        self.0 & (1 << 31) != 0
    }
}

/// Later complete `SCHEDULER_STATE` image used to construct deferred work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerWorkObservation(u32);

impl BluetoothSchedulerWorkObservation {
    /// Wrap one complete register image at the deferred-work temporal point.
    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Construct semantic deferred-work fields for upper-layer validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(busy: bool, reference_state_29: bool) -> Self {
        Self((if busy { 1 << 31 } else { 0 }) | (if reference_state_29 { 1 << 29 } else { 0 }))
    }

    /// Whether the scheduler was busy at the deferred-work temporal point.
    pub const fn is_busy(self) -> bool {
        self.0 & (1 << 31) != 0
    }

    /// Whether the reviewed reference path was active at this temporal point.
    pub const fn reference_path_active(self) -> bool {
        self.0 & ((1 << 31) | (1 << 29)) == ((1 << 31) | (1 << 29))
    }
}

/// Finished hardware-list mask transferred by one scheduler-worker pop attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the scheduler finished-list observation must be drained or explicitly discarded"]
pub struct BluetoothSchedulerFinishedListObservation(u16);

impl BluetoothSchedulerFinishedListObservation {
    /// Retain one complete reviewed field image.
    ///
    /// Every `u16` is a representable subset of the sixteen hardware lists.
    /// Live code obtains this value through the task-owned MMIO transfer;
    /// virtual-time tests may construct it directly.
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// Return the complete sixteen-list mask.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether no hardware list was reported finished.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether one representable hardware-list index was reported finished.
    pub const fn contains(self, list_index: u8) -> bool {
        list_index < 16 && self.0 & (1u16 << list_index) != 0
    }
}

trait BluetoothSchedulerInterruptControl {
    fn read_scheduler_state(&mut self) -> u32;
    fn clear_scheduler_reference(&mut self);
}

struct HardwareSchedulerInterruptControl<'a> {
    registers: &'a super::svd::BluetoothSchedulerInterruptRuntime,
}

impl BluetoothSchedulerInterruptControl for HardwareSchedulerInterruptControl<'_> {
    fn read_scheduler_state(&mut self) -> u32 {
        self.registers.scheduler_state().read().bits()
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
    BluetoothSchedulerReferenceGateObservation::from_bits(control.read_scheduler_state())
}

fn execute_clear_scheduler_reference(control: &mut impl BluetoothSchedulerInterruptControl) {
    control.clear_scheduler_reference();
}

fn execute_work_observation(
    control: &mut impl BluetoothSchedulerInterruptControl,
) -> BluetoothSchedulerWorkObservation {
    BluetoothSchedulerWorkObservation::from_bits(control.read_scheduler_state())
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
    pub fn clear_scheduler_reference(&mut self) {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_clear_scheduler_reference(&mut control);
        device_fence();
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
        execute_clear_scheduler_reference, execute_finished_list_transfer,
        execute_reference_gate_observation, execute_work_observation,
    };
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum InterruptOperation {
        ReadState,
        ClearReference,
    }

    struct InterruptRecorder {
        states: [u32; 2],
        next_state: usize,
        operations: Vec<InterruptOperation>,
    }

    impl BluetoothSchedulerInterruptControl for InterruptRecorder {
        fn read_scheduler_state(&mut self) -> u32 {
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
        WriteReport(u16),
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

        fn write_finished_list_report(&mut self, value: u16) {
            self.operations
                .push(FinishedListOperation::WriteReport(value));
        }
    }

    #[test]
    fn temporal_state_reads_remain_distinct_across_reference_clear() {
        let mut recorder = InterruptRecorder {
            states: [0, u32::MAX],
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

        let observation = execute_finished_list_transfer(&mut recorder);

        assert_eq!(observation.bits(), 0xa55a);
        assert!(!observation.is_empty());
        assert!(observation.contains(1));
        assert!(!observation.contains(0));
        assert!(!observation.contains(16));
        assert_eq!(
            recorder.operations,
            [
                FinishedListOperation::ReadStatus,
                FinishedListOperation::WriteReport(0xa55a),
            ]
        );
    }
}
