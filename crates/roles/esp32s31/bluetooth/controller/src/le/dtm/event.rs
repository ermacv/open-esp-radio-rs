//! Related event operations.

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod prepare;
pub(crate) mod timing;
