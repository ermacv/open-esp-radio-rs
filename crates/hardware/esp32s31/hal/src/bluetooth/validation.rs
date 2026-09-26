//! Isolated-image bridges from compiled Bluetooth probes to PAC transactions.
//!
//! This module is absent from ordinary builds. Each bridge constructs the
//! finite validation owners inside the PAC and executes the same restricted
//! transaction used by production; no writable owner escapes this boundary.

use oer_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothMemoryListPointerImage, BluetoothMemoryListSelector,
    BluetoothMemoryListSlot,
};

/// Execute the exact production NRT acknowledgement transaction.
#[inline(always)]
pub fn capture_and_acknowledge_interrupts() {
    let mut registers = oer_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let _acknowledged = registers.capture_nrt_and_acknowledge();
}

/// Execute the exact finite MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)`.
///
/// # Safety
///
/// The caller must satisfy the common-PHY prerequisite documented by the PAC
/// validation bridge and perform no later radio operation in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the settled Bluetooth PHY-client prerequisite"
)]
#[inline(always)]
pub unsafe fn initialize_baseband_v2(gain_parameter: u8) {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_pac::validation::initialize_bluetooth_baseband_v2(gain_parameter);
    }
}

/// Execute one exact production memory-list pointer publication.
///
/// # Safety
///
/// The caller must satisfy the controller lifecycle, serialization, backing
/// storage, initialization and lifetime prerequisites documented by the PAC
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
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_pac::validation::program_bluetooth_memory_list_pointer(selector, slot, image);
    }
}

/// Execute the complete recovered BLE PHY register-init body.
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
    private_timing_source_byte: u8,
    environment_address: u32,
    resolving_list: BluetoothControllerSramAddress,
    set_branch_control_0470_bit_18: bool,
    runtime_configuration_low_byte: u8,
) -> bool {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_pac::validation::initialize_bluetooth_phy_registers(
            private_timing_source_byte,
            environment_address,
            resolving_list,
            set_branch_control_0470_bit_18,
            runtime_configuration_low_byte,
        )
    }
}
