//! Bluetooth LE images over the production composition and its HCI
//! Controller.
//!
//! Every image starts the shared radio with
//! `oer_esp32s31_radio_system::start`, which spawns its coexistence
//! schedule, starts the Bluetooth client on it, runs the radio's periodic PHY
//! tracking on its own task, which reports a tracking failure, and serves the
//! typed HIL console. The
//! `bluetooth-hil` image drives Direct Test Mode with HCI commands and the
//! `bluetooth-gatt` image runs the Trouble Host and the GATT application; both
//! run the radio runner and the HCI service on their own tasks. The
//! `bluetooth-secure-gatt` image runs them beside its Host and restarts the
//! Controller epoch on request. All reach the radio only through the HCI
//! transport.
//!
//! The `wifi-ble-coex` image instead starts the GATT application as a client
//! of the Wi-Fi image's shared radio ([`shared`]); the Wi-Fi image keeps the
//! radio system, its periodic tasks and the HIL console.

#[cfg(feature = "bluetooth-radio")]
mod console;
#[cfg(feature = "bluetooth-hil")]
mod dtm;
#[cfg(any(feature = "bluetooth-gatt", feature = "wifi-ble-coex"))]
mod gatt;
#[cfg(feature = "bluetooth-secure-gatt")]
mod secure;
#[cfg(feature = "wifi-ble-coex")]
pub(crate) mod shared;

use oer_bluetooth_controller::LeVersionInformation;
#[cfg(any(feature = "bluetooth-hil", feature = "bluetooth-gatt"))]
use oer_esp32s31_bluetooth_system::BluetoothHostTransport;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_bluetooth_system::{BluetoothEntropy, BluetoothParked, start_bluetooth_hci};
#[cfg(not(feature = "bluetooth-secure-gatt"))]
use oer_esp32s31_bluetooth_system::{BluetoothHciService, BluetoothSystem};
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_hal::root::ConcurrentPartitions;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
#[cfg(feature = "bluetooth-radio")]
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
#[cfg(feature = "bluetooth-radio")]
use static_cell::StaticCell;

/// Development Controller identity for Link Layer version exchange: Core
/// 5.4, the unassigned company value and subversion 1.
const VERSION: LeVersionInformation = LeVersionInformation::new(0x0d, 0xffff, 1);

#[cfg(feature = "bluetooth-radio")]
pub(super) type Radio = oer_esp32s31_radio_system::SharedRadio;
#[cfg(feature = "wifi-ble-coex")]
pub(super) type Radio = oer_esp32s31_ieee80211_system::SharedRadio;

#[cfg(all(feature = "bluetooth-radio", not(feature = "bluetooth-secure-gatt")))]
static SYSTEM: StaticCell<BluetoothSystem> = StaticCell::new();
#[cfg(feature = "bluetooth-radio")]
static ENTROPY: StaticCell<BluetoothEntropy<'static>> = StaticCell::new();

#[cfg(feature = "bluetooth-radio")]
pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    wake: crate::ExecutorWake,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let entropy = Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
    crate::transport::CONSOLE.start(boot);
    crate::transport::init_logger();
    let entropy = ENTROPY.init(BluetoothEntropy::new(entropy));
    let public_address = platform.bluetooth_public_address();
    executor.run(wake, |spawner| {
        // The image keeps its own tracking task, which reports a tracking
        // failure as `reason=phy-tracking` before the reset.
        let start = oer_esp32s31_radio_system::RadioStart::new()
            .with_tracking(oer_esp32s31_radio_system::Tracking::Caller);
        let (radio, partitions) = oer_esp32s31_radio_system::start(spawner, platform, start)
            .expect("the shared radio must start once");
        spawner.spawn(
            main(
                spawner,
                radio,
                partitions,
                public_address,
                entropy,
                usb,
                boot,
            )
            .expect("Bluetooth task"),
        );
    })
}

#[cfg(feature = "bluetooth-radio")]
#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the start result crosses one poll boundary; the linked-image stack-frame audit bounds this task"
)]
async fn main(
    spawner: embassy_executor::Spawner,
    radio: &'static Radio,
    partitions: ConcurrentPartitions,
    public_address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    entropy: &'static BluetoothEntropy<'static>,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) {
    let ConcurrentPartitions {
        wifi: _wifi,
        bluetooth,
        ieee802154: _ieee802154,
    } = partitions;
    let Ok(parked) = BluetoothParked::new(bluetooth) else {
        super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-memory\r\n");
    };
    let system = match oer_esp32s31_bluetooth_system::start(radio, parked, public_address).await {
        Ok(system) => system,
        Err(_) => super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-start\r\n"),
    };
    let hci = start_bluetooth_hci(system.runtime(), public_address, Some(VERSION), entropy);
    spawner.spawn(tracking(radio).expect("PHY tracking task"));
    #[cfg(feature = "bluetooth-secure-gatt")]
    secure::run(spawner, radio, system, hci, public_address, usb, boot).await;
    #[cfg(not(feature = "bluetooth-secure-gatt"))]
    {
        spawner.spawn(runner(radio, SYSTEM.init(system)).expect("Bluetooth runner task"));
        spawner.spawn(service(hci.service).expect("Bluetooth HCI task"));
        image(spawner, hci.host, usb, boot).await;
    }
}

#[cfg(feature = "bluetooth-hil")]
async fn image(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) -> ! {
    dtm::run(spawner, host, usb, boot).await
}

#[cfg(feature = "bluetooth-gatt")]
async fn image(
    spawner: embassy_executor::Spawner,
    host: BluetoothHostTransport,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) {
    spawner.spawn(gatt::task(host, usb, boot).expect("Bluetooth GATT task"));
}

#[cfg(feature = "bluetooth-radio")]
#[embassy_executor::task]
async fn tracking(radio: &'static Radio) {
    let _error = radio.run_tracking().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=phy-tracking\r\n");
}

#[cfg(not(feature = "bluetooth-secure-gatt"))]
#[embassy_executor::task]
async fn runner(radio: &'static Radio, system: &'static mut BluetoothSystem) {
    let _fault = system.run(radio).await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n");
}

#[cfg(not(feature = "bluetooth-secure-gatt"))]
#[embassy_executor::task]
async fn service(mut service: BluetoothHciService) {
    let _exit = service.run().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service\r\n");
}
