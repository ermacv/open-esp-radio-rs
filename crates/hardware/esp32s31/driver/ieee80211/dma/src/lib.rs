#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Audited ESP32-S31 Wi-Fi MAC DMA ownership boundary.
//!
//! This crate owns the chip descriptor geometry, finite RX walker operations,
//! live ring state and permanently located RX arena. Protocol decoding and
//! executor policy deliberately remain above this leaf.

pub mod alignment;
pub mod descriptor;
pub mod rx_dma;
#[cfg(feature = "rx-ownership-observation")]
pub mod rx_observation;
pub mod rx_ring;
pub mod rx_storage;
pub mod tx_ampdu_storage;
pub mod tx_storage;

/// Place one steady-state Wi-Fi RX item in executable internal RAM on S31.
///
/// The item joins the `.hot.text` class, which the runtime linker executes
/// from internal SRAM whatever the image's code tier, so the per-frame receive
/// cost does not follow the layout of PSRAM text. Interrupt handlers keep
/// their own `.rwtext` placement; this class is for task-context code.
///
/// Rust 2024 makes section placement an unsafe attribute because an arbitrary
/// section can violate platform invariants. This chip leaf owns that invariant,
/// and the macro is intentionally limited to code items rather than storage.
#[macro_export]
macro_rules! place_rx_hot_path {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $($body:tt)*) => {
        $(#[$attribute])*
        #[cfg_attr(
            target_arch = "riscv32",
            unsafe(link_section = ".hot.text.open_radio_wifi_rx")
        )]
        $visibility fn $name $($body)*
    };
}
