//! Temporary executable reconstructions for the ESP32-S31 investigation.
//!
//! This crate is deliberately independent of production radio code. Its
//! guarded models enrich derived analysis while preserving artifact provenance;
//! they are not hardware truth and cannot make qualification decisions.

/// Selected executable implementation provenance; this is not an evidence
/// verdict or an assertion of equivalence to every possible input artifact.
pub const PROVIDER: ExecutionModelProviderSpec = ExecutionModelProviderSpec {
    id: "esp32s31-radio-reconstruction-models",
    revision: 1,
    kind: ExecutionModelKind::ManualReconstruction,
    applicability: "Exact body/address and relocation/context guards per reconstruction; no caller-owned DTM input bound is assumed.",
    evidence: "verification/vendor/projects/esp32s31/blobray-provider/OWNERSHIP.md",
};

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, ReviewedMemoryAccessClassification, ReviewedMemoryAccessOccurrence,
    ReviewedMemoryAccessOperation, ReviewedMemoryAccessRole, ReviewedMemoryValueDomain,
    RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments, StructuralPointerContext, artifact,
    codegen, execution,
};
pub use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_semantics::*;

mod reviewed_summaries;

const CHIP_SUMMARIES: &RiscvSummaryHooks =
    open_radio_vendor_chip_models_esp32s31_rev0::RISCV_HARNESS.summaries;

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
    // A callee body cannot prove its caller-owned input domain. The former
    // DTM channel bound needs authenticated caller/entry evidence that this
    // hook does not receive; keep unknown input until that proof is available.
    caller_memory_input_domain: CHIP_SUMMARIES.caller_memory_input_domain,
    standard_memory_function: CHIP_SUMMARIES.standard_memory_function,
    wide_signed_divide: reviewed_summaries::wide_signed_divide_intrinsic,
};

pub static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    semantic_cache_domain: "blobray/riscv-harness/esp32s31-rev0-runtime-models@1+esp32s31-radio-reconstruction-models@1",
    contracts: &CONTRACTS,
    summaries: &RISCV_SUMMARIES,
    compressed_pointer_encodings: open_radio_vendor_chip_models_esp32s31_rev0::RISCV_HARNESS
        .compressed_pointer_encodings,
    reviewed_memory_accesses: open_radio_vendor_knowledge_esp32s31::REVIEWED_MEMORY_ACCESSES,
};

pub const fn wide_signed_divide_target_address() -> u32 {
    0x2f81_ce6e
}
