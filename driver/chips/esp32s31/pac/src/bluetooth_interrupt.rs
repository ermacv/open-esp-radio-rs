//! Restricted ownership for the reviewed Bluetooth interrupt transaction.

#![deny(unsafe_code)]

use super::{
    BluetoothInterruptRegisters, BluetoothInterruptSetup, device_fence,
    svd::{fixed_register_image, interrupt_snapshot},
};

/// First-bank sources enabled by the complete primary BTDM IRQ setup helper.
pub const BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK: u32 = 0x0000_8000;

/// Second-bank sources enabled by the complete primary BTDM IRQ setup helper.
pub const BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK: u32 = 0x0000_1300;

/// First-bank sources controlled by the complete dynamic scheduler helper.
///
/// The restricted PAC does not expose an enable transition yet: live shared
/// ISR storage and the scheduler-list consumer remain lifecycle prerequisites.
pub const BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK: u32 = 0x1820_0000;

/// Second-bank source controlled by the complete dynamic scheduler helper.
pub const BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK: u32 = 0x0000_0008;

trait BluetoothInterruptControl {
    fn clear_primary_baseline_bank_0(&mut self);
    fn clear_primary_baseline_bank_1(&mut self);
    fn enable_primary_baseline_bank_0(&mut self);
    fn enable_primary_baseline_bank_1(&mut self);
    fn prepare_output(&mut self);
    fn release_output_0(&mut self);
    fn release_output_1(&mut self);
    fn mask_primary_baseline_bank_0(&mut self);
    fn mask_primary_baseline_bank_1(&mut self);
}

struct HardwareInterruptControl<'a> {
    bank: &'a super::svd::BluetoothInterruptBank,
}

impl BluetoothInterruptControl for HardwareInterruptControl<'_> {
    #[allow(
        unsafe_code,
        reason = "the complete vendor helper qualifies this exact W1C image"
    )]
    fn clear_primary_baseline_bank_0(&mut self) {
        unsafe {
            self.bank.irq_clear_0().write_with_zero(|writer| {
                writer
                    .pending_bits()
                    .bits(BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK)
            });
        }
    }

    #[allow(
        unsafe_code,
        reason = "the complete vendor helper qualifies this exact W1C image"
    )]
    fn clear_primary_baseline_bank_1(&mut self) {
        unsafe {
            self.bank.irq_clear_1().write_with_zero(|writer| {
                writer
                    .pending_bits()
                    .bits(BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK)
            });
        }
    }

    fn enable_primary_baseline_bank_0(&mut self) {
        self.bank
            .irq_enable_0()
            .modify(|_, writer| writer.source_15().set_bit());
    }

    fn enable_primary_baseline_bank_1(&mut self) {
        self.bank.irq_enable_1().modify(|_, writer| {
            writer
                .source_8()
                .set_bit()
                .source_9()
                .set_bit()
                .source_12()
                .set_bit()
        });
    }

    fn prepare_output(&mut self) {
        fixed_register_image::prepare_bluetooth_interrupt_output(self.bank);
    }

    fn release_output_0(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_0(self.bank);
    }

    fn release_output_1(&mut self) {
        fixed_register_image::release_bluetooth_interrupt_output_1(self.bank);
    }

    fn mask_primary_baseline_bank_0(&mut self) {
        self.bank
            .irq_enable_0()
            .modify(|_, writer| writer.source_15().clear_bit());
    }

    fn mask_primary_baseline_bank_1(&mut self) {
        self.bank.irq_enable_1().modify(|_, writer| {
            writer
                .source_8()
                .clear_bit()
                .source_9()
                .clear_bit()
                .source_12()
                .clear_bit()
        });
    }
}

fn execute_primary_prepare(control: &mut impl BluetoothInterruptControl) {
    control.clear_primary_baseline_bank_0();
    control.clear_primary_baseline_bank_1();
    control.enable_primary_baseline_bank_0();
    control.enable_primary_baseline_bank_1();
    control.prepare_output();
}

fn execute_primary_release(control: &mut impl BluetoothInterruptControl) {
    control.release_output_0();
    control.release_output_1();
    control.mask_primary_baseline_bank_0();
    control.mask_primary_baseline_bank_1();
}

