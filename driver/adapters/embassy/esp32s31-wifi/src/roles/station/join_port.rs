//! Concrete ESP32-S31 port for pre-connected STA Authentication/Association.
//!
//! RX ownership, borrowed resources, port ownership and join sequencing live
//! in separate private modules.

mod owner;
mod resources;
#[cfg(target_arch = "riscv32")]
mod rx;
mod service;

pub use owner::Esp32s31StaJoinPort;
pub use resources::{Esp32s31StaJoinRadio, Esp32s31StaJoinStation, Esp32s31StaJoinStorage};
#[cfg(target_arch = "riscv32")]
pub use rx::Esp32s31StaJoinRx;

#[cfg(test)]
mod tests;
