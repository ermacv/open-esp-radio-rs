//! Peripheral connection establishment and completion.

#[cfg(target_arch = "riscv32")]
pub(crate) mod completion;
pub(crate) mod connection;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first_hci;
#[cfg(target_arch = "riscv32")]
pub(crate) mod start;
