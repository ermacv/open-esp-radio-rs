//! ESP32-S31 runtime ABI and lifecycle contracts.
//!
//! This crate is target data.  It deliberately contains no ELF loader,
//! instruction decoder, verifier policy, vendor artifact hashes, or private
//! artifact paths.

use open_radio_vendor_contracts::{DiagnosticCallSpec, HarnessContractSpec};

pub mod entry_contract;
pub mod external_abi;

const EXTERNAL_TABLES: &[open_radio_vendor_contracts::ExternalTableRef] =
    &[external_abi::WIFI_OSI_V9];
const DIAGNOSTIC_CALLS: &[DiagnosticCallSpec] = &[
    DiagnosticCallSpec {
        symbol: "wifi_log",
        argument_count: 6,
    },
    // ESP-IDF's Wi-Fi archive passes condition, expression/file text,
    // function text and line number. The harness records the boundary; it
    // does not import the vendor assertion implementation into production.
    DiagnosticCallSpec {
        symbol: "wifi_assert",
        argument_count: 4,
    },
    // The PHY archive uses a printf-style diagnostic hook. Only the format
    // pointer has a stable ABI position; variadic values are intentionally
    // excluded so they cannot accidentally become verification inputs.
    DiagnosticCallSpec {
        symbol: "phy_printf",
        argument_count: 1,
    },
];

pub const CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_tables: EXTERNAL_TABLES,
    entry_contracts: entry_contract::ALL,
    diagnostic_calls: DIAGNOSTIC_CALLS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_abi_keeps_assertion_and_logging_distinct() {
        assert_eq!(DIAGNOSTIC_CALLS[0].symbol, "wifi_log");
        assert_eq!(DIAGNOSTIC_CALLS[0].argument_count, 6);
        assert_eq!(DIAGNOSTIC_CALLS[1].symbol, "wifi_assert");
        assert_eq!(DIAGNOSTIC_CALLS[1].argument_count, 4);
        assert_eq!(DIAGNOSTIC_CALLS[2].symbol, "phy_printf");
        assert_eq!(DIAGNOSTIC_CALLS[2].argument_count, 1);
    }
}
