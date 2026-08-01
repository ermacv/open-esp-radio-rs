//! Semantic normalization for Rust architectural replacements.
//!
//! These models deliberately live in the validator, not in production PHY
//! code. They relate a pinned vendor call/MMIO timeline to the public actions
//! of a Rust state machine without requiring identical stack layout, polling
//! loops, or instruction structure.

mod bluetooth_tx_power;
mod bluetooth_txdc;
mod bluetooth_txdc_pwdet;
mod channel;
mod rf_init;
mod runner;
mod state;

pub use bluetooth_tx_power::*;
pub use bluetooth_txdc::*;
pub use bluetooth_txdc_pwdet::*;
pub use channel::*;
pub use rf_init::*;
#[cfg(test)]
use rf_init::{rf_phase, vendor_rf_init_phase};
pub(crate) use runner::{
    qualify_esp32s31_bluetooth_tx_power, qualify_esp32s31_bluetooth_txdc,
    qualify_esp32s31_bluetooth_txdc_pwdet, qualify_esp32s31_channel, qualify_esp32s31_rf_init,
};
pub use state::*;
use state::{
    CHANNEL_STATE_FOOTPRINT, RF_INIT_STATE_FOOTPRINT, declare_state_ownership,
    validate_state_footprint,
};

use open_esp_radio_esp32s31_phy::{
    phy_bluetooth::{
        PhyBluetoothTxDcPwdetTransition, PhyBluetoothTxDcTransition, PhyBluetoothTxPowerAction,
        PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerTransition,
    },
    phy_channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, calculate_wifi_tx_gain,
    },
    phy_cold::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdLocalStep, PhyColdObservationRequest,
        PhyColdObservationResult, PhyColdPbusAction, PhyColdPbusHardwareResult, PhyColdState,
        PhyRfColdInit,
    },
    phy_dc_iq::{PhyDcIqAccumulatorSnapshot, PhyDcIqReadinessSnapshot},
    phy_i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixOutcome},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    phy_signal_power::PhySignalPowerAccumulatorSnapshot,
    phy_temperature::{PhyTemperatureAction, PhyTemperatureCompletion},
    phy_tx_cal::{PhyToneSarAction, PhyToneSarCompletion},
    phy_tx_power::{
        PhyPowerControlPointAction, PhyPowerControlPointCompletion, PhyTxPowerAction,
        PhyTxPowerCompletion,
    },
    phy_txdc::{PhyTxDcAction, PhyTxDcCompletion, PhyTxDcParameters},
    phy_txdc_pwdet::{
        PhyTxDcPwdetAction, PhyTxDcPwdetCompletion, PhyTxDcPwdetSearchAction,
        PhyTxDcPwdetSearchCompletion,
    },
};

use crate::{Result, execution, seed_ram_word};

const ROM_PHY_FUNCTION_TABLE: u32 = 0x2f07_f944;
const ROM_PHY_FUNCTION_TABLE_POINTER: u32 = 0x2f07_fc3c;
const ROM_PHY_PARAM_POINTER: u32 = 0x2f07_fc40;
const ROM_PHY_FUNCTIONS: [u32; 13] = [
    0x2f82_9f18,
    0x2f82_9f1a,
    0x2f82_9f84,
    0x2f82_9fc0,
    0x2f82_44fe,
    0x2f82_78b0,
    0x2f82_5dc8,
    0x2f82_5ecc,
    0x2f82_5f7c,
    0x2f82_711c,
    0x2f82_7392,
    0x2f82_66da,
    0x2f82_88de,
];

const TEMPERATURE_DAC: u8 = 5;
const TEMPERATURE_CODE: u8 = 0;
const TX_CAP_READ: u8 = 0;

#[cfg(test)]
mod tests;
