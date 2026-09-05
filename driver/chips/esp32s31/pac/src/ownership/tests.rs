use super::{MacInterruptMask, RadioHardware, RadioPhyReleaseError};

fn occupy_wifi_restore(registers: &mut super::WifiColdRegisters) {
    registers
        .registers
        .peripherals
        .radio_phy
        .occupy_txdc_pwdet_restore_for_test();
}

fn occupy_ieee802154_restore(registers: &mut super::Ieee802154ColdRegisters) {
    registers
        .task
        .peripherals
        .radio_phy
        .occupy_txdc_pwdet_restore_for_test();
}

fn occupy_bluetooth_restore(registers: &mut super::BluetoothColdRegisters) {
    registers
        .task
        .radio_phy
        .occupy_txdc_pwdet_restore_for_test();
}

fn occupy_wifi_txiq_restore(registers: &mut super::WifiColdRegisters) {
    registers
        .registers
        .peripherals
        .radio_phy
        .occupy_txiq_tone_control_restore_for_test();
}

fn occupy_ieee802154_txiq_restore(registers: &mut super::Ieee802154ColdRegisters) {
    registers
        .task
        .peripherals
        .radio_phy
        .occupy_txiq_tone_control_restore_for_test();
}

fn occupy_bluetooth_txiq_restore(registers: &mut super::BluetoothColdRegisters) {
    registers
        .task
        .radio_phy
        .occupy_txiq_tone_control_restore_for_test();
}

fn occupy_wifi_rx_dco_restore(registers: &mut super::WifiColdRegisters) {
    registers
        .registers
        .peripherals
        .radio_phy
        .occupy_rx_dco_control_restore_for_test();
}

fn occupy_ieee802154_rx_dco_restore(registers: &mut super::Ieee802154ColdRegisters) {
    registers
        .task
        .peripherals
        .radio_phy
        .occupy_rx_dco_control_restore_for_test();
}

fn occupy_bluetooth_rx_dco_restore(registers: &mut super::BluetoothColdRegisters) {
    registers
        .task
        .radio_phy
        .occupy_rx_dco_control_restore_for_test();
}

fn occupy_wifi_bluetooth_tx_power_restore(registers: &mut super::WifiColdRegisters) {
    registers
        .registers
        .peripherals
        .radio_phy
        .occupy_bluetooth_tx_power_control_restore_for_test();
}

fn occupy_ieee802154_bluetooth_tx_power_restore(registers: &mut super::Ieee802154ColdRegisters) {
    registers
        .task
        .peripherals
        .radio_phy
        .occupy_bluetooth_tx_power_control_restore_for_test();
}

fn occupy_bluetooth_tx_power_restore(registers: &mut super::BluetoothColdRegisters) {
    registers
        .task
        .radio_phy
        .occupy_bluetooth_tx_power_control_restore_for_test();
}

