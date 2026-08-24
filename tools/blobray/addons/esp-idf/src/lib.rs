//! Reusable ESP-IDF function semantics shared across chip providers.

mod bluetooth_controller;

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, DraftReferenceEvent, ExpressionOperation,
    ExternalArgumentDirection, ExternalArgumentSpec, ExternalReturnModel, ExternalSemanticSpec,
    FunctionAnalysis, MmioMap, SemanticFunctionBodyPolicy, SymbolicValue,
};
use open_radio_vendor_backend_riscv::{
    Rv32CallArguments, Rv32IntrinsicResult, StructuralPointerContext,
    artifact::ArtifactSymbolDefinition,
};

const DELAY_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "micros",
    c_type: "u32",
    direction: ExternalArgumentDirection::Input,
}];
const WIFI_MODE_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "mode",
    c_type: "wifi_mode_t *",
    direction: ExternalArgumentDirection::Output,
}];
const WIFI_MAC_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "interface",
        c_type: "wifi_interface_t",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "mac",
        c_type: "uint8_t[6]",
        direction: ExternalArgumentDirection::Output,
    },
];
const ROUNDUP2_ARGUMENTS: &[ExternalArgumentSpec] = &[
    ExternalArgumentSpec {
        name: "value",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
    ExternalArgumentSpec {
        name: "alignment",
        c_type: "uint32_t",
        direction: ExternalArgumentDirection::Input,
    },
];

static ETS_DELAY_US: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp-idf.ets-delay-us",
    source: "esp-idf-addon",
    c_name: "ets_delay_us",
    argument_count: 1,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::Void,
    semantic: ExternalSemanticSpec {
        operation: "time.blocking-delay",
        arguments: DELAY_ARGUMENTS,
        return_type: "void",
        replacement: Some("Rust async timer"),
        event_dispatch: None,
    },
    evidence: "exact ESP-IDF public symbol and documented SDK ABI",
};

static ESP_WIFI_GET_MODE: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp-idf.esp-wifi-get-mode",
    source: "esp-idf-addon",
    c_name: "esp_wifi_get_mode",
    argument_count: 1,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "wifi.query-mode",
        arguments: WIFI_MODE_ARGUMENTS,
        return_type: "esp_err_t",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact ESP-IDF public symbol and documented SDK ABI; output memory is not execution-modeled",
};

static ESP_WIFI_GET_MAC: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp-idf.esp-wifi-get-mac",
    source: "esp-idf-addon",
    c_name: "esp_wifi_get_mac",
    argument_count: 2,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::Unmodeled,
    semantic: ExternalSemanticSpec {
        operation: "wifi.query-mac-address",
        arguments: WIFI_MAC_ARGUMENTS,
        return_type: "esp_err_t",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact ESP-IDF public symbol and documented SDK ABI; output memory is not execution-modeled",
};

static ROUNDUP2: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp-idf.roundup2",
    source: "esp-idf-addon",
    c_name: "roundup2",
    argument_count: 2,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::SymbolicU32,
    semantic: ExternalSemanticSpec {
        operation: "integer.round-up-power-of-two",
        arguments: ROUNDUP2_ARGUMENTS,
        return_type: "uint32_t",
        replacement: None,
        event_dispatch: None,
    },
    evidence: "exact ESP-IDF/BSD compatibility symbol and roundup2(value, alignment) ABI",
};

pub fn direct_semantic_function(
    symbol: &ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    direct_external_semantic_function(&symbol.name)
}

pub fn direct_external_semantic_function(
    name: &str,
) -> Option<&'static DirectSemanticFunctionSpec> {
    if let Some(function) = bluetooth_controller::direct_external_semantic_function(name) {
        return Some(function);
    }
    match name {
        "ets_delay_us" => Some(&ETS_DELAY_US),
        "esp_wifi_get_mode" => Some(&ESP_WIFI_GET_MODE),
        "esp_wifi_get_mac" => Some(&ESP_WIFI_GET_MAC),
        "roundup2" => Some(&ROUNDUP2),
        _ => None,
    }
}

/// Exact RV32 value model for reusable ESP-IDF compatibility helpers.
pub fn direct_external_intrinsic(
    name: &str,
    arguments: &Rv32CallArguments,
) -> Option<Rv32IntrinsicResult> {
    if name != "roundup2" {
        return None;
    }
    let value = arguments[0].clone();
    let alignment = arguments[1].clone();
    let alignment_minus_one = SymbolicValue::expression(
        ExpressionOperation::Subtract,
        alignment.clone(),
        SymbolicValue::Constant(1),
    );
    let rounded_base =
        SymbolicValue::expression(ExpressionOperation::Add, value, alignment_minus_one);
    let alignment_mask = SymbolicValue::expression(
        ExpressionOperation::Subtract,
        SymbolicValue::Constant(0),
        alignment,
    );
    Some((
        SymbolicValue::expression(ExpressionOperation::BitAnd, rounded_base, alignment_mask),
        None,
    ))
}

/// Executable RV32 reference effect for ESP-IDF calls whose behavior is part
/// of the reusable SDK contract rather than a chip ROM implementation.
pub fn reference_intrinsic_trace(
    symbol: &ArtifactSymbolDefinition,
    _svd: &MmioMap,
    _pointer_context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    (symbol.name == "ets_delay_us").then(|| FunctionAnalysis {
        symbol: symbol.name.clone(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::DelayMicros {
            micros: SymbolicValue::input(0),
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: None,
        unresolved_branch: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_contract_is_exact_and_chip_independent() {
        let contract = direct_external_semantic_function("ets_delay_us").unwrap();
        assert_eq!(contract.source, "esp-idf-addon");
        assert_eq!(contract.semantic.operation, "time.blocking-delay");
        assert_eq!(contract.semantic.arguments.len(), 1);
        assert_eq!(
            direct_external_semantic_function("esp_wifi_get_mode")
                .unwrap()
                .semantic
                .arguments[0]
                .direction,
            ExternalArgumentDirection::Output
        );
        assert_eq!(
            direct_external_semantic_function("esp_wifi_get_mac")
                .unwrap()
                .semantic
                .arguments
                .len(),
            2
        );
        assert!(direct_external_semantic_function("vendor_ets_delay_us").is_none());
        assert!(direct_external_semantic_function("esp_wifi_internal_set_retry_counter").is_none());
        let arguments = core::array::from_fn(|index| SymbolicValue::input(index as u8));
        assert!(direct_external_intrinsic("roundup2", &arguments).is_some());
        assert!(direct_external_intrinsic("vendor_roundup2", &arguments).is_none());
    }
}
