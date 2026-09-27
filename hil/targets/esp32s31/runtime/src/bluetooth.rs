//! Bluetooth LE images over the production composition and its HCI
//! Controller.
//!
//! Every image splits the radio for concurrent clients, starts the Bluetooth
//! client on the shared radio system, runs the radio system's periodic PHY
//! tracking on its own task and serves the typed HIL console. The
//! `bluetooth-hil` image drives Direct Test Mode with HCI commands and the
//! `bluetooth-gatt` image runs the Trouble Host and the GATT application; both
//! run the radio runner and the HCI service on their own tasks. The
//! `bluetooth-secure-gatt` image runs them beside its Host and restarts the
//! Controller epoch on request. All reach the radio only through the HCI
//! transport.

mod console;
#[cfg(feature = "bluetooth-hil")]
mod dtm;
#[cfg(feature = "bluetooth-gatt")]
mod gatt;
#[cfg(feature = "bluetooth-secure-gatt")]
mod secure;

use oer_bluetooth_controller::LeVersionInformation;
use oer_esp32s31_bluetooth_system::{BluetoothEntropy, BluetoothParked, start_bluetooth_hci};
#[cfg(not(feature = "bluetooth-secure-gatt"))]
use oer_esp32s31_bluetooth_system::{BluetoothHciService, BluetoothHostTransport, BluetoothSystem};
use oer_esp32s31_hal::root::{ConcurrentPartitions, RadioHardware};
use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
use oer_esp32s31_radio_runtime::RadioSystem;
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
use static_cell::StaticCell;

/// Development Controller identity for Link Layer version exchange: Core
/// 5.4, the unassigned company value and subversion 1.
const VERSION: LeVersionInformation = LeVersionInformation::new(0x0d, 0xffff, 1);

pub(super) type Radio = RadioSystem<EspHalRadioPlatform, EspHalRadioClocks>;

static RADIO: StaticCell<Radio> = StaticCell::new();
#[cfg(not(feature = "bluetooth-secure-gatt"))]
static SYSTEM: StaticCell<BluetoothSystem> = StaticCell::new();
static ENTROPY: StaticCell<BluetoothEntropy<'static>> = StaticCell::new();

pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let entropy = Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
    let entropy = ENTROPY.init(BluetoothEntropy::new(entropy));
    let identity = platform.phy_calibration_identity();
    let public_address = platform.bluetooth_public_address();
    let hardware = RadioHardware::take().expect("unique radio hardware");
    let (radio, partitions) =
        RadioSystem::new(hardware, platform, EspHalRadioClocks::new(), identity);
    let radio = RADIO.init(radio);
    executor.run(|spawner| {
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
    secure::run(radio, system, hci, public_address, usb, boot).await;
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