#[test]
fn pending_txdc_restore_survives_same_route_transitions_and_blocks_release() {
    let mut wifi = RadioHardware::for_validation().into_wifi();
    occupy_wifi_restore(&mut wifi);
    let (task, interrupts) = wifi.into_running();
    let wifi = task.into_cold(interrupts);
    let Err(failure) = wifi.release() else {
        panic!("Wi-Fi released a pending restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxDcPwdetRestorePending
    );

    let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
    occupy_ieee802154_restore(&mut ieee802154);
    let (task, interrupts) = ieee802154.separate_interrupt_owner();
    let ieee802154 = task.into_cold(interrupts);
    let Err(failure) = ieee802154.release() else {
        panic!("IEEE 802.15.4 released a pending restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxDcPwdetRestorePending
    );

    let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
    occupy_bluetooth_restore(&mut bluetooth);
    let (task, interrupts) = bluetooth.separate_interrupt_owner();
    let bluetooth = task
        .into_cold(interrupts)
        .expect("an idle Bluetooth task owner can be reunited");
    let Err(failure) = bluetooth.release() else {
        panic!("Bluetooth released a pending restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxDcPwdetRestorePending
    );
}

#[test]
fn pending_txiq_restore_survives_same_route_transitions_and_blocks_release() {
    let mut wifi = RadioHardware::for_validation().into_wifi();
    occupy_wifi_txiq_restore(&mut wifi);
    let (task, interrupts) = wifi.into_running();
    let wifi = task.into_cold(interrupts);
    let Err(failure) = wifi.release() else {
        panic!("Wi-Fi released a pending TX-IQ restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxIqToneControlRestorePending
    );

    let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
    occupy_ieee802154_txiq_restore(&mut ieee802154);
    let (task, interrupts) = ieee802154.separate_interrupt_owner();
    let ieee802154 = task.into_cold(interrupts);
    let Err(failure) = ieee802154.release() else {
        panic!("IEEE 802.15.4 released a pending TX-IQ restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxIqToneControlRestorePending
    );

    let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
    occupy_bluetooth_txiq_restore(&mut bluetooth);
    let (task, interrupts) = bluetooth.separate_interrupt_owner();
    let bluetooth = task
        .into_cold(interrupts)
        .expect("an idle Bluetooth task owner can be reunited");
    let Err(failure) = bluetooth.release() else {
        panic!("Bluetooth released a pending TX-IQ restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::TxIqToneControlRestorePending
    );
}

#[test]
fn pending_rx_dco_restore_survives_same_route_transitions_and_blocks_release() {
    let mut wifi = RadioHardware::for_validation().into_wifi();
    occupy_wifi_rx_dco_restore(&mut wifi);
    let (task, interrupts) = wifi.into_running();
    let wifi = task.into_cold(interrupts);
    let Err(failure) = wifi.release() else {
        panic!("Wi-Fi released a pending RX-DCO restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::RxDcoControlRestorePending
    );

    let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
    occupy_ieee802154_rx_dco_restore(&mut ieee802154);
    let (task, interrupts) = ieee802154.separate_interrupt_owner();
    let ieee802154 = task.into_cold(interrupts);
    let Err(failure) = ieee802154.release() else {
        panic!("IEEE 802.15.4 released a pending RX-DCO restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::RxDcoControlRestorePending
    );

    let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
    occupy_bluetooth_rx_dco_restore(&mut bluetooth);
    let (task, interrupts) = bluetooth.separate_interrupt_owner();
    let bluetooth = task
        .into_cold(interrupts)
        .expect("an idle Bluetooth task owner can be reunited");
    let Err(failure) = bluetooth.release() else {
        panic!("Bluetooth released a pending RX-DCO restore");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::RxDcoControlRestorePending
    );
}

#[test]
fn pending_bluetooth_tx_power_restore_survives_route_transitions_and_blocks_release() {
    let mut wifi = RadioHardware::for_validation().into_wifi();
    occupy_wifi_bluetooth_tx_power_restore(&mut wifi);
    let (task, interrupts) = wifi.into_running();
    let wifi = task.into_cold(interrupts);
    let Err(failure) = wifi.release() else {
        panic!("Wi-Fi released pending Bluetooth TX-power control state");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
    );

    let mut ieee802154 = RadioHardware::for_validation().into_ieee802154();
    occupy_ieee802154_bluetooth_tx_power_restore(&mut ieee802154);
    let (task, interrupts) = ieee802154.separate_interrupt_owner();
    let ieee802154 = task.into_cold(interrupts);
    let Err(failure) = ieee802154.release() else {
        panic!("IEEE 802.15.4 released pending Bluetooth TX-power control state");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
    );

    let mut bluetooth = RadioHardware::for_validation().into_bluetooth();
    occupy_bluetooth_tx_power_restore(&mut bluetooth);
    let (task, interrupts) = bluetooth.separate_interrupt_owner();
    let bluetooth = task
        .into_cold(interrupts)
        .expect("an idle Bluetooth task owner can be reunited");
    let Err(failure) = bluetooth.release() else {
        panic!("Bluetooth released pending TX-power control state");
    };
    assert_eq!(
        failure.error(),
        RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
    );
}

#[test]
fn cold_owner_is_consumed_by_interrupt_setup_split() {
    let registers = RadioHardware::for_validation().into_wifi();
    let (_running, _setup) = registers.into_running();
}

#[test]
fn wifi_route_roundtrip_returns_the_complete_root() {
    let wifi = RadioHardware::for_validation().into_wifi();
    let (task, setup) = wifi.into_running();
    let hardware = task
        .into_cold(setup)
        .release()
        .expect("an untouched cold route can be released");

    let bluetooth = hardware.into_bluetooth();
    let _hardware = bluetooth
        .release()
        .expect("an untouched Bluetooth route can be released");
}

#[test]
fn bluetooth_task_and_interrupt_owners_roundtrip_without_mmio() {
    let bluetooth = RadioHardware::for_validation().into_bluetooth();
    let (task, setup) = bluetooth.separate_interrupt_owner();
    let hardware = task
        .into_cold(setup)
        .expect("an idle Bluetooth task owner can be reunited")
        .release()
        .expect("an untouched Bluetooth route can be released");

    let wifi = hardware.into_wifi();
    let _hardware = wifi
        .release()
        .expect("an untouched Wi-Fi route can be released");
}

#[test]
fn ieee802154_route_roundtrip_returns_every_other_protocol_owner() {
    let ieee802154 = RadioHardware::for_validation().into_ieee802154();
    let hardware = ieee802154
        .release()
        .expect("a fresh IEEE 802.15.4 route has no pending PHY restore");

    // The IEEE 802.15.4 epoch retains the complete generated Bluetooth
    // controller partition behind its BTBB role and never consumes either
    // protocol's interrupt owner.
    let bluetooth = hardware.into_bluetooth();
    let hardware = bluetooth
        .release()
        .expect("an untouched Bluetooth route can be released");
    let wifi = hardware.into_wifi();
    let _hardware = wifi
        .release()
        .expect("an untouched Wi-Fi route can be released");
}

#[test]
fn ieee802154_task_and_interrupt_owners_reunite_without_mmio() {
    let ieee802154 = RadioHardware::for_validation().into_ieee802154();
    let (task, setup) = ieee802154.separate_interrupt_owner();
    let hardware = task
        .into_cold(setup)
        .release()
        .expect("an untouched IEEE 802.15.4 route can be released");

    let bluetooth = hardware.into_bluetooth();
    let _hardware = bluetooth
        .release()
        .expect("an untouched Bluetooth route can be released");
}

#[test]
fn mac_hal_tail_rejects_out_of_range_calibration_before_mmio() {
    let mut registers = RadioHardware::for_validation().into_wifi();
    assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::COLD_RX, 0x0004_0000));
    assert!(!registers.initialize_mac_hal_tail(MacInterruptMask::NONE, u32::MAX));
}

#[test]
fn mac_txrx_callbacks_reject_out_of_range_slot_before_mmio() {
    let mut registers = RadioHardware::for_validation().into_wifi().into_running().0;
    assert!(!registers.initialize_mac_txrx_callbacks(11));
    assert!(!registers.initialize_mac_txrx_callbacks(u8::MAX));
}
