//! Semantic normalization for Rust architectural replacements.
//!
//! These models deliberately live in the workbench, not in production PHY
//! code. They relate a pinned vendor call/MMIO timeline to the public actions
//! of a Rust state machine without requiring identical stack layout, polling
//! loops, or instruction structure.

use std::path::Path;

use sha2::{Digest, Sha256};

mod bb_init;
mod bluetooth_tx_gain;
mod bluetooth_tx_power;
mod bluetooth_txdc;
mod bluetooth_txdc_pwdet;
mod channel;
mod completion;
mod hal_mac_txq_enable;
mod iq_estimator;
mod report;
mod rf_init;
mod runner;
mod sta_join_state;
mod state;
mod wdev_append_rx_blocks;
mod wdev_process_fiq;

pub use bb_init::*;
pub use bluetooth_tx_gain::*;
pub use bluetooth_tx_power::*;
pub use bluetooth_txdc::*;
pub use bluetooth_txdc_pwdet::*;
pub use channel::*;
use completion::DeterministicPhyCompletion;
pub use hal_mac_txq_enable::*;
pub use iq_estimator::*;
pub use report::*;
pub use rf_init::*;
#[cfg(test)]
use rf_init::{rf_phase, vendor_rf_init_phase};
pub use runner::{
    verify_esp32s31_bluetooth_tx_gain_init, verify_esp32s31_bluetooth_tx_power,
    verify_esp32s31_bluetooth_txdc, verify_esp32s31_bluetooth_txdc_pwdet, verify_esp32s31_channel,
    verify_esp32s31_rf_init,
};
pub use sta_join_state::*;
pub use state::*;
use state::{
    CHANNEL_STATE_FOOTPRINT, RF_INIT_STATE_FOOTPRINT, declare_state_ownership,
    validate_state_footprint,
};
pub use wdev_append_rx_blocks::*;
pub use wdev_process_fiq::*;

use open_esp_radio_esp32s31_phy::{
    phy_bluetooth::{
        PhyBluetoothTxDcPwdetTransition, PhyBluetoothTxDcTransition, PhyBluetoothTxGainInitAction,
        PhyBluetoothTxGainInitCompletion, PhyBluetoothTxGainInitLocalStep,
        PhyBluetoothTxPowerAction, PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerTransition,
    },
    phy_channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, calculate_wifi_tx_gain,
    },
    phy_cold::{
        PhyColdExternalBinding, PhyColdI2cAction, PhyColdLocalStep, PhyColdObservationRequest,
        PhyColdObservationResult, PhyColdPbusAction, PhyColdPbusHardwareResult, PhyRfColdInit,
    },
    phy_dc_iq::{PhyDcIqAccumulatorSnapshot, PhyDcIqReadinessSnapshot},
    phy_i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixOutcome},
    phy_rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion},
    phy_signal_power::PhySignalPowerAccumulatorSnapshot,
    phy_state::PhyState,
    phy_temperature::{PhyTemperatureAction, PhyTemperatureCompletion},
    phy_tx_cal::{PhyToneSarAction, PhyToneSarCompletion},
    phy_tx_power::{
        PhyPowerControlPointAction, PhyPowerControlPointCompletion, PhyTxPowerAction,
        PhyTxPowerCompletion,
    },
    phy_txdc::{PhyTxDcAction, PhyTxDcCompletion},
    phy_txdc_pwdet::{
        PhyTxDcPwdetAction, PhyTxDcPwdetCompletion, PhyTxDcPwdetSearchAction,
        PhyTxDcPwdetSearchCompletion,
    },
};

use crate::{DriverAdapterVerification, Result, entry_contract, execution, seed_ram_word};

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn inventory_symbol_sha256(path: &Path, member: Option<&str>, symbol_name: &str) -> Result<String> {
    let symbols = crate::artifact::load_code_symbols(
        path,
        symbol_name,
        crate::artifact::CodeSymbolSelection::Exported,
    )?;
    let matches = symbols
        .iter()
        .filter(|symbol| {
            symbol.name == symbol_name
                && member.is_none_or(|member| symbol.member.as_deref() == Some(member))
        })
        .collect::<Vec<_>>();
    let [symbol] = matches.as_slice() else {
        return Err(format!(
            "expected exactly one inventory definition for {}{}, found {}",
            member.map_or_else(String::new, |member| format!("{member}::")),
            symbol_name,
            matches.len()
        )
        .into());
    };

    inventory_symbol_definition_sha256(symbol)
}

fn inventory_symbol_definition_sha256(
    symbol: &crate::artifact::ArtifactSymbolDefinition,
) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"riscv32-inventory-symbol-v1");
    hash_field(
        &mut hasher,
        symbol.member.as_deref().unwrap_or("").as_bytes(),
    );
    hash_field(&mut hasher, symbol.name.as_bytes());
    hash_field(&mut hasher, &[u8::from(symbol.addresses_resolved)]);
    hash_field(&mut hasher, &symbol.bytes);
    hasher.update((symbol.relocations.len() as u64).to_le_bytes());
    let symbol_start = u32::try_from(symbol.address).map_err(|_| {
        format!(
            "inventory symbol {} exceeds RV32 address space",
            symbol.name
        )
    })?;
    for relocation in &symbol.relocations {
        let relative = relocation
            .address
            .checked_sub(symbol_start)
            .ok_or_else(|| {
                format!(
                    "relocation at {:#x} precedes inventory symbol {}",
                    relocation.address, symbol.name
                )
            })?;
        hasher.update(relative.to_le_bytes());
        let kind = match relocation.kind {
            crate::artifact::RelocationKind::GotHi20 => b"got-hi20".as_slice(),
            crate::artifact::RelocationKind::Hi20 => b"hi20".as_slice(),
            crate::artifact::RelocationKind::Lo12I => b"lo12-i".as_slice(),
            crate::artifact::RelocationKind::Lo12S => b"lo12-s".as_slice(),
            crate::artifact::RelocationKind::PcRelHi20 => b"pcrel-hi20".as_slice(),
            crate::artifact::RelocationKind::PcRelLo12I => b"pcrel-lo12-i".as_slice(),
            crate::artifact::RelocationKind::PcRelLo12S => b"pcrel-lo12-s".as_slice(),
            crate::artifact::RelocationKind::GotPcRelLo12I => b"got-pcrel-lo12-i".as_slice(),
            crate::artifact::RelocationKind::Call => b"call".as_slice(),
            crate::artifact::RelocationKind::CallPlt => b"call-plt".as_slice(),
        };
        hash_field(&mut hasher, kind);
        hash_field(&mut hasher, relocation.symbol.as_bytes());
        hasher.update(relocation.addend.to_le_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn code_closure_sha256(image: &execution::ExecutableImage, symbol: &str) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(image.code_closure_identity(symbol)?.as_bytes())
    ))
}

const ROM_PHY_FUNCTION_TABLE: u32 = entry_contract::ROM_PHY_FUNCTION_TABLE;
const ROM_PHY_FUNCTION_TABLE_POINTER: u32 = 0x2f07_fc3c;
const ROM_PHY_PARAM_POINTER: u32 = 0x2f07_fc40;

const TEMPERATURE_DAC: u8 = 5;
const TEMPERATURE_CODE: u8 = 0;
const TX_CAP_READ: u8 = 0;

/// Size of the pinned vendor `phy_param` image.
///
/// This is vendor ABI evidence and deliberately does not describe the layout
/// or size of Rust [`PhyState`].
const VENDOR_PHY_PARAM_LEN: u32 = 0x1fc;

#[cfg(test)]
mod tests;
