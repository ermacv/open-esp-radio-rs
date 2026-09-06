#![no_main]
#![no_std]
#![recursion_limit = "256"]

#[cfg(any(
    all(feature = "owned-network", feature = "compat-network"),
    all(feature = "upstream-network", feature = "owned-network"),
    all(feature = "upstream-network", feature = "compat-network")
))]
compile_error!(
    "select exactly one network integration: upstream-network, owned-network or compat-network"
);
#[cfg(not(any(
    feature = "owned-network",
    feature = "compat-network",
    feature = "upstream-network"
)))]
compile_error!(
    "select exactly one network integration: upstream-network, owned-network or compat-network"
);

use core::num::NonZeroU16;

use embassy_executor::Spawner;

#[cfg(feature = "compat-network")]
use embassy_net_compat as embassy_net;
#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;
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
use open_esp_radio_esp32s31_embassy_runtime::{self as platform_executor, Executor};
use open_esp_radio_esp32s31_embassy_wifi::{
    self as integration, Esp32s31RadioConfig as RadioConfig, Esp32s31RadioParts as RadioParts,
    Esp32s31RadioRunners as RadioRunners, Esp32s31RadioSystem as RadioSystem,
    Esp32s31WifiParts as WifiParts,
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

use oer::wifi::{
    Pmk, StaAssociationPreference, StaReconnectPolicy, StationRequest, StationScanChannels,
    StationScanPolicy, StationSecurity, WifiChannel, WifiMacAddress, WifiScanRequest, WifiSsid,
};
use open_esp_radio as oer;
use open_esp_radio_esp32s31_phy::{PhyCalibrationIdentity, analog::rfpll::phy_get_rf_cal_version};

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
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
    let _psram = unsafe { oer_esp32s31_runtime::adopt_psram(peripherals.PSRAM) };

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    platform_executor::init(OneShotTimer::new(timer_group.timer0));

    TRNG_SOURCE.init(TrngSource::new(peripherals.RNG));
    let trng = Trng::try_new().expect("ESP32-S31 TRNG must have a unique owner");
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

    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // Timer and executor handlers are now bound; the staged handoff kept MIE clear.
    unsafe { core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack)) };
    executor.run(|spawner| {
        let task = station_task(spawner, radio, trng)
            .expect("station task storage must be available once");
        spawner.spawn(task);
    })
}

#[embassy_executor::task]
async fn station_task(spawner: Spawner, radio: EspHalRadioPeripheral, trng: Trng) {
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
    let mut calibration_base_mac_address = [0; 6];
    calibration_base_mac_address.copy_from_slice(efuse::base_mac_address().as_bytes());
    let ssid = WifiSsid::new(STA_SSID.as_bytes()).expect("station SSID must be valid");
    let pmk = Pmk::derive(STA_PASSPHRASE.as_bytes(), ssid.as_bytes())
        .expect("station credentials must be valid");
    let request = StationRequest::new(
        ssid,
        StationSecurity::wpa2_personal(pmk),
        StaReconnectPolicy::new(3, 100, 1_000, 100)
            .expect("station reconnect policy must be valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(200).expect("scan dwell is nonzero"),
            StaAssociationPreference::PreferHe20,
        ),
    );
    let config = RadioConfig::new(
        station_mac,
        access_point_mac,
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            base_mac_address: calibration_base_mac_address,
            mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
        },
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    );
    let RadioSystem { radio, runners } =
        open_esp_radio_wifi_embassy::await_stack_boundary!(integration::new(radio, trng, config))
            .expect("radio initialization must succeed once");
    let RadioRunners {
        hardware: radio_runner,
    } = runners;
    spawner.spawn(radio_task(spawner, radio_runner).expect("radio task storage is available once"));
    let RadioParts {
        wifi,
        initialization: _,
    } = radio.into_parts();
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
async fn radio_task(spawner: embassy_executor::Spawner, runner: integration::Esp32s31RadioRunner) {
    runner.run(spawner).await;
}
