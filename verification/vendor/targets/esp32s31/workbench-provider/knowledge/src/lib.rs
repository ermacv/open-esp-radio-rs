//! Reviewed ESP32-S31 knowledge used to interpret vendor code.
//!
//! This crate is deliberately independent of production radio code. Its
//! summaries enrich derived analysis while preserving artifact provenance;
//! they are not hardware truth and cannot make qualification decisions.

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact, codegen, execution,
};
pub use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_semantics::*;

mod reviewed_summaries;

const RISCV_SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |target| target == wide_signed_divide_target_address(),
    direct_semantic: |symbol| {
        open_radio_vendor_addon_c::direct_semantic_function(symbol)
            .or_else(|| open_radio_vendor_addon_esp_idf::direct_semantic_function(symbol))
            .or_else(|| reviewed_summaries::direct_semantic_function(symbol))
    },
    direct_external_semantic: |symbol| {
        open_radio_vendor_addon_c::direct_external_semantic_function(symbol)
            .or_else(|| open_radio_vendor_addon_esp_idf::direct_external_semantic_function(symbol))
            .or_else(|| reviewed_summaries::direct_external_semantic_function(symbol))
    },
    reference_intrinsic: |symbol, svd, context| {
        open_radio_vendor_addon_c::reference_intrinsic_trace(symbol, svd, context)
            .or_else(|| {
                open_radio_vendor_addon_esp_idf::reference_intrinsic_trace(symbol, svd, context)
            })
            .or_else(|| reviewed_summaries::reference_intrinsic_trace(symbol, svd, context))
    },
    standard_memory_function: open_radio_vendor_addon_c::standard_memory_function,
    wide_signed_divide: reviewed_summaries::wide_signed_divide_intrinsic,
};

pub static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    contracts: &CONTRACTS,
    summaries: &RISCV_SUMMARIES,
};

pub const REVIEWED_SUMMARY_EVIDENCE_SOURCE: EvidenceSource = EvidenceSource {
    name: "reviewed_summaries.rs",
    contents: include_str!("reviewed_summaries.rs"),
};

pub const fn wide_signed_divide_target_address() -> u32 {
    0x2f81_ce6e
}
