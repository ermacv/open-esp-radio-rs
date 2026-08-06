//! Empty platform contract for architecture-only exploratory analysis.

use open_radio_vendor_analysis_model::{
    DirectSemanticFunctionSpec, EntryContractRef, EntryContractSpec, FunctionAnalysis,
    HarnessContractSpec, MmioRegisterMap, SymbolicValue,
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

static CONTRACTS: HarnessContractSpec = HarnessContractSpec {
    external_tables: &[],
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

fn no_reference_intrinsic(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _svd: &MmioRegisterMap,
    _context: &StructuralPointerContext,
) -> Option<FunctionAnalysis> {
    None
}

fn no_standard_memory_intrinsic(
    _symbol: &artifact::ArtifactSymbolDefinition,
    _arguments: &Rv32CallArguments,
) -> Option<std::result::Result<FunctionAnalysis, String>> {
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
    reference_intrinsic: no_reference_intrinsic,
    standard_memory_intrinsic: no_standard_memory_intrinsic,
    wide_signed_divide: no_wide_signed_divide,
};

pub(super) static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    contracts: &CONTRACTS,
    summaries: &SUMMARIES,
};

pub(super) fn entry_contract(id: &str) -> crate::Result<EntryContractRef> {
    CONTRACTS
        .entry_contract(id)
        .ok_or_else(|| format!("generic analysis has no entry contract {id:?}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_harness_exposes_only_the_empty_entry_contract() {
        assert!(CONTRACTS.external_tables.is_empty());
        assert!(CONTRACTS.diagnostic_calls.is_empty());
        assert_eq!(entry_contract("none").unwrap().id(), "none");
        assert!(entry_contract("platform-init").is_err());
    }
}
