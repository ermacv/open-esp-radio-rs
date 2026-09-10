#![no_main]
#![no_std]

#[unsafe(no_mangle)]
#[allow(
    clippy::disallowed_methods,
    reason = "linker retention entry for isolated probes, never a production runtime"
)]
pub extern "C" fn _start() -> ! {
    open_esp_radio_verification_esp32s31_probes::retain_all_probes();
    loop {
        core::hint::spin_loop();
    }
}
