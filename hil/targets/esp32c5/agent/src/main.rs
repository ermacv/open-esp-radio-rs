//! ESP32-C5 HIL runtime.
//!
//! The ESP-IDF second-stage bootloader loads this application. Each image
//! selects one feature: `boot-smoke` proves the boot path with text lines,
//! `system-watchdog` serves the HIL protocol with the SoC deadline watchdog
//! and the reset-retained post-mortem.

#![no_std]
#![no_main]

#[cfg(not(any(feature = "boot-smoke", feature = "system-watchdog")))]
compile_error!("select the boot-smoke or system-watchdog image");
#[cfg(all(feature = "boot-smoke", feature = "system-watchdog"))]
compile_error!("boot-smoke and system-watchdog are separate images");

use embassy_executor::Spawner;

#[cfg(feature = "boot-smoke")]
mod boot_smoke;
#[cfg(feature = "system-watchdog")]
mod system;

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    #[cfg(feature = "system-watchdog")]
    system::postmortem::record_panic(info);
    #[cfg(not(feature = "system-watchdog"))]
    let _ = info;
    // The host finds no answer and reads the next boot's evidence; a halted
    // core keeps the state a debugger can read.
    loop {
        // SAFETY: `wfi` only stalls the hart until an interrupt; it touches
        // no memory.
        unsafe { core::arch::asm!("wfi") };
    }
}

#[esp_hal::main]
async fn main(_spawner: Spawner) {
    // What the previous boot left is taken before anything records.
    #[cfg(feature = "system-watchdog")]
    system::postmortem::begin();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    #[cfg(feature = "boot-smoke")]
    boot_smoke::run(peripherals).await;
    #[cfg(feature = "system-watchdog")]
    system::console::run(peripherals).await;
}
