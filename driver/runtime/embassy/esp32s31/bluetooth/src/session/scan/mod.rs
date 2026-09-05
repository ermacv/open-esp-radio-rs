//! Passive-scanning first-event and recurring-session adaptation.

#[cfg(target_arch = "riscv32")]
pub(crate) mod active;
#[cfg(target_arch = "riscv32")]
pub(crate) mod first;
