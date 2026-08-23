//! Experimental PSRAM thread stacks with per-hart SRAM interrupt stacks.
//!
//! ESP32-S31 uses CLIC hardware vectoring, so interrupts do not pass through
//! the ordinary exception entry.  The runtime therefore replaces every slot
//! in ESP-HAL's active per-hart MTVT with an SRAM wrapper. Each wrapper swaps
//! `sp` with `mscratch` before its first memory access, saves the interrupted
//! caller context on the hart-local SRAM stack, invokes the existing ESP
//! interrupt dispatcher, and restores the original stack immediately before
//! `mret`. Nested traps remain on the same SRAM stack; only the outermost trap
//! swaps back to PSRAM. Interrupt-side code must remain integer-only.

use core::{arch::asm, mem::MaybeUninit, ptr};

use esp_hal::system::Stack;

// The qualification AP path has an observed CPU0 high-water mark of roughly
// 136 KiB with the inherited SRAM stack. Keep that measured requirement plus
// margin in external RAM; a 64-KiB experimental stack silently overwrote the
// preceding PSRAM runtime payload during role setup.
pub(crate) const CPU0_TASK_STACK_BYTES: usize = 192 * 1024;
pub(crate) const IRQ_STACK_BYTES: usize = 32 * 1024;

#[repr(C, align(16))]
struct AlignedStack<const BYTES: usize>(MaybeUninit<[u8; BYTES]>);

impl<const BYTES: usize> AlignedStack<BYTES> {
    const fn new() -> Self {
        Self(MaybeUninit::uninit())
    }
}

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".psram.task_stack.cpu0")]
static mut __open_radio_cpu0_task_stack: Stack<CPU0_TASK_STACK_BYTES> = Stack::new();

#[cfg(feature = "open-radio-hil")]
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".psram.task_stack.cpu1")]
static mut __open_radio_cpu1_task_stack: Stack<{ crate::APP_CORE_TASK_STACK_BYTES }> = Stack::new();

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.psram_task_stack.cpu0_irq")]
static mut __open_radio_cpu0_irq_stack: AlignedStack<IRQ_STACK_BYTES> = AlignedStack::new();

#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.psram_task_stack.cpu1_irq")]
static mut __open_radio_cpu1_irq_stack: AlignedStack<IRQ_STACK_BYTES> = AlignedStack::new();

unsafe extern "C" {
    static _runtime_psram_mtvt_source: [u32; 48];
}

/// Restore the per-hart interrupt stack after ESP-HAL reinitializes CLIC.
///
/// # Safety
///
/// Global interrupts must be disabled. The caller must execute on hart 0 or 1
/// and must not enable interrupts until this function returns.
pub(crate) unsafe fn install_current_hart_interrupt_stack() {
    let hart: usize;
    unsafe { asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack)) };
    let bottom = match hart {
        0 => ptr::addr_of_mut!(__open_radio_cpu0_irq_stack).cast::<u8>(),
        1 => ptr::addr_of_mut!(__open_radio_cpu1_irq_stack).cast::<u8>(),
        _ => crate::fail(c"OPEN_RADIO_HIL runtime=FAIL reason=irq-stack-hart\r\n"),
    };
    let top = unsafe { bottom.add(IRQ_STACK_BYTES) } as usize;
    let table: *mut u32;
    unsafe {
        asm!(
            "csrr {table}, 0x307",
            table = out(reg) table,
            options(nomem, nostack),
        )
    };

    // ESP-HAL owns one writable MTVT per hart and may select a different table
    // while initializing the second core. Preserve that ownership: replace
    // only the hardware-vector slots in whichever table is currently active.
    // Slot zero is not an interrupt entry and remains untouched.
    for (index, source) in unsafe { _runtime_psram_mtvt_source.iter().enumerate().skip(1) } {
        let entry = unsafe { ptr::from_ref(source).read_volatile() };
        unsafe { table.add(index).write_volatile(entry) };
    }
    unsafe {
        asm!(
            "csrw mscratch, {top}",
            "fence.i",
            top = in(reg) top,
            options(nostack),
        )
    };
}

