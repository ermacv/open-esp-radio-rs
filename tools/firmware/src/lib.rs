//! Host construction and validation of staged ESP32-S31 firmware.

/// This crate's directory in the repository: an image build records the
/// crate's sources among its inputs, since they pack and audit every image.
pub const REPOSITORY_DIRECTORY: &str = "tools/firmware";
#[cfg(feature = "image")]
pub mod compiler;
#[cfg(feature = "device")]
pub mod device;
#[cfg(feature = "image")]
pub mod flash;
#[cfg(feature = "image")]
mod image;
#[cfg(feature = "image")]
pub mod interrupt_stack;
#[cfg(feature = "image")]
pub mod network;
#[cfg(feature = "image")]
mod payload;
#[cfg(feature = "image")]
pub mod stack;
#[cfg(feature = "image")]
pub use image::{
    BOOTSTRAP_BIN, CHIP, PARTITION_TABLE, audit_application_image, audit_runtime,
    bootstrap_command, save_image_command, save_rom_image_command, target,
};
#[cfg(feature = "image")]
pub use payload::pack_runtime;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[cfg(test)]
mod repository_directory_tests {
    #[test]
    fn the_declared_directory_is_this_crate() {
        assert!(env!("CARGO_MANIFEST_DIR").ends_with(super::REPOSITORY_DIRECTORY));
    }
}
