//! The boot-smoke image: `hal=INIT` once esp-hal has initialized the chip,
//! `embassy=START` once the esp-rtos scheduler and time driver run, then one
//! Embassy timer wake and the pass line the host waits for, all on the USB
//! Serial/JTAG console.

use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, timer::timg::TimerGroup, usb::usb_serial_jtag::UsbSerialJtag};

/// How long the smoke image waits on its timer before it passes.
const TIMER_WAKE: Duration = Duration::from_millis(100);

pub(crate) async fn run(peripherals: esp_hal::peripherals::Peripherals) -> ! {
    // The console opens before the scheduler starts, so a fault while it
    // starts still leaves the lines before it on the console.
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
