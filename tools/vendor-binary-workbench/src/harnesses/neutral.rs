//! Empty platform contract for architecture-only exploratory analysis.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, EntryContractRef, EntryContractSpec, FunctionAnalysis,
    KnowledgeContractSpec, MmioMap, StandardMemoryFunction, SymbolicValue,
};
use open_radio_vendor_backend_riscv::{
    RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments, StructuralPointerContext, artifact,
};

const NONE_ENTRY_SPEC: EntryContractSpec = EntryContractSpec {
    id: "none",
    function_table: None,
    pointer_symbols: &[],
    data_pointer_binding: None,
};

const ENTRY_CONTRACTS: [EntryContractRef; 1] = [EntryContractRef::new(&NONE_ENTRY_SPEC)];

static CONTRACTS: KnowledgeContractSpec = KnowledgeContractSpec {
    external_call_model_sets: &[],
    entry_contracts: &ENTRY_CONTRACTS,
    diagnostic_calls: &[],
};

fn no_secondary_return_target(_target: u32) -> bool {
    false
}

fn no_direct_semantic(
    _symbol: &artifact::ArtifactSymbolDefinition,
) -> Option<&'static DirectSemanticFunctionSpec> {
    None
}

fn no_direct_external_semantic(_symbol: &str) -> Option<&'static DirectSemanticFunctionSpec> {
    None
}

fn no_reference_intrinsic(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _svd: &MmioMap,
    _context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    None
}

fn no_standard_memory_function(_symbol: &str) -> Option<StandardMemoryFunction> {
    None
}

fn no_wide_signed_divide(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _arguments: &Rv32CallArguments,
) -> Option<(SymbolicValue, SymbolicValue)> {
    None
}

static SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: no_secondary_return_target,
    direct_semantic: no_direct_semantic,
    direct_external_semantic: no_direct_external_semantic,
    reference_intrinsic: no_reference_intrinsic,
    standard_memory_function: no_standard_memory_function,
    wide_signed_divide: no_wide_signed_divide,
};

pub(super) static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    contracts: &CONTRACTS,
    summaries: &SUMMARIES,
};

pub(super) fn entry_contract(id: &str) -> crate::Result<EntryContractRef> {
    CONTRACTS.entry_contract(id).ok_or_else(|| {
        crate::Error::invalid(format!("generic analysis has no entry contract {id:?}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_harness_exposes_only_the_empty_entry_contract() {
        assert!(CONTRACTS.external_call_model_sets.is_empty());
        assert!(CONTRACTS.diagnostic_calls.is_empty());
        assert_eq!(entry_contract("none").unwrap().id(), "none");
        assert!(entry_contract("platform-init").is_err());
    }
}
