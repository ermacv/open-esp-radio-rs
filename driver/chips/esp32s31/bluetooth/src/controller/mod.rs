//! Controller bring-up, HAL readiness and controller-time ownership.

pub(crate) mod boot;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod hal;
pub(crate) mod time;
