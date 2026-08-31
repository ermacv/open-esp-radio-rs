//! Reusable ESP32-S31 rev0 lifting knowledge.

pub use open_radio_vendor_backend_riscv::{
    ReviewedCompressedPointerEncoding, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact,
};
pub use open_radio_vendor_chip_contracts_esp32s31_rev0::{CONTRACTS, entry_contract};
pub use open_radio_vendor_semantics::*;

const RTC_XTAL_FREQUENCY_SEMANTIC: DirectSemanticFunctionSpec = DirectSemanticFunctionSpec {
    id: "esp32s31-rtc-xtal-frequency-v1",
    source: "esp32s31-rev0-chip-addon",
    c_name: "rtc_clk_xtal_freq_get",
    argument_count: 0,
    body_policy: SemanticFunctionBodyPolicy::OpaqueBoundary,
    return_model: ExternalReturnModel::Constant(40),
    semantic: ExternalSemanticSpec {
        operation: "clock.xtal-frequency.read",
        arguments: &[],
        return_type: "u32",
        replacement: Some("fixed ESP32-S31 40 MHz crystal contract"),
        event_dispatch: None,
    },
    evidence: "esp32s31-rev0-fixed-crystal-chip-contract",
};

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
    semantic_cache_domain: "blobray/riscv-harness/esp32s31-rev0-chip/v2",
    contracts: &CONTRACTS,
    summaries: &SUMMARIES,
    compressed_pointer_encodings: &[ReviewedCompressedPointerEncoding::new(
        "esp32s31-controller-sram-low20-word-address-v1",
        0x2f00_0000,
        20,
        2,
    )],
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
