#![no_main]
#![no_std]

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    open_esp_radio_verification_esp32s31_register_probes::retain_all_probes();
    loop {
        core::hint::spin_loop();
    }
}
