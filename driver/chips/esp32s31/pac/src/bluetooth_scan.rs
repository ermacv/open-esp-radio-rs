//! Restricted BLE scanner command publication.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

/// Affine proof that one complete scanner command sequence was published.
#[must_use = "the scanner command publication belongs to a live controller epoch"]
pub struct BluetoothScanStartPublished {
    _private: (),
}

trait BluetoothScanStartTransaction {
    fn publish_command_2_image_1(&mut self);
    fn publish_command_1_image_1(&mut self);
    fn publish_standard_backoff(&mut self);
}

fn execute_scan_start_transaction(transaction: &mut impl BluetoothScanStartTransaction) {
    transaction.publish_command_2_image_1();
    transaction.publish_command_1_image_1();
    transaction.publish_standard_backoff();
}

struct PacBluetoothScanStartTransaction<'registers> {
    registers: &'registers crate::svd::BleScanControl,
}

impl BluetoothScanStartTransaction for PacBluetoothScanStartTransaction<'_> {
    fn publish_command_2_image_1(&mut self) {
        super::svd::fixed_register_image::publish_bluetooth_scan_command_2_image_1(self.registers);
    }

    fn publish_command_1_image_1(&mut self) {
        super::svd::fixed_register_image::publish_bluetooth_scan_command_1_image_1(self.registers);
    }

    fn publish_standard_backoff(&mut self) {
        super::svd::fixed_register_image::publish_bluetooth_scan_standard_backoff(self.registers);
    }
}

impl BluetoothTaskRegisters {
    /// Publish the complete reviewed three-command scanner transaction.
    ///
    /// Descriptor and list writes are ordered before the first command. The
    /// final command selects the standard scan-backoff policy fixed by the
    /// source-owned standalone Controller profile.
    ///
    /// # Safety
    ///
    /// The caller must own a powered controller epoch with a fully initialized
    /// scanner link state, scheduler item and RX list, and must serialize this
    /// publication with every task and interrupt owner of scanner hardware.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the signature retains powered scanner-lifecycle and serialization prerequisites"
    )]
    pub unsafe fn publish_scan_start(&mut self) -> BluetoothScanStartPublished {
        device_fence();
        let mut transaction = PacBluetoothScanStartTransaction {
            registers: &self.bluetooth.ble_scan_control,
        };
        execute_scan_start_transaction(&mut transaction);
        device_fence();
        BluetoothScanStartPublished { _private: () }
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{BluetoothScanStartTransaction, execute_scan_start_transaction};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ScanStartStep {
        Command2,
        Command1,
        StandardBackoff,
    }

    #[derive(Default)]
    struct RecordingScanStartTransaction {
        steps: Vec<ScanStartStep>,
    }

    impl BluetoothScanStartTransaction for RecordingScanStartTransaction {
        fn publish_command_2_image_1(&mut self) {
            self.steps.push(ScanStartStep::Command2);
        }

        fn publish_command_1_image_1(&mut self) {
            self.steps.push(ScanStartStep::Command1);
        }

        fn publish_standard_backoff(&mut self) {
            self.steps.push(ScanStartStep::StandardBackoff);
        }
    }

    #[test]
    fn standard_backoff_start_preserves_the_reviewed_command_order() {
        let mut transaction = RecordingScanStartTransaction::default();
        execute_scan_start_transaction(&mut transaction);
        assert_eq!(
            transaction.steps,
            [
                ScanStartStep::Command2,
                ScanStartStep::Command1,
                ScanStartStep::StandardBackoff,
            ]
        );
    }
}
