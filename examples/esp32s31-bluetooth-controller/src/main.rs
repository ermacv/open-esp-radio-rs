#![no_main]
#![no_std]
#![recursion_limit = "256"]

use bt_hci::{
    cmd::{SyncCmd, controller_baseband::Reset, le::LeTestEnd},
    controller::Controller,
};
use embassy_time::Duration;
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
        let received_packets = match LeTestEnd::new().exec(&hci).await {
            Ok(received_packets) => received_packets,
            Err(_) => panic!("typed HCI LE Test End failed"),
        };
        esp_println::println!(
            "open-radio: idle LE Test End received_packets={}",
            received_packets
        );
        core::future::pending::<()>().await;
    };
    let (_commands, _events, hardware_never) =
        embassy_futures::join::join3(commands, pump_unsolicited_hci(&hci), hardware_runner.run())
            .await;
    match hardware_never {}
}

async fn pump_unsolicited_hci(hci: &BluetoothHost) {
    let mut buffer = [0; PACKET_CAPACITY];
    loop {
        match hci.read(&mut buffer).await {
            Ok(_) => esp_println::println!("open-radio: unsolicited Controller packet"),
            Err(_) => panic!("Bluetooth HCI transport failed"),
        }
    }
}
