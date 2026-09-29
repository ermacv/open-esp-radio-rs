//! ESP32-C5 HIL runtime.
//!
//! The ESP-IDF second-stage bootloader loads this application. The
//! `boot-smoke` image reports `hal=INIT` once esp-hal has initialized the
//! chip, `embassy=START` once the esp-rtos scheduler and time driver run,
//! then waits once on an Embassy timer and reports the pass line the host
//! waits for, all on the USB Serial/JTAG console.

#![no_std]
#![no_main]

#[cfg(not(feature = "boot-smoke"))]
compile_error!("select the boot-smoke image");

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, timer::timg::TimerGroup, usb::usb_serial_jtag::UsbSerialJtag};

esp_bootloader_esp_idf::esp_app_desc!();

/// How long the smoke image waits on its timer before it passes.
const TIMER_WAKE: Duration = Duration::from_millis(100);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // The host sees no pass line and fails the boot; a halted core keeps the
    // state a debugger can read.
    loop {
        // SAFETY: `wfi` only stalls the hart until an interrupt; it touches
        // no memory.
        unsafe { core::arch::asm!("wfi") };
    }
}

#[esp_hal::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // The console opens before the scheduler starts: opened after
    // `esp_rtos::start`, the image printed nothing on the board.
    let mut console: UsbSerialJtag<'static, Blocking> = UsbSerialJtag::new(peripherals.USB_DEVICE);
    let _ = console.write(b"OPEN_RADIO_HIL hal=INIT\r\n");
    let timers = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timers.timer0);
    let _ = console.write(b"OPEN_RADIO_HIL embassy=START\r\n");
    Timer::after(TIMER_WAKE).await;
    let _ = console.write(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS\r\n");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}
