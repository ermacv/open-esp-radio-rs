//! Shared host-only fixture execution and installation contracts.

#[path = "fixture/bluetooth/contract.rs"]
pub mod bluetooth_fixture_contract;

pub mod fixture_install;

// Compile the target-owned command pump on the host against real bt-hci slots.
#[cfg(test)]
#[path = "../../../targets/esp32s31/runtime/src/bluetooth/command_pump.rs"]
mod target_bluetooth_command_pump;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[cfg(test)]
#[path = "../../../targets/esp32s31/runtime/src/bluetooth/security.rs"]
mod target_bluetooth_security;
