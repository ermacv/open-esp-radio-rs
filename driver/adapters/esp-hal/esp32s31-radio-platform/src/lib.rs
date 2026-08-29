#![no_std]
#![deny(unsafe_code)]

//! ESP-HAL ownership coordinator for shared ESP32-S31 radio platform resources.
//!
//! The neutral coordinator is the sole owner of the official system PAC
//! singletons used by modem clocking and common PHY initialization. A
//! Bluetooth platform handle is an affine reservation: raw register blocks
//! cannot escape through it, while every clock transaction and reference
//! count lives in the custom PAC route.
//! This is the boundary required for a later Wi-Fi/Bluetooth
//! coexistence composition; the current Wi-Fi adapter must be migrated onto
//! the same coordinator before simultaneous use.

#[cfg(any(feature = "esp32s31", test))]
mod coordinator;

#[cfg(feature = "esp32s31")]
mod bluetooth_interrupt;

#[cfg(any(feature = "esp32s31", test))]
mod bluetooth_address;

#[cfg(any(feature = "esp32s31", test))]
mod bluetooth_route_policy;

#[cfg(feature = "esp32s31")]
mod esp32s31;

#[cfg(feature = "esp32s31")]
pub use coordinator::BluetoothPlatformBusy;

#[cfg(feature = "esp32s31")]
pub use bluetooth_interrupt::{
    EspHalBluetoothInterruptStorage, EspHalBluetoothModemLpTimerInterruptStep,
    EspHalBluetoothModemLpTimerRestoreFailure, EspHalBluetoothModemLpTimerStorageError,
    EspHalBluetoothNrtInterruptStep, EspHalBluetoothPrimaryInterruptStep,
    PublishedEspHalBluetoothInterruptOwners,
};
#[cfg(feature = "esp32s31")]
pub use bluetooth_route_policy::EspHalBluetoothInterruptStorageError;

#[cfg(feature = "esp32s31")]
pub use esp32s31::{EspHalBluetoothPlatform, EspHalRadioPlatform};

#[cfg(test)]
extern crate std;
