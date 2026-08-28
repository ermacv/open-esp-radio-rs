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

/// Execute the exact production NRT acknowledgement transaction.
#[inline(always)]
pub fn capture_and_acknowledge_interrupts() {
    let mut registers = open_esp_radio_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let _acknowledged = registers.capture_nrt_and_acknowledge();
}

/// Execute the exact production scheduler hardware-list head clear transaction.
#[inline(always)]
pub fn clear_scheduler_hardware_list_heads() {
    let cold = open_esp_radio_esp32s31_hal::BluetoothColdOwner::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (mut task, interrupts) = crate::resources::separate_interrupt_owner(cold);
    task.clear_scheduler_hardware_list_heads();
    // The comparison image deliberately retains the mutated partitions; it
    // must not reconstruct cold ownership without the missing rollback.
    let _powered_owners = (task, interrupts);
}

/// Execute the production PHY-I2C host selection through the standalone
/// Bluetooth route's shared-PHY owner.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub fn configure_and_select_phy_i2c_host(block: u8) -> u32 {
    let cold = open_esp_radio_esp32s31_hal::BluetoothColdOwner::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (mut task, interrupts) = crate::resources::separate_interrupt_owner(cold);
    let host = {
        let mut shared_phy = task.shared_phy_hal();
        open_esp_radio_esp32s31_phy::validation::configure_and_select_phy_i2c_host(
            &mut shared_phy,
            block,
        )
    };
    // Host selection mutates shared PHY state. The isolated comparison image
    // retains both partitions and must not advertise a reusable cold owner.
    let _powered_owners = (task, interrupts);
    host
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
/// clocks, the selected controller-SRAM prefix and an inactive IRQ bank. The
/// powered owners remain retained after return; scheduler, PHY, BTBB,
/// Link-Layer and HCI readiness remain unclaimed.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the complete HAL-init prerequisites"
)]
#[inline(always)]
pub unsafe fn initialize_controller_hal_reviewed_standalone() {
    let cold = open_esp_radio_esp32s31_hal::BluetoothColdOwner::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (mut task, interrupts) = crate::resources::separate_interrupt_owner(cold);
    unsafe {
        task.initialize_controller_hal(BluetoothControllerHalInitConfig::reviewed_standalone());
    }
    let _powered_owners = (task, interrupts);
}

/// Apply the exact modem low-power timer register prefix before source 127.
///
/// This isolated path compiles the shipping opaque HAL transition while
/// retaining both terminal owners. It does not publish ISR storage, allocate
/// the CPU route, or claim that the controller software environment exists.
///
/// # Safety
///
/// The caller must model enabled controller/timer clocks and completed task,
/// HAL, scheduler/event-list and HCI software initialization. No later radio
/// operation may execute in the comparison image.
#[allow(
    unsafe_code,
    reason = "the isolated validation image assumes the missing software-stage prerequisite"
)]
#[inline(always)]
pub unsafe fn prepare_modem_lp_timer_registers() {
    let cold = open_esp_radio_esp32s31_hal::BluetoothColdOwner::from_radio_hardware(
        open_esp_radio_esp32s31_pac::RadioHardware::for_validation(),
    );
    let (task, interrupts) = cold.separate_interrupt_owner();
    let prepared = unsafe { task.prepare_modem_lp_timer_registers() };
    let _terminal_owners = (prepared, interrupts);
}

/// Publish the scheduler-disable command and perform one bounded BUSY sample
/// through the exact production PAC transaction.
///
/// # Safety
///
/// The caller must be an isolated compiled-production probe modeling a
/// powered task-stopping controller and a post-route observation point.
#[allow(
    unsafe_code,
    reason = "the validation-only API preserves the powered scheduler prerequisite"
)]
#[inline(always)]
pub unsafe fn disable_scheduler_and_sample_once() -> bool {
    unsafe {
        open_esp_radio_esp32s31_pac::validation::disable_bluetooth_scheduler_and_sample_once()
    }
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
