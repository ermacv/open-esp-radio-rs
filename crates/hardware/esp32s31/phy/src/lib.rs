#![no_std]
// The private `ieee802154_timing_boundary` module is the sole scoped override.
#![deny(unsafe_code)]

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
#[cfg(target_arch = "riscv32")]
pub mod target_executor;
#[cfg(target_arch = "riscv32")]
pub mod target_port;

pub mod analog;
pub mod calibration;
pub mod channel;
mod hardware;
mod ieee802154_timing_boundary;
#[cfg(any(target_arch = "riscv32", test))]
mod lifecycle;
pub mod rx;
pub mod state;
pub mod tracking;
pub mod tx;

mod registered_bluetooth;
mod registered_radio;
mod registered_wifi;
pub use registered_wifi::{
    RegisteredWifiPhy, RegisteredWifiPhyClientRelease, RegisteredWifiPhyClientReleaseFailure,
    WifiPhyMaintenanceRequest,
};
#[cfg(target_arch = "riscv32")]
pub use registered_wifi::{WifiPhyMaintenanceError, WifiPhyMaintenanceFailure};
mod size_limits;
#[cfg(feature = "validation-probes")]
pub mod validation;

pub use calibration::registration::{
    PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterAction, PhyRegisterCompletion,
    PhyRegisterExternalBinding, PhyRegisterFailure, PhyRegisterLocalStep, PhyRegisterOutcome,
    PhyRegisterTransition, RegisteredPhyState,
};
pub use executor::{
    PhyCalibrationTrackingPort, PhyCalibrationTrackingRunError, PhyParamTrackingPort,
    PhyParamTrackingRunError, PhyRegisterPort, PhyRegisterRunError, run_phy_calibration_tracking,
    run_phy_param_tracking, run_phy_register,
};
#[cfg(any(target_arch = "riscv32", test))]
pub use lifecycle::{
    PhyRfWakeAction, PhyRfWakeCompletion, PhyRfWakeOperation, PhyRfWakeOutcome,
    PhyRfWakeTransition, PhyRfWakeTransitionError,
};
#[cfg(target_arch = "riscv32")]
pub use registered_bluetooth::BluetoothPhyRfCloseFailure;
pub use registered_bluetooth::{
    RegisteredBluetoothPhy, RegisteredBluetoothPhyClient, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyClientRelease,
    RegisteredBluetoothPhyClientReleaseFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, RegisteredBluetoothPhyRfClosed,
    RegisteredBluetoothPhyTrackEvaluation, RegisteredBluetoothPhyTrackEvaluationFailure,
    RegisteredBluetoothPhyTrackPoisoned,
};
pub use registered_radio::{
    RegisteredIeee802154Client, RegisteredIeee802154ClientAcquire,
    RegisteredIeee802154ClientAcquireFailure, RegisteredIeee802154Clocked,
    RegisteredIeee802154FoundationConfigured, RegisteredIeee802154FoundationTransitionFailure,
    RegisteredIeee802154MacPolicyConfigured, RegisteredIeee802154MacPolicyRecovery,
    RegisteredIeee802154MacPolicyTransitionFailure, RegisteredIeee802154OperationCompleted,
    RegisteredIeee802154OperationFailed, RegisteredIeee802154PendingTrack,
    RegisteredIeee802154PendingTracking, RegisteredIeee802154Reset,
    RegisteredIeee802154ResetTransitionFailure, RegisteredIeee802154TimingReady,
    RegisteredIeee802154TrackPoisoned, RegisteredPhyClientAcquire,
    RegisteredPhyClientAcquireFailure, RegisteredPhyClientRelease,
    RegisteredPhyClientReleaseDisposition, RegisteredPhyClientReleaseFailure,
    RegisteredPhyPendingTrack, RegisteredPhyPendingTracking, RegisteredPhyPoweredIdle,
    RegisteredPhyRadio, RegisteredPhyRfClosed, RegisteredPhyTrackEvaluation,
    RegisteredPhyTrackEvaluationFailure, RegisteredPhyTrackPoisoned,
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
    NoopPhyTargetObserver, PhyRfBoundary, PhyTargetObserver, PhyTargetPortCounters,
    TargetBluetoothPhyParamTrackingFailure, TargetBluetoothPhyParamTrackingSuccess,
    TargetBluetoothPhyRegisterConfig, TargetBluetoothPhyRegisterError,
    TargetBluetoothPhyRegisterFailure, TargetBluetoothPhyRegisterSuccess,
    TargetIeee802154PhyParamTrackingFailure, TargetIeee802154PhyParamTrackingSuccess,
    TargetIeee802154PhyRegisterConfig, TargetIeee802154PhyRegisterError,
    TargetIeee802154PhyRegisterFailure, TargetIeee802154PhyRegisterSuccess,
    TargetPhyCalibrationTrackingPort, TargetPhyParamTrackingError, TargetPhyParamTrackingFailure,
    TargetPhyParamTrackingPort, TargetPhyParamTrackingSuccess, TargetPhyRegisterAttempt,
    TargetPhyRegisterError, TargetPhyRegisterFailure, TargetPhyRegisterPort,
    TargetPhyRegisterSuccess, TargetPhyRegisterTerminalParts,
    run_target_bluetooth_phy_param_tracking, run_target_bluetooth_phy_register,
    run_target_ieee802154_phy_param_tracking, run_target_ieee802154_phy_register,
    run_target_phy_param_tracking, run_target_phy_register, select_phy_channel_with_hal,
    select_registered_wifi_channel, switch_phy_channel_with_hal_and_mac_restart,
    switch_registered_wifi_channel,
};
