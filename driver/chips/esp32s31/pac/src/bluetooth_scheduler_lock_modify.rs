//! Affine PAC transaction for scheduler insert-with-lock-modify.
//!
//! The task-side request register and interrupt-side scheduler-state register
//! deliberately have different physical owners. Publication is one finite
//! task-owned MMIO transaction. Wait observations remain split into two value
//! tokens until the controller coordinator joins them at one decision point;
//! neither PAC owner aliases the other register partition.

#![deny(unsafe_code)]

use super::{
    BluetoothControllerSramAddress, BluetoothInterruptRegisters, BluetoothTaskRegisters,
    device_fence,
};

/// One validated scheduler insert-with-lock-modify request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyRequest {
    address: BluetoothControllerSramAddress,
    argument: u8,
}

/// Why a scheduler lock/modify request cannot be represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerLockModifyRequestError {
    /// The reviewed argument field is only four bits wide.
    ArgumentOutsideLowNibble,
}

impl BluetoothSchedulerLockModifyRequest {
    /// Validate the four-bit argument associated with one controller address.
    pub const fn new(
        address: BluetoothControllerSramAddress,
        argument: u8,
    ) -> Result<Self, BluetoothSchedulerLockModifyRequestError> {
        if argument > 0x0f {
            return Err(BluetoothSchedulerLockModifyRequestError::ArgumentOutsideLowNibble);
        }
        Ok(Self { address, argument })
    }

    /// Return the first fresh-read RMW image for `OPERATIONAL_WORD_036C`.
    const fn argument_clear_image(self, first_fresh_read: u32) -> u32 {
        first_fresh_read & !0x0f
    }

    /// Return the second fresh-read RMW image for `OPERATIONAL_WORD_036C`.
    ///
    /// The reviewed path clears the low nibble, then independently ORs the
    /// request argument into a second fresh read. The caller must therefore
    /// supply the second read, not reuse the image observed before the clear.
    /// Hardware changes in the low nibble between the two writes are retained,
    /// matching the observed OR rather than inventing a full field assignment.
    const fn argument_image(self, second_fresh_read: u32) -> u32 {
        second_fresh_read | self.argument as u32
    }

    /// Return the validated CPU address without granting dereference access.
    pub const fn address(self) -> BluetoothControllerSramAddress {
        self.address
    }

    /// Return the validated positional four-bit argument.
    pub const fn argument(self) -> u8 {
        self.argument
    }
}

/// Task-owned request-register fields captured at one event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the task-side scheduler observation must be joined or discarded"]
pub struct BluetoothSchedulerLockModifyTaskObservation {
    start: bool,
    result: u8,
}

impl BluetoothSchedulerLockModifyTaskObservation {
    /// Construct semantic task fields for host-side controller validation.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(start: bool, result: u8) -> Self {
        assert!(
            result <= 0x0f,
            "scheduler result must fit its reviewed field"
        );
        Self { start, result }
    }
}

/// Interrupt-owned scheduler BUSY field captured at one event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the interrupt-side scheduler observation must be joined or discarded"]
pub struct BluetoothSchedulerLockModifyInterruptObservation {
    busy: bool,
}

impl BluetoothSchedulerLockModifyInterruptObservation {
    /// Reconstruct one semantic interrupt observation after it crossed an
    /// atomic value-only handoff. No raw register image is accepted here.
    pub const fn from_busy(busy: bool) -> Self {
        Self { busy }
    }

    /// Whether the captured scheduler state was busy.
    pub const fn is_busy(self) -> bool {
        self.busy
    }
}

/// Scheduler-state and request fields sampled for one transition decision.
///
/// The pure type cannot prove temporal atomicity. A live controller must
/// construct it from the task/ISR coordinator at the exact decision point and
/// must not combine unrelated or stale observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyObservation {
    busy: bool,
    start: bool,
    result: u8,
}

