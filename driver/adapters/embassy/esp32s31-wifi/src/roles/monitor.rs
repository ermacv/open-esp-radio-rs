//! Public standalone-monitor composition.

#[cfg(target_arch = "riscv32")]
mod builder;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod control;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub(crate) mod rx;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod service;

#[cfg(target_arch = "riscv32")]
pub(crate) use control::Esp32s31MonitorCommandReceiver;
pub use control::{
    Esp32s31MonitorCompletion, Esp32s31MonitorControlError, Esp32s31MonitorControlResources,
    Esp32s31MonitorController,
};

#[cfg(target_arch = "riscv32")]
pub use crate::roles::monitor::builder::{
    Esp32s31MonitorBuildError, Esp32s31MonitorBuildReport, Esp32s31MonitorChannelSwitchError,
    Esp32s31MonitorInterruptParts, Esp32s31MonitorInterrupts, Esp32s31MonitorMemory,
    Esp32s31MonitorStopped, Esp32s31MonitorStoppedResourceParts, Esp32s31MonitorStoppedResources,
    Esp32s31MonitorTask, Esp32s31MonitorTaskBuildFailure, Esp32s31MonitorTaskExit,
    Esp32s31MonitorTaskResources, prepare_esp32s31_monitor_task,
};
pub use crate::roles::monitor::rx::{
    Esp32s31MonitorConfigError, Esp32s31MonitorPrepareError, Esp32s31MonitorRxProgress,
};
pub use crate::roles::monitor::service::{
    ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK, Esp32s31MonitorRunError, Esp32s31MonitorRunFailure,
    Esp32s31MonitorRunReport, Esp32s31MonitorStopError, Esp32s31MonitorStoppedAccessError,
};
