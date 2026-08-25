//! Capability acquisition for isolated compiled probe images.
//!
//! Vendor ABI adaptation and named hardware operations belong to the HAL.
//! This module only constructs the same finite PAC owners used by production
//! code; it does not implement semantic operations or comparison verdicts.

#![deny(unsafe_code)]

use crate::{
    BluetoothControllerHalInitConfig, BluetoothInterruptRegisters, BluetoothMemoryListPointerImage,
    BluetoothMemoryListSelector, BluetoothMemoryListSlot, MacInterruptRegisters, MacInterruptSetup,
    MacPowerInterruptRegisters, RadioHardware, svd,
};

use crate::bluetooth_phy_init::{BluetoothPhyEnvironmentAddress, BluetoothPhyRegisterInitInputs};

#[inline(always)]
fn wifi_interrupts() -> svd::peripheral_ownership::WifiInterruptPeripherals {
    let RadioHardware {
        wifi_interrupts, ..
    } = RadioHardware::for_validation();
    wifi_interrupts
}

/// Construct the ordinary task-owned register partition for one probe image.
#[inline(always)]
pub fn wifi_radio_registers() -> crate::WifiRadioRegisters {
    RadioHardware::for_validation().into_wifi().into_running().0
}

/// Construct the task-side interrupt setup owner used by the production
/// connected-STA preparation transaction.
#[inline(always)]
pub fn mac_interrupt_setup() -> MacInterruptSetup {
    RadioHardware::for_validation().into_wifi().into_running().1
}

/// Construct the disjoint hard-MAC interrupt capability for one probe image.
#[inline(always)]
pub fn mac_interrupt_registers() -> MacInterruptRegisters {
    let interrupts = wifi_interrupts();
    MacInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_interrupt)
}

/// Construct the disjoint power-interrupt capability for one probe image.
#[inline(always)]
pub fn mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    let interrupts = wifi_interrupts();
    MacPowerInterruptRegisters::from_peripheral_for_validation(interrupts.wifi_mac_power_interrupt)
}

/// Construct the active Bluetooth interrupt-bank capability for one isolated
/// compiled probe image.
///
/// Ordinary production code cannot call this constructor: the validation
/// module is absent unless `validation-probes` is enabled. Production can
/// reach the same register owner only by consuming inactive ownership through
/// baseline prepare and explicit shared-ISR staging; this isolated probe
/// deliberately bypasses that lifecycle to compare one bounded transaction.
#[inline(always)]
pub fn bluetooth_interrupt_registers() -> BluetoothInterruptRegisters {
    let RadioHardware {
        bluetooth_interrupts,
        ..
    } = RadioHardware::for_validation();
    BluetoothInterruptRegisters {
        peripherals: bluetooth_interrupts,
    }
}

/// Execute the exact bounded MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` inside one isolated comparison image.
///
/// The bridge calls the same hidden PAC SPI used by the production
/// post-common-PHY typestate. It bypasses lifecycle ownership only inside this
/// isolated validation image, so the comparison exercises the shipping MMIO
/// implementation rather than a shadow model.
///
/// # Safety
///
/// The caller must provide an isolated validation image whose modeled state
/// satisfies the common-PHY prerequisite of
/// `BluetoothTaskRegisters::initialize_baseband_v2_arg_one`. No other radio
/// owner may be used after this function returns: the powered task and
/// interrupt partitions are deliberately retained because verified Bluetooth
/// teardown is not implemented yet.
#[allow(
    unsafe_code,
    reason = "the validation bridge explicitly exposes the modeled common-PHY prerequisite"
)]
#[inline(always)]
pub unsafe fn initialize_bluetooth_baseband_v2(gain_parameter: u8) {
    let cold = RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    unsafe {
        task.initialize_baseband_v2_arg_one(gain_parameter);
    }
    let _powered_owners = (task, interrupts);
}

/// Execute the complete recovered BTDM controller HAL-init body inside one
/// isolated comparison image.
///
/// This calls the same hidden PAC transaction used by a future production
/// lifecycle and retains both Bluetooth owners. It neither configures a CPU
/// interrupt route nor claims controller, Link-Layer or HCI readiness.
///
/// # Safety
///
/// The caller must model enabled Bluetooth clocks, completed common PHY and
/// BTBB initialization, quiescent controller software queues and an inactive
/// retained interrupt bank. No other radio owner may be used afterwards.
#[allow(
    unsafe_code,
    reason = "the validation bridge preserves the complete HAL-init lifecycle prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_bluetooth_controller_hal(config: BluetoothControllerHalInitConfig) {
    let cold = RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    unsafe {
        task.initialize_controller_hal(config);
    }
    let _powered_owners = (task, interrupts);
}

/// Execute one exact controller memory-list pointer publication inside an
/// isolated comparison image.
///
/// The bridge calls the shipping hidden PAC SPI and retains both Bluetooth
/// owners locally. It does not manufacture an active lifecycle state or
/// reconstruct cold ownership after the transaction.
///
/// # Safety
///
/// The caller must provide an isolated validation image whose modeled state
/// permits the selected controller list to be changed. For an address image,
/// the caller must also model correctly initialized, exclusively serialized
/// storage that remains alive for the rest of the image execution.
#[allow(
    unsafe_code,
    reason = "the validation bridge preserves controller-lifecycle and pointed-storage prerequisites"
)]
#[inline(always)]
pub unsafe fn program_bluetooth_memory_list_pointer(
    selector: BluetoothMemoryListSelector,
    slot: BluetoothMemoryListSlot,
    image: BluetoothMemoryListPointerImage,
) {
    let cold = RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    unsafe {
        task.program_memory_list_pointer(selector, slot, image);
    }
    let _powered_owners = (task, interrupts);
}

/// Execute the complete recovered BLE PHY register-init body inside one
/// isolated comparison image.
///
/// This bridge projects external vendor state into explicit primitive inputs,
/// validates the two address images, invokes the hidden shipping PAC
/// transaction, and retains both powered owners. It does not create a normal
/// production lifecycle edge.
///
/// # Safety
///
/// The caller must model the complete common-PHY, baseband, coexistence,
/// controller-stack, callback-registration, scheduler-registration, inactive
/// IRQ, backing-storage, serialization, and lifetime prerequisites documented
/// by `BluetoothTaskRegisters::initialize_ble_phy_registers`. No other radio
/// owner may be used after a successful call.
#[allow(
    unsafe_code,
    reason = "the validation bridge preserves the complete recovered BLE PHY lifecycle prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_bluetooth_phy_registers(
    private_configuration_byte_0x10: u8,
    environment_address: u32,
    resolving_list: crate::BluetoothControllerSramAddress,
    option_byte_0x55_nonzero: bool,
    option_byte_0x59: u8,
) -> bool {
    let Ok(environment) = BluetoothPhyEnvironmentAddress::new(environment_address) else {
        return false;
    };
    let inputs = BluetoothPhyRegisterInitInputs::new(
        private_configuration_byte_0x10,
        environment,
        resolving_list,
        option_byte_0x55_nonzero,
        option_byte_0x59,
    );
    let cold = RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    unsafe {
        task.initialize_ble_phy_registers(inputs);
    }
    let _powered_owners = (task, interrupts);
    true
}
