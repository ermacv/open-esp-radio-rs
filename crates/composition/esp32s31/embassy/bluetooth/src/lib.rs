#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-S31 Bluetooth LE radio composition over Embassy-compatible time.
//!
//! [`start_esp32s31_bluetooth`] runs one powered Controller epoch from the
//! stopped radio root to an installed radio runtime:
//!
//! 1. claims the static controller memory once: the BLE PHY environment, the
//!    direction-finding workspace, one pool per role and both global receive
//!    chains, all placed in internal SRAM;
//! 2. enables clocks and runs Controller HAL, scheduler, modem low-power,
//!    common PHY, BTBB and BLE PHY initialization;
//! 3. prepares Controller output, starts the runtime timer, publishes both
//!    interrupt owners and binds the three CPU routes to one dispatcher;
//! 4. installs the radio role in the runtime.
//!
//! The dispatcher services source 124, 127 and 133 and forwards the scheduler
//! and source-127 worker wakes. [`BluetoothSystem::runner`] drives the radio
//! runtime and the source-127 timer task until a fault stops either.
//!
//! [`start_bluetooth_hci`] then creates the HCI Controller over that runtime:
//! the in-process transport, whose Host end goes to the Host stack, and the
//! service that runs the portable Controller core with the runtime as its
//! radio. [`BluetoothEntropy`] binds the SoC entropy service as its random
//! source.
//!
//! The epoch is one-shot: there is no teardown or restart, and periodic PHY
//! tracking maintenance is not composed yet. Any failure after the first
//! Controller write retains its owners in the returned error.

#[cfg(target_arch = "riscv32")]
mod hci;
#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use hci::{
    BluetoothEntropy, BluetoothHci, BluetoothHciService, BluetoothHostTransport,
    CONTROLLER_TO_HOST, HOST_TO_CONTROLLER, OUTPUT, PACKET, start_bluetooth_hci,
};
#[cfg(target_arch = "riscv32")]
pub use system::{
    BluetoothInterruptFault, BluetoothRunner, BluetoothRunnerFault, BluetoothStartError,
    BluetoothSystem, BluetoothSystemMemory, BluetoothSystemRuntime, EVENTS, ITEMS,
    MODEM_TIMER_CAPACITY, start_esp32s31_bluetooth,
};
