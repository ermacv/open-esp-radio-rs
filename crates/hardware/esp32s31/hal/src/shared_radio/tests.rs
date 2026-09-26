use super::*;
use crate::phy::registration::PhyRegistration;
use oer_esp32s31_pac::{RadioPartitions, SharedRadioParts};

fn arbiter() -> SharedRadio {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_validation();
    SharedRadio::new(
        SharedRadioRegisters::new(SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        }),
        PhyRouteState::new(PhyRegistration::new()),
    )
}

#[test]
fn only_one_lease_exists_at_a_time() {
    let radio = arbiter();
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(radio.is_held());
    assert_eq!(radio.try_acquire().err(), Some(SharedRadioBusy));
    drop(lease);
    assert!(!radio.is_held());
    let _again = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a dropped lease frees the arbiter"));
}

#[test]
fn a_forgotten_lease_keeps_the_arbiter_busy() {
    let radio = arbiter();
    core::mem::forget(
        radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease")),
    );
    assert_eq!(radio.try_acquire().err(), Some(SharedRadioBusy));
}

#[test]
fn registration_epoch_persists_across_leases() {
    use crate::owner::PhyInitializationAccess;
    let radio = arbiter();
    let epoch = {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
        assert_eq!(lease.registration_epoch(), None);
        // Protocol transactions and PHY operations borrow the same owner.
        let _shared = lease.registers_mut().radio_phy();
        lease.phy_hal().begin_registration_epoch()
    };
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a dropped lease frees the arbiter"));
    assert_eq!(lease.registration_epoch(), Some(epoch));
    drop(lease);
    let (_registers, phy) = radio
        .into_parts()
        .unwrap_or_else(|_| panic!("an idle arbiter leaves arbitration"));
    assert_eq!(phy.registration_epoch(), Some(epoch));
}

#[test]
fn common_power_membership_follows_the_proving_owner() {
    use oer_esp32s31_pac::{BluetoothTaskRegisters, WifiRadioRegisters};
    let RadioPartitions {
        wifi_mac,
        bluetooth,
        ..
    } = RadioPartitions::for_validation();
    let wifi = WifiRadioRegisters::new(wifi_mac);
    let bluetooth = BluetoothTaskRegisters::new(bluetooth);
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    // Leaving before entering is rejected before any register access.
    assert_eq!(
        lease.exit_common_power(&wifi),
        Err(CommonRadioPowerError::NotEntered)
    );
    assert_eq!(
        lease.exit_common_power(&bluetooth),
        Err(CommonRadioPowerError::NotEntered)
    );
    assert!(!lease.holds_common_power(RadioClient::Wifi));
    assert!(!lease.holds_common_power(RadioClient::Bluetooth));
}
