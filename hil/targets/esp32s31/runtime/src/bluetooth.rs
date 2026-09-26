//! Bluetooth LE images over the production composition and its HCI
//! Controller.
//!
//! Every image starts the powered radio epoch, runs the radio runner and the
//! HCI service on their own tasks and serves the typed HIL console. The
//! `bluetooth-hil` image drives Direct Test Mode with HCI commands; the
//! `bluetooth-gatt` image runs the Trouble Host and the GATT application.
//! Both reach the radio only through the HCI transport.

mod console;
#[cfg(feature = "bluetooth-hil")]
mod dtm;
#[cfg(feature = "bluetooth-gatt")]
mod gatt;

use oer_bluetooth_controller::LeVersionInformation;
use oer_esp32s31_bluetooth::resources::BluetoothRadioHardware;
use oer_esp32s31_bluetooth_system::{
    BluetoothEntropy, BluetoothHciService, BluetoothHostTransport, BluetoothRunner,
    start_bluetooth_hci, start_esp32s31_bluetooth,
};
use oer_esp32s31_radio_esp_hal::EspHalRadioPlatform;
use oer_esp32s31_soc_esp_hal::entropy::Entropy;
use static_cell::StaticCell;

/// Development Controller identity for Link Layer version exchange: Core
/// 5.4, the unassigned company value and subversion 1.
const VERSION: LeVersionInformation = LeVersionInformation::new(0x0d, 0xffff, 1);

static PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static ENTROPY: StaticCell<BluetoothEntropy<'static>> = StaticCell::new();

pub(super) fn start(
    executor: &'static mut super::Executor<0>,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
) -> ! {
    let platform = PLATFORM.init(platform);
    let entropy = Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
    let entropy = ENTROPY.init(BluetoothEntropy::new(entropy));
    let hardware = BluetoothRadioHardware::take().expect("unique Bluetooth hardware");
    executor.run(|spawner| {
        spawner
            .spawn(main(spawner, platform, hardware, entropy, usb, boot).expect("Bluetooth task"));
    })
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the cold-start result crosses one poll boundary; the linked-image stack-frame audit bounds this task"
)]
async fn main(
    spawner: embassy_executor::Spawner,
    platform: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    entropy: &'static BluetoothEntropy<'static>,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
) {
    let system = match start_esp32s31_bluetooth(platform, hardware, None).await {
        Ok(system) => system,
        Err(_) => super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-cold-start\r\n"),
    };
    let hci = start_bluetooth_hci(
        system.runtime,
        system.public_address,
        Some(VERSION),
        entropy,
    );
    spawner.spawn(runner(system.runner).expect("Bluetooth runner task"));
    spawner.spawn(service(hci.service).expect("Bluetooth HCI task"));
    image(spawner, hci.host, usb, boot).await;
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
async fn runner(runner: BluetoothRunner) {
    let _fault = runner.run().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-runner-fault\r\n");
}

#[embassy_executor::task]
async fn service(service: BluetoothHciService) {
    let _exit = service.run().await;
    super::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=bluetooth-hci-service\r\n");
}
