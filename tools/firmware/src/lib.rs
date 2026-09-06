//! Host construction and validation of staged ESP32-S31 firmware.
#[cfg(feature = "device")]
pub mod device;
#[cfg(feature = "image")]
pub mod flash;
#[cfg(feature = "image")]
mod image;
pub mod linker;
#[cfg(feature = "image")]
pub mod network;
#[cfg(feature = "image")]
mod payload;
#[cfg(feature = "image")]
pub mod stack;
#[cfg(feature = "image")]
pub use image::{
    BOOTSTRAP_BIN, TARGET, audit_application_image, audit_runtime, bootstrap_command,
    save_image_command, save_rom_image_command,
};
#[cfg(feature = "image")]
pub use payload::{RUNTIME_CRC_OFFSET, crc32, pack_runtime};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
