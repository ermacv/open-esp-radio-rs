#![no_std]
#![forbid(unsafe_code)]

//! One-call bring-up of the shared ESP32-S31 radio under Embassy.
//!
//! Every protocol composition (Wi-Fi, Bluetooth LE, IEEE 802.15.4) joins one
//! [`SharedRadio`]: the radio arbiter over the esp-hal platform and clock
//! sources. The radio also needs two tasks for as long as it exists: periodic
//! PHY tracking, which the vendor runs from its `phy_track_pll` timer, and
//! the coexistence schedule. [`start`] creates the radio once, places it in
//! static storage and spawns both tasks, so no application can forget one.
//!
//! ```ignore
//! let platform = EspHalRadioPlatform::new(/* the eight radio peripherals */);
//! let (radio, partitions) = oer_esp32s31_radio_system::start(spawner, platform, RadioStart::new())?;
//! // hand `radio` and one partition of `partitions` to each protocol composition
//! ```

#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use system::{RadioStart, RadioStartError, SharedRadio, Tracking, start};

pub use oer_esp32s31_hal::root::ConcurrentPartitions;
pub use oer_esp32s31_phy::PhyCalibrationCache;
