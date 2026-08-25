//! Pure register images for the reviewed always-awake controller-time latch.
//!
//! The complete hardware path performs one read/OR/write publication, waits
//! for hardware to clear the request bit, then reads the first latched-time
//! word. This module deliberately performs no live MMIO and contains no
//! polling loop. A higher owner must preserve that order and decide which
//! interrupt or bounded timer rechecks an in-flight request.

#![deny(unsafe_code)]

const LATCH_REQUEST: u32 = 1 << 26;

/// One controller-time latch request for the always-awake timer path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothControllerTimeLatchRequest;

impl BluetoothControllerTimeLatchRequest {
    /// Construct one request without touching MMIO.
    pub const fn new() -> Self {
        Self
    }

    /// Return the exact fresh-read OR image published to `SLEEP_TIMER_CONTROL`.
    pub const fn publication_image(self, fresh_control_read: u32) -> u32 {
        fresh_control_read | LATCH_REQUEST
    }
}

/// One fresh `SLEEP_TIMER_CONTROL` observation after request publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerTimeLatchObservation(u32);

impl BluetoothControllerTimeLatchObservation {
    /// Retain the complete control-register image used by one decision.
    pub const fn from_control_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether hardware still owns the latch request.
    pub const fn pending(self) -> bool {
        self.0 & LATCH_REQUEST != 0
    }
}

/// First latched controller-time word read after hardware clears the request.
///
/// The value remains a wrapping positional `u32`: its physical unit and
/// effective counter width are not established by current evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerLatchedTime(u32);

impl BluetoothControllerLatchedTime {
    /// Retain one complete `SLEEP_TIMER_LATCHED_TIME_0` image.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Return the complete positional image.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BluetoothControllerLatchedTime, BluetoothControllerTimeLatchObservation,
        BluetoothControllerTimeLatchRequest,
    };

    #[test]
    fn latch_publication_preserves_every_non_request_bit() {
        let request = BluetoothControllerTimeLatchRequest::new();

        assert_eq!(request.publication_image(0xa123_4567), 0xa523_4567);
        assert_eq!(request.publication_image(0xa523_4567), 0xa523_4567);
    }

    #[test]
    fn only_latch_request_bit_controls_the_wait_decision() {
        assert!(!BluetoothControllerTimeLatchObservation::from_control_bits(0).pending());
        assert!(BluetoothControllerTimeLatchObservation::from_control_bits(0x8400_0000).pending());
        assert!(!BluetoothControllerTimeLatchObservation::from_control_bits(0x8000_0007).pending());
    }

    #[test]
    fn latched_time_retains_the_complete_wrapping_image() {
        assert_eq!(
            BluetoothControllerLatchedTime::from_bits(0xffff_fffe).bits(),
            0xffff_fffe
        );
    }
}
