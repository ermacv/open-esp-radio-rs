use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use open_esp_radio_bluetooth_hci::{
    BluetoothPublicDeviceAddress, LeControllerBootstrapConfig, LeControllerHciEndpoints,
    LeControllerHciResources,
    bt_hci::{cmd::controller_baseband::Reset, transport::Transport},
};

use super::{BluetoothControllerHciBindError, validate_hci_bind};

fn hci() -> LeControllerHciResources<NoopRawMutex, 1, 1, 45> {
    let config = LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        27,
        1,
    )
    .expect("nonzero test profile");
    LeControllerHciResources::new(config).expect("profile fits its bounded queues")
}

#[test]
fn post_publication_hci_binding_requires_a_pristine_epoch() {
    let pristine = hci();
    assert_eq!(validate_hci_bind(&pristine), Ok(()));

    let mut used = hci();
    {
        let LeControllerHciEndpoints {
            host,
            controller: _,
        } = used.split();
        block_on(async {
            host.write(&Reset::new())
                .await
                .expect("Reset enters the test queue");
        });
    }
    assert_eq!(
        validate_hci_bind(&used),
        Err(BluetoothControllerHciBindError::ResourcesNotPristine)
    );
}
