//! Reviewed ESP32-S31 knowledge used to interpret vendor code.
//!
//! This crate is deliberately independent of production radio code. Its
//! summaries enrich derived analysis while preserving artifact provenance;
//! they are not hardware truth and cannot make qualification decisions.

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, ReviewedMemoryValueDomain, RiscvHarnessSpec, RiscvSummaryHooks,
    Rv32CallArguments, StructuralPointerContext, artifact, codegen, execution,
};
pub use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_semantics::*;

mod reviewed_inputs;
mod reviewed_summaries;

const CHIP_SUMMARIES: &RiscvSummaryHooks =
    open_radio_vendor_chip_knowledge_esp32s31_rev0::RISCV_HARNESS.summaries;

const RISCV_SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |target| target == wide_signed_divide_target_address(),
    direct_semantic: |symbol| {
        (CHIP_SUMMARIES.direct_semantic)(symbol)
            .or_else(|| reviewed_summaries::direct_semantic_function(symbol))
    },
    direct_external_semantic: |symbol| {
        (CHIP_SUMMARIES.direct_external_semantic)(symbol)
            .or_else(|| reviewed_summaries::direct_external_semantic_function(symbol))
    },
    direct_external_intrinsic: |symbol, arguments| {
        (CHIP_SUMMARIES.direct_external_intrinsic)(symbol, arguments)
    },
    reference_intrinsic: |symbol, svd, context| {
        (CHIP_SUMMARIES.reference_intrinsic)(symbol, svd, context)
            .or_else(|| reviewed_summaries::reference_intrinsic_trace(symbol, svd, context))
    },
    caller_memory_input_domain: |symbol, location, width| {
        (CHIP_SUMMARIES.caller_memory_input_domain)(symbol, location, width)
            .or_else(|| reviewed_inputs::caller_memory_input_domain(symbol, location, width))
    },
    standard_memory_function: CHIP_SUMMARIES.standard_memory_function,
    wide_signed_divide: reviewed_summaries::wide_signed_divide_intrinsic,
};

pub static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    semantic_cache_domain: "blobray/riscv-harness/esp32s31-rev0-chip/v2+radio-investigation-overlay/v5",
    contracts: &CONTRACTS,
    summaries: &RISCV_SUMMARIES,
    compressed_pointer_encodings: open_radio_vendor_chip_knowledge_esp32s31_rev0::RISCV_HARNESS
        .compressed_pointer_encodings,
};

pub const REVIEWED_SUMMARY_EVIDENCE_SOURCE: EvidenceSource = EvidenceSource {
    name: "reviewed_summaries.rs",
    contents: include_str!("reviewed_summaries.rs"),
};

pub const fn wide_signed_divide_target_address() -> u32 {
    0x2f81_ce6e
}
