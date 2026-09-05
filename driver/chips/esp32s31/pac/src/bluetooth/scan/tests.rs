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
