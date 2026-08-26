//! Bounded default-profile disposition for one NRT source-133 interrupt.
//!
//! The pinned default Controller lifecycle has no consumer registered for the
//! NRT callback manager. Its complete hard-handler effect is therefore the
//! restricted PAC raw sample/sample/acknowledge/acknowledge transaction. This
//! module retains that opaque observation without assigning Link-Layer names
//! or publishing synthetic work.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothInterruptRegistersOwner, BluetoothNrtInterruptObservation,
};

/// One acknowledged NRT epoch for the pinned standalone default profile.
///
/// This is intentionally affine even though its contained observation is
/// value-only. A future feature-specific policy must consume the epoch before
/// it can publish any Link-Layer work.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged NRT epoch must be retained or explicitly consumed"]
pub struct BluetoothNrtDefaultInterruptEpoch {
    observation: BluetoothNrtInterruptObservation,
}

impl BluetoothNrtDefaultInterruptEpoch {
    /// Return both opaque raw status images captured before acknowledgement.
    pub const fn observation(&self) -> BluetoothNrtInterruptObservation {
        self.observation
    }
}

trait BluetoothNrtInterruptBackend {
    fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptObservation;
}

impl BluetoothNrtInterruptBackend for BluetoothInterruptRegistersOwner {
    fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptObservation {
        self.capture_nrt_and_acknowledge()
    }
}

fn execute_nrt_default_interrupt_step(
    backend: &mut impl BluetoothNrtInterruptBackend,
) -> BluetoothNrtDefaultInterruptEpoch {
    BluetoothNrtDefaultInterruptEpoch {
        observation: backend.capture_nrt_and_acknowledge(),
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
        observation: Option<BluetoothNrtInterruptObservation>,
        captures: usize,
    }

    impl BluetoothNrtInterruptBackend for Backend {
        fn capture_nrt_and_acknowledge(&mut self) -> BluetoothNrtInterruptObservation {
            self.captures += 1;
            self.observation.take().expect("one NRT epoch")
        }
    }

    #[test]
    fn default_profile_retains_one_epoch_without_synthetic_work() {
        let observation = BluetoothNrtInterruptObservation::for_validation(0, 0);
        let mut backend = Backend {
            observation: Some(observation),
            captures: 0,
        };

        let epoch = execute_nrt_default_interrupt_step(&mut backend);

        assert_eq!(epoch.observation(), observation);
        assert_eq!(backend.captures, 1);
    }
}
