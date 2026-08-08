#![no_main]
#![no_std]

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterruptControl,
    rng::TrngSource,
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio::{
    RadioConfig, WifiConfig, WifiMacAddress, WifiMonitorConfig,
    esp32s31::{
        Esp32s31RadioStartConfig, Esp32s31WifiStartConfig,
        hal::Radio,
        phy::{NoopPhyTargetObserver, PhyCalibrationIdentity, phy_rfpll::phy_get_rf_cal_version},
        start_esp32s31_radio,
        wifi::mac::rx::RxPhyInfo,
    },
    wifi::ieee80211::channel::WifiChannel,
};
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral,
    mac_interrupt_epoch::{
        EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
    },
};
use static_cell::StaticCell;

use open_esp_radio::esp32s31::wifi::embassy::monitor::{
    EmbassyEsp32s31PhyDelay, EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime,
    Esp32s31MonitorControlResources, Esp32s31MonitorInterrupts, Esp32s31MonitorMemory,
    Esp32s31MonitorTaskResources, Esp32s31RxDmaStorage, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, prepare_esp32s31_monitor_task,
};

esp_bootloader_esp_idf::esp_app_desc!();

const INITIAL_CHANNEL: u8 = 6;
const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const RX_DESCRIPTOR_COUNT: usize = 16;
const RX_BUFFER_SIZE: usize = 4_608;
const RX_STORAGE_SIZE: usize = RX_BUFFER_SIZE + 4;
const CAPTURE_DEPTH: usize = 8;
const CAPTURE_SLOTS: usize = 8;
const CAPTURE_CAPACITY: usize = 4_096;

type RxStorage = Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>;
type CapturePool = MonitorCapturePool<CAPTURE_CAPACITY, CAPTURE_SLOTS>;
type CaptureResources = MonitorCaptureResources<
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;
type CaptureReceiver = MonitorCaptureReceiver<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RX_STORAGE: StaticCell<RxStorage> = StaticCell::new();
static RX_BUFFER_ADDRESSES: StaticCell<[u32; RX_DESCRIPTOR_COUNT]> = StaticCell::new();
static CAPTURE_POOL: StaticCell<CapturePool> = StaticCell::new();
static CAPTURE_RESOURCES: StaticCell<CaptureResources> = StaticCell::new();
static MONITOR_CONTROL: StaticCell<Esp32s31MonitorControlResources<CriticalSectionRawMutex>> =
    StaticCell::new();
static IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> = EmbassyMacIrqRuntime::new();
static POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn mac_interrupt() {
    let _ = service_mac_interrupt(&IRQ_RUNTIME);
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn power_interrupt() {
    let _ = service_power_interrupt(&POWER_IRQ_RUNTIME);
}

#[embassy_executor::task]
async fn capture_task(receiver: CaptureReceiver) {
    let mut frames = 0_u64;
    let mut bytes = 0_u64;
    loop {
        let frame = receiver.receive().await;
        frames = frames.saturating_add(1);
        bytes = bytes.saturating_add(frame.captured_length() as u64);
        if frames.is_multiple_of(512) {
            let metadata = frame.metadata();
            esp_println::println!(
                "open-radio-monitor: frames={} bytes={} channel={:?} rssi={:?}",
                frames,
                bytes,
                metadata.rx.channel,
                metadata.rx.rssi_dbm,
            );
        }
    }
}

#[embassy_executor::task]
async fn monitor_task(
    spawner: Spawner,
    platform: EspHalRadioPeripheral,
    _trng_source: TrngSource<'static>,
    control: &'static mut Esp32s31MonitorControlResources<CriticalSectionRawMutex>,
) {
    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
    let mut station_address = [0_u8; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let station_address = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 station eFuse address must be unicast");
    let mut access_point_address = [0_u8; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let access_point_address = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 access-point eFuse address must be unicast");

    let topology = RadioConfig::wifi(WifiConfig::monitor(WifiMonitorConfig::normalized()));
    let radio = Radio::claim(platform)
        .unwrap_or_else(|_| panic!("open-radio register singleton was already claimed"));
    let calibration_identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
        mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
    };
    let started = start_esp32s31_radio::<_, EmbassyEsp32s31PhyDelay, _>(
        radio,
        Esp32s31RadioStartConfig::new(
            topology,
            Esp32s31WifiStartConfig::new(
                calibration_identity,
                WifiChannel::mhz20(INITIAL_CHANNEL).expect("fixed monitor channel is valid"),
            ),
        ),
        None,
        NoopPhyTargetObserver,
    )
    .await
    .unwrap_or_else(|_| panic!("ESP32-S31 radio start failed"));
    let monitor = started
        .try_into_standalone_monitor()
        .unwrap_or_else(|_| panic!("validated topology did not materialize a monitor"));
    let monitor = monitor
        .start_mac(
            MAC_HANDSHAKE_SAMPLE_LIMIT,
            station_address,
            access_point_address,
        )
        .unwrap_or_else(|_| panic!("common MAC start failed"));
    let (plan, wifi) = monitor.into_parts();

    let rx_storage = RX_STORAGE.init_with(RxStorage::new);
    let buffer_addresses = RX_BUFFER_ADDRESSES.init([0; RX_DESCRIPTOR_COUNT]);
    let capture_pool = CAPTURE_POOL.init_with(CapturePool::new);
    let capture_resources = CAPTURE_RESOURCES.init_with(|| CaptureResources::new(capture_pool));
    let (sink, receiver) = capture_resources.split();
    spawner.spawn(
        capture_task(receiver).expect("monitor capture task storage must be available once"),
    );

    let memory = Esp32s31MonitorMemory::new(rx_storage, buffer_addresses)
        .unwrap_or_else(|_| panic!("monitor DMA arena is not addressable"));
    let interrupts = Esp32s31MonitorInterrupts::new(
        EspHalMacInterruptRoute::new(mac_interrupt, power_interrupt),
        &IRQ_RUNTIME,
        &POWER_IRQ_RUNTIME,
    );
    let resources = Esp32s31MonitorTaskResources::new(memory, sink, interrupts, control);
    let (controller, mut task) = prepare_esp32s31_monitor_task(plan, wifi, resources)
        .unwrap_or_else(|_| panic!("standalone monitor materialization failed"));
    esp_println::println!(
        "open-radio-monitor: ready channel={} descriptors={} capture_slots={}",
        task.current_channel().primary(),
        RX_DESCRIPTOR_COUNT,
        CAPTURE_SLOTS,
    );

    // This minimal capture example has no command source. A real application
    // moves the controller into its control plane; dropping it has no hardware
    // effect because this task owns the complete role graph.
    drop(controller);
    let _ = task.run().await;
}

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    open_esp_radio_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));
    let trng_source = TrngSource::new(peripherals.RNG);
    let radio = EspHalRadioPeripheral::new(
        peripherals.WIFI,
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupts.software_interrupt0));
    executor.run(|spawner| {
        let control = MONITOR_CONTROL.init_with(Esp32s31MonitorControlResources::new);
        spawner.spawn(
            monitor_task(spawner, radio, trng_source, control)
                .expect("monitor task storage must be available once"),
        );
    })
}
