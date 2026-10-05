//! Attached development boards as one process owns them: discovery of the
//! attached boards ([`devices`]) by port, MAC and known chip, and a board
//! this process holds under its device lock ([`Opened`]): write an image
//! bundle, start it, reset it, read its console.
//!
//! A write goes through the one write operation of `oer-device-image`,
//! which invalidates the board's receipt before the flash changes and
//! publishes the whole bundle's receipt after; `cargo fw` therefore leaves
//! no stale image state for the stand or HIL, without depending on them.
//!
//! It assembles the leaf crates: `oer-device-discovery`, `-lock`, `-port`,
//! `-flash`, `-image`, `-reset` and `-console`.
#![forbid(unsafe_code)]

pub mod device;

pub use device::{Console, Device, Opened, devices, find};
pub use oer_device_image::Receipt;
pub use oer_device_lock::DeviceId;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
