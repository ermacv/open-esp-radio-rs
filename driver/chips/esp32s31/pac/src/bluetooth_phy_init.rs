//! Exact bounded BLE PHY register-initialization transaction.
//!
//! The recovered vendor lifecycle performs this transaction only after common
//! PHY, Bluetooth baseband, coexistence and controller software initialization
//! have completed. Possessing [`BluetoothTaskRegisters`] alone proves none of
//! those prerequisites, so the operation remains unsafe and is reached by the
//! affine controller lifecycle through the HAL.

#![deny(unsafe_code)]

use super::{BluetoothControllerSramAddress, BluetoothTaskRegisters, device_fence};

const ENVIRONMENT_LAST_OFFSET: u32 = 0x40;

trait BleBaseStackOnTaskEnableHardwareTransaction {
    fn enable_access_address_low_correlation(&mut self);
    fn initialize_ble_phy_registers(&mut self);
}

fn execute_base_stack_on_task_enable_hardware(
    transaction: &mut impl BleBaseStackOnTaskEnableHardwareTransaction,
) {
    transaction.enable_access_address_low_correlation();
    transaction.initialize_ble_phy_registers();
}

struct HardwareBleBaseStackOnTaskEnableTransaction<'registers> {
    registers: &'registers mut BluetoothTaskRegisters,
    inputs: BluetoothPhyRegisterInitInputs,
}

impl BleBaseStackOnTaskEnableHardwareTransaction
    for HardwareBleBaseStackOnTaskEnableTransaction<'_>
{
    fn enable_access_address_low_correlation(&mut self) {
        super::generated::enable_ble_phy_access_address_low_correlation(
            &self.registers.bluetooth.bt_v3_2_baseband,
        );
    }

    #[allow(
        unsafe_code,
        reason = "the composed transaction retains the exact PHY leaf prerequisites"
    )]
    fn initialize_ble_phy_registers(&mut self) {
        // SAFETY: the enclosing composed transaction carries the same
        // lifecycle and pointed-storage prerequisites as the exact PHY leaf.
        unsafe {
            self.registers.initialize_ble_phy_registers(self.inputs);
        }
    }
}

/// Typed base of the linked BLE PHY environment.
///
/// This proves only word alignment and representability of the last published
/// member. Allocation, contents, lifetime and hardware ownership belong to the
/// controller-memory and lifecycle layers above the PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyEnvironmentAddress(u32);

/// Why a BLE PHY environment address cannot be represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPhyEnvironmentAddressError {
    /// The recovered transaction compresses one member as a word address.
    Unaligned,
    /// The final published member would overflow the address space.
    ExtentOverflow,
}

impl BluetoothPhyEnvironmentAddress {
    /// Validate the positional address-image requirements without dereference.
    pub const fn new(address: u32) -> Result<Self, BluetoothPhyEnvironmentAddressError> {
        if !address.is_multiple_of(4) {
            return Err(BluetoothPhyEnvironmentAddressError::Unaligned);
        }
        if address.checked_add(ENVIRONMENT_LAST_OFFSET).is_none() {
            return Err(BluetoothPhyEnvironmentAddressError::ExtentOverflow);
        }
        Ok(Self(address))
    }

    /// Return the validated CPU address without granting dereference.
    pub const fn address(self) -> u32 {
        self.0
    }

    const fn compressed_member(self, offset: u32) -> u32 {
        ((self.0 + offset) >> 2) & 0x000f_ffff
    }
}

/// Complete external inputs read by the recovered BLE PHY init body.
///
/// The timing source remains private Controller policy. Values whose hardware
/// meaning is not proven retain positional configuration names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothPhyRegisterInitInputs {
    private_timing_source_byte: u8,
    environment: BluetoothPhyEnvironmentAddress,
    resolving_list: BluetoothControllerSramAddress,
    set_branch_control_0470_bit_18: bool,
    runtime_configuration_low_byte: u8,
}

impl BluetoothPhyRegisterInitInputs {
    /// Capture the complete external values consumed by the finite MMIO body.
    ///
    /// Both values are typed address images, but this value does not prove
    /// their allocation, contents, lifetime, or exclusive ownership.
    pub const fn new(
        private_timing_source_byte: u8,
        environment: BluetoothPhyEnvironmentAddress,
        resolving_list: BluetoothControllerSramAddress,
        set_branch_control_0470_bit_18: bool,
        runtime_configuration_low_byte: u8,
    ) -> Self {
        Self {
            private_timing_source_byte,
            environment,
            resolving_list,
            set_branch_control_0470_bit_18,
            runtime_configuration_low_byte,
        }
    }
}

