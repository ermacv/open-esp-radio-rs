//! Reusable ESP32-S31 rev0 ROM contracts.
//!
//! This crate is intentionally narrower than an investigation harness. It
//! contains no linked archive symbols, vendor-library ABI tables, body bytes,
//! verification profiles or artifact paths.

use open_radio_vendor_contracts::{DiagnosticCallSpec, KnowledgeContractSpec};

pub mod entry_contract;

pub const ETS_PRINTF_DIAGNOSTIC: DiagnosticCallSpec = DiagnosticCallSpec {
    symbol: "ets_printf",
    argument_count: 1,
};

const DIAGNOSTIC_CALLS: &[DiagnosticCallSpec] = &[ETS_PRINTF_DIAGNOSTIC];

pub const CONTRACTS: KnowledgeContractSpec = KnowledgeContractSpec {
    external_call_model_sets: &[],
    entry_contracts: entry_contract::ALL,
    diagnostic_calls: DIAGNOSTIC_CALLS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusable_contracts_contain_only_cold_rom_knowledge() {
        assert_eq!(CONTRACTS.entry_contracts, entry_contract::ALL);
        assert_eq!(CONTRACTS.diagnostic_calls, [ETS_PRINTF_DIAGNOSTIC]);
        assert!(CONTRACTS.external_call_model_sets.is_empty());
        assert!(CONTRACTS.entry_contract("esp32s31-phy-cold").is_some());
        assert!(
            CONTRACTS
                .entry_contract("esp32s31-phy-registered")
                .is_none()
        );
    }
}
