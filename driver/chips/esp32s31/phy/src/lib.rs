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

mod ieee802154_timing_boundary;
pub mod phy_bb;
pub mod phy_bluetooth;
pub mod phy_channel;
pub mod phy_client;
pub mod phy_cold;
pub mod phy_dc_iq;
pub mod phy_dcode;
pub mod phy_frequency;
mod phy_hardware;
pub mod phy_i2c;
pub mod phy_math;
pub mod phy_param_tracking;
pub mod phy_pbus;
pub mod phy_pbus_memory;
pub mod phy_pwdet;
pub mod phy_register;
pub mod phy_rfpll;
pub mod phy_rx_dco;
pub mod phy_rx_gain;
pub mod phy_rx_gain_cal;
pub mod phy_rx_saturation;
pub mod phy_rxiq;
pub mod phy_signal_power;
pub mod phy_state;
pub mod phy_temperature;
pub mod phy_tx_cal;
pub mod phy_tx_power;
pub mod phy_txdc;
pub mod phy_txdc_pwdet;
pub mod phy_txiq;
pub mod phy_xtal_duty;
mod registered_radio;
mod size_limits;
#[cfg(feature = "validation-probes")]
pub mod validation;

pub use executor::{PhyRegisterPort, PhyRegisterRunError, run_phy_register};
pub use phy_register::{
    PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterAction, PhyRegisterCompletion,
    PhyRegisterExternalBinding, PhyRegisterFailure, PhyRegisterLocalStep, PhyRegisterOutcome,
    PhyRegisterTransition, RegisteredPhyState,
};
pub use phy_state::{
    PHY_CALIBRATION_SNAPSHOT_SCHEMA, PhyBluetoothCalibration, PhyCalibrationCache,
    PhyCalibrationSnapshot, PhyCommonCalibration, PhyConfig, PhyState, PhyWifiCalibration,
};
pub use phy_tx_power::{PhyTxTargetPowerPair, PhyTxTargetPowerProfile};
pub use registered_radio::{
    RegisteredIeee802154Clocked, RegisteredIeee802154FoundationConfigured,
    RegisteredIeee802154FoundationTransitionFailure, RegisteredIeee802154MacPolicyConfigured,
    RegisteredIeee802154MacPolicyRecovery, RegisteredIeee802154MacPolicyTransitionFailure,
    RegisteredIeee802154OperationCompleted, RegisteredIeee802154OperationFailed,
    RegisteredIeee802154Reset, RegisteredIeee802154ResetTransitionFailure,
    RegisteredIeee802154TimingReady, RegisteredPhyRadio,
};
/// Shared one-microsecond sampling bound used by every target executor and by
/// host-side qualification of the same typed timeout contract.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;
#[cfg(target_arch = "riscv32")]
pub use target_executor::{PhyAsyncDelay, PhyTargetPortError};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    NoopPhyTargetObserver, PhyRfBoundary, PhyTargetObserver, PhyTargetPortCounters,
    TargetIeee802154PhyRegisterConfig, TargetIeee802154PhyRegisterError,
    TargetIeee802154PhyRegisterFailure, TargetIeee802154PhyRegisterSuccess,
    TargetPhyRegisterAttempt, TargetPhyRegisterError, TargetPhyRegisterFailure,
    TargetPhyRegisterPort, TargetPhyRegisterSuccess, TargetPhyRegisterTerminalParts,
    run_target_ieee802154_phy_register, run_target_phy_register, select_phy_channel_with_hal,
    switch_phy_channel_with_hal_and_mac_restart,
};
