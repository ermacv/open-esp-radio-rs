//! Exact bounded BLE PHY register-initialization transaction.
//!
//! This module is deliberately crate-private. The recovered vendor lifecycle
//! performs this transaction only after common PHY, Bluetooth baseband,
//! coexistence, controller-stack and callback registration have completed.
//! Possessing [`BluetoothTaskRegisters`] alone proves none of those
//! prerequisites, so this slice must not become an ordinary safe lifecycle
//! transition.

#![deny(unsafe_code)]

use super::{BluetoothControllerSramAddress, BluetoothTaskRegisters, device_fence};

const ENVIRONMENT_LAST_OFFSET: u32 = 0x40;

/// Address of the linked BLE PHY environment consumed by the init body.
///
/// This type proves only the observed address-image constraints: word
/// alignment and that the last address published by the transaction
/// (`environment + 0x40`) is representable. The evidence does not establish a
/// memory window for this full-width pointer. This type therefore does not
/// prove address accessibility, allocation, layout, lifetime, or exclusive
/// controller ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the hidden transaction is staged before its reviewed lifecycle owner"
)]
pub(crate) struct BluetoothPhyEnvironmentAddress(u32);

/// Why an address cannot represent the linked BLE PHY environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the hidden transaction is staged before its reviewed lifecycle owner"
)]
pub(crate) enum BluetoothPhyEnvironmentAddressError {
    /// The recovered transaction shifts the address right by two.
    Unaligned,
    /// The last address published by the transaction is not representable.
    ExtentOverflow,
}

impl BluetoothPhyEnvironmentAddress {
    /// Validate one candidate environment base without granting dereference.
    #[allow(
        dead_code,
        reason = "the hidden transaction is staged before its reviewed lifecycle owner"
    )]
    pub(crate) const fn new(address: u32) -> Result<Self, BluetoothPhyEnvironmentAddressError> {
        if address & 0x3 != 0 {
            return Err(BluetoothPhyEnvironmentAddressError::Unaligned);
        }
        if address.checked_add(ENVIRONMENT_LAST_OFFSET).is_none() {
            return Err(BluetoothPhyEnvironmentAddressError::ExtentOverflow);
        };
        Ok(Self(address))
    }

    const fn address(self) -> u32 {
        self.0
    }

    const fn compressed_member(self, offset: u32) -> u32 {
        ((self.0 + offset) >> 2) & 0x000f_ffff
    }
}

/// Complete external inputs read by the recovered BLE PHY init body.
///
/// Names remain positional where the vendor bytes do not prove hardware
/// meaning. The two option values are deliberately separate because the
/// vendor obtains them through separate linked-state reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the hidden transaction is staged before its reviewed lifecycle owner"
)]
pub(crate) struct BluetoothPhyRegisterInitInputs {
    private_configuration_byte_0x10: u8,
    environment: BluetoothPhyEnvironmentAddress,
    resolving_list: BluetoothControllerSramAddress,
    option_byte_0x55_nonzero: bool,
    option_byte_0x59: u8,
}

impl BluetoothPhyRegisterInitInputs {
    #[allow(
        dead_code,
        reason = "the hidden transaction is staged before its reviewed lifecycle owner"
    )]
    pub(crate) const fn new(
        private_configuration_byte_0x10: u8,
        environment: BluetoothPhyEnvironmentAddress,
        resolving_list: BluetoothControllerSramAddress,
        option_byte_0x55_nonzero: bool,
        option_byte_0x59: u8,
    ) -> Self {
        Self {
            private_configuration_byte_0x10,
            environment,
            resolving_list,
            option_byte_0x55_nonzero,
            option_byte_0x59,
        }
    }
}

const fn replace_byte(image: u32, byte: u32, value: u8) -> u32 {
    let shift = byte * 8;
    (image & !(0xff_u32 << shift)) | ((value as u32) << shift)
}

const fn le_tx_on_delay_image(image: u32, timing_byte: u8) -> u32 {
    (image & 0xf800_ffff) | ((timing_byte.wrapping_sub(10) as u32) << 19)
}

