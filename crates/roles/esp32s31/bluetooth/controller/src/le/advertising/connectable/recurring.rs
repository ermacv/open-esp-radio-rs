//! Connectable recurrence sequence, HCI ordering and host-visible state.

#[cfg(target_arch = "riscv32")]
pub(crate) mod hci;
#[cfg(target_arch = "riscv32")]
pub(crate) mod sequence;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod state;
