#![no_std]
// The private `ieee802154_timing_boundary` module is the sole scoped override.
#![deny(unsafe_code)]
#![doc = "Source-only ESP32-S31 radio frontier."]
#![doc = ""]
#![doc = "This crate deliberately has no dependency on"]
#![doc = "`esp-wifi-sys`, vendor archives, or a radio/Wi-Fi ROM ABI."]

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
pub mod rx;
pub mod state;
pub mod tracking;
pub mod tx;

mod registered_bluetooth;
mod registered_radio;
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
pub use registered_bluetooth::{
    RegisteredBluetoothPhy, RegisteredBluetoothPhyClient, RegisteredBluetoothPhyClientAcquire,
    RegisteredBluetoothPhyClientAcquireFailure, RegisteredBluetoothPhyPendingTrack,
    RegisteredBluetoothPhyPendingTracking, RegisteredBluetoothPhyTrackPoisoned,
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
    RegisteredPhyClientReleaseFailure, RegisteredPhyPendingTrack, RegisteredPhyPendingTracking,
    RegisteredPhyRadio, RegisteredPhyTrackEvaluation, RegisteredPhyTrackEvaluationFailure,
    RegisteredPhyTrackPoisoned,
};
pub use state::{
    PHY_CALIBRATION_SNAPSHOT_SCHEMA, PhyBluetoothCalibration, PhyCalibrationCache,
    PhyCalibrationSnapshot, PhyCommonCalibration, PhyConfig, PhyState, PhyWifiCalibration,
};
pub use tx::power::{PhyTxTargetPowerPair, PhyTxTargetPowerProfile};
/// Shared one-microsecond sampling bound used by every target executor and by
/// host-side qualification of the same typed timeout contract.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;
#[cfg(target_arch = "riscv32")]
pub use target_executor::{PhyAsyncDelay, PhyTargetPortError};
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
    switch_phy_channel_with_hal_and_mac_restart,
};
