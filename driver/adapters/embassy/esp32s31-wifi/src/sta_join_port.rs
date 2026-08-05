//! Concrete ESP32-S31 port for pre-connected STA Authentication/Association.
//!
//! The public facade keeps stable type paths while RX ownership, borrowed
//! resources, port ownership and join sequencing live in separate modules.

mod owner;
mod resources;
mod rx;
mod service;

pub use owner::Esp32s31StaJoinPort;
pub use resources::{Esp32s31StaJoinRadio, Esp32s31StaJoinStation, Esp32s31StaJoinStorage};
pub use rx::Esp32s31StaJoinRx;

#[cfg(test)]
mod tests;