impl BluetoothTaskRegisters {
    /// Execute the complete hardware component of BLE base-stack task enable.
    ///
    /// The registered external-baseband callback first enables access-address
    /// low-correlation handling through one fresh field RMW. The exact BLE PHY
    /// register-initialization body follows without an intervening MMIO edge.
    /// Source-owned memory tracking replaces the preceding vendor software
    /// publication; the standalone profile does not enter the optional BQB
    /// branch.
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
    /// The caller must prove the complete pre-task-enable lifecycle: common
    /// PHY and Bluetooth baseband are initialized, an exclusive standalone or
    /// shared-radio coexistence policy is retained, source-owned Controller,
    /// HCI and scheduler state exists, the IRQ owner remains inactive, and
    /// both pointed-to SRAM objects remain correctly initialized, exclusively
    /// serialized and live for every hardware consumer. This method does not
    /// establish or verify any of those facts.
    #[allow(
        unsafe_code,
        dead_code,
        reason = "the unsafe signature retains unmodeled lifecycle and pointed-storage prerequisites"
    )]
    pub unsafe fn enable_ble_base_stack_hardware(
        &mut self,
        inputs: BluetoothPhyRegisterInitInputs,
    ) {
        let mut transaction = HardwareBleBaseStackOnTaskEnableTransaction {
            registers: self,
            inputs,
        };
        execute_base_stack_on_task_enable_hardware(&mut transaction);
    }

    /// Execute the exact bounded BLE PHY register-initialization body.
    ///
    /// This leaf starts with the ordered positional-word publications. It does
    /// not perform the preceding base-stack access-address correlation update;
    /// [`Self::enable_ble_base_stack_hardware`] owns that composition.
    ///
    /// # Safety
    ///
    /// The caller must prove completed common PHY and Bluetooth baseband
    /// initialization, the registered external-baseband table used by this
    /// body, inactive IRQ ownership, and live exclusive ownership of both
    /// pointed-to SRAM objects for every hardware consumer. After the first
    /// MMIO write, failure must be treated as fail-stop until a complete
    /// controller teardown is independently recovered and verified.
    #[allow(
        unsafe_code,
        dead_code,
        reason = "the unsafe signature retains the exact PHY leaf lifecycle and storage prerequisites"
    )]
    pub unsafe fn initialize_ble_phy_registers(&mut self, inputs: BluetoothPhyRegisterInitInputs) {
        let timing_byte = inputs.private_timing_source_byte.wrapping_sub(1);
        let environment = inputs.environment.address();
        let environment_member = inputs.environment.compressed_member(0x2c);
        let environment_tail = environment + ENVIRONMENT_LAST_OFFSET;
        let resolving_list = inputs.resolving_list.compressed_image();

        let bluetooth = &self.bluetooth;
        let btmac = &bluetooth.btmac_ble_phy_init;

        // Ordered positional images and BTMAC prefix.
        super::svd::zero_register_write::publish_ble_positional_word_1ff0_image_0(
            &bluetooth.ble_hw_positional_word_1ff0,
        );
        super::svd::fixed_register_image::publish_ble_positional_word_1ff0_image_1(
            &bluetooth.ble_hw_positional_word_1ff0,
        );
        super::svd::sampled_bit_zero_write::preserve_ble_phy_interrupt_source_17(btmac);
        super::svd::fixed_register_image::clear_all_ble_base_stack_interrupt_sources(btmac);
        super::generated::clear_ble_phy_lc_tx_on_delay_fields(btmac);
        super::generated::or_ble_phy_init_tx_on_delay(
            btmac,
            super::generated::BluetoothPhyInitTimingByte::new(u32::from(timing_byte))
                .expect("one byte always fits the reviewed BLE PHY timing domain"),
        );

        // The vendor reaches this leaf through the registered external-BB
        // function table. The restricted transaction preserves its MMIO edge
        // at the same position without claiming a static call-table install.
        super::generated::publish_ble_phy_le_tx_on_delay(
            &bluetooth.bt_v3_2_baseband,
            super::generated::BluetoothPhyInitTimingByte::new(u32::from(
                timing_byte.wrapping_sub(10),
            ))
            .expect("one byte always fits the reviewed BLE PHY timing domain"),
        );

        super::svd::fixed_register_image::publish_ble_phy_init_value_0138(btmac);
        super::svd::fixed_register_image::publish_ble_phy_init_bytes_04a4(btmac);
        super::svd::fixed_register_image::publish_ble_phy_init_bytes_04a8(btmac);
        super::svd::register_image_write::publish_ble_phy_init_environment_member(
            btmac,
            environment_member,
        );
        super::svd::fixed_register_image::publish_ble_phy_init_value_04ac(btmac);
        super::svd::fixed_register_image::publish_ble_phy_init_value_045c(btmac);

        // Four independent fresh-read updates at 0x20101654.
        super::generated::clear_ble_phy_init_low_byte_pair(btmac);
        super::generated::or_ble_phy_init_low_byte_pair(btmac);
        super::generated::clear_ble_phy_init_byte_2_low_7(btmac);
        super::generated::or_ble_phy_init_byte_2(btmac);

        super::svd::zero_register_write::clear_ble_phy_init_zero_0074(btmac);
        super::svd::fixed_register_image::publish_ble_positional_word_891c_image_20(
            &bluetooth.ble_hw_positional_word_891c,
        );
        super::svd::fixed_register_image::publish_ble_phy_accelerator_config(
            &bluetooth.ble_hw_accelerator,
        );
        super::svd::fixed_register_image::publish_ble_phy_accelerator_sram_region_0(
            &bluetooth.ble_hw_accelerator,
        );
        super::svd::fixed_register_image::publish_ble_phy_accelerator_sram_region_1(
            &bluetooth.ble_hw_accelerator,
        );
        super::svd::zero_based_field_write::publish_ble_phy_resolving_list_base_pointer(
            &bluetooth.ble_hw_resolving_list,
            resolving_list,
        );

        super::svd::fixed_register_image::publish_ble_phy_init_control_0400(btmac);
        super::generated::enable_ble_phy_init_control_0400(btmac);
        super::svd::fixed_register_image::publish_ble_phy_init_value_0540(btmac);

        // Each byte replacement is a distinct fresh-read RMW in vendor order.
        super::generated::publish_ble_phy_init_0550_byte_0(btmac);
        super::generated::publish_ble_phy_init_0550_byte_1(btmac);
        super::generated::publish_ble_phy_init_0550_byte_2(btmac);

        super::generated::publish_ble_phy_init_0554_byte_0(btmac);
        super::generated::publish_ble_phy_init_0554_byte_1(btmac);
        super::generated::publish_ble_phy_init_0554_byte_2(btmac);
        super::generated::publish_ble_phy_init_0554_byte_3(btmac);

        super::generated::publish_ble_phy_init_055c_byte_0(btmac);
        super::generated::publish_ble_phy_init_055c_byte_1(btmac);
        super::generated::publish_ble_phy_init_055c_byte_2(btmac);
        super::generated::publish_ble_phy_init_055c_byte_3(btmac);

        super::generated::publish_ble_phy_init_0558_byte_0(btmac);
        super::generated::publish_ble_phy_init_0558_byte_1(btmac);
        super::generated::publish_ble_phy_init_0558_byte_2(btmac);
        super::generated::publish_ble_phy_init_0558_byte_3(btmac);

        super::generated::publish_ble_phy_init_high_half_0458(btmac);
        super::generated::publish_ble_phy_init_low_5_054c(btmac);
        super::svd::fixed_register_image::publish_ble_positional_word_891c_image_40(
            &bluetooth.ble_hw_positional_word_891c,
        );

        if inputs.set_branch_control_0470_bit_18 {
            super::generated::set_ble_phy_init_branch_control_0470_bit_18(btmac);
        }

        super::svd::zero_based_field_write::publish_ble_phy_runtime_configuration(
            &bluetooth.ble_hw_runtime_control,
            inputs.runtime_configuration_low_byte,
            true,
        );
        super::svd::fixed_register_image::publish_ble_phy_init_followup_image_1(
            &bluetooth.ble_hw_runtime_control,
        );
        super::generated::enable_ble_phy_interrupt_sources_11_15_20_24(btmac);
        super::generated::enable_ble_phy_init_control_00c4(btmac);

        let controller = &bluetooth.bluetooth_controller_core;
        super::svd::zero_register_write::clear_ble_phy_controller_value_0244(controller);
        super::svd::fixed_register_image::publish_ble_phy_controller_value_01f0(controller);
        super::svd::fixed_register_image::publish_ble_phy_controller_value_0248(controller);
        super::svd::register_image_write::publish_ble_phy_controller_environment_tail(
            controller,
            environment_tail,
        );

        // One ordering boundary is a reviewed Rust-side addition. It does not
        // replace, merge, or reorder any vendor MMIO edge above.
        device_fence();
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{
        BleBaseStackOnTaskEnableHardwareTransaction, execute_base_stack_on_task_enable_hardware,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        EnableAccessAddressLowCorrelation,
        InitializeBlePhyRegisters,
    }

    #[derive(Default)]
    struct Recorder {
        operations: Vec<Operation>,
    }

    impl BleBaseStackOnTaskEnableHardwareTransaction for Recorder {
        fn enable_access_address_low_correlation(&mut self) {
            self.operations
                .push(Operation::EnableAccessAddressLowCorrelation);
        }

        fn initialize_ble_phy_registers(&mut self) {
            self.operations.push(Operation::InitializeBlePhyRegisters);
        }
    }

    #[test]
    fn base_stack_on_task_enable_orders_baseband_before_phy_initialization() {
        let mut recorder = Recorder::default();

        execute_base_stack_on_task_enable_hardware(&mut recorder);

        assert_eq!(
            recorder.operations,
            [
                Operation::EnableAccessAddressLowCorrelation,
                Operation::InitializeBlePhyRegisters,
            ]
        );
    }
}
