//! Executable RV32 runtime adapters composed with declarative ESP32-S31 facts.

/// Selected executable implementation provenance; this is not an evidence
/// verdict or an assertion of equivalence to every possible input artifact.
pub const PROVIDER: ExecutionModelProviderSpec = ExecutionModelProviderSpec {
    id: "esp32s31-rev0-runtime-models",
    revision: 1,
    kind: ExecutionModelKind::RuntimeSemantics,
    applicability: "Selected ESP32-S31 rev0 target and RV32 ABI; exact public runtime symbols; individual C/ESP-IDF adapters enforce body policy.",
    evidence: "verification/vendor/chips/esp32s31/blobray-provider/models/README.md",
};

pub use open_radio_vendor_backend_riscv::{
    ReviewedCompressedPointerEncoding, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact,
};
pub use open_radio_vendor_chip_contracts_esp32s31_rev0::{CONTRACTS, entry_contract};
pub use open_radio_vendor_semantics::*;

use open_radio_vendor_chip_knowledge_esp32s31_rev0::RTC_XTAL_FREQUENCY_SEMANTIC;

fn direct_semantic(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    open_radio_vendor_addon_c::direct_semantic_function(symbol)
        .or_else(|| open_radio_vendor_addon_esp_idf::direct_semantic_function(symbol))
}

fn direct_external_semantic(symbol: &str) -> Option<&'static DirectSemanticFunctionSpec> {
    open_radio_vendor_addon_c::direct_external_semantic_function(symbol)
        .or_else(|| open_radio_vendor_addon_esp_idf::direct_external_semantic_function(symbol))
        .or_else(|| (symbol == "rtc_clk_xtal_freq_get").then_some(&RTC_XTAL_FREQUENCY_SEMANTIC))
}

fn direct_external_intrinsic(
    symbol: &str,
    arguments: &Rv32CallArguments,
) -> Option<open_radio_vendor_backend_riscv::Rv32IntrinsicResult> {
    open_radio_vendor_addon_c::direct_external_intrinsic(symbol, arguments)
        .or_else(|| open_radio_vendor_addon_esp_idf::direct_external_intrinsic(symbol, arguments))
}

fn reference_intrinsic(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    open_radio_vendor_addon_c::reference_intrinsic_trace(symbol, svd, context).or_else(|| {
        open_radio_vendor_addon_esp_idf::reference_intrinsic_trace(symbol, svd, context)
    })
}

fn no_wide_divide(
    _: &artifact::ArtifactSymbolDefinition,
    _: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    None
}

pub static SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |_| false,
    direct_semantic,
    direct_external_semantic,
    direct_external_intrinsic,
    reference_intrinsic,
    caller_memory_input_domain: |_, _, _| None,
    standard_memory_function: open_radio_vendor_addon_c::standard_memory_function,
    wide_signed_divide: no_wide_divide,
};

pub static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    semantic_cache_domain: "blobray/riscv-harness/esp32s31-rev0-runtime-models@1",
    contracts: &CONTRACTS,
    summaries: &SUMMARIES,
    compressed_pointer_encodings:
        open_radio_vendor_chip_knowledge_esp32s31_rev0::COMPRESSED_POINTER_ENCODINGS,
    reviewed_memory_accesses: &[],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_addon_exposes_crystal_fact_but_no_blob_body_summary() {
        let crystal = (SUMMARIES.direct_external_semantic)("rtc_clk_xtal_freq_get").unwrap();
        assert_eq!(crystal.return_model, ExternalReturnModel::Constant(40));
        assert_eq!(crystal.source, "esp32s31-rev0-chip-addon");

        let blob = artifact::ArtifactSymbolDefinition {
            member: Some("pp.o".to_owned()),
            name: "pp_post".to_owned(),
            address: 0,
            bytes: vec![1, 2, 3],
            addresses_resolved: false,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        assert!((SUMMARIES.direct_semantic)(&blob).is_none());
        assert!(!(SUMMARIES.secondary_return_target)(0x2f81_ce6e));
    }
}
