#![no_main]
#![no_std]

use core::num::NonZeroU16;

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterruptControl,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

use open_esp_radio::{
    StationRequest, StationScanChannels, StationScanPolicy, StationSecurity, WifiMacAddress,
    WifiSsid,
    esp32s31::phy::{PhyCalibrationIdentity, phy_rfpll::phy_get_rf_cal_version},
    wifi::{
        ieee80211::{channel::WifiChannel, station::StaAssociationPreference},
        sta::station::StaReconnectPolicy,
        wpa2::Pmk,
    },
};

esp_bootloader_esp_idf::esp_app_desc!();

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();

const STA_SSID: &str = match option_env!("ESP32S31_WIFI_SSID") {
    Some(value) => value,
    None => "",
};
const STA_PASSPHRASE: &str = match option_env!("ESP32S31_WIFI_PASSPHRASE") {
    Some(value) => value,
    None => "",
};

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    open_esp_radio_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));

    let trng_source = TrngSource::new(peripherals.RNG);
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

    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupts.software_interrupt0));
    executor.run(|spawner| {
        let task = station_task(spawner, radio, trng, trng_source)
            .expect("station task storage must be available once");
        spawner.spawn(task);
    })
}

#[embassy_executor::task]
async fn station_task(
    spawner: Spawner,
    radio: EspHalRadioPeripheral,
    trng: Trng,
    _trng_source: TrngSource<'static>,
) {
    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
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
    let config = open_esp_radio_esp32s31_embassy_wifi::Esp32s31StationServiceConfig::new(
        station_mac,
        access_point_mac,
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
            mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
        },
        WifiChannel::mhz20(1).expect("initial channel is valid"),
        request,
    );
    open_esp_radio_esp32s31_embassy_wifi::run(
        spawner,
        radio,
        trng,
        config,
        |mut network| async move {
            let _stack = network.wait_connected().await;
            core::future::pending::<()>().await;
        },
    )
    .await
}
