//! Isolated-image bridge for compiled Bluetooth hardware probes.
//!
//! This module is absent from ordinary builds. It constructs the finite PAC
//! interrupt owner inside one isolated image and immediately executes the
//! same restricted transaction used by production; no writable owner escapes
//! the Bluetooth hardware boundary.

#![deny(unsafe_code)]

pub use open_esp_radio_esp32s31_pac::{
    BluetoothControllerHalInitConfig, BluetoothControllerSramAddress,
    BluetoothControllerSramAddressError, BluetoothHalInitPeriod, BluetoothHalInitScale,
    BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
};

/// Execute the exact production NRT raw-snapshot acknowledgement transaction.
#[inline(always)]
pub fn capture_and_acknowledge_interrupts() -> [u32; 2] {
    let mut registers = open_esp_radio_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let observation = registers.capture_nrt_and_acknowledge();
    [observation.bank_0_bits(), observation.bank_1_bits()]
}

/// Execute the exact primary BT MAC masked-status acknowledgement prefix.
#[inline(always)]
pub fn capture_primary_and_acknowledge_interrupts() -> [u32; 2] {
    let mut registers = open_esp_radio_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let observation = registers.capture_primary_and_acknowledge().observation();
    [observation.bank_0_bits(), observation.bank_1_bits()]
}

/// Execute the reviewed primary baseline clear/enable/output preparation.
#[inline(always)]
pub fn prepare_primary_interrupt_output() {
    let bluetooth = open_esp_radio_esp32s31_pac::RadioHardware::for_validation().into_bluetooth();
    let (task, interrupts) = bluetooth.separate_interrupt_owner();
    let prepared = interrupts.prepare_controller_output();
    let _powered_owners = (task, prepared);
}

/// Execute the exact production scheduler-table low-bit clear transaction.
#[inline(always)]
pub fn clear_scheduler_table_low_bits() {
    let resources = crate::BluetoothPhysicalResources::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (mut task, interrupts) = resources.separate_interrupt_owner();
    task.controller_hal().clear_scheduler_table_low_bits();
    // The comparison image deliberately retains the mutated partitions; it
    // must not reconstruct cold ownership without the missing rollback.
    let _powered_owners = (task, interrupts);
}

/// Execute the exact finite MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` with the reviewed linked `phy_param` byte.
///
/// This validation-only bridge cannot bypass the production Bluetooth
/// lifecycle in an ordinary build because it is absent there. The production
/// edge consumes `BluetoothPhyInitialized` after common-PHY
/// calibration and projects this byte from the owned PHY state; no pre-PHY
/// production transition is exposed.
///
/// # Safety
///
/// The caller must be an isolated compiled-production probe that models a
/// completed common-PHY transition. It must not use another radio owner after
/// this call because the powered owners remain retained until verified
/// teardown exists.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the PAC common-PHY prerequisite"
)]
#[inline(always)]
pub unsafe fn initialize_baseband_v2(gain_parameter: u8) {
    unsafe {
        open_esp_radio_esp32s31_pac::validation::initialize_bluetooth_baseband_v2(gain_parameter);
    }
}

/// Execute the complete production BTDM controller HAL-init component with
/// the exact standalone profile recovered from the pinned controller caller.
///
/// # Safety
///
/// The caller must be an isolated compiled-production probe modeling enabled
/// clocks, common PHY, BTBB, quiescent controller queues and an inactive IRQ
/// bank. The powered owners remain retained after return.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the complete HAL-init prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_controller_hal_reviewed_standalone() {
    let resources = crate::BluetoothPhysicalResources::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (mut task, interrupts) = resources.separate_interrupt_owner();
    unsafe {
        task.initialize_controller_hal(BluetoothControllerHalInitConfig::reviewed_standalone());
    }
    let _powered_owners = (task, interrupts);
}

/// Execute one exact production memory-list pointer publication in an
/// isolated comparison image.
///
/// # Safety
///
/// The caller must satisfy the controller lifecycle, serialization, backing
/// storage, initialization, and lifetime prerequisites documented by the PAC
/// validation bridge. No later radio operation may run in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves controller-list ownership prerequisites"
)]
#[inline(always)]
pub unsafe fn program_memory_list_pointer(
    selector: BluetoothMemoryListSelector,
    slot: BluetoothMemoryListSlot,
    image: BluetoothMemoryListPointerImage,
) {
    unsafe {
        open_esp_radio_esp32s31_pac::validation::program_bluetooth_memory_list_pointer(
            selector, slot, image,
        );
    }
}

/// Execute the complete recovered BLE PHY register-init body in an isolated
/// compiled-production comparison image.
///
/// `false` means one supplied address did not satisfy its observed encoding
/// contract and no MMIO was performed.
///
/// # Safety
///
/// The caller must satisfy the complete lifecycle and backing-storage
/// prerequisites documented by the PAC validation bridge. After a successful
/// call it must not perform another radio operation in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the complete BLE PHY lifecycle prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_phy_registers(
    private_configuration_byte_0x10: u8,
    environment_address: u32,
    resolving_list_address: u32,
    option_byte_0x55_nonzero: bool,
    option_byte_0x59: u8,
) -> bool {
    let Ok(resolving_list) = BluetoothControllerSramAddress::new(resolving_list_address) else {
        return false;
    };
    unsafe {
        open_esp_radio_esp32s31_pac::validation::initialize_bluetooth_phy_registers(
            private_configuration_byte_0x10,
            environment_address,
            resolving_list,
            option_byte_0x55_nonzero,
            option_byte_0x59,
        )
    }
}