impl BluetoothTaskRegisters {
    /// Execute the exact MMIO body of the recovered BLE PHY register init.
    ///
    /// Every read-modify-write below performs a fresh volatile read. In
    /// particular, repeated updates to `0x20101650`, `0x20101654`, and the four
    /// byte-image words intentionally do not carry a prior software image
    /// across operations.
    ///
    /// The transaction has no rollback edge. A higher lifecycle layer must
    /// treat failure after its first write as fail-stop until a complete
    /// controller teardown is independently recovered and verified.
    ///
    /// # Safety
    ///
    /// The caller must prove the complete pre-task-enable vendor lifecycle:
    /// common PHY and Bluetooth baseband are initialized, coexistence is
    /// enabled, controller/base-stack/HCI state and scheduler callbacks are
    /// registered, the IRQ owner remains inactive, and both pointed-to SRAM
    /// objects remain correctly initialized, exclusively serialized and live
    /// for every hardware consumer. This method does not establish or verify
    /// any of those facts.
    #[allow(
        unsafe_code,
        dead_code,
        reason = "the unsafe signature and raw writes retain unmodeled lifecycle and pointed-storage prerequisites"
    )]
    pub(crate) unsafe fn initialize_ble_phy_registers(
        &mut self,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        macro_rules! write_image {
            ($register:expr, $image:expr) => {{
                let register = $register;
                // SAFETY: this method is reachable only through its unsafe,
                // lifecycle-qualified contract and writes a reviewed image to
                // the exact reviewed register.
                unsafe {
                    register.write_with_zero(|writer| writer.bits($image));
                }
            }};
        }

        macro_rules! modify_image {
            ($register:expr, $transform:expr) => {{
                let register = $register;
                register.modify(|reader, writer| {
                    let image = ($transform)(reader.bits());
                    // SAFETY: the transform is the exact reviewed fresh-read
                    // RMW for this register and the caller upholds the outer
                    // hardware lifecycle contract.
                    unsafe { writer.bits(image) }
                });
            }};
        }

        let timing_byte = inputs.private_configuration_byte_0x10.wrapping_sub(1);
        let environment = inputs.environment.address();
        let resolving_list = (inputs.resolving_list.address() >> 2) & 0x000f_ffff;

        let bluetooth = &self.bluetooth;
        let btmac = &bluetooth.btmac_ble_phy_init;

        // Entry toggle and BTMAC prefix.
        write_image!(bluetooth.ble_phy_init_toggle.init_toggle(), 0);
        write_image!(bluetooth.ble_phy_init_toggle.init_toggle(), 1);
        modify_image!(btmac.init_control_00b4(), |image| image & 0x0002_0000);
        write_image!(btmac.init_ones_00b8(), u32::MAX);
        modify_image!(btmac.lc_tx_on_delay_config(), |image| image & 0xff00_ff00);
        modify_image!(btmac.lc_tx_on_delay_config(), |image| {
            image | u32::from(timing_byte) | (u32::from(timing_byte) << 16)
        });

        // The vendor reaches this leaf through the registered external-BB
        // function table. The restricted transaction preserves its MMIO edge
        // at the same position without claiming a static call-table install.
        modify_image!(bluetooth.bt_v3_2_baseband.le_tx_on_delay(), |image| {
            le_tx_on_delay_image(image, timing_byte)
        });

        write_image!(btmac.init_value_0138(), 0x0000_065b);
        write_image!(btmac.init_bytes_04a4(), 0x0202_0202);
        write_image!(btmac.init_bytes_04a8(), 0x0202_0202);
        write_image!(
            btmac.init_dynamic_image_04a0(),
            inputs.environment.compressed_member(0x2c)
        );
        write_image!(btmac.init_value_04ac(), 0x00ff_0002);
        write_image!(btmac.init_value_045c(), 8);

        // Four independent fresh-read updates at 0x20101654.
        modify_image!(btmac.init_bytes_0254(), |image| image & 0xffff_0000);
        modify_image!(btmac.init_bytes_0254(), |image| image | 0x0000_0101);
        modify_image!(btmac.init_bytes_0254(), |image| image & 0xff80_ffff);
        modify_image!(btmac.init_bytes_0254(), |image| image | 0x00b2_0000);

        write_image!(btmac.init_zero_0074(), 0);
        write_image!(bluetooth.ble_phy_init_phase.init_phase(), 0x20);
        write_image!(bluetooth.ble_hw_accelerator.init_config(), 0x0000_02f0);
        write_image!(
            bluetooth.ble_hw_accelerator.init_sram_region_0(),
            0x2f08_0000
        );
        write_image!(
            bluetooth.ble_hw_accelerator.init_sram_region_1(),
            0x2f00_0000
        );
        write_image!(
            bluetooth.ble_hw_resolving_list.base_pointer(),
            resolving_list
        );

        write_image!(btmac.init_control_0400(), 0x8000_0000);
        modify_image!(btmac.init_control_0400(), |image| image | 0x0040_0000);
        write_image!(btmac.init_value_0540(), 0x0000_07d0);

        // Each byte replacement is a distinct fresh-read RMW in vendor order.
        modify_image!(btmac.init_bytes_0550(), |image| replace_byte(
            image, 0, 0x03
        ));
        modify_image!(btmac.init_bytes_0550(), |image| replace_byte(
            image, 1, 0x03
        ));
        modify_image!(btmac.init_bytes_0550(), |image| replace_byte(
            image, 2, 0x44
        ));

        modify_image!(btmac.init_bytes_0554(), |image| replace_byte(
            image, 0, 0x10
        ));
        modify_image!(btmac.init_bytes_0554(), |image| replace_byte(
            image, 1, 0x10
        ));
        modify_image!(btmac.init_bytes_0554(), |image| replace_byte(
            image, 2, 0x3c
        ));
        modify_image!(btmac.init_bytes_0554(), |image| replace_byte(
            image, 3, 0x28
        ));

        modify_image!(btmac.init_bytes_055c(), |image| replace_byte(
            image, 0, 0x08
        ));
        modify_image!(btmac.init_bytes_055c(), |image| replace_byte(
            image, 1, 0x08
        ));
        modify_image!(btmac.init_bytes_055c(), |image| replace_byte(
            image, 2, 0x08
        ));
        modify_image!(btmac.init_bytes_055c(), |image| replace_byte(
            image, 3, 0x08
        ));

        modify_image!(btmac.init_bytes_0558(), |image| replace_byte(
            image, 0, 0x0c
        ));
        modify_image!(btmac.init_bytes_0558(), |image| replace_byte(
            image, 1, 0x08
        ));
        modify_image!(btmac.init_bytes_0558(), |image| replace_byte(
            image, 2, 0x0c
        ));
        modify_image!(btmac.init_bytes_0558(), |image| replace_byte(
            image, 3, 0x0c
        ));

        modify_image!(btmac.init_high_half_0458(), |image| {
            (image & 0x0000_ffff) | 0x000f_0000
        });
        modify_image!(btmac.init_low_5_054c(), |image| {
            (image & 0xffff_ffe0) | 0x12
        });
        write_image!(bluetooth.ble_phy_init_phase.init_phase(), 0x40);

        if inputs.option_byte_0x55_nonzero {
            modify_image!(btmac.init_branch_control_0470(), |image| image
                | 0x0004_0000);
        }

        write_image!(
            bluetooth.ble_hw_runtime_control.phy_init_configuration(),
            0x100 | u32::from(inputs.option_byte_0x59)
        );
        write_image!(
            bluetooth
                .ble_hw_runtime_control
                .phy_init_configuration_latch(),
            1
        );
        modify_image!(btmac.init_control_00b4(), |image| image | 0x0110_8800);
        modify_image!(btmac.init_control_00c4(), |image| image | 0x0000_0200);

        let controller = &bluetooth.bluetooth_controller_core;
        write_image!(controller.phy_init_zero_0244(), 0);
        write_image!(controller.phy_init_value_01f0(), 0x55);
        write_image!(controller.phy_init_value_0248(), 0x0000_0fff);
        write_image!(
            controller.phy_init_dynamic_image_024c(),
            environment + ENVIRONMENT_LAST_OFFSET
        );

        // One ordering boundary is a reviewed Rust-side addition. It does not
        // replace, merge, or reorder any vendor MMIO edge above.
        device_fence();
    }
}

#[cfg(test)]
mod tests {
    use super::{BluetoothPhyEnvironmentAddress, BluetoothPhyEnvironmentAddressError};

    #[test]
    fn environment_address_checks_the_complete_published_extent() {
        assert_eq!(
            BluetoothPhyEnvironmentAddress::new(0x2f00_0001),
            Err(BluetoothPhyEnvironmentAddressError::Unaligned)
        );
        assert_eq!(
            BluetoothPhyEnvironmentAddress::new(0x2000_0000),
            Ok(BluetoothPhyEnvironmentAddress(0x2000_0000))
        );
        assert_eq!(
            BluetoothPhyEnvironmentAddress::new(u32::MAX - 2),
            Err(BluetoothPhyEnvironmentAddressError::Unaligned)
        );
        assert_eq!(
            BluetoothPhyEnvironmentAddress::new(u32::MAX - 3),
            Err(BluetoothPhyEnvironmentAddressError::ExtentOverflow)
        );
    }
}
