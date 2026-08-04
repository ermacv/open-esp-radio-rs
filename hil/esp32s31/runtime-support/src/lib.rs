#![no_std]

//! Compatibility facade retained for old HIL package paths.
//!
//! New board applications and the HIL runtime consume the reusable platform
//! crate directly. No executor or timer implementation remains HIL-owned.

pub use open_esp_radio_esp32s31_embassy_runtime::{Executor, Timer, init};
