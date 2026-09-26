#![no_std]
// The `ieee802154_client` module is the sole scoped override.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
// The registration and tracking graphs run only through the chip target
// ports, which exist only for `riscv32`. Host builds type-check the graphs and
// their hardware bindings, and tests drive the models, but nothing on the host
// calls every binding. Dead code is enforced by the chip build.
#![cfg_attr(
    not(target_arch = "riscv32"),
    allow(
        dead_code,
        unused_imports,
        reason = "host builds type-check the graphs that only chip target ports drive"
    )
)]

//! Source-only ESP32-S31 shared RF/PHY ownership frontier.
//!
//! This crate turns a cold radio owner into a registered calibration epoch,
//! admits protocol clients, performs client-specific maintenance, and closes
//! or retains the shared RF domain. It deliberately has no dependency on
//! `esp-wifi-sys`, vendor archives, or a radio/Wi-Fi ROM ABI. PAC ownership and
//! semantic register access come from the HAL; protocol roles and executors
//! live above this layer.
//!
//! Start with [`PhyConfig`] and the registration state machine, then use
//! [`RegisteredPhyRadio`] to acquire a Wi-Fi, Bluetooth, or IEEE 802.15.4
//! client. [`RegisteredWifiPhy`] and [`RegisteredBluetoothPhy`] keep the
//! registration identity and tracking state attached to the live client.
//! Releasing a non-final client returns a still-powered shared owner; only the
//! final-client close path may proceed toward a cold owner.
//!
//! # Ownership and cancellation
//!
//! Registration, tracking, RF close, and retained wake are hardware
//! transactions. Preparation errors that expose an owner are retryable only
//! through that returned owner. Failures after a physical transition retain a
//! poisoned/fail-stop owner instead of claiming recovery. Once an async close,
//! wake, or maintenance future has been polled across its hardware edge, the
//! caller must drive it to a terminal result; dropping it does not reconstruct
//! the preceding typestate.
//!
//! Timing types identify raw ticks, microseconds, or absolute deadlines at
//! their owning API. [`HARDWARE_EDGE_LIMIT`] is an observation-attempt limit,
//! not a wall-clock duration. The chip Wi-Fi and Bluetooth drivers are the
//! real target consumers; host tests exercise the pure transitions and
//! validation probes without claiming RF readiness.

#[cfg(test)]
extern crate std;

pub mod executor;
#[cfg(feature = "lifecycle-fault-injection")]
pub mod fault_injection;
#[cfg(all(target_arch = "riscv32", feature = "validation-probes"))]
pub mod target_executor;
#[cfg(all(target_arch = "riscv32", not(feature = "validation-probes")))]
mod target_executor;
#[cfg(all(target_arch = "riscv32", feature = "validation-probes"))]
pub mod target_port;
#[cfg(all(target_arch = "riscv32", not(feature = "validation-probes")))]
mod target_port;

#[cfg(feature = "validation-probes")]
pub mod analog;
#[cfg(not(feature = "validation-probes"))]
mod analog;
#[cfg(feature = "validation-probes")]
pub mod calibration;
#[cfg(not(feature = "validation-probes"))]
mod calibration;
#[cfg(feature = "validation-probes")]
pub mod channel;
#[cfg(not(feature = "validation-probes"))]
mod channel;
mod hardware;
#[cfg(any(target_arch = "riscv32", test))]
mod lifecycle;
#[cfg(feature = "validation-probes")]
pub mod rx;
#[cfg(not(feature = "validation-probes"))]
mod rx;
pub mod state;
pub mod tracking;
#[cfg(feature = "validation-probes")]
pub mod tx;
#[cfg(not(feature = "validation-probes"))]
mod tx;

pub mod concurrent;
pub mod ieee802154_client;
mod registered_bluetooth;
mod registered_radio;
pub mod registered_route;
mod registered_wifi;
mod retained;
pub use registered_wifi::{
    RegisteredWifiPhy, RegisteredWifiPhyClientReleaseError, RegisteredWifiPhyClientReleaseFailure,
    WifiPhyMaintenanceRequest,
};
#[cfg(target_arch = "riscv32")]
pub use registered_wifi::{WifiPhyMaintenanceError, WifiPhyMaintenanceFailure};
pub use retained::{RetainedPhy, RetainedPhyMismatch};
mod size_limits;
#[cfg(feature = "validation-probes")]
pub mod validation;

