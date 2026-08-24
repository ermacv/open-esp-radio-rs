//! Restricted ownership for the reviewed Bluetooth interrupt transaction.

#![forbid(unsafe_code)]

use super::{BluetoothInterruptRegisters, device_fence, svd::interrupt_snapshot};

/// Complete opaque observation captured and acknowledged by one NRT epoch.
///
/// The two words intentionally have no public inverse constructor and no
/// inferred bit semantics. They are value-only evidence for later event
/// classification and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothInterruptObservation {
    bank_0: u32,
    bank_1: u32,
}

impl BluetoothInterruptObservation {
    /// Complete first-bank image observed at `0x2010_1340`.
    pub const fn bank_0_bits(self) -> u32 {
        self.bank_0
    }

    /// Complete second-bank image observed at `0x2010_1348`.
    pub const fn bank_1_bits(self) -> u32 {
        self.bank_1
    }
}

impl BluetoothInterruptRegisters {
    /// Capture and acknowledge one complete controller interrupt epoch.
    ///
    /// The order is the exact complete ESP32-S31 NRT ISR prefix:
    ///
    /// 1. read first status snapshot;
    /// 2. read second status snapshot;
    /// 3. write the first image to its write-one-to-clear bank;
    /// 4. write the second image to its write-one-to-clear bank.
    ///
    /// Separate sample or acknowledgement methods are deliberately absent:
    /// the reviewed vendor body does not authorize another ordering.
    pub fn capture_and_acknowledge(&mut self) -> BluetoothInterruptObservation {
        let bank_0 = interrupt_snapshot::sample_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        let bank_1 = interrupt_snapshot::sample_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
        );
        let observation = BluetoothInterruptObservation {
            bank_0: bank_0.bits(),
            bank_1: bank_1.bits(),
        };
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_0(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_0,
        );
        interrupt_snapshot::acknowledge_bluetooth_interrupt_bank_1(
            &self.peripherals.bluetooth_interrupt_bank,
            bank_1,
        );
        device_fence();
        observation
    }
}

#[cfg(test)]
mod tests {
    use super::BluetoothInterruptObservation;

    #[test]
    fn observation_preserves_both_opaque_banks() {
        let observation = BluetoothInterruptObservation {
            bank_0: 0xa55a_00f0,
            bank_1: 0x5aa5_f00f,
        };

        assert_eq!(observation.bank_0_bits(), 0xa55a_00f0);
        assert_eq!(observation.bank_1_bits(), 0x5aa5_f00f);
    }
}