impl BluetoothSchedulerLockModifyObservation {
    /// Join one task value and one interrupt value at the coordinator's
    /// current decision point.
    ///
    /// This performs no MMIO. The upper owner is responsible for accepting
    /// only observations belonging to the same scheduler event epoch.
    pub const fn from_split(
        interrupt: BluetoothSchedulerLockModifyInterruptObservation,
        task: BluetoothSchedulerLockModifyTaskObservation,
    ) -> Self {
        Self {
            busy: interrupt.busy,
            start: task.start,
            result: task.result,
        }
    }

    /// Construct semantic fields for host validation without reproducing PAC
    /// register geometry in a controller test.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(busy: bool, start: bool, result: u8) -> Self {
        assert!(
            result <= 0x0f,
            "scheduler result must fit its reviewed field"
        );
        Self {
            busy,
            start,
            result,
        }
    }

    /// Whether the reference path would continue waiting.
    ///
    /// Progress is blocked only while both scheduler BUSY and request START
    /// remain set.
    pub const fn wait_active(self) -> bool {
        self.busy && self.start
    }

    /// Project the positional publication result after the in-flight wait ends.
    ///
    /// This method encodes the complete current body: idle scheduler state
    /// reports zero; otherwise bits 30:27 from the request word are retained.
    /// This is not a radio-event or descriptor-completion status. The
    /// Bluetooth state machine makes it public only after request publication.
    #[doc(hidden)]
    pub const fn result_code_after_publication(self) -> u8 {
        if !self.busy { 0 } else { self.result }
    }
}

/// Proof that the three ordered publication writes and trailing device fence
/// completed through the unique task owner.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a published scheduler request remains in flight"]
pub struct BluetoothSchedulerLockModifyPublished {
    _private: (),
}

