//! Isolated-image bridge for compiled Bluetooth hardware probes.
//!
//! This module is absent from ordinary builds. It constructs the finite PAC
//! interrupt owner inside one isolated image and immediately executes the
//! same restricted transaction used by production; no writable owner escapes
//! the Bluetooth hardware boundary.

#![forbid(unsafe_code)]

/// Execute the exact production interrupt sample/acknowledgement transaction.
#[inline(always)]
pub fn capture_and_acknowledge_interrupts() -> [u32; 2] {
    let mut registers = open_esp_radio_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let observation = registers.capture_and_acknowledge();
    [observation.bank_0_bits(), observation.bank_1_bits()]
}

/// Execute the exact production scheduler-table initialization transaction.
#[inline(always)]
pub fn initialize_scheduler_table() {
    let cold = open_esp_radio_esp32s31_pac::RadioHardware::for_validation().into_bluetooth();
    let (mut task, interrupts) = cold.separate_interrupt_owner();
    task.initialize_scheduler_table();
    let _cold = task.into_cold(interrupts);
}

/// Execute the exact finite MMIO transaction recovered for
/// `bt_bb_v2_init_cmplx(1)` with the reviewed linked `phy_param` byte.
///
/// This validation-only bridge cannot bypass the production Bluetooth
/// lifecycle because it is absent from ordinary builds. The eventual public
/// edge must consume [`crate::BluetoothPhyInitialized`] after common-PHY
/// calibration; no pre-PHY transition is exposed here.
#[inline(always)]
pub fn initialize_baseband_v2(gain_parameter: u8) {
    open_esp_radio_esp32s31_pac::validation::initialize_bluetooth_baseband_v2(gain_parameter);
}