/// Controller-side interrupt output prepared before a CPU route is installed.
///
/// This state owns the exact baseline clear/enable transaction followed by
/// `IRQ_CONTROL_0 = 1`, immediately before the vendor platform allocates the
/// primary CPU interrupt. It still exposes no status capture: both CPU routes
/// must share one staged ISR owner before either route is enabled.
#[must_use = "the prepared Bluetooth interrupt output must be routed or released"]
pub struct BluetoothInterruptOutputPrepared {
    peripherals: super::svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

impl BluetoothInterruptSetup {
    /// Prepare the controller-side interrupt output before installing a CPU
    /// route.
    ///
    /// SOURCE: complete ESP32-S31 `libbtdm_common.a` `btdm_hal.c` helpers. The
    /// composite setup clears bank images `0x0000_8000` and `0x0000_1300`,
    /// ORs those same baseline sources into the two enable banks, writes one
    /// to `0x2010_100c`, then the outer path calls the platform interrupt
    /// allocator for source 124. The earlier HAL-init part of that composite
    /// remains a separate lifecycle prerequisite.
    pub fn prepare_controller_output(self) -> BluetoothInterruptOutputPrepared {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_prepare(&mut control);
        device_fence();
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptOutputPrepared {
    /// Release a prepared controller output after any CPU route has been
    /// removed.
    ///
    /// The complete teardown leaf frees the CPU route before this transaction,
    /// writes image one to `0x2010_1010` and then `0x2010_1014`, and finally
    /// clears the same baseline enable groups. Dynamic Link-Layer sources must
    /// already have been quiesced by their own owners.
    pub fn release_controller_output(self) -> BluetoothInterruptSetup {
        let mut control = HardwareInterruptControl {
            bank: &self.peripherals.bluetooth_interrupt_bank,
        };
        execute_primary_release(&mut control);
        device_fence();
        BluetoothInterruptSetup {
            peripherals: self.peripherals,
        }
    }

    /// Transfer the prepared bank into stable storage shared by both hard
    /// handlers before either CPU route is enabled.
    ///
    /// This conversion performs no MMIO and does not itself prove that source
    /// 124 or source 133 has been routed. A platform adapter must retain the
    /// returned value in interrupt-safe storage, bind both routes on one core,
    /// and recover it only after both routes have been disabled.
    pub fn stage_for_cpu_routes(self) -> BluetoothInterruptRegisters {
        BluetoothInterruptRegisters {
            peripherals: self.peripherals,
        }
    }
}

impl BluetoothInterruptRegisters {
    /// Return the interrupt partition to controller-output-only ownership.
    ///
    /// The caller must mask events and disable the CPU route first. This
    /// method performs no controller transaction; the separate output-release
    /// edge publishes the reviewed teardown strobes.
    pub fn deactivate(self) -> BluetoothInterruptOutputPrepared {
        BluetoothInterruptOutputPrepared {
            peripherals: self.peripherals,
        }
    }
}

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

/// Masked primary BT MAC status captured from `IRQ_STATUS_0/1`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPrimaryInterruptObservation {
    bank_0: u32,
    bank_1: u32,
}

impl BluetoothPrimaryInterruptObservation {
    /// Complete first-bank image observed at `0x2010_105c`.
    pub const fn bank_0_bits(self) -> u32 {
        self.bank_0
    }

    /// Complete second-bank image observed at `0x2010_1068`.
    pub const fn bank_1_bits(self) -> u32 {
        self.bank_1
    }
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
    /// Capture and acknowledge one complete primary BT MAC interrupt image.
    ///
    /// This is the exact prefix of the source-124 handler: read masked status
    /// bank zero, read masked status bank one, copy the first image to clear
    /// bank zero, then copy the second image to clear bank one. Callback
    /// dispatch, diagnostics and the special scheduler-event suffix remain a
    /// higher-layer responsibility and are not claimed by this transaction.
    pub fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptObservation {
        let bank = &self.peripherals.bluetooth_interrupt_bank;
        let bank_0 = interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_0(bank);
        let bank_1 = interrupt_snapshot::sample_bluetooth_primary_interrupt_bank_1(bank);
        let observation = BluetoothPrimaryInterruptObservation {
            bank_0: bank_0.bits(),
            bank_1: bank_1.bits(),
        };
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_0(bank, bank_0);
        interrupt_snapshot::acknowledge_bluetooth_primary_interrupt_bank_1(bank, bank_1);
        device_fence();
        observation
    }

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
    use std::vec::Vec;

