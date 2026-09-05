use super::{Radio, state};

#[derive(Debug, Eq, PartialEq)]
struct TestPeripheral {
    id: u8,
    ready: bool,
}

fn require_owned(_: &Radio<TestPeripheral, state::Owned>) {}
fn require_powered(_: &Radio<TestPeripheral, state::Powered>) {}

#[test]
#[cfg(target_arch = "riscv32")]
fn peripheral_token_follows_the_type_state_owner() {
    let owned = Radio::claim(TestPeripheral { id: 7, ready: true })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
    require_owned(&owned);

    let powered = owned
        .power_up()
        .unwrap_or_else(|_| panic!("fake prerequisite sequence failed"));
    require_powered(&powered);
    assert_eq!(powered.peripheral(), &TestPeripheral { id: 7, ready: true });
}

#[test]
fn validation_powered_owner_preserves_the_unique_owner() {
    let owned = Radio::claim(TestPeripheral { id: 8, ready: true })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
    require_owned(&owned);

    let powered = owned.assume_powered_for_validation();
    require_powered(&powered);
    assert_eq!(powered.peripheral(), &TestPeripheral { id: 8, ready: true });
}

#[test]
fn channel_capability_is_a_temporary_borrow_not_a_consuming_split() {
    let owned = Radio::claim(TestPeripheral {
        id: 10,
        ready: true,
    })
    .unwrap_or_else(|_| panic!("test radio claim failed"));
    let mut powered = owned.assume_powered_for_validation();
    let channel = powered.channel_hal();
    drop(channel);
    assert_eq!(powered.peripheral().id, 10);
}

#[test]
fn unpowered_owner_releases_platform_and_neutral_radio_roots() {
    let owned = Radio::claim(TestPeripheral { id: 9, ready: true })
        .unwrap_or_else(|_| panic!("test radio claim failed"));
    let (peripheral, hardware) = owned
        .release()
        .expect("an untouched Wi-Fi route can be released");
    assert_eq!(peripheral, TestPeripheral { id: 9, ready: true });

    let hardware = hardware
        .into_bluetooth()
        .release()
        .expect("an untouched Bluetooth route can be released");
    let _wifi = hardware.into_wifi();
}

#[test]
fn released_hardware_reenters_wifi_after_exclusive_bluetooth_route() {
    let owned = Radio::claim(TestPeripheral {
        id: 12,
        ready: true,
    })
    .unwrap_or_else(|_| panic!("test radio claim failed"));
    let (peripheral, hardware) = owned
        .release()
        .expect("an untouched Wi-Fi route can be released");
    let hardware = hardware
        .into_bluetooth()
        .release()
        .expect("an untouched Bluetooth route can be released");

    let returned = Radio::from_hardware(peripheral, hardware);
    require_owned(&returned);
    let (peripheral, _hardware) = returned
        .release()
        .expect("the returned untouched route can be released");
    assert_eq!(
        peripheral,
        TestPeripheral {
            id: 12,
            ready: true
        }
    );
}

#[test]
#[cfg(target_arch = "riscv32")]
fn failed_power_transition_can_only_retry_the_unique_owner() {
    let owned = Radio::claim(TestPeripheral {
        id: 11,
        ready: false,
    })
    .unwrap_or_else(|_| panic!("test radio claim failed"));
    let failure = match owned.power_up() {
        Ok(_) => panic!("stuck reset unexpectedly powered the radio"),
        Err(failure) => failure,
    };
    let first_error = failure.error();
    let retried = match failure.retry() {
        Ok(_) => panic!("stuck reset unexpectedly powered the radio on retry"),
        Err(failure) => failure,
    };
    assert_eq!(retried.error(), first_error);
}