#[cfg(any(feature = "validation-probes", test))]
impl BluetoothSchedulerLockModifyPublished {
    /// Construct a host-only publication proof for controller state-machine
    /// validation. Production builds can obtain this value only from live PAC
    /// publication.
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

trait BluetoothSchedulerLockModifyControl {
    fn read_operational_word(&mut self) -> u32;
    fn write_operational_word(&mut self, image: u32);
    fn publish_request(&mut self, address: BluetoothControllerSramAddress);
    fn order_after_publication(&mut self);
}

struct HardwareBluetoothSchedulerLockModifyControl<'registers> {
    registers: &'registers super::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerLockModifyControl for HardwareBluetoothSchedulerLockModifyControl<'_> {
    fn read_operational_word(&mut self) -> u32 {
        self.registers.operational_word_036c().read().bits()
    }

    fn write_operational_word(&mut self, image: u32) {
        super::generated::publish_bluetooth_scheduler_operational_word(
            self.registers,
            super::generated::BluetoothSchedulerOperationalWordImage::new(image),
        );
    }

    fn publish_request(&mut self, address: BluetoothControllerSramAddress) {
        super::svd::zero_based_field_write::publish_bluetooth_scheduler_lock_modify_request(
            self.registers,
            address.compressed_image(),
            true,
        );
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_lock_modify_publication(
    control: &mut impl BluetoothSchedulerLockModifyControl,
    request: BluetoothSchedulerLockModifyRequest,
) -> BluetoothSchedulerLockModifyPublished {
    let first = control.read_operational_word();
    control.write_operational_word(request.argument_clear_image(first));

    let second = control.read_operational_word();
    control.write_operational_word(request.argument_image(second));

    control.publish_request(request.address());
    control.order_after_publication();
    BluetoothSchedulerLockModifyPublished { _private: () }
}

impl BluetoothTaskRegisters {
    /// Execute the finite task-side publication body and return after one
    /// device-ordering fence.
    ///
    /// The controller layer admits this operation only after a fresh split
    /// observation ended the pre-publication wait. The PAC method itself owns
    /// all three writes: two independent fresh-read updates of the operational
    /// word followed by the typed request publication.
    #[doc(hidden)]
    pub fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        let mut control = HardwareBluetoothSchedulerLockModifyControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_scheduler_lock_modify_publication(&mut control, request)
    }

    /// Capture task-owned START and RESULT fields without exposing their raw
    /// register image or its generated bit geometry.
    pub fn capture_scheduler_lock_modify_task(
        &mut self,
    ) -> BluetoothSchedulerLockModifyTaskObservation {
        let request = self
            .bluetooth
            .bluetooth_controller_core
            .scheduler_lock_modify_request()
            .read();
        BluetoothSchedulerLockModifyTaskObservation {
            start: request.start().bit_is_set(),
            result: request.result().bits(),
        }
    }
}

impl BluetoothInterruptRegisters {
    /// Capture the interrupt-owned BUSY field without borrowing any task-side
    /// controller register.
    pub fn capture_scheduler_lock_modify_interrupt(
        &mut self,
    ) -> BluetoothSchedulerLockModifyInterruptObservation {
        let scheduler = self
            .peripherals
            .bluetooth_scheduler_interrupt_runtime
            .scheduler_state()
            .read();
        BluetoothSchedulerLockModifyInterruptObservation {
            busy: scheduler.busy().bit_is_set(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BluetoothSchedulerLockModifyControl, BluetoothSchedulerLockModifyObservation,
        BluetoothSchedulerLockModifyRequest, BluetoothSchedulerLockModifyRequestError,
        execute_scheduler_lock_modify_publication,
    };
    use crate::BluetoothControllerSramAddress;

    #[test]
    fn request_validation_rejects_an_argument_outside_the_protocol_field() {
        let address = BluetoothControllerSramAddress::new(0x2f12_3454)
            .expect("test address is representable");
        assert_eq!(
            BluetoothSchedulerLockModifyRequest::new(address, 0x10),
            Err(BluetoothSchedulerLockModifyRequestError::ArgumentOutsideLowNibble)
        );
    }

    #[test]
    fn wait_requires_busy_and_start_simultaneously() {
        for (busy, start, expected) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            assert_eq!(
                BluetoothSchedulerLockModifyObservation::from_fields_for_validation(
                    busy, start, 0,
                )
                .wait_active(),
                expected
            );
        }
    }

    #[test]
    fn result_projection_uses_idle_zero_or_busy_result_nibble() {
        assert_eq!(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(false, true, 15)
                .result_code_after_publication(),
            0
        );
        assert_eq!(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, false, 11)
                .result_code_after_publication(),
            0x0b
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ReadOperationalWord,
        WriteOperationalWord,
        PublishRequest(BluetoothControllerSramAddress),
        DeviceFence,
    }

    struct Recorder {
        reads: [u32; 2],
        next_read: usize,
        operations: Vec<Operation>,
    }

    impl BluetoothSchedulerLockModifyControl for Recorder {
        fn read_operational_word(&mut self) -> u32 {
            let value = self.reads[self.next_read];
            self.next_read += 1;
            self.operations.push(Operation::ReadOperationalWord);
            value
        }

        fn write_operational_word(&mut self, _image: u32) {
            self.operations.push(Operation::WriteOperationalWord);
        }

        fn publish_request(&mut self, address: BluetoothControllerSramAddress) {
            self.operations.push(Operation::PublishRequest(address));
        }

        fn order_after_publication(&mut self) {
            self.operations.push(Operation::DeviceFence);
        }
    }

    #[test]
    fn publication_uses_two_fresh_rmw_edges_before_request_and_fence() {
        let address = BluetoothControllerSramAddress::new(0x2f00_0040)
            .expect("test address is representable");
        let request = BluetoothSchedulerLockModifyRequest::new(address, 6)
            .expect("test argument is representable");
        let mut recorder = Recorder {
            reads: [u32::MAX, 0],
            next_read: 0,
            operations: Vec::new(),
        };

        let _published = execute_scheduler_lock_modify_publication(&mut recorder, request);

        assert_eq!(recorder.next_read, 2);
        assert_eq!(
            recorder.operations,
            [
                Operation::ReadOperationalWord,
                Operation::WriteOperationalWord,
                Operation::ReadOperationalWord,
                Operation::WriteOperationalWord,
                Operation::PublishRequest(address),
                Operation::DeviceFence,
            ]
        );
    }
}
