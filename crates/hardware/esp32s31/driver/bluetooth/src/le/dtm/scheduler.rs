//! Related scheduler operations.

pub(crate) mod item;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod reservation;
