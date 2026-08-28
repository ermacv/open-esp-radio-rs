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
        reason = "the unsafe signature and field accessors retain unmodeled lifecycle and pointed-storage prerequisites"
    )]
    pub(crate) unsafe fn initialize_ble_phy_registers(
        &mut self,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        // SAFETY: every call below writes through a named SVD field accessor.
        // The reviewed transaction supplies values inside those fields; the
        // caller upholds the surrounding lifecycle and pointed-storage rules.
        unsafe {
            let timing_byte = inputs.private_configuration_byte_0x10.wrapping_sub(1);
            let environment = inputs.environment.address();
            let resolving_list = (inputs.resolving_list.address() >> 2) & 0x000f_ffff;

            let bluetooth = &self.bluetooth;
            let btmac = &bluetooth.btmac_ble_phy_init;

            // Entry toggle and BTMAC prefix.
            bluetooth
                .ble_phy_init_toggle
                .init_toggle()
                .write_with_zero(|writer| writer.image().bits(0));
            bluetooth
                .ble_phy_init_toggle
                .init_toggle()
                .write_with_zero(|writer| writer.image().bits(1));
            let init_control = btmac.init_control_00b4();
            let preserved_bit_17 = init_control.read().init_preserve_17().bit();
            init_control.write_with_zero(|writer| writer.init_preserve_17().bit(preserved_bit_17));
            btmac
                .init_ones_00b8()
                .write_with_zero(|writer| writer.init_image().bits(u32::MAX));
            btmac.lc_tx_on_delay_config().modify(|_, writer| {
                writer
                    .lc_tx_on_delay()
                    .bits(0)
                    .init_duplicate_byte()
                    .bits(0)
            });
            super::generated::or_ble_phy_init_tx_on_delay(
                btmac,
                super::generated::BluetoothPhyInitTimingByte::new(u32::from(timing_byte))
                    .expect("one byte always fits the reviewed BLE PHY timing domain"),
            );

            // The vendor reaches this leaf through the registered external-BB
            // function table. The restricted transaction preserves its MMIO edge
            // at the same position without claiming a static call-table install.
            bluetooth
                .bt_v3_2_baseband
                .le_tx_on_delay()
                .modify(|_, writer| {
                    writer
                        .force_zero_bits_16_18()
                        .bits(0)
                        .encoded_value_minus_10()
                        .bits(timing_byte.wrapping_sub(10))
                });

            btmac
                .init_value_0138()
                .write_with_zero(|writer| writer.init_image().bits(0x0000_065b));
            btmac.init_bytes_04a4().write_with_zero(|writer| {
                writer
                    .init_byte_0()
                    .bits(2)
                    .init_byte_1()
                    .bits(2)
                    .init_byte_2()
                    .bits(2)
                    .init_byte_3()
                    .bits(2)
            });
            btmac.init_bytes_04a8().write_with_zero(|writer| {
                writer
                    .init_byte_0()
                    .bits(2)
                    .init_byte_1()
                    .bits(2)
                    .init_byte_2()
                    .bits(2)
                    .init_byte_3()
                    .bits(2)
            });
            btmac.init_dynamic_image_04a0().write_with_zero(|writer| {
                writer
                    .init_image()
                    .bits(inputs.environment.compressed_member(0x2c))
            });
            btmac
                .init_value_04ac()
                .write_with_zero(|writer| writer.init_image().bits(0x00ff_0002));
            btmac
                .init_value_045c()
                .write_with_zero(|writer| writer.init_image().bits(8));

            // Four independent fresh-read updates at 0x20101654.
            let init_bytes = btmac.init_bytes_0254();
            init_bytes.modify(|_, writer| writer.init_byte_0().bits(0).init_byte_1().bits(0));
            super::generated::or_ble_phy_init_low_byte_pair(btmac);
            init_bytes.modify(|_, writer| writer.init_byte_2_low_7().bits(0));
            super::generated::or_ble_phy_init_byte_2(btmac);

            btmac
                .init_zero_0074()
                .write_with_zero(|writer| writer.init_image().bits(0));
            bluetooth
                .ble_phy_init_phase
                .init_phase()
                .write_with_zero(|writer| writer.image().bits(0x20));
            bluetooth
                .ble_hw_accelerator
                .init_config()
                .write_with_zero(|writer| writer.image().bits(0x0000_02f0));
            bluetooth
                .ble_hw_accelerator
                .init_sram_region_0()
                .write_with_zero(|writer| writer.image().bits(0x2f08_0000));
            bluetooth
                .ble_hw_accelerator
                .init_sram_region_1()
                .write_with_zero(|writer| writer.image().bits(0x2f00_0000));
            bluetooth
                .ble_hw_resolving_list
                .base_pointer()
                .write_with_zero(|writer| writer.compressed_sram_pointer().bits(resolving_list));

            btmac
                .init_control_0400()
                .write_with_zero(|writer| writer.init_enable_31().set_bit());
            btmac
                .init_control_0400()
                .modify(|_, writer| writer.init_enable_22().set_bit());
            btmac
                .init_value_0540()
                .write_with_zero(|writer| writer.init_image().bits(0x0000_07d0));

            // Each byte replacement is a distinct fresh-read RMW in vendor order.
            let bytes_0550 = btmac.init_bytes_0550();
            bytes_0550.modify(|_, writer| writer.init_byte_0().bits(0x03));
            bytes_0550.modify(|_, writer| writer.init_byte_1().bits(0x03));
            bytes_0550.modify(|_, writer| writer.init_byte_2().bits(0x44));

            let bytes_0554 = btmac.init_bytes_0554();
            bytes_0554.modify(|_, writer| writer.init_byte_0().bits(0x10));
            bytes_0554.modify(|_, writer| writer.init_byte_1().bits(0x10));
            bytes_0554.modify(|_, writer| writer.init_byte_2().bits(0x3c));
            bytes_0554.modify(|_, writer| writer.init_byte_3().bits(0x28));

            let bytes_055c = btmac.init_bytes_055c();
            bytes_055c.modify(|_, writer| writer.init_byte_0().bits(0x08));
            bytes_055c.modify(|_, writer| writer.init_byte_1().bits(0x08));
            bytes_055c.modify(|_, writer| writer.init_byte_2().bits(0x08));
            bytes_055c.modify(|_, writer| writer.init_byte_3().bits(0x08));

            let bytes_0558 = btmac.init_bytes_0558();
            bytes_0558.modify(|_, writer| writer.init_byte_0().bits(0x0c));
            bytes_0558.modify(|_, writer| writer.init_byte_1().bits(0x08));
            bytes_0558.modify(|_, writer| writer.init_byte_2().bits(0x0c));
            bytes_0558.modify(|_, writer| writer.init_byte_3().bits(0x0c));

            btmac
                .init_high_half_0458()
                .modify(|_, writer| writer.init_high_half().bits(0x000f));
            btmac
                .init_low_5_054c()
                .modify(|_, writer| writer.init_low_5().bits(0x12));
            bluetooth
                .ble_phy_init_phase
                .init_phase()
                .write_with_zero(|writer| writer.image().bits(0x40));

            if inputs.option_byte_0x55_nonzero {
                btmac
                    .init_branch_control_0470()
                    .modify(|_, writer| writer.init_enable_18().set_bit());
            }

            bluetooth
                .ble_hw_runtime_control
                .phy_init_configuration()
                .write_with_zero(|writer| {
                    writer
                        .value_low_8()
                        .bits(inputs.option_byte_0x59)
                        .config_8()
                        .set_bit()
                });
            bluetooth
                .ble_hw_runtime_control
                .phy_init_configuration_latch()
                .write_with_zero(|writer| writer.image().bits(1));
            btmac.init_control_00b4().modify(|_, writer| {
                writer
                    .init_set_11()
                    .set_bit()
                    .init_set_15()
                    .set_bit()
                    .init_set_20()
                    .set_bit()
                    .init_set_24()
                    .set_bit()
            });
            btmac
                .init_control_00c4()
                .modify(|_, writer| writer.init_enable_9().set_bit());

            let controller = &bluetooth.bluetooth_controller_core;
            controller
                .phy_init_zero_0244()
                .write_with_zero(|writer| writer.image().bits(0));
            controller
                .phy_init_value_01f0()
                .write_with_zero(|writer| writer.image().bits(0x55));
            controller
                .phy_init_value_0248()
                .write_with_zero(|writer| writer.image().bits(0x0000_0fff));
            controller
                .phy_init_dynamic_image_024c()
                .write_with_zero(|writer| {
                    writer.image().bits(environment + ENVIRONMENT_LAST_OFFSET)
                });

            // One ordering boundary is a reviewed Rust-side addition. It does not
            // replace, merge, or reorder any vendor MMIO edge above.
            device_fence();
        }
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
