#![no_main]
#![no_std]
#![recursion_limit = "256"]

mod startup;

#[cfg(all(feature = "advertising-smoke", feature = "trouble-gatt"))]
compile_error!("advertising-smoke and trouble-gatt select different Host owners");

#[cfg(not(any(feature = "advertising-smoke", feature = "trouble-gatt")))]
use bt_hci::cmd::le::{LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2};
#[cfg(feature = "advertising-smoke")]
use bt_hci::{
    cmd::le::{LeSetAdvData, LeSetAdvEnable, LeSetRandomAddr},
    param::BdAddr,
};
#[cfg(feature = "advertising-smoke")]
use open_esp_radio_esp32s31_bluetooth_controller_example::AdvertisingSmokeCase;
#[cfg(feature = "trouble-gatt")]
use open_esp_radio_esp32s31_bluetooth_controller_example::{
    TROUBLE_GATT_DEVICE_NAME, TROUBLE_GATT_SERVICE_UUID, TROUBLE_GATT_VALUE_UUID,
    encode_trouble_gatt_advertising,
};

#[cfg(feature = "trouble-gatt")]
use bt_hci::param::AdvChannelMap;
#[cfg(not(feature = "trouble-gatt"))]
use bt_hci::{
    cmd::{SyncCmd, controller_baseband::Reset},
    controller::Controller,
};

#[cfg(feature = "trouble-gatt")]
use embassy_time::Duration;
#[cfg(not(feature = "trouble-gatt"))]
use embassy_time::{Duration, Timer, with_timeout};

use esp_backtrace as _;

use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterrupt,
    timer::{OneShotTimer, timg::TimerGroup},
};

#[cfg(feature = "trouble-gatt")]
use oer_esp32s31_bluetooth::le::peripheral::PeripheralConnectionRuntimeConfig;
use oer_esp32s31_bluetooth::{
    le::{
        dtm::{DtmDefaultTxPowerDbm, DtmRuntimeConfig},
        scanning::PassiveScanRuntimeConfig,
    },
    resources::BluetoothRadioHardware,
};

use oer_esp32s31_bluetooth_embassy::controller::DtmRecheckPeriod;

use oer_esp32s31_bluetooth_integration::{
    BluetoothColdStartConfig, BluetoothSystemStorage, start_esp32s31_bluetooth,
};
#[cfg(not(feature = "trouble-gatt"))]
use oer_esp32s31_bluetooth_integration::{BluetoothHostController, BluetoothSystem};

#[cfg(feature = "trouble-gatt")]
use oer_esp32s31_bluetooth_memory::PeripheralConnectionDefaultTxPowerDbm;
use oer_esp32s31_bluetooth_memory::{
    DtmSchedulerAllocationConfig, PassiveScanDefaultTxPowerDbm,
    PassiveScanSchedulerAllocationConfig,
};

use oer_esp32s31_embassy_runtime::Executor;

use oer_esp32s31_radio_platform_esp_hal::{EspHalBluetoothPlatform, EspHalRadioPlatform};

use static_cell::StaticCell;

#[cfg(feature = "trouble-gatt")]
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
#[cfg(feature = "trouble-gatt")]
use trouble_host::prelude::*;

const MODEM_TIMER_CAPACITY: usize = 4;
const SCHEDULER_CAPACITY: usize = 1;
const HOST_TO_CONTROLLER_DEPTH: usize = 4;
const CONTROLLER_TO_HOST_DEPTH: usize = 4;
const PACKET_CAPACITY: usize = 258;
#[cfg(not(feature = "trouble-gatt"))]
const LE_TEST_DWELL: Duration = Duration::from_secs(1);
#[cfg(not(feature = "trouble-gatt"))]
const HCI_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

type BluetoothStorage = BluetoothSystemStorage<
    EspHalBluetoothPlatform<'static>,
    MODEM_TIMER_CAPACITY,
    SCHEDULER_CAPACITY,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;
#[cfg(not(feature = "trouble-gatt"))]
type BluetoothHost =
    BluetoothHostController<HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>;

