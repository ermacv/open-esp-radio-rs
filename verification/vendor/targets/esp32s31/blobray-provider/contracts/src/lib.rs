//! ESP32-S31 runtime ABI and lifecycle contracts.
//!
//! This crate is target data.  It deliberately contains no ELF loader,
//! instruction decoder, verifier policy, vendor artifact hashes, or private
//! artifact paths.

use open_radio_vendor_chip_contracts_esp32s31_rev0::ETS_PRINTF_DIAGNOSTIC;
use open_radio_vendor_contracts::{DiagnosticCallSpec, KnowledgeContractSpec};

pub mod entry_contract;
pub mod external_abi;

const EXTERNAL_CALL_MODEL_SETS: &[open_radio_vendor_contracts::ExternalCallModelSetRef] = &[
    external_abi::WIFI_OSI_MODELS_V9,
    external_abi::COEX_ADAPTER_MODELS_V2,
    external_abi::WIFI_RUNTIME_CALLBACKS_V1,
    external_abi::BLE_EXTERNAL_FUNCTION_MODELS_20250819,
];
const DIAGNOSTIC_CALLS: &[DiagnosticCallSpec] = &[
    // The complete BLE log wrappers only filter, timestamp and publish one
    // diagnostic record through the separately reviewed BLE logging ABI.
    // Keep their stable payload arity visible without importing the private
    // nullable logging environment into Controller behavior.
    DiagnosticCallSpec {
        symbol: "r_ble_log_internal_hex",
        argument_count: 3,
    },
    DiagnosticCallSpec {
        symbol: "r_ble_log_internal_x0",
        argument_count: 1,
    },
    DiagnosticCallSpec {
        symbol: "r_ble_log_internal_x1",
        argument_count: 2,
    },
    DiagnosticCallSpec {
        symbol: "r_ble_log_internal_x2",
        argument_count: 3,
    },
    DiagnosticCallSpec {
        symbol: "r_ble_log_internal_x3",
        argument_count: 4,
    },
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
    // These private archive hooks are printf-style diagnostic sinks. Their
    // stable boundary is the format pointer; variadic payload registers are
    // deliberately not promoted to reviewed semantic inputs.
    DiagnosticCallSpec {
        symbol: "pp_printf",
        argument_count: 1,
    },
    DiagnosticCallSpec {
        symbol: "net80211_printf",
        argument_count: 1,
    },
    DiagnosticCallSpec {
        symbol: "coexist_printf",
        argument_count: 1,
    },
    // ESP32-S31 archive relocations target the ROM printf entry directly.
    // The format pointer is the only stable input retained at this variadic
    // diagnostic boundary; payload registers do not become semantic inputs.
    ETS_PRINTF_DIAGNOSTIC,
];

pub const CONTRACTS: KnowledgeContractSpec = KnowledgeContractSpec {
    external_call_model_sets: EXTERNAL_CALL_MODEL_SETS,
    entry_contracts: entry_contract::ALL,
    diagnostic_calls: DIAGNOSTIC_CALLS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_abi_keeps_assertion_and_logging_distinct() {
        let call = |symbol| {
            DIAGNOSTIC_CALLS
                .iter()
                .find(|call| call.symbol == symbol)
                .expect("reviewed diagnostic call")
        };
        assert_eq!(call("wifi_log").argument_count, 6);
        assert_eq!(call("wifi_assert").argument_count, 4);
        assert_eq!(call("phy_printf").argument_count, 1);
        assert_eq!(call("r_ble_log_internal_hex").argument_count, 3);
        assert_eq!(call("r_ble_log_internal_x0").argument_count, 1);
        assert_eq!(call("r_ble_log_internal_x1").argument_count, 2);
        assert_eq!(call("r_ble_log_internal_x2").argument_count, 3);
        assert_eq!(call("r_ble_log_internal_x3").argument_count, 4);
        assert!(
            [
                "pp_printf",
                "net80211_printf",
                "coexist_printf",
                "ets_printf"
            ]
            .iter()
            .all(|symbol| call(symbol).argument_count == 1)
        );
    }

    #[test]
    fn project_contracts_are_an_explicit_superset_of_chip_contracts() {
        let chip = open_radio_vendor_chip_contracts_esp32s31_rev0::CONTRACTS;
        assert!(chip.entry_contracts.iter().all(|base| {
            CONTRACTS
                .entry_contracts
                .iter()
                .any(|combined| combined.id() == base.id())
        }));
        assert!(chip.diagnostic_calls.iter().all(|base| {
            CONTRACTS
                .diagnostic_calls
                .iter()
                .any(|combined| combined == base)
        }));
        assert!(
            CONTRACTS
                .entry_contract("esp32s31-phy-registered")
                .is_some()
        );
    }
}
