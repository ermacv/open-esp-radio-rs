use super::{
    BluetoothTaskParts, BluetoothTaskRegisters, Ieee802154TaskParts, Ieee802154TaskRegisters,
    MacInterruptMask, RadioPartitions, WifiRadioParts, WifiRadioRegisters,
};

fn wifi_registers(partitions: RadioPartitions) -> (WifiRadioRegisters, super::MacInterruptSetup) {
    let RadioPartitions {
        wifi_mac,
        wifi_interrupts,
        radio_phy,
        coexistence,
        shared_radio,
        ieee802154,
        ..
    } = partitions;
    (
        WifiRadioRegisters::new(WifiRadioParts {
            wifi_mac,
            ieee802154,
            radio_phy,
            coexistence,
            shared_radio,
        }),
        wifi_interrupts,
    )
}

#[test]
fn wifi_register_set_returns_every_consumed_partition() {
    let (registers, _interrupts) = wifi_registers(RadioPartitions::for_validation());
    let WifiRadioParts { .. } = registers.into_parts();
}

#[test]
fn bluetooth_register_set_returns_every_consumed_partition() {
    let RadioPartitions {
        bluetooth,
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_validation();
    let registers = BluetoothTaskRegisters::new(BluetoothTaskParts {
        bluetooth,
        radio_phy,
        coexistence,
        shared_radio,
    });
    assert!(!registers.controller_time_latch_in_flight());
    let _parts = registers.into_parts();
}

#[test]
fn ieee802154_register_set_reunites_its_interrupt_owner() {
    let RadioPartitions {
        ieee802154,
        radio_phy,
        coexistence,
        bluetooth,
        shared_radio,
        ..
    } = RadioPartitions::for_validation();
    let (task, interrupts) = Ieee802154TaskRegisters::new(Ieee802154TaskParts {
        ieee802154,
        radio_phy,
        coexistence,
        bluetooth,
        shared_radio,
    });
    let Ieee802154TaskParts { ieee802154, .. } = task.into_parts(interrupts);
    let _ = ieee802154;
}

#[test]
fn mac_hal_tail_rejects_out_of_range_calibration_before_mmio() {
    let (mut registers, mut interrupts) = wifi_registers(RadioPartitions::for_validation());
    assert!(!registers.initialize_mac_hal_tail(
        &mut interrupts,
        MacInterruptMask::COLD_RX,
        0x0004_0000
    ));
    assert!(!registers.initialize_mac_hal_tail(&mut interrupts, MacInterruptMask::NONE, u32::MAX));
}

#[test]
fn mac_txrx_callbacks_reject_out_of_range_slot_before_mmio() {
    let (mut registers, _interrupts) = wifi_registers(RadioPartitions::for_validation());
    assert!(!registers.initialize_mac_txrx_callbacks(11));
    assert!(!registers.initialize_mac_txrx_callbacks(u8::MAX));
}
