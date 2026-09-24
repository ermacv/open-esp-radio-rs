#![no_main]
#![no_std]

extern crate open_esp_radio_verification_esp32s31_probes as _;
include!(concat!(env!("OUT_DIR"), "/probe_catalog.rs"));

#[allow(
    clippy::disallowed_methods,
    reason = "verification image idle entry is never a production runtime"
)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
