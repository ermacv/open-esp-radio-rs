#![no_std]
//! Stage-two entry, relocation and interrupt-stack ownership for ESP32-S31.
mod entry;
#[cfg(feature = "psram-task-stack")]
pub mod stacks;

/// Adopt the board mapping and install the stage-two interrupt context.
///
/// # Safety
/// Call once on CPU0 after `_runtime_start`, with interrupts disabled and the
/// bootstrap's board mapping intact. Keep interrupts disabled until the
/// application's timer and executor handlers have been bound.
pub unsafe fn adopt_psram(
    peripheral: esp_hal::peripherals::PSRAM<'static>,
) -> esp_hal::psram::Psram {
    unsafe {
        let psram = oer_esp32s31_board::adopt_initialized_psram(peripheral);
        esp_hal::interrupt::reinitialize_vectoring_after_handoff();
        #[cfg(feature = "psram-task-stack")]
        stacks::install_current_hart_interrupt_stack();
        unsafe extern "C" {
            static _stack_end: u8;
        }
        esp_hal::debugger::set_stack_watchpoint(core::ptr::addr_of!(_stack_end) as usize);
        psram
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static mut __stack_chk_guard: u32 = 0xDEED_BAAD;