/// Vendor RF-calibration version stamped into calibration caches.
pub use analog::rfpll::phy_get_rf_cal_version;
// Value results of registration children that protocol reports and HIL
// evidence carry. The transitions that produce them stay crate-private.
#[cfg(feature = "registration-diagnostics")]
pub use calibration::registration::RfCalibrationDiagnostics;
pub use calibration::registration::{
    PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterBindingError, PhyRegisterFailure,
    PhyRegisterOutcome, PhyRegisterStage, RegisteredPhyState,
};
#[cfg(feature = "validation-probes")]
pub use calibration::registration::{
    PhyRegisterAction, PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterLocalStep,
    PhyRegisterTransition,
};
#[cfg(feature = "validation-probes")]
pub use executor::{
    PhyCalibrationTrackingPort, PhyParamTrackingPort, PhyRegisterPort,
    run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
};
pub use executor::{PhyCalibrationTrackingRunError, PhyParamTrackingRunError, PhyRegisterRunError};
#[cfg(all(any(target_arch = "riscv32", test), feature = "validation-probes"))]
pub use lifecycle::{
    PhyRfWakeAction, PhyRfWakeCompletion, PhyRfWakeOperation, PhyRfWakeOutcome,
    PhyRfWakeTransition, PhyRfWakeTransitionError,
};
#[cfg(target_arch = "riscv32")]
pub use registered_bluetooth::{
    BluetoothPhyMaintenanceFailure, BluetoothPhyRfCloseFailure, BluetoothPhyRfWakeFailure,
};
pub use registered_bluetooth::{
    RegisteredBluetoothPhy, RegisteredBluetoothPhyClient, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyClientRelease,
    RegisteredBluetoothPhyClientReleaseFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, RegisteredBluetoothPhyRfClosed,
    RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure,
    RegisteredBluetoothPhyTrackPoisoned,
};
pub use registered_radio::{
    RegisteredPhyClientAcquire, RegisteredPhyClientAcquireFailure, RegisteredPhyClientRelease,
    RegisteredPhyClientReleaseDisposition, RegisteredPhyClientReleaseFailure,
    RegisteredPhyPendingTrack, RegisteredPhyPendingTracking, RegisteredPhyPoweredIdle,
    RegisteredPhyRadio, RegisteredPhyRetainedReleaseFailure, RegisteredPhyRfClosed,
    RegisteredPhyTrackEvaluation, RegisteredPhyTrackEvaluationFailure, RegisteredPhyTrackPoisoned,
};
#[cfg(target_arch = "riscv32")]
pub use registered_radio::{
    RegisteredPhyColdReleaseFailure, RegisteredPhyColdReleased, RegisteredPhyRfCloseFailure,
    RegisteredPhyRfClosePoisoned, RegisteredPhyRfClosePreparationFailure,
    RegisteredPhyRfWakePoisoned,
};
pub use state::{
    PHY_CALIBRATION_SNAPSHOT_SCHEMA, PhyBluetoothCalibration, PhyCalibrationCache,
    PhyCalibrationSnapshot, PhyCommonCalibration, PhyConfig, PhyState, PhyWifiCalibration,
};
pub use tx::power::{PhyTxTargetPowerPair, PhyTxTargetPowerProfile};
pub use {
    analog::{
        dcode::PhyDcodeOutcome, pbus::PhyPbusClearOutcome, temperature::PhyTemperatureOutcome,
    },
    channel::PhyChipChannelFailure,
    rx::{gain::PhyRxGainInitOutcome, gain_calibration::PhyRxGainDcQuality},
    tx::dc_power_detector::PhyTxDcPwdetOutcome,
};
/// Shared finite observation/attempt bound used by target executors and host
/// checks of typed timeout paths. This is not a microsecond duration: direct
/// readiness sampling and timer-backed bus retries have different costs.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_hal::phy::delay::RomShortDelay;
#[cfg(target_arch = "riscv32")]
pub use target_executor::{PhyAsyncDelay, PhyShortDelay, PhyTargetPortError};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyRegistration, ConcurrentPhyTrackingError,
    ConcurrentRfError, close_concurrent_rf, maintain_concurrent_phy, register_concurrent_phy,
    wake_concurrent_rf,
};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    NoopPhyTargetObserver, PhyDomainRegisterFailure, PhyDomainRegistered, PhyRegisterConfig,
    PhyRfBoundary, PhyRfCloseFailure, PhyRfWakeFailure, PhyRfWakePoisoned, PhyTargetObserver,
    PhyTargetPortCounters, PhyTrackingFailure, PhyTrackingSuccess, TargetPhyParamTrackingError,
    TargetPhyParamTrackingFailure, TargetPhyParamTrackingSuccess, TargetPhyRegisterAttempt,
    TargetPhyRegisterError, TargetPhyRegisterFailure, TargetPhyRegisterSuccess,
    TargetPhyRegisterTerminalParts, run_target_phy_param_tracking, run_target_phy_register,
    select_registered_wifi_channel, switch_registered_wifi_channel,
};
#[cfg(all(target_arch = "riscv32", feature = "validation-probes"))]
pub use target_port::{
    TargetPhyCalibrationTrackingPort, TargetPhyParamTrackingPort, TargetPhyRegisterPort,
};
