//! Pure register images for the reviewed scheduler lock/modify transaction.
//!
//! The task-side request register and interrupt-side scheduler-state register
//! deliberately have different physical owners. This module therefore does
//! not perform live MMIO or pretend that the two words can be borrowed through
//! one peripheral. It only represents exact images and a same-decision-point
//! observation for the controller-level event-driven state machine.

#![deny(unsafe_code)]

use super::BluetoothControllerSramAddress;

const SCHEDULER_BUSY: u32 = 1 << 31;
const REQUEST_START: u32 = 1 << 31;
const REQUEST_RESULT_SHIFT: u32 = 27;
const REQUEST_RESULT_MASK: u32 = 0x0f;

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
    pub const fn argument_clear_image(self, first_fresh_read: u32) -> u32 {
        first_fresh_read & !0x0f
    }

    /// Return the second fresh-read RMW image for `OPERATIONAL_WORD_036C`.
    ///
    /// The reviewed path clears the low nibble, then independently ORs the
    /// request argument into a second fresh read. The caller must therefore
    /// supply the second read, not reuse the image observed before the clear.
    /// Hardware changes in the low nibble between the two writes are retained,
    /// matching the observed OR rather than inventing a full field assignment.
    pub const fn argument_image(self, second_fresh_read: u32) -> u32 {
        second_fresh_read | self.argument as u32
    }

    /// Return the exact complete request publication image.
    pub const fn publication_image(self) -> u32 {
        REQUEST_START | self.address.compressed()
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

/// Scheduler-state and request images sampled for one transition decision.
///
/// The pure type cannot prove temporal atomicity. A live controller must
/// construct it from the task/ISR coordinator at the exact decision point and
/// must not combine unrelated or stale observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyObservation {
    scheduler_state: u32,
    request: u32,
}

impl BluetoothSchedulerLockModifyObservation {
    /// Retain the two complete words used by the reviewed predicate.
    pub const fn from_registers(scheduler_state: u32, request: u32) -> Self {
        Self {
            scheduler_state,
            request,
        }
    }

    /// Whether the reference path would continue waiting.
    ///
    /// Progress is blocked only while both scheduler BUSY and request START
    /// remain set.
    pub const fn wait_active(self) -> bool {
        self.scheduler_state & SCHEDULER_BUSY != 0 && self.request & REQUEST_START != 0
    }

    /// Project the positional publication result after the in-flight wait ends.
    ///
    /// This method encodes the complete current body: idle scheduler state
    /// reports zero; otherwise bits 30:27 from the request word are retained.
    /// This is not a radio-event or descriptor-completion status. The
    /// Bluetooth state machine makes it public only after request publication.
    #[doc(hidden)]
    pub const fn result_code_after_publication(self) -> u8 {
        if self.scheduler_state & SCHEDULER_BUSY == 0 {
            0
        } else {
            ((self.request >> REQUEST_RESULT_SHIFT) & REQUEST_RESULT_MASK) as u8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyRequest,
        BluetoothSchedulerLockModifyRequestError,
    };
    use crate::BluetoothControllerSramAddress;

    #[test]
    fn request_images_match_the_reviewed_transaction() {
        let address = BluetoothControllerSramAddress::new(0x2f12_3454)
            .expect("test address is representable");
        let request = BluetoothSchedulerLockModifyRequest::new(address, 0x0b)
            .expect("low-nibble argument is accepted");

        assert_eq!(request.address(), address);
        assert_eq!(request.argument(), 0x0b);
        assert_eq!(request.argument_clear_image(0xdead_bee5), 0xdead_bee0);
        assert_eq!(request.argument_image(0xdead_bee0), 0xdead_beeb);
        assert_eq!(request.argument_image(0xdead_bee4), 0xdead_beef);
        assert_eq!(request.publication_image(), 0x8004_8d15);
        assert_eq!(
            BluetoothSchedulerLockModifyRequest::new(address, 0x10),
            Err(BluetoothSchedulerLockModifyRequestError::ArgumentOutsideLowNibble)
        );
    }

    #[test]
    fn wait_requires_busy_and_start_simultaneously() {
        for (scheduler_state, request, expected) in [
            (0, 0, false),
            (1 << 31, 0, false),
            (0, 1 << 31, false),
            (1 << 31, 1 << 31, true),
        ] {
            assert_eq!(
                BluetoothSchedulerLockModifyObservation::from_registers(scheduler_state, request,)
                    .wait_active(),
                expected
            );
        }
    }

    #[test]
    fn result_projection_uses_idle_zero_or_busy_result_nibble() {
        assert_eq!(
            BluetoothSchedulerLockModifyObservation::from_registers(0, 0x7800_0000)
                .result_code_after_publication(),
            0
        );
        assert_eq!(
            BluetoothSchedulerLockModifyObservation::from_registers(1 << 31, 0x5800_0000)
                .result_code_after_publication(),
            0x0b
        );
    }
}
