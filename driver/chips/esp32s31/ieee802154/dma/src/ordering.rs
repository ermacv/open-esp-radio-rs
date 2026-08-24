//! Device-memory ordering at the CPU/DMA ownership boundary.

/// Order SRAM and MMIO observations across a hardware ownership handoff.
#[inline]
#[allow(
    unsafe_code,
    reason = "the RISC-V device fence only orders memory and I/O accesses"
)]
pub(crate) fn device_fence() {
    #[cfg(target_arch = "riscv32")]
    // SAFETY: `fence iorw, iorw` has no operands and changes no memory or
    // register value; it only orders prior and subsequent memory/I/O accesses.
    unsafe {
        core::arch::asm!("fence iorw, iorw")
    }

    #[cfg(not(target_arch = "riscv32"))]
    // Native builds contain no external DMA actor. Preserve compiler order so
    // the ownership model still exercises the same publication positions.
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
