#![no_main]
#![no_std]

struct IsolatedVerifierCriticalSection;

critical_section::set_impl!(IsolatedVerifierCriticalSection);

// SAFETY: Blobray executes one entry synchronously in an isolated image. No
// interrupt handler, second hart, or concurrent caller exists in this modeled
// environment, so an empty restore token provides the critical-section
// contract needed by the production ownership coordinator.
unsafe impl critical_section::Impl for IsolatedVerifierCriticalSection {
    unsafe fn acquire() -> critical_section::RawRestoreState {}

    unsafe fn release(_: critical_section::RawRestoreState) {}
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    open_esp_radio_verification_esp32s31_bluetooth_probes::retain_all_probes();
    loop {
        core::hint::spin_loop();
    }
}
