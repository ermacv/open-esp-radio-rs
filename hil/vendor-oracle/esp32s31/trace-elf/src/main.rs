#![no_main]
#![no_std]

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

#[used]
#[unsafe(link_section = ".note.open_esp_radio.oracle")]
static ORACLE_PROVENANCE: [u8; 80] =
    *b"libphy.a sha256=51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223";
