//! Reusable ESP-IDF function semantics shared across chip providers.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, DraftReferenceEvent, ExternalArgumentDirection,
    ExternalArgumentSpec, ExternalReturnModel, ExternalSemanticSpec, FunctionAnalysis, MmioMap,
    SymbolicValue,
};
use open_radio_vendor_backend_riscv::{
    StructuralPointerContext, artifact::ArtifactSymbolDefinition,
};

const DELAY_ARGUMENTS: &[ExternalArgumentSpec] = &[ExternalArgumentSpec {
    name: "micros",
    c_type: "u32",
    direction: ExternalArgumentDirection::Input,
}];

static ETS_DELAY_US: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp-idf.ets-delay-us",
    source: "esp-idf-addon",
    c_name: "ets_delay_us",
    argument_count: 1,
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

pub fn direct_semantic_function(
    symbol: &ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    direct_external_semantic_function(&symbol.name)
}

pub fn direct_external_semantic_function(
    name: &str,
) -> Option<&'static DirectSemanticFunctionSpec> {
    match name {
        "ets_delay_us" => Some(&ETS_DELAY_US),
        _ => None,
    }
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
        assert!(direct_external_semantic_function("vendor_ets_delay_us").is_none());
    }
}
