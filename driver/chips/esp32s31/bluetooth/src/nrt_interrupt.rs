//! Bounded default-profile disposition for one NRT source-133 interrupt.
//!
//! The pinned default Controller lifecycle has no consumer registered for the
//! NRT callback manager. Its complete hard-handler effect is therefore the
//! restricted PAC sample/sample/acknowledge/acknowledge transaction. This
//! module retains only an acknowledged token without assigning Link-Layer
//! names or publishing synthetic work.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothNrtInterruptAcknowledged,
};

/// One acknowledged NRT epoch for the pinned standalone default profile.
///
/// A future feature-specific policy must consume the epoch before it can
/// publish any Link-Layer work. No raw status image crosses the PAC boundary.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged NRT epoch must be retained or explicitly consumed"]
pub struct BluetoothNrtDefaultInterruptEpoch {
    _acknowledged: BluetoothNrtInterruptAcknowledged,
}

trait BluetoothNrtInterruptBackend {
    fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged;
}

impl BluetoothNrtInterruptBackend for BluetoothInterruptRegistersOwner {
    fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
        self.capture_nrt_and_acknowledge()
    }
}

fn execute_nrt_default_interrupt_step(
    backend: &mut impl BluetoothNrtInterruptBackend,
) -> BluetoothNrtDefaultInterruptEpoch {
    BluetoothNrtDefaultInterruptEpoch {
        _acknowledged: backend.capture_nrt_and_acknowledge(),
    }
}

/// Capture and acknowledge one source-133 epoch for the default profile.
///
/// The function is finite and publishes no scheduler, timer, HCI or
/// Link-Layer event. Supporting a later feature that registers an NRT consumer
/// requires a different typed policy rather than changing this disposition.
pub fn step_nrt_default_interrupt(
    interrupts: &mut BluetoothInterruptRegistersOwner,
) -> BluetoothNrtDefaultInterruptEpoch {
    execute_nrt_default_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Backend {
        acknowledged: Option<BluetoothNrtInterruptAcknowledged>,
        captures: usize,
    }

    impl BluetoothNrtInterruptBackend for Backend {
        fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptAcknowledged {
            self.captures += 1;
            self.acknowledged.take().expect("one NRT epoch")
        }
    }

    #[test]
    fn default_profile_retains_one_epoch_without_synthetic_work() {
        let mut backend = Backend {
            acknowledged: Some(BluetoothNrtInterruptAcknowledged::for_validation()),
            captures: 0,
        };

        let _epoch = execute_nrt_default_interrupt_step(&mut backend);

        assert_eq!(backend.captures, 1);
    }
}