#[cfg(feature = "trouble-gatt")]
type TroubleResources = HostResources<DefaultPacketPool, 1, 3>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static BLUETOOTH_STORAGE: BluetoothStorage = BluetoothStorage::new();
#[cfg(feature = "trouble-gatt")]
static TROUBLE_RESOURCES: StaticCell<TroubleResources> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    esp_println::logger::init_logger_from_env();
    esp_println::println!("open-radio: application entered");
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // SAFETY: the common stage-two entry runs after the board bootstrap,
    // with global interrupts disabled and the PSRAM mapping intact.
    let _psram = unsafe { oer_esp32s31_runtime::adopt_psram(peripherals.PSRAM) };

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    oer_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));
    let platform = RADIO_PLATFORM.init(EspHalRadioPlatform::new(
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    ));
    let hardware =
        BluetoothRadioHardware::take().expect("Bluetooth radio must have a unique owner");
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    esp_println::println!("open-radio: executor starting");
    // Timer and executor handlers are now bound; the staged handoff kept MIE clear.
    unsafe { core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack)) };
    executor.run(|spawner| {
        spawner.spawn(
            bluetooth_controller_task(platform, hardware)
                .expect("Bluetooth Controller task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn bluetooth_controller_task(
    platform: &'static EspHalRadioPlatform,
    hardware: BluetoothRadioHardware,
) {
    let dtm = DtmRuntimeConfig::new(
        DtmSchedulerAllocationConfig::new(0, 0, 0),
        DtmDefaultTxPowerDbm::new(0),
    );
    let passive_scan = PassiveScanRuntimeConfig::new(
        PassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the standalone Controller limits fit the scanner graph"),
        PassiveScanDefaultTxPowerDbm::new(0),
    );
    let recheck_period = DtmRecheckPeriod::from_duration(Duration::from_micros(50))
        .expect("the Controller-time recheck period must be nonzero");
    let config = BluetoothColdStartConfig::new(251, 4, None, dtm, passive_scan, recheck_period);
    #[cfg(feature = "trouble-gatt")]
    let config = config.with_peripheral_connection(
        PeripheralConnectionRuntimeConfig::new(PeripheralConnectionDefaultTxPowerDbm::new(0))
            .with_software_recurring_timing(500)
            .expect("the broadest BLE sleep-clock bound is valid")
            .with_version_information(oer_bluetooth_ll::control::LeVersionInformation::new(
                0x0d, 0xffff, 1,
            )),
    );
    esp_println::println!("open-radio: Bluetooth Controller cold start submitted");
    let mut startup = core::pin::pin!(start_esp32s31_bluetooth(
        platform,
        hardware,
        &BLUETOOTH_STORAGE,
        config
    ));
    let output = match startup.as_mut().await {
        Ok(output) => output,
        Err(error) => startup::fail(&error),
    };
    #[cfg(feature = "trouble-gatt")]
    {
        let resources = TROUBLE_RESOURCES.init(HostResources::new());
        let system = output.system.into_trouble(resources);
        let stack = system.stack;
        let hardware_runner = system.hardware;
        let mut host_runner = stack.runner();
        let mut peripheral = stack.peripheral();

        let appearance = [0x00, 0x00];
        let mut value_storage = [0_u8; 1];
        let mut table: AttributeTable<'_, NoopRawMutex, 10> = AttributeTable::new();
        let mut gap = table.add_service(Service::new(0x1800_u16));
        let _ = gap.add_characteristic_ro(0x2a00_u16, TROUBLE_GATT_DEVICE_NAME);
        let _ = gap.add_characteristic_ro(0x2a01_u16, &appearance);
        gap.build();
        table.add_service(Service::new(0x1801_u16));
        let value = table
            .add_service(Service::new(TROUBLE_GATT_SERVICE_UUID))
            .add_characteristic(
                TROUBLE_GATT_VALUE_UUID,
                &[CharacteristicProp::Read, CharacteristicProp::Write],
                0_u8,
                &mut value_storage,
            )
            .build();
        let server = AttributeServer::<NoopRawMutex, DefaultPacketPool, 10, 1>::new(table);

        let application = async {
            let mut adv_data = [0_u8; 31];
            let mut scan_data = [0_u8; 31];
            let (adv_data_len, scan_data_len) =
                encode_trouble_gatt_advertising(&mut adv_data, &mut scan_data);
            let parameters = AdvertisementParameters {
                channel_map: Some(AdvChannelMap::CHANNEL_37),
                ..Default::default()
            };

            loop {
                esp_println::println!("open-radio: Trouble advertising");
                let advertiser = peripheral
                    .advertise(
                        &parameters,
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &adv_data[..adv_data_len],
                            scan_data: &scan_data[..scan_data_len],
                        },
                    )
                    .await
                    .unwrap_or_else(|error| panic!("Trouble advertising failed: {:?}", error));
                let connection = advertiser
                    .accept()
                    .await
                    .unwrap_or_else(|error| panic!("Trouble accept failed: {:?}", error));
                let connection = connection
                    .with_attribute_server(&server)
                    .unwrap_or_else(|error| panic!("Trouble GATT attach failed: {:?}", error));
                esp_println::println!("open-radio: Trouble GATT connected");

                loop {
                    match connection.next().await {
                        GattConnectionEvent::Disconnected { reason } => {
                            esp_println::println!(
                                "open-radio: Trouble GATT disconnected reason={:?}",
                                reason
                            );
                            break;
                        }
                        GattConnectionEvent::Gatt {
                            event: GattEvent::Write(event),
                        } if event.handle() == value.handle => {
                            event
                                .accept()
                                .expect("the writable characteristic accepts writes")
                                .send()
                                .await;
                            let current: u8 = connection
                                .get(&value)
                                .expect("the characteristic value remains typed");
                            esp_println::println!("open-radio: Trouble GATT value={}", current);
                        }
                        _ => {}
                    }
                }
            }
        };

        let mut hardware = core::pin::pin!(hardware_runner.run());
        match embassy_futures::select::select3(host_runner.run(), application, hardware.as_mut())
            .await
        {
            embassy_futures::select::Either3::First(result) => {
                panic!("Trouble Host runner stopped: {:?}", result)
            }
            embassy_futures::select::Either3::Second(never) => match never {},
            embassy_futures::select::Either3::Third(never) => match never {},
        }
    }

    #[cfg(not(feature = "trouble-gatt"))]
    let BluetoothSystem { hci, runners } = output.system;
    #[cfg(not(feature = "trouble-gatt"))]
    let hardware_runner = runners.hardware;
    #[cfg(not(feature = "trouble-gatt"))]
    esp_println::println!("open-radio: Bluetooth Controller ready");

    #[cfg(all(feature = "advertising-smoke", not(feature = "trouble-gatt")))]
    let commands = async {
        advertising_smoke(&hci).await;
        core::future::pending::<()>().await;
    };
    #[cfg(not(any(feature = "advertising-smoke", feature = "trouble-gatt")))]
    let commands = async {
        const LE_TEST_CHANNEL: u8 = 0;
        const LE_PHY_1M: u8 = 1;
        const LE_MODULATION_INDEX_STANDARD: u8 = 0;
        const LE_PAYLOAD_PRBS9: u8 = 0;
        const LE_TX_PAYLOAD_LENGTH: u8 = 37;
        esp_println::println!("open-radio: HCI Reset submitted");
        match with_timeout(HCI_COMMAND_TIMEOUT, Reset::new().exec(&hci)).await {
            Ok(Ok(_)) => esp_println::println!("open-radio: HCI Reset complete"),
            Ok(Err(_)) => panic!("typed HCI Reset failed"),
            Err(_) => panic!("typed HCI Reset timed out"),
        }

        esp_println::println!("open-radio: LE Receiver Test v2 submitted");
        match with_timeout(
            HCI_COMMAND_TIMEOUT,
            LeReceiverTestV2::new(LE_TEST_CHANNEL, LE_PHY_1M, LE_MODULATION_INDEX_STANDARD)
                .exec(&hci),
        )
        .await
        {
            Ok(Ok(_)) => esp_println::println!("open-radio: LE Receiver Test v2 running"),
            Ok(Err(_)) => panic!("typed HCI LE Receiver Test v2 failed"),
            Err(_) => panic!("typed HCI LE Receiver Test v2 timed out"),
        }
        Timer::after(LE_TEST_DWELL).await;
        esp_println::println!("open-radio: LE Receiver Test End submitted");
        let received_packets =
            match with_timeout(HCI_COMMAND_TIMEOUT, LeTestEnd::new().exec(&hci)).await {
                Ok(Ok(received_packets)) => received_packets,
                Ok(Err(_)) => panic!("typed HCI LE Test End after receiver test failed"),
                Err(_) => panic!("typed HCI LE Test End after receiver test timed out"),
            };
        esp_println::println!(
            "open-radio: LE Receiver Test v2 received_packets={}",
            received_packets
        );

        esp_println::println!("open-radio: LE Transmitter Test v2 submitted");
        match with_timeout(
            HCI_COMMAND_TIMEOUT,
            LeTransmitterTestV2::new(
                LE_TEST_CHANNEL,
                LE_TX_PAYLOAD_LENGTH,
                LE_PAYLOAD_PRBS9,
                LE_PHY_1M,
            )
            .exec(&hci),
        )
        .await
        {
            Ok(Ok(_)) => esp_println::println!("open-radio: LE Transmitter Test v2 running"),
            Ok(Err(_)) => panic!("typed HCI LE Transmitter Test v2 failed"),
            Err(_) => panic!("typed HCI LE Transmitter Test v2 timed out"),
        }
        Timer::after(LE_TEST_DWELL).await;
        esp_println::println!("open-radio: LE Transmitter Test End submitted");
        let transmitter_packet_count =
            match with_timeout(HCI_COMMAND_TIMEOUT, LeTestEnd::new().exec(&hci)).await {
                Ok(Ok(packet_count)) => packet_count,
                Ok(Err(_)) => panic!("typed HCI LE Test End after transmitter test failed"),
                Err(_) => panic!("typed HCI LE Test End after transmitter test timed out"),
            };
        if transmitter_packet_count != 0 {
            panic!("LE Transmitter Test must end with packet count zero");
        }
        esp_println::println!("open-radio: LE Transmitter Test v2 complete");

        core::future::pending::<()>().await;
    };
    #[cfg(not(feature = "trouble-gatt"))]
    let mut hardware = core::pin::pin!(hardware_runner.run());
    #[cfg(not(feature = "trouble-gatt"))]
    let (_commands, _events, hardware_never) =
        embassy_futures::join::join3(commands, pump_unsolicited_hci(&hci), hardware.as_mut()).await;
    #[cfg(not(feature = "trouble-gatt"))]
    match hardware_never {}
}

#[cfg(feature = "advertising-smoke")]
async fn advertising_command<E: core::fmt::Debug>(
    name: &'static str,
    command: impl core::future::Future<Output = Result<(), E>>,
) {
    esp_println::println!("open-radio: advertising {} submitted", name);
    match with_timeout(HCI_COMMAND_TIMEOUT, command).await {
        Ok(Ok(())) => esp_println::println!("open-radio: advertising {} complete", name),
        Ok(Err(error)) => panic!("advertising {} failed: {:?}", name, error),
        Err(_) => panic!("advertising {} timed out", name),
    }
}

#[cfg(feature = "advertising-smoke")]
async fn configure_advertising(hci: &BluetoothHost, case: AdvertisingSmokeCase) {
    // HCI address bytes are least-significant first; C2 makes this static random.
    advertising_command(
        "Set Random Address",
        LeSetRandomAddr::new(BdAddr::new([0x31, 0x53, 0x50, 0x45, 0x52, 0xc2])).exec(hci),
    )
    .await;
    advertising_command("Set Parameters", case.parameters().exec(hci)).await;
    // Complete local name, suitable for either advertising kind.
    let name = b"open-radio";
    let mut data = [0; 31];
    data[0] = (name.len() + 1) as u8;
    data[1] = 0x09;
    data[2..2 + name.len()].copy_from_slice(name);
    advertising_command(
        "Set Data",
        LeSetAdvData::new((name.len() + 2) as u8, data).exec(hci),
    )
    .await;
}

#[cfg(feature = "advertising-smoke")]
async fn advertising_dwell() {
    esp_println::println!("open-radio: advertising dwell started");
    Timer::after(LE_TEST_DWELL).await;
    esp_println::println!("open-radio: advertising dwell complete");
}

#[cfg(feature = "advertising-smoke")]
async fn advertising_smoke(hci: &BluetoothHost) {
    advertising_command("initial Reset", Reset::new().exec(hci)).await;
    for case in AdvertisingSmokeCase::ALL {
        let label = case.label();
        esp_println::println!("open-radio: advertising {} smoke started", label);
        configure_advertising(hci, case).await;
        advertising_command("Enable", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("Disable", LeSetAdvEnable::new(false).exec(hci)).await;
        advertising_command("re-enable", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("active Reset", Reset::new().exec(hci)).await;
        configure_advertising(hci, case).await;
        advertising_command("Enable after Reset", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("final Disable", LeSetAdvEnable::new(false).exec(hci)).await;
        esp_println::println!("open-radio: advertising {} smoke complete", label);
    }
    esp_println::println!("open-radio: advertising smoke complete (HCI lifecycle only)");
}

#[cfg(not(feature = "trouble-gatt"))]
async fn pump_unsolicited_hci(hci: &BluetoothHost) {
    let mut buffer = match hci.alloc_buf() {
        Ok(buffer) => buffer,
        Err(_) => panic!("Bluetooth HCI receive-buffer allocation failed"),
    };
    loop {
        match hci.read(&mut buffer).await {
            Ok(_) => esp_println::println!("open-radio: unsolicited Controller packet"),
            Err(_) => panic!("Bluetooth HCI transport failed"),
        }
    }
}
