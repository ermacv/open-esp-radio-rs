//! Application-facing ESP32-S31 station lifecycle facade.
//!
//! The public facade preserves stable application paths while the implementation
//! separates command publication, one connected epoch and the outer reconnect
//! lifecycle.

pub use crate::station_tasks::{
    Esp32s31ConnectedTaskGroup, Esp32s31ConnectedTaskStopOutcome,
    stop_esp32s31_connected_task_group,
};

mod command;
mod connected_epoch;
mod lifecycle;

pub use command::{
    Esp32s31StationCommand, Esp32s31StationCommandReceiver, Esp32s31StationControlResources,
    Esp32s31StationController,
};
pub use connected_epoch::{
    Esp32s31ConnectedStationExit, Esp32s31StationReconnectSource,
    run_esp32s31_connected_station_epoch,
};
pub use lifecycle::{
    Esp32s31Station, Esp32s31StationConfig, Esp32s31StationExit, Esp32s31StationResources,
    Esp32s31StationRunner, Esp32s31StationStopReason,
};

#[cfg(test)]
mod tests;
