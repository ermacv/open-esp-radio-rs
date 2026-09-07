//! Peripheral-connection first-event and stopping adaptation.

#[cfg(target_arch = "riscv32")]
pub(crate) mod first;

#[cfg(target_arch = "riscv32")]
pub use first::{
    LegacyConnectablePeripheralFirstControllerTimeReady,
    LegacyConnectablePeripheralFirstControllerTimeWait, LegacyConnectablePeripheralFirstDrive,
    LegacyConnectablePeripheralFirstDriveStep, LegacyConnectablePeripheralFirstResponsePublication,
    LegacyConnectablePeripheralFirstStoppingStep, PeripheralFirstSessionRetry,
    begin_legacy_connectable_peripheral_first_command_ready,
    begin_legacy_connectable_peripheral_first_response_pending,
    begin_legacy_connectable_peripheral_first_stopping,
};
