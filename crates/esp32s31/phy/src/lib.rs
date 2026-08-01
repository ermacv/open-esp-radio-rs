#![no_std]
#![forbid(unsafe_code)]
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

pub mod phy_bb;
pub mod phy_bluetooth;
pub mod phy_channel;
pub mod phy_cold;
pub mod phy_dc_iq;
pub mod phy_dcode;
pub mod phy_frequency;
pub mod phy_i2c;
mod phy_param;
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
pub mod phy_temperature;
pub mod phy_tx_cal;
pub mod phy_tx_power;
pub mod phy_txdc;
pub mod phy_txdc_pwdet;
pub mod phy_txiq;
pub mod phy_xtal_duty;
mod radio_hal;
mod size_limits;

pub use executor::{PhyRegisterPort, PhyRegisterRunError, run_phy_register};
pub use phy_register::{
    PhyRegisterAction, PhyRegisterCompletion, PhyRegisterExternalBinding, PhyRegisterFailure,
    PhyRegisterLocalStep, PhyRegisterOutcome, PhyRegisterTransition,
    default_phy_register_init_profile,
};
pub use phy_tx_power::{PhyTxTargetPowerPair, PhyTxTargetPowerProfile};
/// Shared one-microsecond sampling bound used by every target executor and by
/// host-side qualification of the same typed timeout contract.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;
#[cfg(target_arch = "riscv32")]
pub use target_executor::{PhyAsyncDelay, PhyTargetPortError};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    NoopPhyTargetObserver, PhyRfBoundary, PhyTargetObserver, PhyTargetPortCounters,
    TargetPhyRegisterPort, select_phy_channel, switch_phy_channel_with_mac_restart,
};
