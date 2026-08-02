#![no_main]
#![no_std]

include!(concat!(env!("OUT_DIR"), "/linked_oracle_stubs.rs"));

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
