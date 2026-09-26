use super::{
    BluetoothTaskRegisters, Ieee802154TaskParts, Ieee802154TaskRegisters, MacInterruptMask,
    RadioPartitions, SharedRadioParts, SharedRadioRegisters, WifiRadioRegisters,
};

fn wifi_registers(partitions: RadioPartitions) -> (WifiRadioRegisters, super::MacInterruptSetup) {
    let RadioPartitions {
        wifi_mac,
        wifi_interrupts,
        ..
    } = partitions;
    (WifiRadioRegisters::new(wifi_mac), wifi_interrupts)
}

fn shared_registers(partitions: RadioPartitions) -> SharedRadioRegisters {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = partitions;
    SharedRadioRegisters::new(SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    })
}

#[test]
fn wifi_register_set_returns_its_mac_partition() {
    let (registers, _interrupts) = wifi_registers(RadioPartitions::for_validation());
    let _partition = registers.into_partition();
}

#[test]
fn bluetooth_register_set_returns_its_controller_partition() {
    let RadioPartitions { bluetooth, .. } = RadioPartitions::for_validation();
    let registers = BluetoothTaskRegisters::new(bluetooth);
    let _partition = registers.into_partition();
}

#[test]
fn shared_radio_owner_returns_every_shared_partition() {
    let shared = shared_registers(RadioPartitions::for_validation());
    let SharedRadioParts { .. } = shared.into_parts();
}

#[test]
fn ieee802154_register_set_reunites_its_interrupt_owner() {
    let partitions = RadioPartitions::for_validation();
    let RadioPartitions {
        ieee802154,
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = partitions;
    let (task, interrupts) = Ieee802154TaskRegisters::new(Ieee802154TaskParts {
        ieee802154,
        shared: SharedRadioRegisters::new(SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        }),
    });
    let Ieee802154TaskParts {
        ieee802154, shared, ..
    } = task.into_parts(interrupts);
    let _ = (ieee802154, shared.into_parts());
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