#[cfg(feature = "open-radio-hil")]
pub(crate) fn cpu1_task_stack_bottom() -> u32 {
    ptr::addr_of!(__open_radio_cpu1_task_stack) as usize as u32
}

#[cfg(feature = "open-radio-hil")]
unsafe extern "C" {
    fn _runtime_enter_cpu1_psram() -> !;
}

/// Abandon the ESP-HAL SRAM bootstrap stack and enter CPU1 thread mode on its
/// PSRAM task stack. This function never returns to the abandoned call chain.
///
/// # Safety
///
/// Must be called exactly once by CPU1 with global interrupts disabled, after
/// ESP-HAL has initialized that hart and consumed the bootstrap closure.
#[cfg(feature = "open-radio-hil")]
pub(crate) unsafe fn enter_cpu1_task_context() -> ! {
    unsafe { _runtime_enter_cpu1_psram() }
}

core::arch::global_asm!(
    r#"
    .section .trap.psram_task_stack, "ax", @progbits
    .option push
    .option norelax
    .option norvc

    .balign 4
    .global _runtime_stack_bootstrap
    .type _runtime_stack_bootstrap, @function
_runtime_stack_bootstrap:
    la t0, __runtime_cpu0_task_stack_bottom
    addi t0, t0, 256
    la t1, __runtime_cpu0_task_stack_top
    addi t1, t1, -256
    li t2, 0xa55aa55a
1:
    bgeu t0, t1, 2f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    la sp, __runtime_cpu0_task_stack_top
    la t0, __runtime_cpu0_irq_stack_top
    csrw mscratch, t0
    ret
    .size _runtime_stack_bootstrap, . - _runtime_stack_bootstrap

    // Swap before the first memory access. The second CSR exchange preserves
    // the interrupted t0 in mscratch and gives t0 the other stack pointer:
    //
    // outer:  sp = IRQ top, t0 = task sp
    // nested: sp = task sp, t0 = interrupted IRQ sp
    //
    // The unsigned address comparison distinguishes internal SRAM from PSRAM.
    // A nested entry swaps sp/t0 back with register-only XOR operations, then
    // both paths allocate an aligned frame on the SRAM stack. During dispatch,
    // mscratch always contains the task sp so deeper nesting is handled alike.
    .macro PSRAM_TRAP_ENTER
    csrrw sp, mscratch, sp
    csrrw t0, mscratch, t0
    bltu sp, t0, 90f

    // Nested trap: normalize sp back to the interrupted SRAM stack while
    // retaining the task stack pointer in t0.
    xor sp, sp, t0
    xor t0, sp, t0
    xor sp, sp, t0
    addi sp, sp, -80
    sw t1, 8(sp)
    sw zero, 64(sp)
    j 91f

90:
    // Outermost trap: sp already names the hart-local SRAM stack.
    addi sp, sp, -80
    sw t1, 8(sp)
    li t1, 1
    sw t1, 64(sp)

91:
    sw ra, 0(sp)
    // Both paths saved t1 before using it to recover the interrupted t0.
    csrr t1, mscratch
    sw t1, 4(sp)
    sw t2, 12(sp)
    sw t3, 16(sp)
    sw t4, 20(sp)
    sw t5, 24(sp)
    sw t6, 28(sp)
    sw a0, 32(sp)
    sw a1, 36(sp)
    sw a2, 40(sp)
    sw a3, 44(sp)
    sw a4, 48(sp)
    sw a5, 52(sp)
    sw a6, 56(sp)
    sw a7, 60(sp)
    csrw mscratch, t0
    .endm

    .macro PSRAM_TRAP_RESTORE_REGISTERS
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw t3, 16(sp)
    lw t4, 20(sp)
    lw t5, 24(sp)
    lw t6, 28(sp)
    lw a0, 32(sp)
    lw a1, 36(sp)
    lw a2, 40(sp)
    lw a3, 44(sp)
    lw a4, 48(sp)
    lw a5, 52(sp)
    lw a6, 56(sp)
    lw a7, 60(sp)
    .endm

    // Direct exceptions enter through mtvec slot zero. This deliberately
    // mirrors riscv-rt's caller-register TrapFrame on the SRAM stack.
    .balign 4
    .global _start_trap
    .type _start_trap, @function
_start_trap:
    PSRAM_TRAP_ENTER
    mv a0, sp
    call _start_trap_rust
    j _runtime_psram_trap_restore
    .size _start_trap, . - _start_trap

    // Every hardware-vector entry performs the stack swap itself because CLIC
    // dispatches through mtvt without visiting the direct exception entry.
    .macro PSRAM_IRQ_ENTRY number
    .balign 4
    .global _runtime_psram_irq_entry_\number
    .type _runtime_psram_irq_entry_\number, @function
_runtime_psram_irq_entry_\number:
    PSRAM_TRAP_ENTER
    la a0, Trap\number
    j _runtime_psram_interrupt_continue
    .size _runtime_psram_irq_entry_\number, . - _runtime_psram_irq_entry_\number
    .endm

    PSRAM_IRQ_ENTRY 1
    PSRAM_IRQ_ENTRY 2
    PSRAM_IRQ_ENTRY 3
    PSRAM_IRQ_ENTRY 4
    PSRAM_IRQ_ENTRY 5
    PSRAM_IRQ_ENTRY 6
    PSRAM_IRQ_ENTRY 7
    PSRAM_IRQ_ENTRY 8
    PSRAM_IRQ_ENTRY 9
    PSRAM_IRQ_ENTRY 10
    PSRAM_IRQ_ENTRY 11
    PSRAM_IRQ_ENTRY 12
    PSRAM_IRQ_ENTRY 13
    PSRAM_IRQ_ENTRY 14
    PSRAM_IRQ_ENTRY 15
    PSRAM_IRQ_ENTRY 16
    PSRAM_IRQ_ENTRY 17
    PSRAM_IRQ_ENTRY 18
    PSRAM_IRQ_ENTRY 19
    PSRAM_IRQ_ENTRY 20
    PSRAM_IRQ_ENTRY 21
    PSRAM_IRQ_ENTRY 22
    PSRAM_IRQ_ENTRY 23
    PSRAM_IRQ_ENTRY 24
    PSRAM_IRQ_ENTRY 25
    PSRAM_IRQ_ENTRY 26
    PSRAM_IRQ_ENTRY 27
    PSRAM_IRQ_ENTRY 28
    PSRAM_IRQ_ENTRY 29
    PSRAM_IRQ_ENTRY 30
    PSRAM_IRQ_ENTRY 31
    PSRAM_IRQ_ENTRY 32
    PSRAM_IRQ_ENTRY 33
    PSRAM_IRQ_ENTRY 34
    PSRAM_IRQ_ENTRY 35
    PSRAM_IRQ_ENTRY 36
    PSRAM_IRQ_ENTRY 37
    PSRAM_IRQ_ENTRY 38
    PSRAM_IRQ_ENTRY 39
    PSRAM_IRQ_ENTRY 40
    PSRAM_IRQ_ENTRY 41
    PSRAM_IRQ_ENTRY 42
    PSRAM_IRQ_ENTRY 43
    PSRAM_IRQ_ENTRY 44
    PSRAM_IRQ_ENTRY 45
    PSRAM_IRQ_ENTRY 46
    PSRAM_IRQ_ENTRY 47

    .balign 4
    .global _runtime_psram_interrupt_continue
    .type _runtime_psram_interrupt_continue, @function
_runtime_psram_interrupt_continue:
    jalr ra, a0, 0

_runtime_psram_trap_restore:
    lw t0, 64(sp)
    beqz t0, 92f

    // Outermost return restores the task stack and arms mscratch with the IRQ
    // stack top for the next first-level trap.
    PSRAM_TRAP_RESTORE_REGISTERS
    addi sp, sp, 80
    csrrw sp, mscratch, sp
    mret

92:
    // A nested return resumes the interrupted handler on its existing SRAM
    // stack. mscratch remains the task stack pointer for further nesting.
    PSRAM_TRAP_RESTORE_REGISTERS
    addi sp, sp, 80
    mret

    // Source image copied into ESP-HAL's active per-hart hardware-vector table.
    // Slot zero remains unused because synchronous exceptions use mtvec and
    // `_start_trap` directly.
    .balign 0x40
    .global _runtime_psram_mtvt_source
    .type _runtime_psram_mtvt_source, @object
_runtime_psram_mtvt_source:
    .word 0
    .word _runtime_psram_irq_entry_1
    .word _runtime_psram_irq_entry_2
    .word _runtime_psram_irq_entry_3
    .word _runtime_psram_irq_entry_4
    .word _runtime_psram_irq_entry_5
    .word _runtime_psram_irq_entry_6
    .word _runtime_psram_irq_entry_7
    .word _runtime_psram_irq_entry_8
    .word _runtime_psram_irq_entry_9
    .word _runtime_psram_irq_entry_10
    .word _runtime_psram_irq_entry_11
    .word _runtime_psram_irq_entry_12
    .word _runtime_psram_irq_entry_13
    .word _runtime_psram_irq_entry_14
    .word _runtime_psram_irq_entry_15
    .word _runtime_psram_irq_entry_16
    .word _runtime_psram_irq_entry_17
    .word _runtime_psram_irq_entry_18
    .word _runtime_psram_irq_entry_19
    .word _runtime_psram_irq_entry_20
    .word _runtime_psram_irq_entry_21
    .word _runtime_psram_irq_entry_22
    .word _runtime_psram_irq_entry_23
    .word _runtime_psram_irq_entry_24
    .word _runtime_psram_irq_entry_25
    .word _runtime_psram_irq_entry_26
    .word _runtime_psram_irq_entry_27
    .word _runtime_psram_irq_entry_28
    .word _runtime_psram_irq_entry_29
    .word _runtime_psram_irq_entry_30
    .word _runtime_psram_irq_entry_31
    .word _runtime_psram_irq_entry_32
    .word _runtime_psram_irq_entry_33
    .word _runtime_psram_irq_entry_34
    .word _runtime_psram_irq_entry_35
    .word _runtime_psram_irq_entry_36
    .word _runtime_psram_irq_entry_37
    .word _runtime_psram_irq_entry_38
    .word _runtime_psram_irq_entry_39
    .word _runtime_psram_irq_entry_40
    .word _runtime_psram_irq_entry_41
    .word _runtime_psram_irq_entry_42
    .word _runtime_psram_irq_entry_43
    .word _runtime_psram_irq_entry_44
    .word _runtime_psram_irq_entry_45
    .word _runtime_psram_irq_entry_46
    .word _runtime_psram_irq_entry_47
    .size _runtime_psram_mtvt_source, . - _runtime_psram_mtvt_source

    .option pop
"#
);

// The boot-smoke image never starts CPU1. Keep the second-hart stack switch
// out of that minimal link graph instead of satisfying it with an unreachable
// alternate entry point.
#[cfg(feature = "open-radio-hil")]
core::arch::global_asm!(
    r#"
    .section .trap.psram_task_stack, "ax", @progbits
    .option push
    .option norelax
    .option norvc
    .balign 4
    .global _runtime_enter_cpu1_psram
    .type _runtime_enter_cpu1_psram, @function
_runtime_enter_cpu1_psram:
    la sp, __runtime_cpu1_task_stack_top
    tail runtime_cpu1_psram_main
    .size _runtime_enter_cpu1_psram, . - _runtime_enter_cpu1_psram
    .option pop
"#
);