    use super::{
        BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK, BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK,
        BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK, BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK,
        BluetoothInterruptControl, BluetoothInterruptObservation, BluetoothInterruptSetup,
        BluetoothPrimaryInterruptObservation, execute_primary_prepare, execute_primary_release,
    };
    use crate::RadioHardware;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        ClearBank0,
        ClearBank1,
        EnableBank0,
        EnableBank1,
        PrepareOutput,
        ReleaseOutput0,
        ReleaseOutput1,
        MaskBank0,
        MaskBank1,
    }

    #[derive(Default)]
    struct Recorder {
        operations: Vec<Operation>,
    }

    impl BluetoothInterruptControl for Recorder {
        fn clear_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::ClearBank0);
        }

        fn clear_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::ClearBank1);
        }

        fn enable_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::EnableBank0);
        }

        fn enable_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::EnableBank1);
        }

        fn prepare_output(&mut self) {
            self.operations.push(Operation::PrepareOutput);
        }

        fn release_output_0(&mut self) {
            self.operations.push(Operation::ReleaseOutput0);
        }

        fn release_output_1(&mut self) {
            self.operations.push(Operation::ReleaseOutput1);
        }

        fn mask_primary_baseline_bank_0(&mut self) {
            self.operations.push(Operation::MaskBank0);
        }

        fn mask_primary_baseline_bank_1(&mut self) {
            self.operations.push(Operation::MaskBank1);
        }
    }

    #[test]
    fn observation_preserves_both_opaque_banks() {
        let observation = BluetoothInterruptObservation {
            bank_0: 0xa55a_00f0,
            bank_1: 0x5aa5_f00f,
        };

        assert_eq!(observation.bank_0_bits(), 0xa55a_00f0);
        assert_eq!(observation.bank_1_bits(), 0x5aa5_f00f);
    }

    #[test]
    fn primary_observation_preserves_both_masked_banks() {
        let observation = BluetoothPrimaryInterruptObservation {
            bank_0: 0x1820_8000,
            bank_1: 0x0000_1308,
        };

        assert_eq!(observation.bank_0_bits(), 0x1820_8000);
        assert_eq!(observation.bank_1_bits(), 0x0000_1308);
    }

    #[test]
    fn primary_baseline_masks_are_exact_complete_helper_images() {
        assert_eq!(BLUETOOTH_PRIMARY_BASELINE_BANK_0_MASK, 1 << 15);
        assert_eq!(
            BLUETOOTH_PRIMARY_BASELINE_BANK_1_MASK,
            (1 << 8) | (1 << 9) | (1 << 12)
        );
    }

    #[test]
    fn primary_dynamic_masks_are_exact_complete_helper_images() {
        assert_eq!(
            BLUETOOTH_PRIMARY_DYNAMIC_BANK_0_MASK,
            (1 << 21) | (1 << 27) | (1 << 28)
        );
        assert_eq!(BLUETOOTH_PRIMARY_DYNAMIC_BANK_1_MASK, 1 << 3);
    }

    #[test]
    fn primary_prepare_preserves_clear_enable_strobe_order() {
        let mut recorder = Recorder::default();
        execute_primary_prepare(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                Operation::ClearBank0,
                Operation::ClearBank1,
                Operation::EnableBank0,
                Operation::EnableBank1,
                Operation::PrepareOutput,
            ]
        );
    }

    #[test]
    fn primary_release_preserves_strobe_then_mask_order() {
        let mut recorder = Recorder::default();
        execute_primary_release(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                Operation::ReleaseOutput0,
                Operation::ReleaseOutput1,
                Operation::MaskBank0,
                Operation::MaskBank1,
            ]
        );
    }

    #[test]
    fn interrupt_control_geometry_matches_the_three_reviewed_strobes() {
        let bluetooth = RadioHardware::for_validation().into_bluetooth();
        let (_task, setup) = bluetooth.separate_interrupt_owner();
        let BluetoothInterruptSetup { peripherals } = setup;
        let bank = &peripherals.bluetooth_interrupt_bank;

        assert_eq!(bank.irq_control_0().as_ptr() as usize, 0x2010_100c);
        assert_eq!(bank.irq_control_1().as_ptr() as usize, 0x2010_1010);
        assert_eq!(bank.irq_control_2().as_ptr() as usize, 0x2010_1014);
    }
}
