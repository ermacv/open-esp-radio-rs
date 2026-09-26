use super::{RadioHardware, RadioPhyReleaseError};
use crate::{
    bluetooth::ColdOwner, ieee802154::role::Ieee802154Owned, owner::WifiColdRegisters,
    phy::restore::PhyRouteState,
};

#[derive(Clone, Copy)]
enum Restore {
    TxDcPwdet,
    TxIqToneControl,
    RxDcoControl,
    BluetoothTxPowerControl,
}

impl Restore {
    const ALL: [Self; 4] = [
        Self::TxDcPwdet,
        Self::TxIqToneControl,
        Self::RxDcoControl,
        Self::BluetoothTxPowerControl,
    ];

    const fn error(self) -> RadioPhyReleaseError {
        match self {
            Self::TxDcPwdet => RadioPhyReleaseError::TxDcPwdetRestorePending,
            Self::TxIqToneControl => RadioPhyReleaseError::TxIqToneControlRestorePending,
            Self::RxDcoControl => RadioPhyReleaseError::RxDcoControlRestorePending,
            Self::BluetoothTxPowerControl => {
                RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
            }
        }
    }
}

impl Restore {
    fn occupy(self, slot: &mut PhyRouteState) {
        match self {
            Self::TxDcPwdet => slot.occupy_txdc_for_test(),
            Self::TxIqToneControl => slot.occupy_txiq_for_test(),
            Self::RxDcoControl => slot.occupy_rx_dco_for_test(),
            Self::BluetoothTxPowerControl => slot.occupy_bluetooth_tx_power_control_for_test(),
        }
    }
}

#[test]
fn pending_restore_survives_same_route_transitions_and_blocks_every_release() {
    for restore in Restore::ALL {
        let mut wifi = WifiColdRegisters::from_hardware(RadioHardware::for_validation());
        restore.occupy(wifi.phy_state_mut());
        let (registers, interrupts, route) = wifi.into_running();
        let wifi = WifiColdRegisters::from_running(registers, interrupts, route);
        let Err((_wifi, error)) = wifi.release() else {
            panic!("Wi-Fi released a pending restore");
        };
        assert_eq!(error, restore.error());

        let mut ieee802154 = Ieee802154Owned::from_hardware((), RadioHardware::for_validation());
        restore.occupy(ieee802154.phy_state_mut());
        let Err(failure) = ieee802154.release() else {
            panic!("IEEE 802.15.4 released a pending restore");
        };
        assert_eq!(failure.error(), restore.error());

        let bluetooth = ColdOwner::from_radio_hardware(RadioHardware::for_validation());
        let (mut task, interrupts) = bluetooth.separate_interrupt_owner();
        restore.occupy(task.phy_state_mut());
        let bluetooth = task
            .into_cold(interrupts)
            .expect("an idle Bluetooth task owner can be reunited");
        let Err(failure) = bluetooth.release() else {
            panic!("Bluetooth released a pending restore");
        };
        assert_eq!(failure.error(), restore.error());
    }
}

#[test]
fn wifi_route_roundtrip_returns_the_complete_root() {
    let wifi = WifiColdRegisters::from_hardware(RadioHardware::for_validation());
    let (registers, interrupts, route) = wifi.into_running();
    let Ok(hardware) = WifiColdRegisters::from_running(registers, interrupts, route).release()
    else {
        panic!("an untouched cold route can be released");
    };

    let _hardware = ColdOwner::from_radio_hardware(hardware)
        .release()
        .expect("an untouched Bluetooth route can be released");
}

#[test]
fn bluetooth_task_and_interrupt_owners_roundtrip_without_mmio() {
    let bluetooth = ColdOwner::from_radio_hardware(RadioHardware::for_validation());
    let (task, setup) = bluetooth.separate_interrupt_owner();
    let hardware = task
        .into_cold(setup)
        .expect("an idle Bluetooth task owner can be reunited")
        .release()
        .expect("an untouched Bluetooth route can be released");

    let Ok(_hardware) = WifiColdRegisters::from_hardware(hardware).release() else {
        panic!("an untouched Wi-Fi route can be released");
    };
}

