//! Trouble owns HCI/ATT; HIL owns only USB and value-only observations.
#[cfg(not(feature = "bluetooth-secure-gatt"))]
#[path = "bluetooth_gatt/plaintext.rs"]
mod application;
#[cfg(feature = "bluetooth-secure-gatt")]
#[path = "bluetooth_gatt/secure.rs"]
mod application;
mod console;
use embassy_time::Duration;
use oer_esp32s31_bluetooth::{
    le::{
        dtm::{DtmDefaultTxPowerDbm, DtmRuntimeConfig},
        peripheral::PeripheralConnectionRuntimeConfig,
        scanning::PassiveScanRuntimeConfig,
    },
    resources::BluetoothRadioHardware,
};
use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckPeriod;
use oer_esp32s31_bluetooth_integration::entropy::BluetoothEntropy;
use oer_esp32s31_bluetooth_integration::{
    BluetoothColdStartConfig, BluetoothHardwareRunner, BluetoothHostController, BluetoothSystem,
    BluetoothSystemStorage, BluetoothTroubleSystem, start_esp32s31_bluetooth,
};
use oer_esp32s31_bluetooth_memory::{
    DtmSchedulerAllocationConfig, PassiveScanDefaultTxPowerDbm,
    PassiveScanSchedulerAllocationConfig, PeripheralConnectionDefaultTxPowerDbm,
};
use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};
use oer_esp32s31_soc::watchdog::DeadlineWatchdog;
use static_cell::StaticCell;
use trouble_host::prelude::*;

static PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static STORAGE: BluetoothSystemStorage<EspHalBluetoothPlatform<'static>, 4, 1, 4, 4, 258> =
    BluetoothSystemStorage::new();
static RESOURCES: StaticCell<HostResources<DefaultPacketPool, 1, 3>> = StaticCell::new();
static ENTROPY: StaticCell<BluetoothEntropy<'static>> = StaticCell::new();

pub(crate) fn start(
    executor: &'static mut crate::Executor<0>,
    platform: EspHalRadioPlatform,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    rng: esp_hal::peripherals::RNG<'static>,
    service: &'static DeadlineWatchdog,
) -> ! {
    let platform = PLATFORM.init(platform);
    let entropy = oer_esp32s31_soc::entropy::Entropy::new(rng);
    let boot = u64::from_le_bytes(entropy.random_bytes().expect("HIL boot entropy")).max(1);
    let entropy = ENTROPY.init(BluetoothEntropy::new(entropy));
    let hardware = BluetoothRadioHardware::take().expect("unique Bluetooth hardware");
    executor.run(|spawner| {
        spawner.spawn(task(platform, hardware, usb, boot, service, entropy).expect("GATT task"))
    })
}

#[embassy_executor::task]
async fn task(
    platform: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    boot: u64,
    service: &'static DeadlineWatchdog,
    entropy: &'static BluetoothEntropy<'static>,
) {
    let peripheral =
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(0))
            .with_software_recurring_timing(500)
            .expect("explicit clock bound")
            .with_version_information(oer_bluetooth_ll::control::LeVersionInformation::new(
                0x0d, 0xffff, 1,
            ));
    let config = BluetoothColdStartConfig::new(
        crate::watchdog::bluetooth(service),
        251,
        4,
        None,
        DtmRuntimeConfig::new(
            DtmSchedulerAllocationConfig::new(0, 0, 0),
            DtmDefaultTxPowerDbm::new(0),
        ),
        PassiveScanRuntimeConfig::new(
            PassiveScanSchedulerAllocationConfig::new(0, 0).expect("scan allocation"),
            PassiveScanDefaultTxPowerDbm::new(0),
        ),
        DtmRecheckPeriod::from_duration(Duration::from_micros(50)).expect("nonzero recheck"),
    )
    .with_peripheral_connection(peripheral);
    let mut startup = core::pin::pin!(start_esp32s31_bluetooth(
        platform, hardware, &STORAGE, config
    ));
    let output = match startup.as_mut().await {
        Ok(output) => output,
        Err(_) => crate::fail(c"OPEN_RADIO_HIL GATT cold start failed\r\n"),
    };
    #[cfg(feature = "bluetooth-secure-gatt")]
    {
        let mut application = core::pin::pin!(application::run(usb, boot, output, entropy));
        match core::future::poll_fn(|cx| poll_live(application.as_mut(), cx)).await {}
    }

    #[cfg(not(feature = "bluetooth-secure-gatt"))]
    {
        // Retain the exact platform owner beside the live Controller. This image
        // proves reconnect, not coordinated cold retirement or shutdown.
        let _platform_owner = output.platform;
        let mut system = initialize_host(output.system, RESOURCES.init_with(HostResources::new));
        system
            .hardware
            .install_entropy(entropy)
            .expect("initial HCI entropy binding");
        let stack = system.stack;
        let mut runner = stack.runner();
        let mut hardware = core::pin::pin!(system.hardware.run());
        let mut application_and_console = core::pin::pin!(application::run(usb, boot, &stack));
        let mut host = core::pin::pin!(runner.run());
        let mut live = core::pin::pin!(embassy_futures::select::select3(
            host.as_mut(),
            hardware.as_mut(),
            application_and_console.as_mut(),
        ));
        // Keep live Host/ATT/USB polling out of the cold-start owner-transfer frame.
        // The future stays pinned in the same task storage across this boundary.
        match core::future::poll_fn(|cx| poll_live(live.as_mut(), cx)).await {
            embassy_futures::select::Either3::First(result) => {
                panic!("Trouble Host stopped: {:?}", result)
            }
            embassy_futures::select::Either3::Second(never) => match never {},
            embassy_futures::select::Either3::Third(never) => match never {},
        }
    }
}

#[inline(never)]
fn poll_live<F: Future>(
    future: core::pin::Pin<&mut F>,
    cx: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    let poll: fn(
        core::pin::Pin<&mut F>,
        &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> = F::poll;
    core::hint::black_box(poll)(future, cx)
}

// Host construction materializes security-manager and connection state, beyond
// the caller-owned resource arrays. Keep that work out of the cold-start result
// transfer frame. The same affine Controller and hardware owners are returned.
#[inline(never)]
fn initialize_host<'a>(
    system: BluetoothSystem<4, 1, 4, 4, 258>,
    resources: &'a mut HostResources<DefaultPacketPool, 1, 3>,
) -> BluetoothTroubleSystem<
    'a,
    BluetoothHostController<4, 4, 258>,
    DefaultPacketPool,
    BluetoothHardwareRunner<4, 1, 4, 4, 258>,
> {
    system.into_trouble(resources)
}
