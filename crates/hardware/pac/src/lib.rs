//! The radio peripheral access crate of the selected chip.
//!
//! Code written once for every chip reaches the PAC through this crate and
//! never through `oer-<chip>-pac` directly: the one enabled chip feature
//! (`esp32s31` or `esp32c5`) re-exports that chip's PAC here, and its
//! properties from `platform/<chip>/chip.toml` as [`properties`]. Built with
//! no chip feature the crate is empty; with two it does not build.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "esp32c5")]
pub use oer_esp32c5_pac::*;
#[cfg(feature = "esp32s31")]
pub use oer_esp32s31_pac::*;

/// The numeric properties of the selected chip.
pub mod properties {
    include!(concat!(env!("OUT_DIR"), "/chip_properties.rs"));
}
