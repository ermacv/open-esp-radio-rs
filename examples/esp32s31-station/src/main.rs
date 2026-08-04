#![no_main]
#![no_std]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterruptControl,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};
use open_esp_radio_esp32s31_embassy_runtime::Executor;
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use static_cell::StaticCell;

mod connected;
mod station;

esp_bootloader_esp_idf::esp_app_desc!();

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();

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

    let executor = EXECUTOR.init(Executor::new(software_interrupts.software_interrupt0));
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
    station::run(spawner, radio, trng).await
}
