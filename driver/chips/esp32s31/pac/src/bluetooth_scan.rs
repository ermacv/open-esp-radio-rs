//! Restricted BLE scanner command publication.

#![deny(unsafe_code)]

use super::{BluetoothTaskRegisters, device_fence};

pub use super::generated::BluetoothScanCommand0Image;

/// Closed input for one reviewed scanner command publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothScanStartRequest {
    command_0: BluetoothScanCommand0Image,
}

impl BluetoothScanStartRequest {
    /// Select one of the two complete reviewed positional `COMMAND_0` images.
    ///
    /// The type deliberately assigns no guessed mode meaning to either image.
    pub const fn new(command_0: BluetoothScanCommand0Image) -> Self {
        Self { command_0 }
    }

    /// Return the retained positional command-zero selection.
    pub const fn command_0(self) -> BluetoothScanCommand0Image {
        self.command_0
    }
}

/// Affine proof that one complete scanner command sequence was published.
#[must_use = "the scanner command publication belongs to a live controller epoch"]
pub struct BluetoothScanStartPublished {
    request: BluetoothScanStartRequest,
}

impl BluetoothScanStartPublished {
    /// Return the closed request consumed by this publication.
    pub const fn request(&self) -> BluetoothScanStartRequest {
        self.request
    }
}

trait BluetoothScanStartTransaction {
    fn publish_command_2_image_1(&mut self);
    fn publish_command_1_image_1(&mut self);
    fn publish_command_0(&mut self, image: BluetoothScanCommand0Image);
}

fn execute_scan_start_transaction(
    transaction: &mut impl BluetoothScanStartTransaction,
    request: BluetoothScanStartRequest,
) {
    transaction.publish_command_2_image_1();
    transaction.publish_command_1_image_1();
    transaction.publish_command_0(request.command_0());
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

    fn publish_command_0(&mut self, image: BluetoothScanCommand0Image) {
        match image {
            BluetoothScanCommand0Image::Image1 => {
                super::svd::fixed_register_image::publish_bluetooth_scan_command_0_image_1(
                    self.registers,
                );
            }
            BluetoothScanCommand0Image::Image256 => {
                super::svd::fixed_register_image::publish_bluetooth_scan_command_0_image_256(
                    self.registers,
                );
            }
        }
    }
}

impl BluetoothTaskRegisters {
    /// Publish the complete reviewed three-command scanner transaction.
    ///
    /// Descriptor and list writes are ordered before the first command. The
    /// positional command-zero choice remains explicit because its branch
    /// predicate has not yet been reduced to a portable scanner policy.
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
    pub unsafe fn publish_scan_start(
        &mut self,
        request: BluetoothScanStartRequest,
    ) -> BluetoothScanStartPublished {
        device_fence();
        let mut transaction = PacBluetoothScanStartTransaction {
            registers: &self.bluetooth.ble_scan_control,
        };
        execute_scan_start_transaction(&mut transaction, request);
        device_fence();
        BluetoothScanStartPublished { request }
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BluetoothScanCommand0Image, BluetoothScanStartRequest, BluetoothScanStartTransaction,
        execute_scan_start_transaction,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ScanStartStep {
        Command2,
        Command1,
        Command0(BluetoothScanCommand0Image),
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

        fn publish_command_0(&mut self, image: BluetoothScanCommand0Image) {
            self.steps.push(ScanStartStep::Command0(image));
        }
    }

    #[test]
    fn both_closed_requests_preserve_the_reviewed_command_order() {
        for image in [
            BluetoothScanCommand0Image::Image1,
            BluetoothScanCommand0Image::Image256,
        ] {
            let mut transaction = RecordingScanStartTransaction::default();
            execute_scan_start_transaction(&mut transaction, BluetoothScanStartRequest::new(image));
            assert_eq!(
                transaction.steps,
                [
                    ScanStartStep::Command2,
                    ScanStartStep::Command1,
                    ScanStartStep::Command0(image),
                ]
            );
        }
    }
}