#[test]
fn ieee802154_route_roundtrip_returns_every_other_protocol_owner() {
    let (_, hardware) = Ieee802154Owned::from_hardware((), RadioHardware::for_validation())
        .release()
        .unwrap_or_else(|_| panic!("a fresh IEEE 802.15.4 route has no pending PHY restore"));

    // The IEEE 802.15.4 epoch retains the complete Bluetooth controller
    // partition behind its BTBB role and never consumes either protocol's
    // interrupt owner.
    let hardware = ColdOwner::from_radio_hardware(hardware)
        .release()
        .expect("an untouched Bluetooth route can be released");
    let Ok(_hardware) = WifiColdRegisters::from_hardware(hardware).release() else {
        panic!("an untouched Wi-Fi route can be released");
    };
}

#[test]
fn registration_epoch_is_replaced_by_registration_and_retired_by_every_route_release() {
    let mut wifi = RadioHardware::for_validation().into_wifi();
    let phy = &mut wifi.phy;
    assert_eq!(phy.registration_epoch(), None);
    let first = phy.begin_registration_epoch();
    assert_eq!(phy.registration_epoch(), Some(first));
    let second = phy.begin_registration_epoch();
    assert_ne!(first, second);
    assert_eq!(phy.registration_epoch(), Some(second));

    let mut bluetooth =
        RadioHardware::from_wifi(wifi.registers, wifi.interrupts, wifi.phy, wifi.retained)
            .into_bluetooth();
    let phy = &mut bluetooth.phy;
    assert_eq!(phy.registration_epoch(), None);
    let third = phy.begin_registration_epoch();
    assert!(third != first && third != second);

    let mut ieee802154 = RadioHardware::from_bluetooth(
        bluetooth.task,
        bluetooth.modem_lp_timer,
        bluetooth.interrupts,
        bluetooth.phy,
        bluetooth.retained,
    )
    .into_ieee802154();
    let phy = &mut ieee802154.phy;
    assert_eq!(phy.registration_epoch(), None);
    let fourth = phy.begin_registration_epoch();
    assert!(fourth != first && fourth != second && fourth != third);

    let wifi = RadioHardware::from_ieee802154(ieee802154).into_wifi();
    assert_eq!(wifi.phy.registration_epoch(), None);
}

#[test]
fn a_concurrent_split_reunites_and_retires_the_registration() {
    use crate::owner::PhyInitializationAccess;
    let (shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    let epoch = {
        let mut lease = shared
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        lease.phy_hal().begin_registration_epoch()
    };
    let (hardware, ()) = RadioHardware::from_concurrent(shared, partitions)
        .unwrap_or_else(|_| panic!("an idle split reunites"));
    // Returning to the neutral root retires the registration, as a route does.
    let wifi = WifiColdRegisters::from_hardware(hardware);
    assert_ne!(wifi.phy_state().registration_epoch(), Some(epoch));
}

#[test]
fn reunion_waits_for_common_power_and_calibration_restore() {
    use crate::shared_radio::{RadioClient, SharedRadioReleaseError};
    let (mut shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    shared.hold_common_power_for_test(RadioClient::Bluetooth);
    let Err(failure) = RadioHardware::from_concurrent(shared, partitions) else {
        panic!("a client still holds common power");
    };
    assert_eq!(
        failure.error(),
        super::ConcurrentReunionError::Shared(SharedRadioReleaseError::CommonPowerHeld)
    );

    let (_shared, partitions) = failure.into_parts();
    let (mut shared, _) = RadioHardware::for_validation().into_concurrent(());
    shared.phy_state_mut_for_test().occupy_txdc_for_test();
    let Err(failure) = RadioHardware::from_concurrent(shared, partitions) else {
        panic!("a calibration still owns a restore obligation");
    };
    assert_eq!(
        failure.error(),
        super::ConcurrentReunionError::Shared(SharedRadioReleaseError::Restore(
            RadioPhyReleaseError::TxDcPwdetRestorePending
        ))
    );
}
