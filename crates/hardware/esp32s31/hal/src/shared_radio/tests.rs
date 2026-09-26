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
        (),
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
    let (_registers, phy, ()) = radio
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

#[test]
fn btbb_joins_an_initialized_baseband_without_register_access() {
    use crate::owner::PhyInitializationAccess;
    use oer_esp32s31_pac::Ieee802154TaskRegisters;
    let RadioPartitions {
        bluetooth,
        ieee802154,
        ..
    } = RadioPartitions::for_validation();
    let bluetooth = oer_esp32s31_pac::BluetoothTaskRegisters::new(bluetooth);
    let (ieee802154, _interrupts) = Ieee802154TaskRegisters::new(ieee802154);
    let mut radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    // Without a registration no gain parameter can be valid.
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let unregistered = unsafe { lease.btbb_acquire(&bluetooth, 0) };
    assert_eq!(unregistered, Err(BtbbError::Unregistered));
    assert_eq!(
        lease.override_ieee802154_tx_on_delay(&ieee802154),
        Err(BtbbError::NotAcquired)
    );
    assert_eq!(lease.btbb_release(&bluetooth), Err(BtbbError::NotAcquired));
    let _epoch = lease.phy_hal().begin_registration_epoch();
    drop(lease);

    // Bluetooth already initialized the baseband.
    radio.hold_btbb_for_test(RadioClient::Bluetooth);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // SAFETY: the baseband is initialized, so joining touches no register.
    #[allow(unsafe_code)]
    let joined = unsafe { lease.btbb_acquire(&ieee802154, 0) };
    assert_eq!(joined, Ok(BtbbAcquired::Joined));
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let again = unsafe { lease.btbb_acquire(&ieee802154, 0) };
    assert_eq!(again, Err(BtbbError::AlreadyAcquired));
    assert!(lease.holds_btbb(RadioClient::Ieee802154));
    assert_eq!(lease.btbb_release(&ieee802154), Ok(()));
    assert_eq!(lease.btbb_release(&bluetooth), Ok(()));
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_arbiter_stays_while_btbb_is_held() {
    let mut radio = arbiter();
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let Err((_radio, error)) = radio.into_parts() else {
        panic!("IEEE 802.15.4 still holds BTBB");
    };
    assert_eq!(error, SharedRadioReleaseError::BtbbHeld);
}

#[test]
fn an_until_proof_needs_a_non_empty_window() {
    let mut wifi =
        oer_esp32s31_pac::WifiRadioRegisters::new(RadioPartitions::for_validation().wifi_mac);
    assert_eq!(
        ClientQuiescence::until(&mut wifi, 20, 20).err(),
        Some(EmptyQuiescentWindow)
    );
    let proof = ClientQuiescence::until(&mut wifi, 10, 20)
        .unwrap_or_else(|_| panic!("a non-empty window is accepted"));
    assert_eq!(proof.client(), RadioClient::Wifi);
    assert_eq!(
        proof.span(),
        QuiescentSpan::Until {
            issued_at_micros: 10,
            release_by_micros: 20
        }
    );
}

struct NoPlatform;

impl crate::power::PlatformClockProvider for NoPlatform {
    fn acquire_pll_f160m(&mut self) -> Result<(), crate::power::PlatformClockError> {
        Err(crate::power::PlatformClockError)
    }
    fn release_pll_f160m(&mut self) -> Result<(), crate::power::PlatformClockError> {
        Err(crate::power::PlatformClockError)
    }
    fn acquire_analog_i2c_clock(&mut self) -> Result<(), crate::power::PlatformClockError> {
        Err(crate::power::PlatformClockError)
    }
    fn release_analog_i2c_clock(&mut self) -> Result<(), crate::power::PlatformClockError> {
        Err(crate::power::PlatformClockError)
    }
}

#[test]
fn a_client_cannot_disable_modem_clocks_it_never_enabled() {
    let wifi =
        oer_esp32s31_pac::WifiRadioRegisters::new(RadioPartitions::for_validation().wifi_mac);
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // Rejected before any register access.
    assert_eq!(
        lease.disable_modem_clocks(&wifi, &mut NoPlatform),
        Err(ModemClockError::NotEnabled)
    );
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_domain_cannot_disable_a_clock_module_it_never_enabled() {
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // Rejected before any register access, per module slot.
    for module in [PhyClockModule::Phy, PhyClockModule::Calibration] {
        assert_eq!(
            lease.disable_phy_modem_clocks(module, &mut NoPlatform),
            Err(ModemClockError::NotEnabled)
        );
    }
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_arbiter_keeps_one_coexistence_priority_table_across_leases() {
    use crate::coex::{CoexEventId, CoexPti, CoexPtiTable};

    let radio = arbiter();
    let rx_ack = CoexEventId::new(3).unwrap_or_else(|| panic!("event 3 exists"));
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
        assert_eq!(lease.coex_pti_table(), CoexPtiTable::VENDOR);
        lease.set_coex_pti(
            rx_ack,
            CoexPti::new(9).unwrap_or_else(|| panic!("9 is a priority")),
        );
    }
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a released arbiter grants its lease"));
    assert_eq!(lease.coex_pti(rx_ack).value(), 9);
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_grant_protect_request_cannot_be_withdrawn_before_it_is_programmed() {
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(!lease.phy_grant_protected());
    // Rejected before any register access.
    assert_eq!(
        lease.release_phy_grant_protect(),
        Err(PhyGrantProtectError::NotProtected)
    );
    drop(lease);
    assert!(radio.into_parts().is_ok());
}
