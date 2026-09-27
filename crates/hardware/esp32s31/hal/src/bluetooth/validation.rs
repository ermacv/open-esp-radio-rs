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

/// Take the diagnostic scheduler-BUSY sample that opens the production
/// scheduler stop sequence.
#[inline(always)]
pub fn sample_scheduler_stop_busy() -> bool {
    oer_esp32s31_pac::validation::sample_bluetooth_scheduler_stop_busy()
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
    configuration_word_40: u32,
) -> bool {
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    unsafe {
        oer_esp32s31_pac::validation::initialize_bluetooth_phy_registers(
            private_timing_source_byte,
            environment_address,
            resolving_list,
            set_branch_control_0470_bit_18,
            configuration_word_40,
        )
    }
}

/// Run the production Bluetooth low-power timer clock selection once, on
/// isolated validation owners of the shared radio.
///
/// Returns whether the lease accepted the selection.
#[inline(always)]
pub fn select_low_power_clock() -> bool {
    let (shared, partitions) = crate::root::RadioHardware::for_validation().into_concurrent(());
    let (controller, timer, interrupts) = partitions.bluetooth.into_parts();
    let task = oer_esp32s31_pac::BluetoothTaskRegisters::new(controller);
    let accepted = match shared.try_acquire() {
        Ok(mut lease) => lease.select_bluetooth_low_power_clock(&task).is_ok(),
        Err(_) => false,
    };
    // The comparison image retains every partition it mutated.
    let _retained = (shared, task, timer, interrupts);
    accepted
}

/// Run the production Bluetooth low-power timer clock deselection
/// transaction once, on isolated validation owners of the shared radio.
#[inline(always)]
pub fn deselect_low_power_clock() {
    let (shared, _partitions) = crate::root::RadioHardware::for_validation().into_concurrent(());
    if let Ok(mut lease) = shared.try_acquire() {
        crate::shared_radio::deselect_bluetooth_low_power_clock(
            lease.registers_mut().radio_phy_mut(),
        );
    }
}

/// Run the production execution modify of hardware list `index` until it
/// leaves `Pending`, stepping the finite machine on isolated validation
/// owners.
///
/// Returns `None` for an index outside `0..16`, otherwise zero when command
/// one became ready and one when hardware rejected the request. START stays
/// published, as the vendor body leaves it to its caller.
///
/// # Safety
///
/// The caller must model the list ownership the production admission
/// requires and perform no later radio operation in this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API forwards the execution-modify admission contract"
)]
#[inline(always)]
pub unsafe fn run_scheduler_execution_modify(index: u8, list_deletion: bool) -> Option<u32> {
    use oer_esp32s31_pac::BluetoothSchedulerExecutionModifyDisposition as Disposition;
    let index = oer_esp32s31_pac::BluetoothSchedulerHardwareListIndex::new(index)?;
    let (_shared, partitions) = crate::root::RadioHardware::for_validation().into_concurrent(());
    let (controller, timer, bank) = partitions.bluetooth.into_parts();
    let mut task = oer_esp32s31_pac::BluetoothTaskRegisters::new(controller);
    let mut interrupts = oer_esp32s31_pac::validation::bluetooth_interrupt_registers();
    let mut modify = super::BluetoothSchedulerExecutionModify::new(index, list_deletion);
    let result = loop {
        match super::scheduler_execution_modify::step_hardware(
            &mut task,
            &mut interrupts,
            &mut modify,
        ) {
            Disposition::Pending => {}
            Disposition::Ready => break 0,
            Disposition::HardwareRejected => break 1,
        }
    };
    // The comparison image retains every partition it mutated.
    let _retained = (task, interrupts, timer, bank, modify);
    Some(result)
}

/// Publish the production execution lock of `address` on hardware list
/// `index` and observe it until it leaves `Pending`, on isolated validation
/// owners.
///
/// Returns `None` for an index outside `0..16` or an address outside the
/// controller SRAM window, otherwise the disposition: zero retained, one
/// current-head reconciliation, two an unsupported hardware result.
///
/// # Safety
///
/// The caller must model the merge-selected item and list serialization the
/// production publication requires and perform no later radio operation in
/// this image.
#[allow(
    unsafe_code,
    reason = "the validation-only API forwards the execution-lock publication contract"
)]
#[inline(always)]
pub unsafe fn run_scheduler_execution_lock(address: u32, index: u8) -> Option<u32> {
    use oer_esp32s31_pac::BluetoothSchedulerExecutionLockDisposition as Disposition;
    let index = oer_esp32s31_pac::BluetoothSchedulerHardwareListIndex::new(index)?;
    let address = BluetoothControllerSramAddress::new(address).ok()?;
    let (_shared, partitions) = crate::root::RadioHardware::for_validation().into_concurrent(());
    let (controller, timer, bank) = partitions.bluetooth.into_parts();
    let mut task = oer_esp32s31_pac::BluetoothTaskRegisters::new(controller);
    let mut interrupts = oer_esp32s31_pac::validation::bluetooth_interrupt_registers();
    // SAFETY: forwarded unchanged from this function's `# Safety` contract.
    let published = unsafe {
        task.publish_scheduler_execution_lock(
            oer_esp32s31_pac::BluetoothSchedulerExecutionLockRequest::new(address, index),
        )
    };
    let result = loop {
        let busy = interrupts.capture_scheduler_busy();
        match task.observe_scheduler_execution_lock(busy) {
            Disposition::Pending => {}
            Disposition::ExecutionLockRetained => break 0,
            Disposition::ReconcileCurrentHead => break 1,
            Disposition::UnsupportedHardwareResult => break 2,
        }
    };
    let _retained = (task, interrupts, timer, bank, published);
    Some(result)
}
