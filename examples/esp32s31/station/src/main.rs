#![no_main]
#![no_std]
#![recursion_limit = "256"]

#[cfg(any(
    all(feature = "owned-network", feature = "embassy-network"),
    all(feature = "upstream-network", feature = "owned-network"),
    all(feature = "upstream-network", feature = "embassy-network")
))]
compile_error!(
    "select exactly one network integration: upstream-network, owned-network or embassy-network"
);
#[cfg(not(any(
    feature = "owned-network",
    feature = "embassy-network",
    feature = "upstream-network"
)))]
compile_error!(
    "select exactly one network integration: upstream-network, owned-network or embassy-network"
);

use core::num::NonZeroU16;

use embassy_executor::Spawner;

#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;
#[cfg(feature = "embassy-network")]
use embassy_net_released as embassy_net;
#[cfg(feature = "upstream-network")]
use embassy_net_upstream as embassy_net;

use esp_backtrace as _;

use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};

use oer::wifi::{
    Preference, StaReconnectPolicy, StationRequest, StationScanChannels, StationScanPolicy,
    StationSecurity, WifiChannel, WifiMacAddress, WifiScanRequest, WifiSsid,
};

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
// Socket/IP state belongs to the application, not to the radio driver. Static
// placement avoids moving the stack arena through the executor task frame.
mod network;

const STA_SSID: &str = match option_env!("ESP32S31_WIFI_SSID") {
    Some(value) => value,
    None => "",
};
const STA_PASSPHRASE: &str = match option_env!("ESP32S31_WIFI_PASSPHRASE") {
    Some(value) => value,
    None => "",
};

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
        let task = station_task(spawner, radio, partitions, wifi_platform, trng, watchdog)
            .expect("station task storage must be available once");
        spawner.spawn(task);
    })
}

#[embassy_executor::task]
async fn station_task(
    spawner: Spawner,
    radio: &'static SharedRadio,
    partitions: ConcurrentPartitions,
    wifi_platform: EspHalWifiPlatform,
    trng: Trng,
    watchdog: &'static DeadlineWatchdog,
) {
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address)
        .expect("ESP32-S31 eFuse must contain a unicast station address");
    let access_point_mac = WifiMacAddress::new(access_point_address)
        .expect("ESP32-S31 eFuse must contain a unicast access-point address");
    let ssid = WifiSsid::new(STA_SSID.as_bytes()).expect("station SSID must be valid");
    let security = StationSecurity::wpa2_personal(STA_PASSPHRASE.as_bytes(), ssid.as_bytes())
        .expect("station credentials must be valid");
    let request = StationRequest::new(
        ssid,
        security,
        StaReconnectPolicy::new(3, 100, 1_000, 100)
            .expect("station reconnect policy must be valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(200).expect("scan dwell is nonzero"),
            Preference::PreferHe20,
        ),
    );
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
        station_mac,
        access_point_mac,
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    );
    spawner.spawn(tracking_task(radio).expect("PHY tracking task storage is available once"));
    spawner.spawn(coex_schedule_task(radio).expect("coexistence schedule task storage is available once"));
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
        station_device,
        access_point_device: _,
        monitor_frames: _,
        station_status: _,
        access_point_status: _,
    } = wifi.into_parts();
    let network = network::run(
        station_device,
        u64::from_le_bytes([
            station_address[0],
            station_address[1],
            station_address[2],
            station_address[3],
            station_address[4],
            station_address[5],
            0xa5,
            0x31,
        ]),
    );
    let application = async move {
        let completed = wifi
            .scan(WifiScanRequest::new(
                StationScanChannels::CHANNELS_1_TO_13,
                NonZeroU16::new(50).expect("standalone scan dwell is nonzero"),
            ))
            .await
            .unwrap_or_else(|error| panic!("standalone scan failed: {error:?}"));
        let (wifi, report) = completed.into_parts();
        esp_println::println!(
            "open-radio: scan generation={} networks={}",
            report.generation().value(),
            report.results().len(),
        );
        match wifi.start_station(request).await {
            Ok(station) => {
                esp_println::println!(
                    "open-radio: station active generation={}",
                    station.generation().value(),
                );
                let _station = station;
            }
            Err(error) => esp_println::println!("open-radio: station start failed: {error:?}"),
        }
        core::future::pending::<()>().await;
    };
    embassy_futures::join::join(application, network).await;
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
/// The coexistence schedule's phase timer for the shared radio.
#[embassy_executor::task]
async fn coex_schedule_task(radio: &'static SharedRadio) {
    radio.run_coex_schedule().await
}

#[embassy_executor::task]
async fn tracking_task(radio: &'static SharedRadio) {
    let error = radio.run_tracking().await;
    panic!("shared PHY tracking failed: {error:?}");
}
