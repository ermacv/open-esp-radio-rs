#![no_main]
#![no_std]
#![recursion_limit = "256"]

#[cfg(not(feature = "advertising-smoke"))]
use bt_hci::cmd::le::{LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2};
#[cfg(feature = "advertising-smoke")]
use bt_hci::{
    cmd::le::{LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetRandomAddr},
    param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration as HciDuration},
};
use bt_hci::{
    cmd::{SyncCmd, controller_baseband::Reset},
    controller::Controller,
};
use embassy_time::{Duration, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterrupt,
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmDefaultTxPowerDbm, BluetoothDtmRuntimeConfig, BluetoothPassiveScanRuntimeConfig,
    BluetoothRadioHardware,
};
use open_esp_radio_esp32s31_bluetooth_embassy::EmbassyBluetoothDtmRecheckPeriod;
use open_esp_radio_esp32s31_bluetooth_integration::{
    Esp32s31BluetoothColdStartConfig, Esp32s31BluetoothHostController, Esp32s31BluetoothSystem,
    Esp32s31BluetoothSystemStorage, start_esp32s31_bluetooth,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmSchedulerAllocationConfig, BluetoothPassiveScanDefaultTxPowerDbm,
    BluetoothPassiveScanSchedulerAllocationConfig,
};
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
const LE_TEST_DWELL: Duration = Duration::from_secs(1);
const HCI_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

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
    esp_println::println!("open-radio: application entered");
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
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
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    esp_println::println!("open-radio: executor starting");
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
    let passive_scan = BluetoothPassiveScanRuntimeConfig::new(
        BluetoothPassiveScanSchedulerAllocationConfig::new(0, 0)
            .expect("the standalone Controller limits fit the scanner graph"),
        BluetoothPassiveScanDefaultTxPowerDbm::new(0),
    );
    let recheck_period = EmbassyBluetoothDtmRecheckPeriod::from_duration(Duration::from_micros(50))
        .expect("the Controller-time recheck period must be nonzero");
    let config =
        Esp32s31BluetoothColdStartConfig::new(251, 4, None, dtm, passive_scan, recheck_period);
    esp_println::println!("open-radio: Bluetooth Controller cold start submitted");
    let output =
        match start_esp32s31_bluetooth(platform, hardware, &BLUETOOTH_STORAGE, config).await {
            Ok(output) => output,
            Err(_) => panic!("Bluetooth Controller cold start failed"),
        };
    let Esp32s31BluetoothSystem { hci, runners } = output.system;
    let hardware_runner = runners.hardware;
    esp_println::println!("open-radio: Bluetooth Controller ready");

    #[cfg(feature = "advertising-smoke")]
    let commands = async {
        advertising_smoke(&hci).await;
        core::future::pending::<()>().await;
    };
    #[cfg(not(feature = "advertising-smoke"))]
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
    let (_commands, _events, hardware_never) =
        embassy_futures::join::join3(commands, pump_unsolicited_hci(&hci), hardware_runner.run())
            .await;
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
async fn configure_advertising(hci: &BluetoothHost, kind: AdvKind) {
    // HCI address bytes are least-significant first; C2 makes this static random.
    advertising_command(
        "Set Random Address",
        LeSetRandomAddr::new(BdAddr::new([0x31, 0x53, 0x50, 0x45, 0x52, 0xc2])).exec(hci),
    )
    .await;
    advertising_command(
        "Set Parameters",
        LeSetAdvParams::new(
            HciDuration::from_millis(100),
            HciDuration::from_millis(100),
            kind,
            AddrKind::RANDOM,
            AddrKind::PUBLIC,
            BdAddr::default(),
            AdvChannelMap::ALL,
            AdvFilterPolicy::Unfiltered,
        )
        .exec(hci),
    )
    .await;
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
    for (label, kind) in [
        ("nonconnectable", AdvKind::AdvNonconnInd),
        ("connectable", AdvKind::AdvInd),
    ] {
        esp_println::println!("open-radio: advertising {} smoke started", label);
        configure_advertising(hci, kind).await;
        advertising_command("Enable", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("Disable", LeSetAdvEnable::new(false).exec(hci)).await;
        advertising_command("re-enable", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("active Reset", Reset::new().exec(hci)).await;
        configure_advertising(hci, kind).await;
        advertising_command("Enable after Reset", LeSetAdvEnable::new(true).exec(hci)).await;
        advertising_dwell().await;
        advertising_command("final Disable", LeSetAdvEnable::new(false).exec(hci)).await;
        esp_println::println!("open-radio: advertising {} smoke complete", label);
    }
    esp_println::println!("open-radio: advertising smoke complete (HCI lifecycle only)");
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
