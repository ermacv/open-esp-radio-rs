#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// A forgotten owner never releases what it holds, such as a platform clock
// reference, so radio ownership must end in `Drop`.
#![deny(clippy::mem_forget)]

//! ESP-HAL platform of the shared ESP32-S31 radio.
//!
//! [`EspHalRadioPlatform`] retains the official system PAC singletons that
//! modem clocking and common PHY initialization touch, so no other safe
//! owner can be constructed from them while the radio system holds it;
//! [`EspHalRadioClocks`] supplies the platform clock sources of the modem
//! clocks. The module also binds the Bluetooth Controller's interrupt
//! sources to their CPU routes and stable ISR storage.

#[cfg(feature = "esp32s31")]
mod bluetooth_handlers;
#[cfg(feature = "esp32s31")]
mod bluetooth_interrupt;

#[cfg(any(feature = "esp32s31", test))]
mod bluetooth_address;

#[cfg(any(feature = "esp32s31", test))]
mod bluetooth_route_policy;

#[cfg(feature = "esp32s31")]
mod esp32s31;

#[cfg(feature = "esp32s31")]
mod platform_clocks;

#[cfg(feature = "esp32s31")]
pub use bluetooth_handlers::{
    bluetooth_modem_lp_timer_interrupt_handler, bluetooth_nrt_default_interrupt_handler,
    bluetooth_primary_interrupt_handler,
};
#[cfg(feature = "esp32s31")]
pub use bluetooth_interrupt::{
    BoundEspHalBluetoothInterruptEpoch, EspHalBluetoothInterruptDisposition,
    EspHalBluetoothInterruptRouteDisableFailure, EspHalBluetoothInterruptRoutes,
    EspHalBluetoothInterruptSource, EspHalBluetoothInterruptStorage,
    EspHalBluetoothModemLpTimerInterruptStep, EspHalBluetoothModemLpTimerRestoreFailure,
    EspHalBluetoothModemLpTimerStorageError, EspHalBluetoothNrtInterruptStep,
    EspHalBluetoothPrimaryInterruptStep, EspHalBluetoothSchedulerRunInterruptError,
    EspHalBluetoothSharedInterruptDispatchError, PublishedEspHalBluetoothInterruptOwners,
    RetiredEspHalBluetoothInterruptRegisters,
};
#[cfg(feature = "esp32s31")]
pub use bluetooth_route_policy::{
    EspHalBluetoothInterruptRetirementError, EspHalBluetoothInterruptRouteError,
    EspHalBluetoothInterruptStorageError, EspHalBluetoothModemLpTimerRetirementError,
};

#[cfg(feature = "esp32s31")]
pub use esp32s31::EspHalRadioPlatform;
#[cfg(feature = "esp32s31")]
pub use platform_clocks::EspHalRadioClocks;

#[cfg(test)]
extern crate std;
