#![no_main]
#![no_std]
#![recursion_limit = "256"]

use bt_hci::{
    cmd::{
        SyncCmd,
        controller_baseband::Reset,
        le::{LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
    },
    controller::Controller,
};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterruptControl,
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmRuntimeConfig, BluetoothRadioHardware,
};
use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothDtmRecheckPeriod;
use open_esp_radio_esp32s31_bluetooth_integration::{
    Esp32s31BluetoothColdStartConfig, Esp32s31BluetoothHostController, Esp32s31BluetoothSystem,
    Esp32s31BluetoothSystemStorage, start_esp32s31_bluetooth,
};
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerAllocationConfig;
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use open_esp_radio_esp32s31_radio_platform_esp_hal::{
    EspHalBluetoothPlatform, EspHalRadioPlatform,
};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const MODEM_TIMER_CAPACITY: usize = 4;
const SCHEDULER_CAPACITY: usize = 1;
const HOST_TO_CONTROLLER_DEPTH: usize = 4;
const CONTROLLER_TO_HOST_DEPTH: usize = 4;
const PACKET_CAPACITY: usize = 258;
const LE_TEST_CHANNEL: u8 = 0;
const LE_PHY_1M: u8 = 1;
const LE_MODULATION_INDEX_STANDARD: u8 = 0;
const LE_PAYLOAD_PRBS9: u8 = 0;
const LE_TX_PAYLOAD_LENGTH: u8 = 37;
const LE_TEST_DWELL: Duration = Duration::from_secs(1);

type BluetoothStorage = Esp32s31BluetoothSystemStorage<
    EspHalBluetoothPlatform<'static>,
    MODEM_TIMER_CAPACITY,
    SCHEDULER_CAPACITY,
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;
type BluetoothHost = Esp32s31BluetoothHostController<
    HOST_TO_CONTROLLER_DEPTH,
    CONTROLLER_TO_HOST_DEPTH,
    PACKET_CAPACITY,
>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_PLATFORM: StaticCell<EspHalRadioPlatform> = StaticCell::new();
static BLUETOOTH_STORAGE: BluetoothStorage = BluetoothStorage::new();

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    open_esp_radio_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));
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
    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupts.software_interrupt0));
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
    let dtm = BluetoothDtmRuntimeConfig::new(
        BluetoothDtmSchedulerAllocationConfig::new(0, 0, 0),
        BluetoothDtmDefaultTxPowerDbm::new(0),
    );
    let recheck_period = EmbassyBluetoothDtmRecheckPeriod::from_duration(Duration::from_micros(50))
        .expect("the Controller-time recheck period must be nonzero");
    let config = Esp32s31BluetoothColdStartConfig::new(251, 4, None, dtm, recheck_period);
    let output =
        match start_esp32s31_bluetooth(platform, hardware, &BLUETOOTH_STORAGE, config).await {
            Ok(output) => output,
            Err(_) => panic!("Bluetooth Controller cold start failed"),
        };
    let Esp32s31BluetoothSystem { hci, runners } = output.system;
    let hardware_runner = runners.hardware;
    esp_println::println!("open-radio: Bluetooth Controller ready");

    let commands = async {
        if Reset::new().exec(&hci).await.is_err() {
            panic!("typed HCI Reset failed");
        }

        if LeReceiverTestV2::new(LE_TEST_CHANNEL, LE_PHY_1M, LE_MODULATION_INDEX_STANDARD)
            .exec(&hci)
            .await
            .is_err()
        {
            panic!("typed HCI LE Receiver Test v2 failed");
        }
        Timer::after(LE_TEST_DWELL).await;
        let received_packets = match LeTestEnd::new().exec(&hci).await {
            Ok(received_packets) => received_packets,
            Err(_) => panic!("typed HCI LE Test End after receiver test failed"),
        };
        esp_println::println!(
            "open-radio: LE Receiver Test v2 received_packets={}",
            received_packets
        );

        if LeTransmitterTestV2::new(
            LE_TEST_CHANNEL,
            LE_TX_PAYLOAD_LENGTH,
            LE_PAYLOAD_PRBS9,
            LE_PHY_1M,
        )
        .exec(&hci)
        .await
        .is_err()
        {
            panic!("typed HCI LE Transmitter Test v2 failed");
        }
        Timer::after(LE_TEST_DWELL).await;
        let transmitter_packet_count = match LeTestEnd::new().exec(&hci).await {
            Ok(packet_count) => packet_count,
            Err(_) => panic!("typed HCI LE Test End after transmitter test failed"),
        };
        if transmitter_packet_count != 0 {
            panic!("LE Transmitter Test must end with packet count zero");
        }
        esp_println::println!("open-radio: LE Transmitter Test v2 complete");

        core::future::pending::<()>().await;
    };
    let (_commands, _events, hardware_never) =
        embassy_futures::join::join3(commands, pump_unsolicited_hci(&hci), hardware_runner.run())
            .await;
    match hardware_never {}
}

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
