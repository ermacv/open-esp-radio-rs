#![no_main]
#![no_std]
#![recursion_limit = "256"]

use esp_backtrace as _;

use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};

use oer::wifi::{MonitorRequest, WifiChannel, WifiMacAddress, WifiMonitorConfig};

use oer_esp32s31_executor_embassy::{self as platform_executor, Executor};

use oer::systems::esp32s31::embassy::wifi::{
    self as integration, ConcurrentPartitions, DeadlineBudget, DeadlineWatchdog, EspHalRadioClocks,
    EspHalRadioPlatform, EspHalWifiPlatform, RadioConfig, RadioHardware, SharedRadio,
    WatchdogConfig, WifiParts, WifiStarted,
};

use static_cell::StaticCell;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
// The shared radio outlives every client and its periodic PHY tracking task.
static RADIO: StaticCell<SharedRadio> = StaticCell::new();
// The entropy source owns RNG hardware for the entire process. It must not be
// dropped while the radio keeps the nested `Trng` owner across await points.
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // SAFETY: the common stage-two entry runs after the board bootstrap,
    // with global interrupts disabled and the PSRAM mapping intact.
    let _psram = unsafe { oer_esp32s31_platform_runtime::adopt_psram(peripherals.PSRAM) };

    static WATCHDOG: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let watchdog = WATCHDOG.init(DeadlineWatchdog::new(peripherals.TIMG1));
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    platform_executor::init(OneShotTimer::new(timer_group.timer0));
    TRNG_SOURCE.init(TrngSource::new(peripherals.RNG));
    let trng = Trng::try_new().expect("ESP32-S31 TRNG must have a unique owner");
    let wifi_platform = EspHalWifiPlatform::new(peripherals.WIFI);
    let radio_platform = EspHalRadioPlatform::new(
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let identity = radio_platform.phy_calibration_identity();
    let hardware =
        RadioHardware::take().expect("ESP32-S31 radio hardware must have a unique owner");
    let (radio, partitions) =
        SharedRadio::new(hardware, radio_platform, EspHalRadioClocks::new(), identity);
    let radio = RADIO.init(radio);
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // SAFETY: timer and executor handlers are now bound on CPU0, and the staged
    // handoff has kept MIE clear since `adopt_psram`.
    unsafe { oer_esp32s31_platform_runtime::enable_interrupts_after_handoff() };
    executor.run(|spawner| {
        spawner.spawn(
            monitor_task(spawner, radio, partitions, wifi_platform, trng, watchdog)
                .expect("monitor task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn monitor_task(
    spawner: embassy_executor::Spawner,
    radio: &'static SharedRadio,
    partitions: ConcurrentPartitions,
    wifi_platform: EspHalWifiPlatform,
    trng: Trng,
    watchdog: &'static DeadlineWatchdog,
) {
    let mut station = [0; 6];
    station.copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point = [0; 6];
    access_point
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    // Board-selected engineering limits, not qualified timing bounds.
    use core::num::NonZeroU32;
    static WATCHDOG_CONFIG: StaticCell<WatchdogConfig> = StaticCell::new();
    let watchdog = WATCHDOG_CONFIG.init(WatchdogConfig::new(
        watchdog,
        DeadlineBudget::from_micros(NonZeroU32::new(5_000_000).unwrap()),
        DeadlineBudget::from_micros(NonZeroU32::new(1_000_000).unwrap()),
    ));
    let config = RadioConfig::new(
        watchdog,
        WifiMacAddress::new(station).expect("station MAC must be unicast"),
        WifiMacAddress::new(access_point).expect("AP MAC must be unicast"),
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    );
    spawner.spawn(tracking_task(radio).expect("PHY tracking task storage is available once"));
    let ConcurrentPartitions {
        wifi: partition, ..
    } = partitions;
    let WifiStarted {
        wifi,
        initialization: _,
        runner: radio_runner,
    } = integration::await_stack_boundary!(integration::new(
        radio,
        partition,
        wifi_platform,
        trng,
        config
    ))
    .expect("Wi-Fi initialization must succeed once");
    spawner.spawn(radio_task(spawner, radio_runner).expect("radio task storage is available once"));
    let WifiParts {
        control: wifi,
        station_device: _,
        access_point_device: _,
        monitor_frames: frames,
        station_status: _,
        access_point_status: _,
    } = wifi.into_parts();
    let application = async move {
        let _monitor = wifi
            .start_monitor(MonitorRequest::new(
                WifiChannel::mhz20(6).expect("fixed monitor channel is valid"),
                WifiMonitorConfig::normalized(),
            ))
            .await
            .expect("monitor role must start");
        let mut count = 0_u64;
        let mut bytes = 0_u64;
        loop {
            let frame = frames.receive().await;
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(frame.captured_length() as u64);
            if count.is_multiple_of(512) {
                let metadata = frame.metadata();
                esp_println::println!(
                    "open-radio-monitor: frames={} bytes={} channel={:?} rssi={:?}",
                    count,
                    bytes,
                    metadata.rx.channel,
                    metadata.rx.rssi_dbm,
                );
            }
        }
    };
    application.await;
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the sole radio runner enters its static task arena once; the final ELF frame audit bounds CPU stack use"
)]
async fn radio_task(spawner: embassy_executor::Spawner, runner: integration::SystemRunner) {
    runner.run(spawner).await;
}

/// ESP-IDF's periodic `phy_track_pll` timer for the shared radio.
#[embassy_executor::task]
async fn tracking_task(radio: &'static SharedRadio) {
    let error = radio.run_tracking().await;
    panic!("shared PHY tracking failed: {error:?}");
}
