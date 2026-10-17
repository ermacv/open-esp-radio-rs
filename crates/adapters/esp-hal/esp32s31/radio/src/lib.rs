#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// A forgotten owner never releases what it holds, such as an arbiter lease,
// so radio ownership must end in `Drop`.
#![deny(clippy::mem_forget)]

//! ESP-HAL platform of the shared ESP32-S31 radio.
//!
//! [`EspHalRadioPlatform`] retains the official system PAC singletons that
//! modem clocking and common PHY initialization touch, so no other safe
//! owner can be constructed from them while the shared radio's owner holds it;
//! [`EspHalRadioClocks`] supplies the platform clock sources of the modem
//! clocks.

#[cfg(any(feature = "esp32s31", test))]
mod bluetooth_address;

#[cfg(feature = "esp32s31")]
mod esp32s31;

#[cfg(feature = "esp32s31")]
mod platform_clocks;

#[cfg(feature = "esp32s31")]
pub use esp32s31::EspHalRadioPlatform;
#[cfg(feature = "esp32s31")]
pub use platform_clocks::EspHalRadioClocks;

#[cfg(test)]
extern crate std;
