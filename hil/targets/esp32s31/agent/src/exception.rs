//! Fatal CPU exception reporting independent of the executor and USB logger.

use core::arch::asm;

use esp_hal::trapframe::TrapFrame;

/// Watch the boundary word after the hart adopts its final task stack. HAL
/// reserves watchpoint zero for stack monitoring and respects debugger ownership.
pub fn install_stack_guard(bottom: usize) {
    unsafe {
        // Every caller supplies the aligned bottom of its own live stack.
        esp_hal::debugger::set_stack_watchpoint(bottom);
        let (control, address, hart): (usize, usize, usize);
        asm!("csrr {}, 0x7a1", out(reg) control, options(nomem, nostack));
        asm!("csrr {}, 0x7a2", out(reg) address, options(nomem, nostack));
        asm!("csrr {}, mhartid", out(reg) hart, options(nomem, nostack));
        super::ets_printf(
            c"OPEN_RADIO_HIL stack_guard hart=%u control=%08x address=%08x\r\n".as_ptr(),
            hart,
            control,
            address,
        );
    }
}

/// # Safety
/// Called only by the trap entry with its saved caller registers. The HIL trap
/// entry supplies an SRAM stack, including when the interrupted task uses PSRAM.
#[unsafe(export_name = "ExceptionHandler")]
#[unsafe(link_section = ".rwtext.exception")]
unsafe extern "C" fn exception(context: &TrapFrame) -> ! {
    let (cause, pc, value, task_sp, hart): (usize, usize, usize, usize, usize);
    unsafe {
        asm!("csrr {}, mcause", out(reg) cause, options(nomem, nostack));
        asm!("csrr {}, mepc", out(reg) pc, options(nomem, nostack));
        asm!("csrr {}, mtval", out(reg) value, options(nomem, nostack));
        asm!("csrr {}, mscratch", out(reg) task_sp, options(nomem, nostack));
        asm!("csrr {}, mhartid", out(reg) hart, options(nomem, nostack));
        // A fatal exception cannot rely on a live executor or acquire a logger
        // lock that the interrupted code may already hold.
        super::ets_printf(
            c"OPEN_RADIO_HIL runtime=EXCEPTION hart=%u mcause=%08x mepc=%08x mtval=%08x ra=%08x mscratch=%08x\r\n".as_ptr(),
            hart, cause, pc, value, context.ra, task_sp,
        );
    }
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
