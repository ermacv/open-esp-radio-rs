//! Legacy advertising session adaptation, including connectable roles.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod connectable;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first;
