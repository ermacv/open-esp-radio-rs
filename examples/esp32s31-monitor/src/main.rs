#![no_main]
#![no_std]

use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    interrupt::software::SoftwareInterruptControl,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio::{
    MonitorRequest, WifiMacAddress, WifiMonitorConfig,
    esp32s31::phy::{PhyCalibrationIdentity, phy_rfpll::phy_get_rf_cal_version},
    wifi::ieee80211::channel::WifiChannel,
};
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use open_esp_radio_esp32s31_embassy_wifi::{Esp32s31RadioConfig, new};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
// The entropy source owns RNG hardware for the entire process. It must not be
// dropped while the radio keeps the nested `Trng` owner across await points.
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let software_interrupts = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    open_esp_radio_esp32s31_embassy_runtime::init(OneShotTimer::new(timer_group.timer0));
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
    let executor = EXECUTOR.init(Executor::<0>::new(software_interrupts.software_interrupt0));
    executor.run(|spawner| {
        spawner.spawn(
            monitor_task(spawner, radio, trng)
                .expect("monitor task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn monitor_task(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPeripheral,
    trng: Trng,
) {
    let efuse_registers = esp_hal::peripherals::EFUSE::regs();
    let mut station = [0; 6];
    station.copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point = [0; 6];
    access_point
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let config = Esp32s31RadioConfig::new(
        WifiMacAddress::new(station).expect("station MAC must be unicast"),
        WifiMacAddress::new(access_point).expect("AP MAC must be unicast"),
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            mac_sys0: efuse_registers.rd_mac_sys0().read().bits(),
            mac_sys1: efuse_registers.rd_mac_sys1().read().bits(),
        },
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    );
    let (radio, runner) = new(spawner.make_send(), platform, trng, config)
        .await
        .expect("radio initialization must succeed once");
    let open_esp_radio_esp32s31_embassy_wifi::Esp32s31RadioParts {
        wifi,
        initialization: _,
    } = radio.into_parts();
    let open_esp_radio_esp32s31_embassy_wifi::Esp32s31WifiParts {
        control: wifi,
        device: _,
        monitor_frames: frames,
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
    let (_application, never) = embassy_futures::join::join(application, runner.run()).await;
    match never {}
}
