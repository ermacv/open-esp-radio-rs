#![no_main]
#![no_std]
#![recursion_limit = "256"]

use embassy_executor::Spawner;

use esp_backtrace as _;

use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};

use oer::wifi::{
    AccessPointClientLimit, AccessPointRequest, AccessPointSecurity, Pmk, WifiChannel,
    WifiMacAddress, WifiSsid,
};

use oer_esp32s31_example_access_point::{dhcp, network, services};

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
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();

const AP_SSID: &str = match option_env!("ESP32S31_AP_SSID") {
    Some(value) => value,
    None => "open-esp-radio",
};
const AP_PASSPHRASE: &str = match option_env!("ESP32S31_AP_PASSPHRASE") {
    Some(value) => value,
    None => "open-radio-password",
};
const AP_CHANNEL: u8 = 6;
const AP_CLIENT_LIMIT: u8 = 4;

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
            access_point_task(spawner, radio, partitions, wifi_platform, trng, watchdog)
                .expect("access-point task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn access_point_task(
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
    let station_mac = WifiMacAddress::new(station_address).expect("valid station MAC in eFuse");
    let access_point_mac =
        WifiMacAddress::new(access_point_address).expect("valid AP MAC in eFuse");
    let ssid = WifiSsid::new(AP_SSID.as_bytes()).expect("AP SSID must be valid");
    let pmk = Pmk::derive(AP_PASSPHRASE.as_bytes(), ssid.as_bytes())
        .expect("AP passphrase must be valid WPA2-Personal input");
    let request = AccessPointRequest::new(
        ssid,
        AccessPointSecurity::wpa2_personal(pmk),
        WifiChannel::mhz20(AP_CHANNEL).expect("AP channel must be valid"),
        AccessPointClientLimit::new(AP_CLIENT_LIMIT).expect("AP client limit must be valid"),
    )
    .expect("AP request must be supported");
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
        WifiChannel::mhz20(AP_CHANNEL).expect("initial channel must be valid"),
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
        station_device: _,
        access_point_device,
        monitor_frames: _,
        station_status: _,
        mut access_point_status,
    } = wifi.into_parts();
    let seed = u64::from_le_bytes([
        access_point_address[0],
        access_point_address[1],
        access_point_address[2],
        access_point_address[3],
        access_point_address[4],
        access_point_address[5],
        0xa5,
        0x31,
    ]);
    integration::await_stack_boundary!(network::run(
        access_point_device,
        seed,
        |stack| async move {
            let access_point = async move {
                let active = wifi
                    .start_access_point(request)
                    .await
                    .expect("AP must start");
                esp_println::println!(
                    "open-radio: AP active generation={}",
                    active.generation().value()
                );
                let _active = active;
                core::future::pending::<()>().await;
            };
            let status = async move {
                loop {
                    let snapshot = access_point_status.changed().await;
                    esp_println::println!(
                        "open-radio: AP generation={:?} associated={} authorized={}/{}",
                        snapshot.generation,
                        snapshot.associated,
                        snapshot.authorized,
                        snapshot.client_limit,
                    );
                }
            };
            let echoes = async move {
                let (_udp, _tcp) = embassy_futures::join::join(
                    services::udp_echo(stack),
                    services::tcp_echo(stack),
                )
                .await;
            };
            let (_ap, _status, _dhcp, _echoes) =
                embassy_futures::join::join4(access_point, status, dhcp::run(stack), echoes).await;
        }
    ));
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
