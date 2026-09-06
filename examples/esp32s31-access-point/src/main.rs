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
use open_esp_radio as oer;
use open_esp_radio_esp32s31_access_point::{dhcp, network, services};
use open_esp_radio_esp32s31_embassy_runtime::{self as platform_executor, Executor};
use open_esp_radio_esp32s31_embassy_wifi::{
    self as integration, Esp32s31RadioConfig as RadioConfig, Esp32s31RadioParts as RadioParts,
    Esp32s31RadioRunners as RadioRunners, Esp32s31RadioSystem as RadioSystem,
    Esp32s31WifiParts as WifiParts,
};
use open_esp_radio_esp32s31_phy::{PhyCalibrationIdentity, analog::rfpll::phy_get_rf_cal_version};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
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
        spawner.spawn(
            access_point_task(spawner, radio, trng)
                .expect("access-point task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn access_point_task(spawner: Spawner, radio: EspHalRadioPeripheral, trng: Trng) {
    let mut station_address = [0; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let station_mac = WifiMacAddress::new(station_address).expect("valid station MAC in eFuse");
    let access_point_mac =
        WifiMacAddress::new(access_point_address).expect("valid AP MAC in eFuse");
    let mut calibration_base_mac_address = [0; 6];
    calibration_base_mac_address.copy_from_slice(efuse::base_mac_address().as_bytes());
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
    let config = RadioConfig::new(
        station_mac,
        access_point_mac,
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            base_mac_address: calibration_base_mac_address,
            mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
        },
        WifiChannel::mhz20(AP_CHANNEL).expect("initial channel must be valid"),
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
    open_esp_radio_wifi_embassy::await_stack_boundary!(network::run(
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
async fn radio_task(spawner: embassy_executor::Spawner, runner: integration::Esp32s31RadioRunner) {
    runner.run(spawner).await;
}
