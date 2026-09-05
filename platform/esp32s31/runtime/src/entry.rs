// Stage two does not return through the bootloader reset entry. It owns data,
// BSS, the SRAM interrupt closure and vector registers, and initializes each
// one before entering Rust or enabling interrupts.
core::arch::global_asm!(
    r#"
    .section .text._start, "ax", @progbits
    .balign 4
    .global _runtime_start
    .type _runtime_start, @function
_runtime_start:
    .option push
    .option norelax
    la gp, __global_pointer$
    la a0, __runtime_data_load_start
    la a1, __runtime_data_start
    la a2, __runtime_data_end
1:
    beq a1, a2, 2f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 1b
2:
    la a0, __runtime_data_bss_start
    la a1, __runtime_data_bss_end
3:
    beq a0, a1, 4f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 3b
4:
    la a0, __runtime_isr_load_start
    la a1, __runtime_isr_start
    la a2, __runtime_isr_end
5:
    beq a1, a2, 6f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 5b
6:
    la a0, __runtime_critical_data_load_start
    la a1, __runtime_critical_data_start
    la a2, __runtime_critical_data_end
7:
    beq a1, a2, 8f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 7b
8:
    la a0, __runtime_critical_bss_start
    la a1, __runtime_critical_bss_end
9:
    beq a0, a1, 10f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 9b
10:
    la a0, __runtime_dma_data_load_start
    la a1, __runtime_dma_data_start
    la a2, __runtime_dma_data_end
11:
    beq a1, a2, 12f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 11b
12:
    la a0, __runtime_dma_bss_start
    la a1, __runtime_dma_bss_end
13:
    beq a0, a1, 14f
    sw zero, 0(a0)
    addi a0, a0, 4
    j 13b
14:
    la a0, __runtime_hot_text_load_start
    la a1, __runtime_hot_text_start
    la a2, __runtime_hot_text_end
15:
    beq a1, a2, 16f
    lw a3, 0(a0)
    sw a3, 0(a1)
    addi a0, a0, 4
    addi a1, a1, 4
    j 15b
16:
    call _runtime_stack_bootstrap
    fence.i
    la t0, _vector_table
    ori t0, t0, 3
    csrw mtvec, t0
    la t0, _runtime_mtvt_table
    csrw 0x307, t0
    .option pop
    li t0, 0x6000
    csrrs zero, mstatus, t0
    .option push
    .option arch, +f
    fscsr zero
    .option pop
    tail runtime_main
    .size _runtime_start, . - _runtime_start

    # Control profile: paint the inherited SRAM stack. The PSRAM stack module
    # supplies a strong `_runtime_stack_bootstrap` implementation.
    .balign 4
    .global _runtime_default_stack_bootstrap
    .type _runtime_default_stack_bootstrap, @function
_runtime_default_stack_bootstrap:
    la t0, _stack_end
    addi t0, t0, 256
    mv t1, sp
    addi t1, t1, -256
    li t2, 0xa55aa55a
17:
    bgeu t0, t1, 18f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 17b
18:
    ret
    .size _runtime_default_stack_bootstrap, . - _runtime_default_stack_bootstrap
"#
);
