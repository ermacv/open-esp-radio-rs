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
        let psram = oer_esp32s31_platform_board::adopt_initialized_psram(peripheral);
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

/// Hand global interrupt enable (`mstatus.MIE`) to the current hart's executor.
///
/// The staged handoff keeps MIE clear, so interrupt handlers cannot run
/// against unbound timer, executor or stack state.
///
/// # Safety
/// Call once per hart, after its interrupt vectors, interrupt stack, timer and
/// executor handlers are bound. MIE must have stayed clear since that hart
/// entered the runtime. Earlier enabling can dispatch an interrupt into
/// uninitialized ownership state.
pub unsafe fn enable_interrupts_after_handoff() {
    // SAFETY: the caller guarantees that every handler this hart can dispatch
    // is bound; setting MIE touches no memory and leaves the stack unchanged.
    unsafe { core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack)) };
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static mut __stack_chk_guard: u32 = 0xDEED_BAAD;
