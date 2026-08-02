//! ESP32-S31 runtime ABI and lifecycle contracts.
//!
//! This crate is target data.  It deliberately contains no ELF loader,
//! instruction decoder, verifier policy, vendor artifact hashes, or private
//! artifact paths.

use open_radio_vendor_validator_core::{DiagnosticCallSpec, HarnessContractSpec};

pub mod entry_contract;
pub mod external_abi;

const EXTERNAL_TABLES: &[open_radio_vendor_validator_core::ExternalTableRef] =
    &[external_abi::WIFI_OSI_V9];
const DIAGNOSTIC_CALLS: &[DiagnosticCallSpec] = &[DiagnosticCallSpec {
    symbol: "wifi_log",
    argument_count: 6,
}];

pub const CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_tables: EXTERNAL_TABLES,
    entry_contracts: entry_contract::ALL,
    diagnostic_calls: DIAGNOSTIC_CALLS,
};
